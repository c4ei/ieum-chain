#!/usr/bin/env bash
set -euo pipefail

binary="$(realpath "${1:-target/release/ieum-chain}")"
test_root="$(mktemp -d)"
declare -A pids peer_ids peer_addrs

cleanup() {
  for pid in "${pids[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  rm -rf -- "$test_root"
}
trap cleanup EXIT

dump_logs() {
  for index in 1 2 3 4; do
    echo "===== node $index ====="
    tail -150 "$test_root/node-$index.log" 2>/dev/null || echo "로그 없음"
  done
}

rpc() {
  local index="$1" method="$2" params="$3"
  curl -fsS --connect-timeout 2 --max-time 5 \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" \
    "http://127.0.0.1:$((9300 + index))"
}

status_field() {
  local index="$1" field="$2"
  rpc "$index" ieum_nodeStatus '[]' |
    python3 -c "import json,sys; print(json.load(sys.stdin)['result']['$field'])"
}

start_node() {
  local index="$1"
  shift
  local args=(
    server --git_action_test --validator-index "$index"
    --port "$((7300 + index))" --rpc-port "$((9300 + index))"
    --rpc-data-dir "$test_root/node-$index/ledger"
    --node-key "$test_root/node-$index/keys/p2p_identity.key"
    --validator-key "$test_root/node-$index/keys/consensus_signing.key"
    --validators-config "$test_root/node-$index/validators.json"
  )
  mkdir -p "$test_root/node-$index"
  for peer in "$@"; do args+=(--peer "$peer"); done
  (cd "$test_root/node-$index" && exec "$binary" "${args[@]}") \
    >>"$test_root/node-$index.log" 2>&1 &
  pids[$index]="$!"
}

wait_for_peer_id() {
  local index="$1"
  for _ in $(seq 1 100); do
    peer_ids[$index]="$(sed -n 's/^IEUM 서버 노드 시작: //p' "$test_root/node-$index.log" | tail -1)"
    if [[ -n "${peer_ids[$index]}" ]]; then
      peer_addrs[$index]="/ip4/127.0.0.1/udp/$((7300 + index))/quic-v1/p2p/${peer_ids[$index]}"
      return 0
    fi
    kill -0 "${pids[$index]}" 2>/dev/null || { dump_logs; return 1; }
    sleep 0.2
  done
  echo "[실패] Node $index PeerId 확인 시간 초과"
  dump_logs
  return 1
}

wait_for_rpc() {
  local index="$1"
  for _ in $(seq 1 120); do
    rpc "$index" ieum_nodeStatus '[]' >/dev/null 2>&1 && return 0
    kill -0 "${pids[$index]}" 2>/dev/null || { dump_logs; return 1; }
    sleep 0.5
  done
  echo "[실패] Node $index RPC 준비 시간 초과"
  dump_logs
  return 1
}

wait_for_mesh() {
  for _ in $(seq 1 120); do
    local ready=true
    for index in "$@"; do
      [[ "$(status_field "$index" peers 2>/dev/null || echo 0)" -ge 3 ]] || ready=false
    done
    [[ "$ready" == true ]] && return 0
    sleep 0.5
  done
  echo "[실패] 4노드 P2P 연결 시간 초과"
  dump_logs
  return 1
}

send_transfer() {
  local faucet recipient response
  faucet="$(rpc 2 eth_coinbase '[]' | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"])')"
  recipient="0x3252b7b65e50B54508974dB8d634134B0bd6be90"
  response="$(rpc 2 eth_sendTransaction "[{\"from\":\"$faucet\",\"to\":\"$recipient\",\"value\":\"0x1\",\"gas\":\"0x5208\",\"gasPrice\":\"0x1\"}]")"
  python3 -c 'import json,sys; r=json.load(sys.stdin); assert "error" not in r, r; print(r["result"])' <<<"$response"
}

receipt_confirmed() {
  local index="$1" transaction_hash="$2"
  rpc "$index" eth_getTransactionReceipt "[\"$transaction_hash\"]" 2>/dev/null |
    python3 -c 'import json,sys; raise SystemExit(0 if json.load(sys.stdin).get("result") is not None else 1)'
}

wait_for_three_node_advance() {
  local previous="$1" transaction_hash="$2"
  for _ in $(seq 1 240); do
    local h2 h3 h4
    h2="$(status_field 2 height 2>/dev/null || echo 0)"
    h3="$(status_field 3 height 2>/dev/null || echo 0)"
    h4="$(status_field 4 height 2>/dev/null || echo 0)"
    if [[ "$h2" -gt "$previous" && "$h2" == "$h3" && "$h3" == "$h4" ]] &&
      receipt_confirmed 2 "$transaction_hash" &&
      receipt_confirmed 3 "$transaction_hash" &&
      receipt_confirmed 4 "$transaction_hash"; then
      echo "$h2"
      return 0
    fi
    sleep 0.5
  done
  echo "[실패] Node 1 중단 상태에서 3검증자 합의 실패" >&2
  dump_logs >&2
  return 1
}

echo "[준비] 운영 장애 재현: 최초 부트스트랩 Node 1의 영구 원장·키 재사용"
start_node 1
wait_for_peer_id 1
start_node 2 "${peer_addrs[1]}"
wait_for_peer_id 2
start_node 3 "${peer_addrs[1]}" "${peer_addrs[2]}"
wait_for_peer_id 3
start_node 4 "${peer_addrs[1]}" "${peer_addrs[2]}" "${peer_addrs[3]}"
wait_for_peer_id 4
for index in 1 2 3 4; do wait_for_rpc "$index"; done
wait_for_mesh 1 2 3 4

initial_transaction_hash="$(send_transfer)"
initial_confirmed=false
for _ in $(seq 1 120); do
  base_height="$(status_field 1 height 2>/dev/null || echo 0)"
  [[ "$base_height" -ge 1 ]] &&
    [[ "$base_height" == "$(status_field 2 height)" ]] &&
    [[ "$base_height" == "$(status_field 3 height)" ]] &&
    [[ "$base_height" == "$(status_field 4 height)" ]] &&
    receipt_confirmed 1 "$initial_transaction_hash" &&
    receipt_confirmed 2 "$initial_transaction_hash" &&
    receipt_confirmed 3 "$initial_transaction_hash" &&
    receipt_confirmed 4 "$initial_transaction_hash" && {
      initial_confirmed=true
      break
    }
  sleep 0.5
done
[[ "$initial_confirmed" == true ]] || {
  echo "[실패] 최초 송금이 네 노드에서 확정되지 않았습니다."
  dump_logs
  exit 1
}

kill "${pids[1]}"
wait "${pids[1]}" 2>/dev/null || true
echo "[진행] Node 1 중단: height=$base_height"

target_height="$base_height"
for round in 1 2 3; do
  transaction_hash="$(send_transfer)"
  target_height="$(wait_for_three_node_advance "$target_height" "$transaction_hash")"
  echo "[진행] 생존 3검증자 확정 $round/3: height=$target_height"
done

old_peer_id="${peer_ids[1]}"
start_node 1 "${peer_addrs[2]}" "${peer_addrs[3]}" "${peer_addrs[4]}"
wait_for_peer_id 1
[[ "${peer_ids[1]}" == "$old_peer_id" ]] || {
  echo "[실패] Node 1 재시작 후 영구 PeerId가 변경됐습니다."
  exit 1
}
wait_for_rpc 1

highest_seen=false
for _ in $(seq 1 30); do
  sync_highest="$(status_field 1 syncHighest 2>/dev/null || echo 0)"
  [[ "$sync_highest" -ge "$target_height" ]] && { highest_seen=true; break; }
  sleep 0.5
done
[[ "$highest_seen" == true ]] || {
  echo "[실패] Node 1이 15초 안에 피어 최고 높이를 발견하지 못했습니다: local=$(status_field 1 height), syncHighest=${sync_highest:-확인불가}, expected=$target_height"
  dump_logs
  exit 1
}

for _ in $(seq 1 120); do
  heights=(); roots=()
  for index in 1 2 3 4; do
    heights+=("$(status_field "$index" height 2>/dev/null || echo -1)")
    roots+=("$(status_field "$index" stateRoot 2>/dev/null || echo RPC실패)")
  done
  if [[ "${heights[0]}" == "$target_height" ]] &&
     [[ "${heights[0]}" == "${heights[1]}" && "${heights[1]}" == "${heights[2]}" && "${heights[2]}" == "${heights[3]}" ]] &&
     [[ "${roots[0]}" == "${roots[1]}" && "${roots[1]}" == "${roots[2]}" && "${roots[2]}" == "${roots[3]}" ]]; then
    grep -q '\[동기화 요청 수신\]' "$test_root/node-2.log" "$test_root/node-3.log" "$test_root/node-4.log" || {
      echo "[실패] 생존 피어에서 동기화 요청 수신 로그를 찾지 못했습니다."
      dump_logs
      exit 1
    }
    echo "[성공] Node 1 영구 원장 재합류: heights=${heights[*]}, stateRoot=${roots[0]}"
    exit 0
  fi
  sleep 0.5
done

echo "[실패] Node 1 자동 동기화 시간 초과: heights=${heights[*]}, expected=$target_height"
dump_logs
exit 1

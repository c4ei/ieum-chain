#!/usr/bin/env bash
set -euo pipefail

binary="${1:-target/release/ieum-chain}"
binary="$(realpath "$binary")"
test_root="$(mktemp -d)"
pids=()

cleanup() {
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -rf "$test_root"
}
trap cleanup EXIT

dump_logs() {
  for index in 1 2 3 4; do
    echo "===== node $index ====="

    if [[ -f "$test_root/node-$index.log" ]]; then
      tail -100 "$test_root/node-$index.log"
    else
      echo "로그 파일 없음"
    fi
  done
}

start_node() {
  local index="$1"
  shift
  local peers=("$@")
  local p2p_port="$((7200 + index))"
  local rpc_port="$((9200 + index))"
  local args=(
    server
    --git_action_test
    --validator-index "$index"
    --port "$p2p_port"
    --rpc-port "$rpc_port"
    --rpc-data-dir "$test_root/node-$index/ledger"
    --node-key "$test_root/node-$index/keys/p2p_identity.key"
    --validator-key "$test_root/node-$index/keys/consensus_signing.key"
    --validators-config "$test_root/node-$index/validators.json"
  )

  # 각 노드를 빈 임시 작업 디렉터리에서 실행한다. 저장소의 운영용
  # config/network.json·config/update.json을 읽지 않으므로 CI 테스트가
  # 운영 DNS, 공개 광고 주소 또는 자동 업데이트에 접근하지 않는다.
  mkdir -p "$test_root/node-$index"

  for peer in "${peers[@]}"; do
    args+=(--peer "$peer")
  done

  (
    cd "$test_root/node-$index"
    exec "$binary" "${args[@]}"
  ) >"$test_root/node-$index.log" 2>&1 &
  pids+=("$!")
}

wait_for_peer_id() {
  local index="$1"
  local pid="${pids[$((index - 1))]}"
  local log="$test_root/node-$index.log"
  local peer_id=""

  for _ in $(seq 1 50); do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "노드 $index 프로세스가 PeerId 생성 전에 종료됐습니다." >&2
      dump_logs >&2
      return 1
    fi

    peer_id="$(sed -n 's/^IEUM 서버 노드 시작: //p' "$log" | head -1)"
    if [[ -n "$peer_id" ]]; then
      printf '%s\n' "$peer_id"
      return 0
    fi

    sleep 0.2
  done

  echo "노드 $index의 PeerId를 확인하지 못했습니다." >&2
  dump_logs >&2
  return 1
}

rpc() {
  local port="$1"
  local method="$2"
  local params="$3"

  curl --fail --silent --show-error \
    --connect-timeout 2 \
    --max-time 5 \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" \
    "http://127.0.0.1:$port"
}

wait_for_rpc() {
  local index="$1"
  local port="$((9200 + index))"
  local pid="${pids[$((index - 1))]}"

  for _ in $(seq 1 120); do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "노드 $index 프로세스가 RPC 준비 전에 종료됐습니다."
      dump_logs
      return 1
    fi

    if rpc "$port" ieum_nodeStatus '[]' >/dev/null 2>&1; then
      echo "노드 $index RPC 준비 완료: 127.0.0.1:$port"
      return 0
    fi

    sleep 0.5
  done

  echo "노드 $index RPC가 60초 안에 준비되지 않았습니다: 127.0.0.1:$port"
  dump_logs
  return 1
}

start_node 1
peer_id_1="$(wait_for_peer_id 1)"
peer_1="/ip4/127.0.0.1/udp/7201/quic-v1/p2p/$peer_id_1"

# 동기화는 서로 다른 두 피어의 동일한 tip/state root를 요구한다. 스타 토폴로지에서는
# 리프가 합의 확정을 한 번 놓치면 교차검증할 두 번째 피어가 없어 복구할 수 없으므로,
# 네 검증자를 완전 연결망으로 시작한다.
start_node 2 "$peer_1"
peer_id_2="$(wait_for_peer_id 2)"
peer_2="/ip4/127.0.0.1/udp/7202/quic-v1/p2p/$peer_id_2"

start_node 3 "$peer_1" "$peer_2"
peer_id_3="$(wait_for_peer_id 3)"
peer_3="/ip4/127.0.0.1/udp/7203/quic-v1/p2p/$peer_id_3"

start_node 4 "$peer_1" "$peer_2" "$peer_3"

for index in 1 2 3 4; do
  wait_for_rpc "$index"
done

for index in 1 2 3 4; do
  port="$((9200 + index))"
  chain_id_response="$(rpc "$port" eth_chainId '[]')"
  python3 - "$index" "$chain_id_response" <<'PY'
import json
import sys

index, raw = sys.argv[1:]
chain_id = int(json.loads(raw)["result"], 16)
if chain_id != 21005:
    raise SystemExit(f"노드 {index} CI chain ID 불일치: {chain_id}")
print(f"노드 {index} CI chain ID 확인: {chain_id}")
PY
done

# RPC listen은 P2P mesh보다 먼저 준비될 수 있다. 연결 전에 거래를 넣으면 노드별로
# 서로 다른 round를 시작할 수 있으므로 모든 노드의 3개 연결을 확인한 뒤 송금한다.
for _ in $(seq 1 120); do
  peer_counts=()
  topology_ready=true
  for index in 1 2 3 4; do
    port="$((9200 + index))"
    if ! status="$(rpc "$port" ieum_nodeStatus '[]' 2>/dev/null)"; then
      topology_ready=false
      break
    fi
    if ! peers="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["peers"])' <<<"$status" 2>/dev/null)"; then
      topology_ready=false
      break
    fi
    peer_counts+=("$peers")
  done
  if [[ "$topology_ready" == true ]] &&
     [[ "${#peer_counts[@]}" -eq 4 ]] &&
     [[ "${peer_counts[0]}" -ge 3 ]] &&
     [[ "${peer_counts[1]}" -ge 3 ]] &&
     [[ "${peer_counts[2]}" -ge 3 ]] &&
     [[ "${peer_counts[3]}" -ge 3 ]]; then
    echo "4노드 P2P 토폴로지 준비 완료: peers=${peer_counts[*]}"
    break
  fi
  sleep 0.5
done

if [[ "${#peer_counts[@]}" -ne 4 ]] ||
   [[ "${peer_counts[0]:-0}" -lt 3 ]] ||
   [[ "${peer_counts[1]:-0}" -lt 3 ]] ||
   [[ "${peer_counts[2]:-0}" -lt 3 ]] ||
   [[ "${peer_counts[3]:-0}" -lt 3 ]]; then
  echo "4노드 P2P 토폴로지가 60초 안에 준비되지 않았습니다: peers=${peer_counts[*]:-확인불가}"
  dump_logs
  exit 1
fi

# GossipSub 구독 정보가 연결 직후 전파될 시간을 짧게 보장한다.
sleep 2

faucet_response="$(rpc 9202 eth_coinbase '[]')"

faucet="$(
  python3 -c '
import json
import sys

response = json.load(sys.stdin)
result = response.get("result")

if not isinstance(result, str) or not result:
    raise SystemExit("eth_coinbase 응답에 유효한 result가 없습니다.")

print(result)
' <<<"$faucet_response"
)"

recipient="0x3252b7b65e50B54508974dB8d634134B0bd6be90"
transfer_value="0x16345785d8a0000" # 0.1 IEUM
transfer_amount_wei=100000000000000000

for index in 1 2 3 4; do
  port="$((9200 + index))"
  balance_response="$(rpc "$port" eth_getBalance "[\"$faucet\",\"latest\"]")"
  python3 - "$index" "$faucet" "$balance_response" <<'PY'
import json
import sys

index, faucet, raw = sys.argv[1:]
response = json.loads(raw)
if "error" in response:
    raise SystemExit(f"노드 {index} faucet 잔액 조회 실패: {response['error']}")
balance = int(response.get("result", "0x0"), 16)
required = 10**18  # 1 IEUM
if balance < required:
    raise SystemExit(
        f"노드 {index} faucet 잔액 부족: address={faucet}, "
        f"balance={balance}, required={required}"
    )
print(f"노드 {index} faucet 확인: {faucet}, balance={balance} wei")
PY
done

# 수신 주소는 고정 개발 제네시스에서 이미 잔액을 보유할 수 있다. 송금 후 절대
# 잔액을 0.1 IEUM으로 가정하지 않고, 네 노드가 관측한 송금 전 잔액을 기준으로
# 정확히 0.1 IEUM 증가했는지 확인한다.
initial_recipient_balances=()
for index in 1 2 3 4; do
  port="$((9200 + index))"
  balance_response="$(rpc "$port" eth_getBalance "[\"$recipient\",\"latest\"]")"
  initial_recipient_balance="$(python3 - "$index" "$recipient" "$balance_response" <<'PY'
import json
import sys

index, recipient, raw = sys.argv[1:]
response = json.loads(raw)
if "error" in response:
    raise SystemExit(f"노드 {index} 수신 주소 잔액 조회 실패: {response['error']}")
print(int(response.get("result", "0x0"), 16))
PY
)"
  initial_recipient_balances+=("$initial_recipient_balance")
done

if [[ "${initial_recipient_balances[0]}" != "${initial_recipient_balances[1]}" ]] ||
   [[ "${initial_recipient_balances[1]}" != "${initial_recipient_balances[2]}" ]] ||
   [[ "${initial_recipient_balances[2]}" != "${initial_recipient_balances[3]}" ]]; then
  echo "송금 전 수신 주소 잔액이 노드별로 다릅니다: balances=${initial_recipient_balances[*]}"
  dump_logs
  exit 1
fi

expected_recipient_balance="$((initial_recipient_balances[0] + transfer_amount_wei))"
echo "수신 주소 송금 전 잔액 확인: balance=${initial_recipient_balances[0]} wei, expectedAfter=$expected_recipient_balance wei"

send_response="$(rpc 9202 eth_sendTransaction \
  "[{
    \"from\":\"$faucet\",
    \"to\":\"$recipient\",
    \"value\":\"$transfer_value\",
    \"gas\":\"0x5208\",
    \"gasPrice\":\"0x1\"
  }]")"

transaction_hash="$(python3 - "$send_response" <<'PY'
import json
import sys

response = json.loads(sys.argv[1])
if "error" in response:
    raise SystemExit(f"0.1 IEUM 송금 제출 실패: {response['error']}")
result = response.get("result")
if not isinstance(result, str) or not result.startswith("0x"):
    raise SystemExit(f"송금 해시가 없는 응답: {response}")
print(result)
PY
)"
echo "0.1 IEUM 송금 제출 완료: $transaction_hash"

bft_passed=false
for _ in $(seq 1 60); do
  heights=()
  roots=()
  recipient_balances=()
  status_read_failed=false

  for index in 1 2 3 4; do
    port="$((9200 + index))"
    pid="${pids[$((index - 1))]}"

    if ! kill -0 "$pid" 2>/dev/null; then
      echo "합의 대기 중 노드 $index 프로세스가 종료됐습니다."
      dump_logs
      exit 1
    fi

    if ! status="$(rpc "$port" ieum_nodeStatus '[]' 2>/dev/null)"; then
      status_read_failed=true
      break
    fi

    if ! parsed="$(
      python3 -c '
import json
import sys

response = json.load(sys.stdin)
result = response["result"]
print(result["height"])
print(result["stateRoot"])
' <<<"$status" 2>/dev/null
    )"; then
      status_read_failed=true
      break
    fi

    height="$(sed -n '1p' <<<"$parsed")"
    state_root="$(sed -n '2p' <<<"$parsed")"

    heights+=("$height")
    roots+=("$state_root")

    if ! balance_response="$(rpc "$port" eth_getBalance "[\"$recipient\",\"latest\"]" 2>/dev/null)"; then
      status_read_failed=true
      break
    fi
    if ! recipient_balance="$(python3 - "$balance_response" <<'PY'
import json
import sys
response = json.loads(sys.argv[1])
if "error" in response:
    raise SystemExit(1)
print(int(response.get("result", "0x0"), 16))
PY
)"; then
      status_read_failed=true
      break
    fi
    recipient_balances+=("$recipient_balance")
  done

  if [[ "$status_read_failed" == false ]] &&
     [[ "${#heights[@]}" -eq 4 ]] &&
     [[ "${heights[0]}" -ge 1 ]] &&
     [[ "${heights[0]}" == "${heights[1]}" ]] &&
     [[ "${heights[1]}" == "${heights[2]}" ]] &&
     [[ "${heights[2]}" == "${heights[3]}" ]] &&
     [[ "${roots[0]}" == "${roots[1]}" ]] &&
     [[ "${roots[1]}" == "${roots[2]}" ]] &&
     [[ "${roots[2]}" == "${roots[3]}" ]] &&
     [[ "${recipient_balances[0]}" == "$expected_recipient_balance" ]] &&
     [[ "${recipient_balances[1]}" == "$expected_recipient_balance" ]] &&
     [[ "${recipient_balances[2]}" == "$expected_recipient_balance" ]] &&
     [[ "${recipient_balances[3]}" == "$expected_recipient_balance" ]]; then
    echo "4-process BFT passed: heights=${heights[*]}, stateRoot=${roots[0]}, recipientBalance=${recipient_balances[0]}"
    bft_passed=true
    break
  fi

  sleep 0.5
done

if [[ "$bft_passed" != true ]]; then
  echo "4프로세스 BFT 합의가 제한 시간 안에 완료되지 않았습니다."
  echo "마지막 관측: heights=${heights[*]:-확인불가}, roots=${roots[*]:-확인불가}, recipientBalances=${recipient_balances[*]:-확인불가}"
  dump_logs
  exit 1
fi

# 한 검증자를 실제로 중단한 동안 나머지 3개 노드가 블록을 확정하고, 같은 데이터
# 디렉터리로 재기동한 검증자가 자동 동기화한 뒤 다시 동일 상태를 제공하는지 검증한다.
height_before_rejoin="${heights[0]}"
kill "${pids[3]}"
wait "${pids[3]}" 2>/dev/null || true
echo "노드 4 중단 완료: height=$height_before_rejoin"

second_send_response="$(rpc 9202 eth_sendTransaction \
  "[{\"from\":\"$faucet\",\"to\":\"$recipient\",\"value\":\"$transfer_value\",\"gas\":\"0x5208\",\"gasPrice\":\"0x1\"}]")"
second_transaction_hash="$(python3 - "$second_send_response" <<'PY'
import json,sys
response=json.loads(sys.argv[1])
if "error" in response:
    raise SystemExit(f"재합류 시험용 송금 제출 실패: {response['error']}")
print(response["result"])
PY
)"
expected_after_rejoin="$((expected_recipient_balance + transfer_amount_wei))"

three_node_passed=false
for _ in $(seq 1 80); do
  three_node_passed=true
  for index in 1 2 3; do
    status="$(rpc "$((9200 + index))" ieum_nodeStatus '[]')" || { three_node_passed=false; break; }
    height="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["height"])' <<<"$status")"
    balance="$(rpc "$((9200 + index))" eth_getBalance "[\"$recipient\",\"latest\"]" | python3 -c 'import json,sys; print(int(json.load(sys.stdin)["result"],16))')"
    if [[ "$height" -le "$height_before_rejoin" || "$balance" != "$expected_after_rejoin" ]]; then
      three_node_passed=false
      break
    fi
  done
  [[ "$three_node_passed" == true ]] && break
  sleep 0.5
done
[[ "$three_node_passed" == true ]] || { echo "노드 4 중단 중 3노드 확정 실패"; dump_logs; exit 1; }

start_node 4 "$peer_1" "$peer_2" "$peer_3"
pids[3]="${pids[4]}"
unset 'pids[4]'
wait_for_rpc 4

rejoin_passed=false
for _ in $(seq 1 120); do
  heights=(); roots=(); recipient_balances=(); rejoin_passed=true
  for index in 1 2 3 4; do
    status="$(rpc "$((9200 + index))" ieum_nodeStatus '[]' 2>/dev/null)" || { rejoin_passed=false; break; }
    parsed="$(python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r["height"]);print(r["stateRoot"])' <<<"$status")" || { rejoin_passed=false; break; }
    heights+=("$(sed -n '1p' <<<"$parsed")")
    roots+=("$(sed -n '2p' <<<"$parsed")")
    recipient_balances+=("$(rpc "$((9200 + index))" eth_getBalance "[\"$recipient\",\"latest\"]" | python3 -c 'import json,sys;print(int(json.load(sys.stdin)["result"],16))')")
  done
  if [[ "$rejoin_passed" == true ]] &&
     [[ "${heights[0]}" == "${heights[1]}" && "${heights[1]}" == "${heights[2]}" && "${heights[2]}" == "${heights[3]}" ]] &&
     [[ "${roots[0]}" == "${roots[1]}" && "${roots[1]}" == "${roots[2]}" && "${roots[2]}" == "${roots[3]}" ]] &&
     [[ "${recipient_balances[3]}" == "$expected_after_rejoin" ]]; then
    receipt="$(rpc 9204 eth_getTransactionReceipt "[\"$second_transaction_hash\"]")"
    python3 -c 'import json,sys; assert json.load(sys.stdin).get("result") is not None' <<<"$receipt" || rejoin_passed=false
    [[ "$rejoin_passed" == true ]] && break
  else
    rejoin_passed=false
  fi
  sleep 0.5
done

[[ "$rejoin_passed" == true ]] || { echo "노드 4 자동 재합류 실패"; dump_logs; exit 1; }
echo "4-process restart/rejoin passed: heights=${heights[*]}, stateRoot=${roots[0]}, receipt=$second_transaction_hash"

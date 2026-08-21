#!/usr/bin/env bash
set -euo pipefail

RPC_URL="${IEUM_RPC_URL:-https://irpc.aah.name}"
TX_HASH=""
DOCKER_MODE=false

usage() {
  echo "사용법: $0 [--rpc URL] [--tx 0xHASH] [--docker]"
  echo "  --docker  현재 호스트의 ieum-node1~4 로그와 상태도 읽기 전용으로 검사합니다."
}

while (($#)); do
  case "$1" in
    --rpc) RPC_URL="${2:?RPC URL이 필요합니다.}"; shift 2 ;;
    --tx) TX_HASH="${2:?거래 해시가 필요합니다.}"; shift 2 ;;
    --docker) DOCKER_MODE=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "알 수 없는 옵션: $1" >&2; usage >&2; exit 2 ;;
  esac
done

rpc() {
  local method="$1" params="${2:-[]}" result
  result="$(curl -fsS --max-time 8 "$RPC_URL" -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}")" || {
      echo "RPC_ERROR"
      return 1
    }
  printf '%s\n' "$result"
}

json_value() {
  python3 -c 'import json,sys
path=sys.argv[1].split(".")
value=json.load(sys.stdin)
for key in path:
    value=value.get(key) if isinstance(value,dict) else None
print("" if value is None else str(value).lower() if isinstance(value,bool) else value)' "$1"
}

echo "IEUM Doctor v0.23.11"
echo "RPC: $RPC_URL"
identity="$(rpc ieum_networkIdentity)"
protocol="$(rpc ieum_protocolVersion)"
sync="$(rpc ieum_syncStatus)"
node="$(rpc ieum_nodeStatus)"
pool="$(rpc txpool_status)"
printf '신원: %s\n버전: %s\n동기화: %s\n노드: %s\n메모리풀: %s\n' \
  "$identity" "$protocol" "$sync" "$node" "$pool"

chain_id="$(printf '%s' "$identity" | json_value result.chainId)"
genesis_hash="$(printf '%s' "$identity" | json_value result.genesisHash)"
version="$(printf '%s' "$protocol" | json_value result.nodeVersion)"
ready="$(printf '%s' "$sync" | json_value result.readyForTransactions)"
height="$(printf '%s' "$node" | json_value result.height)"
pending_raw="$(printf '%s' "$pool" | json_value result.pending)"
if [[ "$pending_raw" =~ ^0x[0-9a-fA-F]+$ ]]; then
  pending="$((pending_raw))"
else
  pending="$(printf '%s' "$node" | json_value result.mempoolTransactions)"
fi

issues=0
[[ "$chain_id" == "21004" ]] || { echo "[위험] Chain ID가 21004가 아닙니다."; issues=$((issues+1)); }
[[ "$genesis_hash" == "0x82cfc3615112766f3eb151a8677890c1b74ce6bce8463a1a3590991c383650f6" ]] || { echo "[위험] 운영 Genesis hash가 일치하지 않습니다."; issues=$((issues+1)); }
[[ "$ready" == "true" ]] || { echo "[경고] 아직 송금 가능한 동기화 상태가 아닙니다."; issues=$((issues+1)); }
[[ "$version" == "0.23.11" ]] || echo "[안내] 노드 버전은 $version 입니다."

if [[ -n "$TX_HASH" ]]; then
  tx="$(rpc eth_getTransactionByHash "[\"$TX_HASH\"]")"
  receipt="$(rpc eth_getTransactionReceipt "[\"$TX_HASH\"]")"
  printf '거래: %s\n영수증: %s\n' "$tx" "$receipt"
fi

if [[ "$pending" =~ ^[0-9]+$ ]] && ((pending > 0)); then
  echo "[경고] mempool에 $pending개 거래가 대기 중입니다. 같은 거래를 다시 보내지 마세요."
  first_height="$height"
  sleep 12
  later="$(rpc ieum_nodeStatus)"
  later_height="$(printf '%s' "$later" | json_value result.height)"
  if [[ "$first_height" == "$later_height" ]]; then
    echo "[위험] 대기 거래가 있는데 블록 높이 $height가 12초 동안 증가하지 않았습니다. BFT 로그 점검이 필요합니다."
    issues=$((issues+1))
  fi
fi

if $DOCKER_MODE; then
  command -v docker >/dev/null || { echo "docker 명령을 찾을 수 없습니다." >&2; exit 2; }
  for container in ieum-node1 ieum-node2 ieum-node3 ieum-node4; do
    echo "===== $container ====="
    if ! docker inspect "$container" --format '상태={{.State.Status}} 시작={{.State.StartedAt}} 이미지={{.Config.Image}}'; then
      issues=$((issues+1)); continue
    fi
    docker logs --since=10m "$container" 2>&1 \
      | grep -E '합의 참여|검증자 자동 등록|BFT|블록 확정|안전 제안 보류|오류|거부' \
      | tail -n 20 || true
  done
fi

if ((issues)); then
  echo "결론: 자동 재시작하지 않았습니다. 위 위험 항목을 확인한 후 명시적으로 복구하세요."
  exit 1
fi
echo "결론: 즉시 조치가 필요한 이상을 찾지 못했습니다."

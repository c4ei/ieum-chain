#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'EOF'
IEUM 외부 PC RPC 진단 도구 v1.0.1.1

사용법:
  diagnose-ieum-external.sh [-h] -H RPC_HOST [-p PORTS] [-t TIMEOUT]

옵션:
  -h            사용법 출력
  -H RPC_HOST   IEUM 노드 RPC 주소 또는 IP (필수)
  -p PORTS      쉼표 구분 포트 (기본값: 8989,8990,8991,8992)
  -t TIMEOUT    요청 제한 시간 초 (기본값: 5)

서버 SSH·Docker 권한 없이 RPC 접근성, 버전, Chain ID, genesis hash,
현재·확정·동기화 높이, tip hash, state root, 피어, mempool과 공통 블록을 비교합니다.
EOF
}

rpc_host=
ports_csv=8989,8990,8991,8992
timeout=5
while getopts ':hH:p:t:' option; do
  case "$option" in
    h) usage; exit 0 ;;
    H) rpc_host="$OPTARG" ;;
    p) ports_csv="$OPTARG" ;;
    t) timeout="$OPTARG" ;;
    :) echo "[오류] -$OPTARG 옵션에 값이 필요합니다." >&2; usage >&2; exit 2 ;;
    \?) echo "[오류] 알 수 없는 옵션: -$OPTARG" >&2; usage >&2; exit 2 ;;
  esac
done
[[ -n "$rpc_host" ]] || { echo "[오류] -H RPC_HOST가 필요합니다." >&2; usage >&2; exit 2; }
for command_name in curl python3; do command -v "$command_name" >/dev/null || { echo "[오류] 필요한 명령이 없습니다: $command_name" >&2; exit 2; }; done

IFS=',' read -r -a ports <<<"$ports_csv"
work_dir="$(mktemp -d)"
trap 'rm -rf -- "$work_dir"' EXIT

rpc() {
  local port="$1" method="$2" params="$3" output="$4"
  curl -fsS --connect-timeout "$timeout" --max-time "$timeout" -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" \
    "http://$rpc_host:$port" >"$output"
}

echo "===== IEUM 외부 RPC 진단: $rpc_host (${ports[*]}) ====="
for index in "${!ports[@]}"; do
  port="${ports[$index]}"; node=$((index + 1)); prefix="$work_dir/node-$node"
  if ! rpc "$port" ieum_nodeStatus '[]' "$prefix-status.json"; then echo "[오류] Node $node RPC $port 연결 실패"; continue; fi
  rpc "$port" ieum_networkIdentity '[]' "$prefix-identity.json" || true
  rpc "$port" ieum_protocolVersion '[]' "$prefix-protocol.json" || true
  rpc "$port" ieum_finalizedBlock '[]' "$prefix-finalized.json" || true
  rpc "$port" txpool_status '[]' "$prefix-pool.json" || true
done

python3 - "$work_dir" "${#ports[@]}" "$rpc_host" "${ports[@]}" <<'PY'
import json, pathlib, sys
root, count, host = pathlib.Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
ports = sys.argv[4:]
def load(path):
    try: return json.loads(path.read_text()).get('result')
    except Exception: return None
rows=[]
for n in range(1,count+1):
    s=load(root/f'node-{n}-status.json')
    if not isinstance(s,dict): continue
    i=load(root/f'node-{n}-identity.json') or {}; p=load(root/f'node-{n}-protocol.json') or {}; f=load(root/f'node-{n}-finalized.json') or {}; pool=load(root/f'node-{n}-pool.json') or {}
    rows.append((n,s,i,p,f,pool))
    print(f"[Node {n}] RPC={host}:{ports[n-1]} 버전={s.get('version')} 프로토콜={p.get('protocolVersion')} 높이={s.get('height')} 확정={f.get('height',f)} 피어={s.get('peers')} 동기화={s.get('syncing')} 최고={s.get('syncHighest')} mempool={pool.get('pending')}")
if not rows: print('[실패] 응답 가능한 노드가 없습니다.'); sys.exit(1)
ids={(i.get('chainId'),i.get('genesisHash')) for _,_,i,_,_,_ in rows}; heights=[int(s['height']) for _,s,_,_,_,_ in rows]; top=max(heights); low=[f'Node {n}={s["height"]}' for n,s,_,_,_,_ in rows if int(s['height'])<top]
print('[정상] 네트워크 신원 일치' if len(ids)==1 else '[오류] Chain ID 또는 genesis hash 불일치')
print('[정상] 모든 노드 높이 일치' if not low else '[오류] 지연 노드: '+', '.join(low)+f' / 최고={top}')
for n,s,_,_,_,_ in rows:
    if int(s['height'])<top and not s.get('syncing') and int(s.get('syncHighest',s['height']))<=int(s['height']): print(f'[오류] Node {n}: 피어 최고 높이를 발견하지 못했습니다. sync 토픽 요청·응답을 확인하세요.')
sys.exit(3 if len(ids)>1 else (4 if low else 0))
PY

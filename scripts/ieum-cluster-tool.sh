#!/usr/bin/env bash
set -Eeuo pipefail

compose_dir=/opt/ieum-docker-four-node
rpc_host=192.168.1.148
ports=8989,8990,8991,8992
since=15m

usage() {
  cat <<'EOF'
IEUM 4노드 상태관리·진단 도구 v1.0.2.1

사용법:
  ieum-cluster-tool.sh [공통 옵션] COMMAND [COMMAND 옵션]

공통 옵션:
  -h              사용법 출력
  -c DIR          Docker Compose 디렉터리
  -H HOST         RPC 호스트
  -p PORTS        RPC 포트 목록(쉼표 구분)
  -s SINCE        로그 범위(기본값: 15m)

COMMAND:
  status          컨테이너·버전·높이·피어를 한 번에 확인
  diagnose        전체 서버 진단 실행
  logs [1-4|all]  동기화·P2P·BFT 핵심 로그만 출력
  snapshot DIR    설정·상태·로그를 DIR에 읽기 전용 수집
  restart N       지정한 노드 하나만 재시작하고 상태 확인
  reproduce BIN   개발용 Node 1 영구 원장 재합류 테스트 실행

예:
  ./scripts/ieum-cluster-tool.sh status
  ./scripts/ieum-cluster-tool.sh logs 1
  ./scripts/ieum-cluster-tool.sh snapshot /tmp/ieum-report
  ./scripts/ieum-cluster-tool.sh restart 1
  ./scripts/ieum-cluster-tool.sh reproduce target/release/ieum-chain

주의:
  restart 외 명령은 운영 데이터를 변경하지 않습니다.
  데이터 삭제·초기화·강제 롤백 기능은 제공하지 않습니다.
EOF
}

while getopts ':hc:H:p:s:' option; do
  case "$option" in
    h) usage; exit 0 ;;
    c) compose_dir="$OPTARG" ;;
    H) rpc_host="$OPTARG" ;;
    p) ports="$OPTARG" ;;
    s) since="$OPTARG" ;;
    :) echo "[오류] -$OPTARG 옵션에 값이 필요합니다." >&2; exit 2 ;;
    \?) echo "[오류] 알 수 없는 옵션: -$OPTARG" >&2; usage >&2; exit 2 ;;
  esac
done
shift $((OPTIND - 1))

command_name="${1:-}"
shift || true
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

require_compose() {
  [[ -d "$compose_dir" ]] || { echo "[오류] Compose 디렉터리가 없습니다: $compose_dir" >&2; exit 2; }
  command -v docker >/dev/null || { echo "[오류] docker 명령이 없습니다." >&2; exit 2; }
}

case "$command_name" in
  status)
    require_compose
    (cd "$compose_dir" && sudo docker compose ps)
    for node in 1 2 3 4; do
      echo "===== Node $node ====="
      sudo docker exec "ieum-node$node" /image/ieum-chain --version 2>&1 || true
    done
    "$script_dir/diagnose-ieum-external.sh" -H "$rpc_host" -p "$ports"
    ;;
  diagnose)
    "$script_dir/diagnose-ieum-server.sh" -H "$rpc_host" -p "$ports" -c "$compose_dir" -s "$since"
    ;;
  logs)
    require_compose
    target="${1:-all}"
    [[ "$target" == all || "$target" =~ ^[1-4]$ ]] || { echo "[오류] logs 대상은 1~4 또는 all입니다." >&2; exit 2; }
    nodes=(1 2 3 4)
    [[ "$target" == all ]] || nodes=("$target")
    for node in "${nodes[@]}"; do
      echo "===== Node $node 핵심 로그 ====="
      sudo docker logs --since "$since" "ieum-node$node" 2>&1 |
        grep -E '동기화|P2P 토픽|P2P 전파 대기|P2P 연결|P2P 종료|BFT 확정|BFT 라운드|거부|오류|error|warn' |
        tail -n 300 || true
    done
    ;;
  snapshot)
    require_compose
    output="${1:-}"
    [[ -n "$output" ]] || { echo "[오류] snapshot 저장 디렉터리가 필요합니다." >&2; exit 2; }
    mkdir -p -- "$output"
    (cd "$compose_dir" && sudo docker compose ps) >"$output/compose-ps.txt" 2>&1 || true
    for node in 1 2 3 4; do
      sudo docker inspect "ieum-node$node" >"$output/node-$node-inspect.json" 2>&1 || true
      sudo docker logs --since "$since" "ieum-node$node" >"$output/node-$node.log" 2>&1 || true
      for file in genesis.json validators.json network.json bootstrap.json upgrades.json events.json; do
        if [[ -f "/opt/ieum-node$node/config/$file" ]]; then
          sha256sum "/opt/ieum-node$node/config/$file" >>"$output/config-sha256.txt"
        fi
      done
    done
    "$script_dir/diagnose-ieum-external.sh" -H "$rpc_host" -p "$ports" >"$output/rpc-diagnosis.txt" 2>&1 || true
    echo "[완료] 비밀키·원장 본문을 제외한 진단 자료: $output"
    ;;
  restart)
    require_compose
    node="${1:-}"
    [[ "$node" =~ ^[1-4]$ ]] || { echo "[오류] restart 대상은 1~4입니다." >&2; exit 2; }
    echo "[진행] Node $node 하나만 재시작합니다. 데이터는 삭제하지 않습니다."
    (cd "$compose_dir" && sudo docker compose restart "node$node")
    sleep 10
    "$script_dir/diagnose-ieum-external.sh" -H "$rpc_host" -p "$ports" || true
    ;;
  reproduce)
    binary="${1:-}"
    [[ -x "$binary" ]] || { echo "[오류] 실행 가능한 IEUM 바이너리가 필요합니다: $binary" >&2; exit 2; }
    exec "$script_dir/../tests/four_process_node1_persistent_rejoin.sh" "$binary"
    ;;
  ''|-h|--help) usage ;;
  *) echo "[오류] 알 수 없는 COMMAND: $command_name" >&2; usage >&2; exit 2 ;;
esac

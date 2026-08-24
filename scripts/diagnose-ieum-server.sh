#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'EOF'
IEUM 실서버 종합 진단 도구 v1.0.2.1

사용법:
  diagnose-ieum-server.sh [-h] [-H RPC_HOST] [-p PORTS] [-c COMPOSE_DIR] [-s LOG_SINCE]

옵션:
  -h                사용법 출력
  -H RPC_HOST       RPC 주소 (기본값: 127.0.0.1)
  -p PORTS          쉼표 구분 RPC 포트 (기본값: 8989,8990,8991,8992)
  -c COMPOSE_DIR    Compose 디렉터리 (기본값: /opt/ieum-docker-four-node)
  -s LOG_SINCE      Docker 로그 범위 (기본값: 10m)

점검 항목:
  RPC·체인 신원·공통 블록·동기화 높이·tip/state root·P2P 토픽 가입·
  Docker 상태·마운트·포트·PeerId·설정 파일 hash·자동 업데이트 curl
EOF
}

rpc_host=127.0.0.1
ports_csv=8989,8990,8991,8992
compose_dir=/opt/ieum-docker-four-node
log_since=10m
while getopts ':hH:p:c:s:' option; do
  case "$option" in
    h) usage; exit 0 ;;
    H) rpc_host="$OPTARG" ;;
    p) ports_csv="$OPTARG" ;;
    c) compose_dir="$OPTARG" ;;
    s) log_since="$OPTARG" ;;
    :) echo "[오류] -$OPTARG 옵션에 값이 필요합니다." >&2; usage >&2; exit 2 ;;
    \?) echo "[오류] 알 수 없는 옵션: -$OPTARG" >&2; usage >&2; exit 2 ;;
  esac
done

for command_name in curl python3 docker sha256sum; do
  command -v "$command_name" >/dev/null || { echo "[오류] 필요한 명령이 없습니다: $command_name" >&2; exit 2; }
done

IFS=',' read -r -a ports <<<"$ports_csv"
echo "===== IEUM 실서버 진단 시작 ====="
echo "RPC=$rpc_host 포트=${ports[*]} Compose=$compose_dir 로그=$log_since"

echo; echo "===== Docker 실행 상태 ====="
if [[ -f "$compose_dir/compose.yml" || -f "$compose_dir/docker-compose.yml" || -f "$compose_dir/compose.yaml" ]]; then
  (cd "$compose_dir" && sudo docker compose ps) || echo "[경고] Compose 상태 조회 실패"
else
  echo "[경고] Compose 파일을 찾지 못했습니다: $compose_dir"
fi

for node in 1 2 3 4; do
  container="ieum-node$node"
  echo; echo "===== $container 컨테이너 ====="
  sudo docker inspect "$container" --format '상태={{.State.Status}} 시작={{.State.StartedAt}} 재시작={{.RestartCount}} 마운트={{range .Mounts}}{{.Source}}->{{.Destination}} {{end}}' 2>/dev/null || echo "[오류] 컨테이너 없음: $container"
done

external_script="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/diagnose-ieum-external.sh"
if [[ -x "$external_script" ]]; then
  echo; "$external_script" -H "$rpc_host" -p "$ports_csv"
else
  echo "[오류] 외부 RPC 진단 도구가 없습니다: $external_script"
fi

echo; echo "===== 설정 파일 hash ====="
for file in genesis.json validators.json upgrades.json events.json; do
  echo "--- $file ---"
  paths=()
  for node in 1 2 3 4; do paths+=("/opt/ieum-node$node/config/$file"); done
  sha256sum "${paths[@]}" 2>&1 || true
done
echo "[안내] network.json과 bootstrap.json은 노드별 PeerId·포트 때문에 hash가 다른 것이 정상입니다."

echo; echo "===== P2P·동기화 로그 판정 ====="
for node in 1 2 3 4; do
  container="ieum-node$node"
  echo "--- $container ---"
  log="$(sudo docker logs --since "$log_since" "$container" 2>&1 || true)"
  peer_id="$(sed -n 's/^IEUM 서버 노드 시작: //p' <<<"$log" | tail -1)"
  [[ -n "$peer_id" ]] && echo "[정상] PeerId=$peer_id" || echo "[경고] 시작 PeerId 로그 없음"
  if grep -q '\[P2P 전파 대기\]' <<<"$log"; then
    echo "[오류] P2P 연결은 있어도 gossipsub 토픽 가입 피어가 없어 메시지가 폐기됩니다."
  fi
  if grep -q '\[P2P 토픽 연결\]' <<<"$log"; then
    echo "[정상] bootstrap 연결 피어가 토픽 전파 대상으로 등록됐습니다."
  else
    echo "[경고] 토픽 연결 로그가 없습니다. 실행 바이너리와 P2P 연결을 확인하세요."
  fi
  if grep -q '\[동기화 직접 응답 완료\]' <<<"$log"; then
    echo "[정상] v1.0.2.1 직접 동기화 응답 경로가 작동했습니다."
  elif grep -q '\[동기화 직접 \(요청\|수신\|응답\) 실패\]' <<<"$log"; then
    echo "[오류] 직접 동기화 request-response 경로에서 실패가 발생했습니다."
  else
    echo "[안내] 최근 로그에 직접 동기화 응답 기록이 없습니다. 높이가 같으면 정상일 수 있습니다."
  fi
  grep -E '\[동기화 (요청|요청 수신|응답 수신|완료)|\[동기화 직접|\[동기화 교차검증\]|\[P2P 전파 대기\]' <<<"$log" | tail -n 40 || true
  if grep -q 'curl 실행 실패: No such file or directory' <<<"$log"; then
    echo "[오류] 컨테이너에 curl이 없어 자동 업데이트가 실패합니다."
  fi
done

echo; echo "===== 실서버 진단 종료 ====="

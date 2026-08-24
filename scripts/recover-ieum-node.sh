#!/usr/bin/env bash
set -Eeuo pipefail

mode=auto
compose_dir=/opt/ieum-docker-four-node
rpc_host=
target_node=
source_node=
install_dir=/opt/ieum-chain
service_name=ieum-chain.service

usage() {
  cat <<'EOF'
IEUM 노드 원클릭 복구 도구 v1.0.3.1

사용법:
  sudo recover-ieum-node.sh [옵션]

기본 동작:
  Docker 4노드 서버를 발견하면 뒤처지거나 재시작 중인 노드를 자동 선택하고,
  다수 노드와 같은 상태인 정상 노드 원장을 백업·복제합니다.
  Docker 구성이 없으면 systemd 단일 노드의 키·설정을 보존하고 원장만 초기화해
  P2P 재동기화를 시작합니다.

옵션:
  -h, --help             사용법 출력
  --docker               Docker 4노드 복구
  --systemd              systemd 단일 노드 복구
  -c, --compose-dir DIR  Compose 경로(기본: /opt/ieum-docker-four-node)
  -H, --rpc-host HOST    RPC 호스트(기본: 127.0.0.1)
  -n, --node N           복구 대상 Docker 노드(1~4, 생략 시 자동 선택)
  --from-node N          복제 원본 Docker 노드(1~4, 생략 시 자동 선택)
  -d, --install-dir DIR  단일 노드 설치 경로(기본: /opt/ieum-chain)
  -s, --service NAME     systemd 서비스(기본: ieum-chain.service)

보존 항목:
  config 전체, validator.key, server.node.key, data/keys 전체

변경 항목:
  data/ledger만 날짜가 붙은 백업으로 이동한 뒤 정상 원장을 복제하거나 P2P로 재구성

예:
  sudo ./scripts/recover-ieum-node.sh
  sudo ./scripts/recover-ieum-node.sh --docker --node 1 --from-node 2
  sudo ./scripts/recover-ieum-node.sh --systemd -d /opt/ieum-chain
EOF
}

die() { echo "[중단] $*" >&2; exit 1; }
log() { echo "[$1] ${*:2}"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --docker) mode=docker; shift ;;
    --systemd) mode=systemd; shift ;;
    -c|--compose-dir) compose_dir="${2:?값이 필요합니다}"; shift 2 ;;
    -H|--rpc-host) rpc_host="${2:?값이 필요합니다}"; shift 2 ;;
    -n|--node) target_node="${2:?값이 필요합니다}"; shift 2 ;;
    --from-node) source_node="${2:?값이 필요합니다}"; shift 2 ;;
    -d|--install-dir) install_dir="${2:?값이 필요합니다}"; shift 2 ;;
    -s|--service) service_name="${2:?값이 필요합니다}"; shift 2 ;;
    *) die "알 수 없는 옵션: $1 (-h로 사용법 확인)" ;;
  esac
done

[[ $EUID -eq 0 ]] || die "sudo로 실행하세요."
for command_name in curl python3 sha256sum; do
  command -v "$command_name" >/dev/null || die "$command_name 명령이 없습니다."
done

if [[ "$mode" == auto ]]; then
  if command -v docker >/dev/null && [[ -d "$compose_dir" ]] && docker inspect ieum-node1 >/dev/null 2>&1; then
    mode=docker
  else
    mode=systemd
  fi
fi

if [[ "$mode" == docker && -r "$compose_dir/.env" ]]; then
  # shellcheck disable=SC1090
  source "$compose_dir/.env"
fi
rpc_host="${rpc_host:-${IEUM_RPC_HOST:-127.0.0.1}}"

rpc_status() {
  local port="$1"
  curl -fsS --max-time 4 \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"ieum_nodeStatus","params":[]}' \
    "http://$rpc_host:$port"
}

recover_docker() {
  command -v docker >/dev/null || die "docker 명령이 없습니다."
  [[ -d "$compose_dir" ]] || die "Compose 경로가 없습니다: $compose_dir"
  local ports=(0 8989 8990 8991 8992)
  local report timestamp source_ledger target_ledger source_version target_version
  report="$(mktemp)"
  trap 'rm -f -- "$report"' RETURN

  for node in 1 2 3 4; do
    status="$(rpc_status "${ports[$node]}" 2>/dev/null || true)"
    running="$(docker inspect -f '{{.State.Running}}' "ieum-node$node" 2>/dev/null || echo false)"
    restarting="$(docker inspect -f '{{.State.Restarting}}' "ieum-node$node" 2>/dev/null || echo false)"
    STATUS="$status" NODE="$node" RUNNING="$running" RESTARTING="$restarting" python3 - <<'PY' >>"$report"
import json, os
try:
    value = (json.loads(os.environ["STATUS"]).get("result") or {})
except Exception:
    value = {}
print("\t".join([
    os.environ["NODE"], os.environ["RUNNING"], os.environ["RESTARTING"],
    str(value.get("height", -1)), str(value.get("blockHash", "")),
    str(value.get("stateRoot", "")), str(value.get("syncing", "unknown")),
]))
PY
  done

  log 진단 "노드\t실행\t재시작\t높이\t블록해시\t상태루트\t동기화"
  cat "$report"

  selection="$(REPORT="$report" TARGET="$target_node" SOURCE="$source_node" python3 - <<'PY'
import collections, os
rows=[]
for line in open(os.environ["REPORT"], encoding="utf-8"):
    node,running,restarting,height,block_hash,state_root,syncing=line.rstrip("\n").split("\t")
    rows.append(dict(node=int(node), running=running=="true", restarting=restarting=="true",
                     height=int(height), block_hash=block_hash, state_root=state_root, syncing=syncing))
healthy=[r for r in rows if r["running"] and not r["restarting"] and r["height"] >= 0 and r["syncing"] == "False"]
groups=collections.defaultdict(list)
for row in healthy:
    groups[(row["height"],row["block_hash"],row["state_root"])].append(row)
majority=max(groups.values(), key=lambda g:(len(g),g[0]["height"]), default=[])
if len(majority) < 2:
    raise SystemExit("정상 상태가 같은 원본 노드가 2개 이상 없습니다.")
source=int(os.environ["SOURCE"] or majority[0]["node"])
if source not in [r["node"] for r in majority]:
    raise SystemExit("지정한 원본 노드가 다수 정상 상태와 일치하지 않습니다.")
target_text=os.environ["TARGET"]
if target_text:
    target=int(target_text)
else:
    candidates=[r for r in rows if r["node"] not in [m["node"] for m in majority]]
    if not candidates:
        raise SystemExit("네 노드가 이미 같은 정상 상태입니다. 복구할 대상이 없습니다.")
    target=candidates[0]["node"]
if target == source or target not in range(1,5):
    raise SystemExit("복구 대상과 원본 노드 선택이 올바르지 않습니다.")
print(target, source)
PY
)" || die "$selection"
  read -r target_node source_node <<<"$selection"
  log 선택 "복구 Node $target_node · 정상 원본 Node $source_node"

  source_ledger="/opt/ieum-node$source_node/data/ledger"
  target_ledger="/opt/ieum-node$target_node/data/ledger"
  [[ -d "$source_ledger" ]] || die "정상 원본 원장이 없습니다: $source_ledger"
  [[ -d "$(dirname "$target_ledger")" ]] || die "복구 대상 data 경로가 없습니다."

  source_version="$(docker exec "ieum-node$source_node" /node/ieum-chain --version 2>/dev/null | awk '{print $NF}' || true)"
  target_version="$(docker run --rm --entrypoint /image/ieum-chain ieum-chain-local:latest --version 2>/dev/null | awk '{print $NF}' || true)"
  [[ -n "$source_version" && -n "$target_version" ]] || die "실행 버전을 확인하지 못했습니다."
  [[ "$source_version" == "$target_version" ]] || die "원본($source_version)과 재기동 이미지($target_version) 버전이 다릅니다."

  timestamp="$(date +%Y%m%d-%H%M%S)"
  cd "$compose_dir"
  docker update --restart=no "ieum-node$target_node" >/dev/null 2>&1 || true
  docker compose stop -t 30 "node$target_node" "node$source_node"
  if [[ -e "$target_ledger" ]]; then
    mv -- "$target_ledger" "${target_ledger}.before-recovery-$timestamp"
  fi
  cp -a -- "$source_ledger" "$target_ledger"
  sync
  docker compose up -d --no-deps "node$source_node"
  source_ready=false
  for _ in $(seq 1 60); do
    if rpc_status "${ports[$source_node]}" >/dev/null 2>&1; then
      source_ready=true
      break
    fi
    sleep 2
  done
  [[ "$source_ready" == true ]] || die "원본 Node $source_node 재기동 확인 실패"
  docker compose up -d --no-deps --force-recreate "node$target_node"

  for _ in $(seq 1 60); do
    if target_status="$(rpc_status "${ports[$target_node]}" 2>/dev/null)"; then
      TARGET_STATUS="$target_status" SOURCE_STATUS="$(rpc_status "${ports[$source_node]}")" python3 - <<'PY' && {
import json, os
a=json.loads(os.environ["TARGET_STATUS"])["result"]
b=json.loads(os.environ["SOURCE_STATUS"])["result"]
assert (a["height"],a["blockHash"],a["stateRoot"]) == (b["height"],b["blockHash"],b["stateRoot"])
PY
        log 완료 "Node $target_node 복구 완료 · 키/설정 보존 · 기존 원장 ${target_ledger}.before-recovery-$timestamp"
        return 0
      }
    fi
    sleep 2
  done
  die "Node $target_node가 120초 안에 정상 원본 상태와 일치하지 않았습니다."
}

recover_systemd() {
  command -v systemctl >/dev/null || die "systemctl 명령이 없습니다."
  [[ -d "$install_dir/config" ]] || die "설정 경로가 없습니다: $install_dir/config"
  [[ -d "$install_dir/data" ]] || die "데이터 경로가 없습니다: $install_dir/data"
  [[ -f "$install_dir/data/server.node.key" || -d "$install_dir/data/keys" ]] ||
    die "보존할 노드 키를 찾지 못했습니다. 원장을 초기화하지 않습니다."
  local ledger="$install_dir/data/ledger"
  local timestamp="$(date +%Y%m%d-%H%M%S)"
  systemctl stop "$service_name"
  if [[ -e "$ledger" ]]; then
    mv -- "$ledger" "${ledger}.before-recovery-$timestamp"
  fi
  install -d -m 750 "$ledger"
  systemctl start "$service_name"
  log 완료 "키·설정을 보존하고 빈 원장에서 P2P 재동기화를 시작했습니다."
  log 백업 "${ledger}.before-recovery-$timestamp"
  log 확인 "journalctl -u $service_name -f"
}

case "$mode" in
  docker) recover_docker ;;
  systemd) recover_systemd ;;
  *) die "지원하지 않는 모드입니다: $mode" ;;
esac

#!/usr/bin/env bash
set -Eeuo pipefail

# IEUM Chain 4노드 무중단 순차 업데이트 스크립트
#
# 주요 안전장치
#   1. GitHub 최신 Release의 정확한 태그와 SHA-256을 확인합니다.
#   2. 실행 중인 Docker 노드 외의 프로세스가 7001~7004를 점유하면 중단합니다.
#   3. 후보 이미지를 별도 태그로 빌드하고 버전을 검증합니다.
#   4. 노드를 한 대씩 정지하고 P2P 포트가 해제된 뒤 교체합니다.
#   5. Chain ID, Genesis, 버전, 동기화, 피어 수를 모두 확인합니다.
#   6. 한 대라도 실패하면 이번 실행에서 교체한 모든 노드를 이전 이미지로 복구합니다.

[[ "${EUID}" -eq 0 ]] || {
  echo "sudo $0 로 실행하세요." >&2
  exit 1
}

STACK_DIR="${IEUM_STACK_DIR:-/opt/ieum-docker-four-node}"
COMPOSE_FILE="$STACK_DIR/compose.yml"
LATEST_IMAGE="ieum-chain-local:latest"
PREVIOUS_IMAGE="ieum-chain-local:previous"

# IEUM 운영망 고정값입니다. 다른 체인의 바이너리나 데이터가 섞이면 중단합니다.
EXPECTED_CHAIN_ID=21004
EXPECTED_GENESIS="0x82cfc3615112766f3eb151a8677890c1b74ce6bce8463a1a3590991c383650f6"

services=(node1 node2 node3 node4)
containers=(ieum-node1 ieum-node2 ieum-node3 ieum-node4)
p2p_ports=(7001 7002 7003 7004)
rpc_ports=(8989 8990 8991 8992)

die() {
  echo "[중단] $*" >&2
  exit 1
}

[[ -r "$STACK_DIR/.env" ]] || die "$STACK_DIR/.env 파일이 없습니다."
[[ -r "$COMPOSE_FILE" ]] || die "$COMPOSE_FILE 파일이 없습니다."
[[ -r "$STACK_DIR/Dockerfile" ]] || die "$STACK_DIR/Dockerfile 파일이 없습니다."
[[ -r "$STACK_DIR/entrypoint.sh" ]] || die "$STACK_DIR/entrypoint.sh 파일이 없습니다."

# shellcheck disable=SC1090
source "$STACK_DIR/.env"
RPC_HOST="${IEUM_RPC_HOST:?IEUM_RPC_HOST가 .env에 없습니다}"

for command_name in curl sha256sum docker python3 ss sed sort; do
  command -v "$command_name" >/dev/null || die "$command_name 명령이 없습니다."
done
docker compose version >/dev/null

# compose.yml의 서비스명과 이미지명이 예상과 같은지 확인합니다.
for service in "${services[@]}"; do
  docker compose -f "$COMPOSE_FILE" config --services |
    grep -Fxq "$service" || die "compose.yml에 $service 서비스가 없습니다."
done
docker compose -f "$COMPOSE_FILE" config --images |
  grep -Fxq "$LATEST_IMAGE" || die "compose.yml이 $LATEST_IMAGE 이미지를 사용하지 않습니다."

# 지정한 UDP 포트를 사용 중인 PID를 중복 없이 출력합니다.
udp_owner_pids() {
  local port="$1"
  ss -H -lunp "sport = :$port" 2>/dev/null |
    sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p' |
    sort -u
}

# 업데이트 시작 전에 각 포트를 해당 Docker 컨테이너만 점유하는지 검사합니다.
# 예전 systemd IEUM 서비스가 남아 있으면 여기서 발견하고 안전하게 중단합니다.
check_existing_port_owners() {
  local i container expected_pid port owners owner
  for i in "${!containers[@]}"; do
    container="${containers[$i]}"
    port="${p2p_ports[$i]}"
    expected_pid="$(docker inspect -f '{{.State.Pid}}' "$container" 2>/dev/null || true)"
    [[ "$expected_pid" =~ ^[1-9][0-9]*$ ]] ||
      die "$container 컨테이너가 실행 중이 아닙니다. 먼저 현재 클러스터를 정상화하세요."

    owners="$(udp_owner_pids "$port")"
    [[ -n "$owners" ]] || die "UDP $port 포트를 사용하는 프로세스가 없습니다."
    while IFS= read -r owner; do
      [[ "$owner" == "$expected_pid" ]] || {
        echo "UDP $port 예상 PID: $expected_pid ($container)" >&2
        echo "UDP $port 실제 추가 PID: $owner" >&2
        ps -fp "$owner" >&2 || true
        die "구형 systemd 서비스 또는 다른 IEUM 프로세스가 포트를 중복 사용합니다."
      }
    done <<< "$owners"
  done
}

# JSON-RPC 호출 함수입니다.
rpc() {
  local port="$1" method="$2"
  curl -fsS --max-time 5 \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":[]}" \
    "http://$RPC_HOST:$port"
}

# 노드 하나가 운영망에서 정상 동작하는지 엄격하게 검사합니다.
node_healthy() {
  local container="$1" port="$2" expected_version="$3"
  local identity version status

  [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null)" == true ]] || return 1
  identity="$(rpc "$port" ieum_networkIdentity)" || return 1
  version="$(rpc "$port" ieum_protocolVersion)" || return 1
  status="$(rpc "$port" ieum_nodeStatus)" || return 1

  IDENTITY="$identity" VERSION="$version" STATUS="$status" \
    EXPECTED_VERSION="$expected_version" EXPECTED_GENESIS="$EXPECTED_GENESIS" \
    EXPECTED_CHAIN_ID="$EXPECTED_CHAIN_ID" python3 - <<'PY'
import json
import os
import sys

identity = json.loads(os.environ["IDENTITY"]).get("result") or {}
version = json.loads(os.environ["VERSION"]).get("result") or {}
status = json.loads(os.environ["STATUS"]).get("result") or {}

healthy = (
    int(identity.get("chainId", -1)) == int(os.environ["EXPECTED_CHAIN_ID"])
    and str(identity.get("genesisHash", "")).lower() == os.environ["EXPECTED_GENESIS"].lower()
    and str(version.get("nodeVersion", "")).replace("-", ".") == os.environ["EXPECTED_VERSION"]
    and status.get("syncing") is False
    and int(status.get("peers", 0)) >= 2
)
sys.exit(0 if healthy else 1)
PY
}

# 최대 120초 동안 노드가 정상화되기를 기다립니다.
wait_healthy() {
  local container="$1" port="$2" expected_version="$3"
  local attempt
  for attempt in $(seq 1 60); do
    if node_healthy "$container" "$port" "$expected_version"; then
      return 0
    fi
    sleep 2
  done
  return 1
}

# 컨테이너를 정지한 뒤 P2P UDP 포트가 실제로 해제됐는지 확인합니다.
wait_port_released() {
  local port="$1" attempt owners
  for attempt in $(seq 1 30); do
    owners="$(udp_owner_pids "$port")"
    [[ -z "$owners" ]] && return 0
    sleep 1
  done
  echo "UDP $port 포트를 계속 점유하는 프로세스:" >&2
  owners="$(udp_owner_pids "$port")"
  while IFS= read -r owner; do
    [[ -n "$owner" ]] && ps -fp "$owner" >&2 || true
  done <<< "$owners"
  return 1
}

# 네 노드의 높이, 블록 해시, State Root가 모두 같은지 확인합니다.
cluster_consistent() {
  local statuses="" i status
  for i in "${!rpc_ports[@]}"; do
    status="$(rpc "${rpc_ports[$i]}" ieum_nodeStatus)" || return 1
    statuses+="$status"$'\n'
  done

  STATUSES="$statuses" python3 - <<'PY'
import json
import os
import sys

rows = [json.loads(line).get("result") or {} for line in os.environ["STATUSES"].splitlines() if line]
if len(rows) != 4:
    sys.exit(1)
reference = (rows[0].get("height"), rows[0].get("blockHash"), rows[0].get("stateRoot"))
consistent = (
    all((row.get("height"), row.get("blockHash"), row.get("stateRoot")) == reference for row in rows)
    and all(row.get("syncing") is False for row in rows)
    and all(int(row.get("peers", 0)) == 3 for row in rows)
)
sys.exit(0 if consistent else 1)
PY
}

# 높이가 다른 복구 대상 클러스터도 가장 낮은 tip까지 같은 정규 체인인지 확인합니다.
# 단순히 경고만 출력하고 진행하면 fork된 원장을 덮을 수 있으므로, 공통 블록이
# 확인되지 않으면 업데이트를 시작하지 않습니다.
cluster_has_common_prefix() {
  local statuses="" i status minimum_height expected_hash block block_hash
  for i in "${!rpc_ports[@]}"; do
    status="$(rpc "${rpc_ports[$i]}" ieum_nodeStatus)" || return 1
    statuses+="$status"$'\n'
  done
  minimum_height="$(STATUSES="$statuses" python3 - <<'PY'
import json
import os

rows = [json.loads(line).get("result") or {} for line in os.environ["STATUSES"].splitlines() if line]
if len(rows) != 4 or any(row.get("height") is None for row in rows):
    raise SystemExit(1)
print(min(int(row["height"]) for row in rows))
PY
)" || return 1

  expected_hash=""
  for port in "${rpc_ports[@]}"; do
    block="$(curl -fsS --max-time 5 \
      -H 'content-type: application/json' \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_getBlockByNumber\",\"params\":[\"$(printf '0x%x' "$minimum_height")\",false]}" \
      "http://$RPC_HOST:$port")" || return 1
    block_hash="$(BLOCK="$block" python3 - <<'PY'
import json
import os

value = json.loads(os.environ["BLOCK"]).get("result") or {}
block_hash = value.get("hash")
if not block_hash:
    raise SystemExit(1)
print(block_hash)
PY
)" || return 1
    if [[ -z "$expected_hash" ]]; then
      expected_hash="$block_hash"
    elif [[ "$block_hash" != "$expected_hash" ]]; then
      return 1
    fi
  done
}

# 현재 클러스터가 정상일 때만 업데이트를 시작합니다.
check_existing_port_owners
current_version="$(docker run --rm --entrypoint /image/ieum-chain \
  "$LATEST_IMAGE" --version | awk '{print $NF}')"
for i in "${!containers[@]}"; do
  wait_healthy "${containers[$i]}" "${rpc_ports[$i]}" "$current_version" ||
    die "${containers[$i]}의 업데이트 전 상태가 정상이 아닙니다."
done
if ! cluster_consistent; then
  cluster_has_common_prefix ||
    die "노드 높이가 다르고 가장 낮은 높이의 공통 블록도 일치하지 않습니다."
  echo "[경고] 노드 높이는 다르지만 가장 낮은 높이까지 공통 블록이 일치하여 복구 버전 업데이트를 계속합니다."
fi

# 임시 빌드 디렉터리는 종료 시 자동 삭제합니다.
ctx="$(mktemp -d)"
cleanup() { rm -rf -- "$ctx"; }
trap cleanup EXIT
cp -- "$STACK_DIR/Dockerfile" "$STACK_DIR/entrypoint.sh" "$ctx/"

# latest/download 리다이렉트 대신 정확한 Release 태그를 먼저 확인합니다.
release_json="$(curl -fsSL --retry 3 --retry-delay 2 \
  https://api.github.com/repos/c4ei/ieum-chain/releases/latest)"
release_tag="$(RELEASE_JSON="$release_json" python3 -c \
  'import json, os; print(json.loads(os.environ["RELEASE_JSON"])["tag_name"])')"
expected_version="${release_tag#v}"
[[ "$expected_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] ||
  die "GitHub 최신 Release 버전을 확인할 수 없습니다: $release_tag"

base_url="https://github.com/c4ei/ieum-chain/releases/download/$release_tag"
echo "GitHub IEUM Chain $release_tag 바이너리를 다운로드합니다."
curl -fL --retry 3 --retry-delay 2 \
  "$base_url/ieum-chain-linux-x86_64" \
  -o "$ctx/ieum-chain-linux-x86_64"
curl -fL --retry 3 --retry-delay 2 \
  "$base_url/ieum-chain-linux-x86_64.sha256" \
  -o "$ctx/ieum-chain-linux-x86_64.sha256"
(
  cd "$ctx"
  sha256sum --check ieum-chain-linux-x86_64.sha256
  chmod 755 ieum-chain-linux-x86_64
)

# 현재 이미지는 롤백용으로 보존하고 새 이미지는 후보 태그로 먼저 빌드합니다.
docker image inspect "$LATEST_IMAGE" >/dev/null 2>&1 || die "$LATEST_IMAGE 이미지가 없습니다."
docker tag "$LATEST_IMAGE" "$PREVIOUS_IMAGE"
previous_version="$current_version"

candidate_image="ieum-chain-local:candidate-$expected_version"
docker build --pull -t "$candidate_image" "$ctx"
actual_version="$(docker run --rm --entrypoint /image/ieum-chain \
  "$candidate_image" --version | awk '{print $NF}')"
[[ "$actual_version" == "$expected_version" ]] ||
  die "후보 이미지 버전($actual_version)이 Release($expected_version)와 다릅니다."
docker run --rm --entrypoint curl "$candidate_image" --version >/dev/null ||
  die "후보 이미지에 자동 업데이트용 curl이 없습니다. 새 Dockerfile을 적용하세요."

# --build-only는 실행 중인 노드와 latest 태그를 변경하지 않습니다.
if [[ "${1:-}" == "--build-only" ]]; then
  echo "후보 이미지 빌드 완료: $candidate_image"
  echo "실행 중인 네 노드와 $LATEST_IMAGE 태그는 변경하지 않았습니다."
  exit 0
fi

# 실제 업데이트에 사용할 latest 태그를 검증된 후보 이미지로 전환합니다.
docker tag "$candidate_image" "$LATEST_IMAGE"
updated=()

# 실패하면 이번 실행에서 이미 교체한 모든 노드를 이전 버전으로 복구합니다.
rollback() {
  local failed_service="$1" service i
  echo "[$failed_service] 업데이트 실패. 변경한 노드를 $previous_version으로 복구합니다." >&2
  docker logs --tail 200 "ieum-$failed_service" >&2 2>/dev/null || true
  docker tag "$PREVIOUS_IMAGE" "$LATEST_IMAGE"

  for service in "${updated[@]}"; do
    docker compose -f "$COMPOSE_FILE" up -d --no-deps --force-recreate "$service" || true
  done
  for i in "${!services[@]}"; do
    if [[ " ${updated[*]} " == *" ${services[$i]} "* ]]; then
      wait_healthy "${containers[$i]}" "${rpc_ports[$i]}" "$previous_version" ||
        echo "[위험] ${services[$i]} 자동 복구 후에도 정상화되지 않았습니다." >&2
    fi
  done
  exit 1
}

# 노드를 한 대씩 정지하고 포트 해제를 확인한 뒤 새 이미지로 교체합니다.
for i in "${!services[@]}"; do
  service="${services[$i]}"
  container="${containers[$i]}"
  p2p_port="${p2p_ports[$i]}"
  rpc_port="${rpc_ports[$i]}"

  echo "[$service] $previous_version -> $expected_version 순차 업데이트"
  updated+=("$service")

  docker compose -f "$COMPOSE_FILE" stop -t 30 "$service" || rollback "$service"
  wait_port_released "$p2p_port" || rollback "$service"
  docker compose -f "$COMPOSE_FILE" up -d --no-deps --force-recreate "$service" || rollback "$service"
  wait_healthy "$container" "$rpc_port" "$expected_version" || rollback "$service"

  echo "[$service] 포트·Chain ID·Genesis·버전·동기화·피어 확인 완료"
done

# 순차 교체가 끝난 뒤 네 노드가 같은 블록 상태로 모일 때까지 기다립니다.
consistent=0
for _ in $(seq 1 30); do
  if cluster_consistent; then
    consistent=1
    break
  fi
  sleep 2
done
[[ "$consistent" -eq 1 ]] || rollback "cluster-consistency"

echo "네 노드가 모두 IEUM Chain $expected_version으로 업데이트되었습니다."
docker compose -f "$COMPOSE_FILE" ps

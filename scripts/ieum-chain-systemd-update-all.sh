#!/usr/bin/env bash
set -euo pipefail

if [[ "${EUID}" -ne 0 ]]; then
    echo "sudo -E $0 [서비스명 ...] 으로 실행하세요." >&2
    exit 1
fi

manifest_url="${IEUM_UPDATE_MANIFEST_URL:?IEUM_UPDATE_MANIFEST_URL is required}"
release_public_key="${IEUM_RELEASE_PUBLIC_KEY:?IEUM_RELEASE_PUBLIC_KEY is required}"

if [[ "$#" -gt 0 ]]; then
    services=("$@")
else
    mapfile -t services < <(
        systemctl list-unit-files --type=service --no-legend 'ieum-chain*.service' 'ieum-node*.service' |
            awk '$2 != "masked" {print $1}' |
            sort -u
    )
fi

if [[ "${#services[@]}" -eq 0 ]]; then
    echo "업데이트할 IEUM systemd 서비스를 찾지 못했습니다." >&2
    exit 1
fi

declare -A updated_binaries=()

for service in "${services[@]}"; do
    exec_start="$(systemctl show "$service" -p ExecStart --value)"
    if [[ "$exec_start" =~ path=([^[:space:]\;]+) ]]; then
        binary_path="${BASH_REMATCH[1]}"
    else
        echo "$service: ExecStart 실행 파일을 확인할 수 없습니다." >&2
        exit 1
    fi
    if [[ "$exec_start" =~ --rpc-port[[:space:]]+([0-9]+) ]]; then
        rpc_port="${BASH_REMATCH[1]}"
    else
        echo "$service: --rpc-port를 확인할 수 없습니다." >&2
        exit 1
    fi
    if [[ ! -x "$binary_path" ]]; then
        echo "$service: 실행 파일이 없습니다: $binary_path" >&2
        exit 1
    fi

    echo "[$service] 중지 후 $binary_path 업데이트"
    systemctl stop "$service"
    if [[ -z "${updated_binaries[$binary_path]:-}" ]]; then
        if ! "$binary_path" update \
            --manifest-url "$manifest_url" \
            --release-public-key "$release_public_key"; then
            systemctl start "$service"
            exit 1
        fi
        updated_binaries[$binary_path]=1
    fi
    systemctl start "$service"

    healthy=0
    for _ in $(seq 1 30); do
        if curl --fail --silent --show-error \
            -H 'content-type: application/json' \
            --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
            "http://127.0.0.1:${rpc_port}" | grep -q '"result"'; then
            healthy=1
            break
        fi
        sleep 2
    done
    if [[ "$healthy" -eq 1 ]]; then
        echo "[$service] RPC 정상, 다음 노드를 업데이트합니다."
        continue
    fi

    systemctl stop "$service"
    if [[ -f "${binary_path}.previous" ]]; then
        cp --preserve=mode,ownership,timestamps "${binary_path}.previous" "$binary_path"
        systemctl start "$service"
        echo "[$service] RPC 실패로 이전 바이너리를 복구했습니다." >&2
    else
        echo "[$service] RPC 실패, 복구용 ${binary_path}.previous 파일도 없습니다." >&2
    fi
    exit 1
done

echo "모든 IEUM Chain 서비스를 순차 업데이트했습니다."

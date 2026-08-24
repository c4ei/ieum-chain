#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  echo "사용법: $0 <현재 태그 vMAJOR.MINOR.PATCH.DOC> [이전 태그]" >&2
}
[[ "${1:-}" == -h || "${1:-}" == --help ]] && { usage; exit 0; }
current="${1:-}"; previous="${2:-}"
[[ "$current" =~ ^v[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "[오류] 현재 태그 형식이 올바르지 않습니다: $current" >&2; usage; exit 2; }
if [[ -z "$previous" ]]; then echo true; exit 0; fi
[[ "$previous" =~ ^v[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "[오류] 이전 태그 형식이 올바르지 않습니다: $previous" >&2; exit 2; }
current_code="${current%.*}"
previous_code="${previous%.*}"
[[ "$current_code" != "$previous_code" ]] && echo true || echo false

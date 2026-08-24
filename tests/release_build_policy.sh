#!/usr/bin/env bash
set -Eeuo pipefail

test "$(bash scripts/should-build-release.sh v1.0.1.1)" = true
test "$(bash scripts/should-build-release.sh v1.0.1.1 v1.0.0.9)" = true
test "$(bash scripts/should-build-release.sh v1.0.1.2 v1.0.1.1)" = false
test "$(bash scripts/should-build-release.sh v2.0.0.1 v1.9.9.9)" = true
if bash scripts/should-build-release.sh v1.0.1 >/dev/null 2>&1; then
  echo "잘못된 3자리 태그가 허용됐습니다." >&2
  exit 1
fi
echo "릴리스 빌드 정책 검증 완료"

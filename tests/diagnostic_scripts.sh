#!/usr/bin/env bash
set -Eeuo pipefail

for script in scripts/diagnose-ieum-server.sh scripts/diagnose-ieum-external.sh scripts/ieum-cluster-tool.sh; do
  bash -n "$script"
  output="$(bash "$script" -h)"
  grep -q '사용법' <<<"$output"
  grep -q 'v1.0.2.1' <<<"$output"
done

grep -q 'restart N' <<<"$(bash scripts/ieum-cluster-tool.sh -h)"
grep -q 'reproduce BIN' <<<"$(bash scripts/ieum-cluster-tool.sh -h)"

if bash scripts/diagnose-ieum-external.sh >/tmp/ieum-external-no-host.log 2>&1; then
  echo "외부 진단 도구가 필수 -H 없이 성공했습니다." >&2
  exit 1
fi
grep -q '\-H RPC_HOST가 필요합니다' /tmp/ieum-external-no-host.log
echo "한국어 진단 쉘 회귀 검증 완료"

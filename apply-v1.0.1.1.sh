#!/usr/bin/env bash
set -Eeuo pipefail

target="${1:-.}"
test -f "$target/Cargo.toml" || { echo "[오류] IEUM Chain 저장소 루트가 아닙니다: $target" >&2; exit 2; }
grep -q 'name = "ieum-chain"' "$target/Cargo.toml" || { echo "[오류] ieum-chain 저장소가 아닙니다." >&2; exit 2; }
source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/files" && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT

mkdir -p "$temporary/history"
if [[ -d "$target/docs/end" ]]; then
  while IFS= read -r -d '' file; do
    relative="${file#"$target/"}"
    mkdir -p "$temporary/history/$(dirname "$relative")"
    cp -- "$file" "$temporary/history/$relative"
  done < <(find "$target/docs/end" -type f -print0)
fi
old_docs=(
  docs/SECOND_PHASE_OPERATION_READINESS_2026-08-13.md
  docs/VERSION_0.23.9_FOUNDATION_GENESIS.md
  docs/VERSION_0.23.10_BFT_VALID_ROUND_RECONSTRUCTION.md
  docs/VERSION_0.23.11_PENDING_TRANSACTION_DIAGNOSTICS.md
  docs/VERSION_0.23.12_PENDING_NONCE.md
  docs/VERSION_1.0.0.1_OPERATION_FINAL.md
)
for path in "${old_docs[@]}"; do
  if [[ -f "$target/$path" ]]; then
    mkdir -p "$temporary/history/$(dirname "$path")"
    cp -- "$target/$path" "$temporary/history/$path"
  fi
done

rm -rf -- "$target/docs/end"
mkdir -p -- "$target/docs/end"
archive="$target/docs/end/IEUM_CHAIN_HISTORY_BEFORE_1.0.1.1.md"
{
  echo '# IEUM Chain v1.0.1.1 이전 문서 통합 백업'
  echo
  echo '> 적용 시점의 docs/end 및 이전 버전 작업 문서를 하나로 합친 로컬 백업입니다. Git에는 포함하지 않습니다.'
  find "$temporary/history" -type f -print0 | sort -z | while IFS= read -r -d '' file; do
    echo; echo '---'; echo; echo "## ${file#"$temporary/history/"}"; echo; sed -n '1,100000p' "$file"
  done
} >"$archive"

for path in "${old_docs[@]}"; do rm -f -- "$target/$path"; done
cp -a -- "$source_dir/." "$target/"
chmod +x "$target/scripts/diagnose-ieum-server.sh" "$target/scripts/diagnose-ieum-external.sh" \
  "$target/scripts/should-build-release.sh" "$target/tests/diagnostic_scripts.sh" "$target/tests/release_build_policy.sh"

echo "[완료] IEUM Chain v1.0.1.1 변경분 적용"
echo "과거 문서: $archive"
echo "다음 단계: cargo fmt --all --check && cargo test --all-targets --all-features --locked"

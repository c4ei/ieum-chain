#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

[[ "$(git branch --show-current)" == "dev" ]] || {
  echo "[중단] dev 브랜치에서 실행하세요: git switch dev" >&2
  exit 1
}
[[ -z "$(git diff --cached --name-only)" ]] || {
  echo "[중단] 이미 stage된 파일이 있습니다. git status를 먼저 확인하세요." >&2
  exit 1
}

bash -n scripts/commit-push-pr-v1.0.3.1-sync-loop-fix.sh
bash tests/ci_regression_guard.sh
git diff --check

git add -- \
  src/main.rs tests/ci_regression_guard.sh \
  docs/VERSION_1.0.3.1_CHECKPOINT_P2P_RECOVERY.md \
  scripts/commit-push-pr-v1.0.3.1-sync-loop-fix.sh

git diff --cached --check
git commit -m "fix: stop snapshot sync request feedback loop"
git push origin dev

echo "[완료] 기존 PR #21이 갱신됐습니다."
echo "https://github.com/c4ei/ieum-chain/pull/21"
echo "CI 성공 전에는 병합하거나 v1.0.3.1 태그를 만들지 마세요."

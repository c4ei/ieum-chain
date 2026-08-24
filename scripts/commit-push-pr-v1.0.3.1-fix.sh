#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

[[ "$(git branch --show-current)" == "dev" ]] || {
  echo "[중단] dev 브랜치에서 실행하세요: git switch dev" >&2
  exit 1
}

if [[ -n "$(git diff --cached --name-only)" ]]; then
  echo "[중단] 이미 stage된 파일이 있습니다. 먼저 git status를 확인하세요." >&2
  exit 1
fi

expected_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
[[ "$expected_version" == "1.0.3-1" ]] || {
  echo "[중단] 예상 Cargo 버전은 1.0.3-1이지만 현재는 $expected_version 입니다." >&2
  exit 1
}

cargo fmt --all --check
bash tests/ci_regression_guard.sh
bash -n tests/four_process_node1_persistent_rejoin.sh
bash -n scripts/commit-push-pr-v1.0.3.1-fix.sh
git diff --check

git add -- \
  build.rs src/lib.rs src/main.rs \
  tests/ci_regression_guard.sh tests/four_process_node1_persistent_rejoin.sh \
  docs/VERSION_1.0.3.1_CHECKPOINT_P2P_RECOVERY.md \
  scripts/commit-push-pr-v1.0.3.1-fix.sh

git diff --cached --check
git commit -m "fix: separate sync peers from BFT quorum"
git push origin dev

echo "[완료] dev에 보완 커밋을 푸시했습니다. 기존 PR #21이 자동 갱신됩니다."
echo "https://github.com/c4ei/ieum-chain/pull/21"
echo "CI가 성공하기 전에는 병합하거나 v1.0.3.1 태그를 만들지 마세요."

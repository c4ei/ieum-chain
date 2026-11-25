#!/usr/bin/env bash
# IEUM Chain 릴리스: 버전 확인 → 커밋 → 브랜치 푸시 → PR(→ CI 대기 → merge) → main 태그 푸시 → 릴리스 빌드 대기
# 사용법 (VS Code 내장 터미널에서): bash scripts/release.sh 1.0.10.0 "안전 종료와 snapshot 서명 안정화"
# - Cargo.toml/Cargo.lock 버전이 다르면 자동으로 올린다. CHANGELOG.md에는 "## <버전> - 날짜" 항목이 먼저 있어야 한다.
# - gh CLI가 로그인돼 있으면 PR 생성·CI 대기·merge·릴리스 대기까지 자동, 없으면 PR merge 후 Enter만 누르면 된다.
set -euo pipefail

VER="${1:?버전을 넣어 주세요. 예: bash scripts/release.sh 1.0.10.0 \"요약\"}"
SUMMARY="${2:-}"
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "버전 형식은 1.0.10.0 처럼 네 자리여야 합니다."; exit 1; }

cd "$(git rev-parse --show-toplevel)"
IFS=. read -r a b c d <<<"$VER"
CARGO_VER="$a.$b.$c-$d"
TAG="v$VER"
BRANCH="release/$TAG"
TITLE="IEUM Chain $TAG${SUMMARY:+ — $SUMMARY}"
step() { printf '\n\033[1;36m▶ %s\033[0m\n' "$*"; }
has_gh() { command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; }

step "1/6 확인"
git fetch -q origin --tags
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then echo "태그 $TAG 가 이미 있습니다."; exit 1; fi
changelog_line="$(grep -m1 -n "^## $VER " CHANGELOG.md || true)"
[[ -n "$changelog_line" ]] || { echo "CHANGELOG.md에 '## $VER - 날짜' 항목을 먼저 써 주세요."; exit 1; }

cur="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [[ "$cur" != "$CARGO_VER" ]]; then
  echo "Cargo 버전 $cur → $CARGO_VER 로 올립니다."
  sed -i "0,/^version = \".*\"/s//version = \"$CARGO_VER\"/" Cargo.toml
  awk -v v="$CARGO_VER" 'prev ~ /^name = "ieum-chain"$/ && /^version = / {$0="version = \"" v "\""} {print; prev=$0}' \
    Cargo.lock > Cargo.lock.tmp && mv Cargo.lock.tmp Cargo.lock
fi
cargo fmt --all --check || { echo "cargo fmt 검사 실패: cargo fmt --all 실행 후 다시 시도하세요."; exit 1; }

step "2/6 커밋할 파일"
git status --short
read -r -p "위 파일을 $BRANCH 브랜치로 커밋할까요? [y/N] " ok
[[ "$ok" == [yY] ]] || { echo "중단했습니다."; exit 1; }
if git show-ref -q --verify "refs/heads/$BRANCH"; then git switch -q "$BRANCH"; else git switch -q -c "$BRANCH"; fi
git add -A
git diff --cached --quiet || git commit -q -m "$TITLE" -m "$(sed -n "$((${changelog_line%%:*}+1)),/^## /p" CHANGELOG.md | sed '/^## /d')"

step "3/6 브랜치 푸시"
git push -u origin "$BRANCH"

step "4/6 PR → CI → merge"
if has_gh; then
  body="$(sed -n "$((${changelog_line%%:*}+1)),/^## /p" CHANGELOG.md | sed '/^## /d')"
  gh pr view "$BRANCH" >/dev/null 2>&1 || gh pr create --base main --head "$BRANCH" --title "$TITLE" \
    --body "$body"
  # PR 직후에는 CI가 아직 등록되지 않아 "no checks reported"가 나오므로 등록될 때까지 기다립니다.
  for _ in $(seq 1 36); do
    checks="$(gh pr checks "$BRANCH" 2>&1 || true)"
    [[ "$checks" == *"no checks reported"* ]] || break
    sleep 5
  done
  gh pr checks "$BRANCH" --watch --fail-fast || { echo "CI 실패: GitHub에서 로그를 확인하세요."; exit 1; }
  gh pr merge "$BRANCH" --merge --delete-branch
else
  echo "브라우저에서 PR을 만들고, CI 통과 후 merge 해 주세요:"
  echo "  https://github.com/c4ei/ieum-chain/compare/main...$BRANCH?expand=1"
  read -r -p "merge를 마쳤으면 Enter " _
fi

step "5/6 main 최신화 후 태그 푸시"
git switch -q main
git pull -q --ff-only origin main
main_ver="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[[ "$main_ver" == "$CARGO_VER" ]] || { echo "main 버전($main_ver)이 $CARGO_VER 가 아닙니다. PR이 merge됐는지 확인하세요."; exit 1; }
git tag -a "$TAG" -m "$TITLE"
git push origin "$TAG"   # 이 태그 하나만 푸시 (예전 태그가 같이 올라가지 않게)

step "6/6 릴리스 빌드"
if has_gh; then
  sleep 20
  run_id="$(gh run list --workflow chain-release.yml --limit 1 --json databaseId -q '.[0].databaseId')"
  gh run watch "$run_id" --exit-status && echo "릴리스 $TAG 게시 완료. 이제 update-four-nodes.sh로 4노드를 업데이트하세요."
else
  echo "빌드 진행: https://github.com/c4ei/ieum-chain/actions/workflows/chain-release.yml"
  echo "Release에 $TAG 가 올라오면 sudo /opt/ieum-docker-four-node/update-four-nodes.sh 로 4노드를 업데이트하세요."
fi

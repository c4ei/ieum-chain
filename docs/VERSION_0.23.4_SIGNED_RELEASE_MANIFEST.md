# IEUM Chain v0.23.4 서명 Release manifest 자동화

## 변경 내용

- 표시 버전을 `0.23.4`로 올렸다.
- GitHub Release가 Linux, Windows, macOS 바이너리와 SHA-256을 생성한다.
- Linux 바이너리는 GLIBC 호환성을 높이기 위해 Ubuntu 22.04에서 빌드한다.
- GitHub Secret `IEUM_RELEASE_PRIVATE_KEY`로 `update-manifest.json`을 Ed25519 서명한다.
- workflow에서 서명 키가 `config/update.json`의 공개키와 일치하는지 확인한다.
- 생성 직후 공개키로 manifest 서명을 재검증한다.
- `config/update.json`은 GitHub 최신 Release의 manifest를 사용한다.
- 프로토콜 및 원장 형식 변경은 없다.

## 적용 후 검증

```bash
cargo check
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
git diff --check
```

`cargo check`는 `Cargo.lock`의 로컬 `ieum-chain` 패키지 버전을 `0.23.4`로 동기화한다.

## Release

PR을 `main`에 병합하고 CI 성공 후 실행한다.

```bash
git switch main
git pull --ff-only origin main
git tag -s v0.23.4 -m "IEUM Chain v0.23.4"
git push origin v0.23.4
```

완료 확인:

```bash
gh run list --repo c4ei/ieum-chain --workflow chain-release.yml --limit 3
gh release view v0.23.4 --repo c4ei/ieum-chain
curl -fsSL https://github.com/c4ei/ieum-chain/releases/latest/download/update-manifest.json | python3 -m json.tool
```

기존 노드는 `config/update.json`의 `manifest_url`을 새 Release URL로 한 번 갱신해야 한다. 이후 버전부터는 최신 서명 manifest를 자동 확인한다.

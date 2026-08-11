# IEUM Chain 0.21.13 변경 및 배포 기록

운영 로그 용량을 줄이고 블록 익스플로러 연동 계약을 검증하는 릴리스입니다.

## 변경 사항

- 블록 수신 로그에는 `PeerId`, 높이, 블록 해시, 거래 수, 시스템 이벤트 수만 남깁니다.
- 동일 블록 해시는 여러 피어에서 도착해도 최초 수신만 기록합니다.
- 중복 판정 메모리는 최근 4,096개 블록으로 제한합니다.
- raw transaction과 서명은 일반 운영 로그에 출력하지 않습니다.
- `eth_blockNumber`, `eth_getBlockByNumber`, `eth_getBlockByHash`,
  `eth_getTransactionByHash`, `eth_getTransactionReceipt` 기반 익스플로러 연동 계약을
  회귀 테스트로 보호합니다.

로그 변경은 출력 형식만 바꾸며 P2P 수신, 검증, 합의, 원장 저장 및 JSON-RPC 응답을
변경하지 않습니다. 따라서 익스플로러는 로그를 파싱하지 않고 JSON-RPC를 사용해야
합니다.

## 개발 서버 검사

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
./target/release/ieum-chain --version
```

정상 버전은 `ieum-chain 0.21.13`입니다.

## GitHub 커밋과 태그

전체 검사가 성공한 커밋에만 태그를 답니다.

```bash
git status --short
git add Cargo.toml Cargo.lock CHANGELOG.md src/main.rs src/network.rs src/rpc.rs \
  docs/IEUM_USER_MANUAL_FIRST_RELEASE.md docs/VERSION_0.21.13.md
git commit -m "IEUM Chain v0.21.13"
git push origin HEAD:main

git tag -a v0.21.13 -m "IEUM Chain v0.21.13"
git push origin v0.21.13
```

태그가 최신 `main`과 같은 커밋인지 확인합니다.

```bash
git fetch origin --tags
test "$(git rev-parse origin/main)" = "$(git rev-list -n 1 v0.21.13)" \
  && echo "main/tag 일치" \
  || echo "main/tag 불일치"
```

이미 로컬 태그가 있다는 오류가 나오면 무조건 덮어쓰지 말고 먼저 확인합니다.

```bash
git show --no-patch --decorate v0.21.13
git log -1 --oneline origin/main
```

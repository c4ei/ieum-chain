# IEUM Chain v0.23.12 변경분 적용

```bash
cd ~/www/ieum-chain
git switch dev
tar -xJf ~/다운로드/ieum-chain-v0.23.12-changed-only.tar.xz
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

검증 후 커밋·PR 병합하고 Cargo 버전이 `0.23.12`인지 확인한 다음 태그를 만듭니다.
Wallet v0.0.10.26보다 먼저 네 운영 노드에 배포하세요.

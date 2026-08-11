# IEUM Chain 0.21.11 변경분 적용

이 압축은 GitHub `c4ei/ieum-chain`의 현재 `0.21.10` 소스 위에
`0.21.11` 기본 계정 기능 완성을 적용합니다.

```bash
cd ~/www/ieum-chain
tar -xJf /다운로드경로/ieum-chain-v0.21.11-changed-files.tar.xz

cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
./target/release/ieum-chain --version
```

서비스 바이너리를 교체하기 전에 기존 `data/keys`와 `data/keystore`를 백업하세요.
빌드 성공 후 `docs/USER_MANUAL_0.21.11.md` 순서대로 계정 생성, import, 잔액,
송금, transaction, receipt를 확인합니다.

월렛은 이미 secp256k1 주소, EIP-155 raw 거래, 잔액·nonce·transaction·receipt RPC를
사용하므로 이번 변경에 소스 수정이나 버전 상승이 필요하지 않습니다.

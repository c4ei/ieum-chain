# IEUM Chain 0.21.11

기본 계정 운용을 완성한 호환성 릴리스입니다.

- CLI/RPC 다중 계정 keystore 경로 통일
- 주소가 포함된 Geth식 누적 파일명
- raw secp256k1 개인키 import
- 계정 잔액, IEUM 전송, 트랜잭션 및 receipt 조회 CLI
- 기존 keystore 파일 읽기 호환

빌드 검증:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

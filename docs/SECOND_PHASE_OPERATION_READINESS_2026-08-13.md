# IEUM Chain 2차 운영 전환 점검 — 2026-08-13

## 현재 판정

v0.22.5 소스와 GitHub Actions CI는 운영 후보로 사용할 수 있으나, 현재 번들
`config/genesis.json`은 `network_name`이 `ieum-devnet`이고 공개 개발키 주소 네 곳에
잔액이 있어 `--mainnet-strict` 검사를 통과하지 않습니다. 따라서 이를 그대로
메인넷 신규 운영망으로 선언해서는 안 됩니다.

기존 네트워크를 유지하는 운영이라면 genesis를 수정하지 말고 현재 hash
`0x497e04ac4faec01b78b57d3caef7951fca98b1928a1af558ea03a663aa622418`을 모든
Wallet·Manager·Explorer 설정에 고정합니다.

## 메인넷 전환 전에 결정할 항목

1. 기존 네트워크 유지인지 새 메인넷 genesis 생성인지 결정합니다.
2. 새 genesis라면 재단, 이용자, 노드 보상, 락업·베스팅 주소와 총량을 확정합니다.
3. 네 검증자 키를 서로 다른 장비에서 새로 생성하고 공개키만 genesis에 기록합니다.
4. 새 genesis의 원본, SHA-256, IEUM genesis hash와 전환 높이·시각을 별도 공개합니다.
5. 네 노드 모두 `--mainnet-strict` 시작 성공, 동일 identity, 2/3 초과 BFT 확정,
   인증 snapshot 생성·복구, RPC 장애조치와 롤백을 사전 운영망에서 검증합니다.
6. validator key, node key, 원장, 인증 snapshot, PostgreSQL을 분리 백업하고 실제 복구
   훈련을 완료합니다.

## 배포 전 확인 명령

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
./target/release/ieum-chain server --mainnet-strict --help
```

GitHub Actions `Rust CI` 성공만으로 운영 승인을 대신하지 않습니다. 실제 네 노드의
키·방화벽·Caddy/Cloudflare·백업·복구·모니터링은 배포 환경에서 별도 확인해야 합니다.

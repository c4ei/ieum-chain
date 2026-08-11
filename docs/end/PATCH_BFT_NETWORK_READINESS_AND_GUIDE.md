# BFT 네트워크 준비·재전파 안정화 후속 변경분

이 압축은 `ieum-chain_VERSION_0.21.7_bft_node_wallet_changes.tar.xz`까지 적용된 소스에 덮어쓰는 후속 변경분이다.

포함 파일:

- `src/main.rs`: 비제안자 거래 재전파를 거래별 2초 간격으로 제한
- `tests/four_process_network.sh`: RPC 준비 후 P2P 토폴로지 `3·1·1·1`을 확인하고 거래 제출
- `docs/CONSENSUS_PROCESS_AND_TROUBLESHOOTING.md`: 시작부터 블록 확정까지의 흐름과 장애 판정 가이드

검증 명령:

```bash
bash -n tests/four_process_network.sh
cargo test --release
bash tests/four_process_network.sh target/release/ieum-chain
```

마지막 두 명령은 Rust 도구체인이 설치된 환경 또는 GitHub Actions에서 실행한다.

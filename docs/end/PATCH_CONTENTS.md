# IEUM Chain v0.21.0 변경 파일

- `Cargo.toml`, `Cargo.lock`: v0.21.0
- `src/recovery.rs`, `src/lib.rs`: 거래 복구와 체크포인트 롤백 승인 기준 분리
- `src/rpc.rs`: 운영망 신원, 프로토콜, 동기화, 최종 확정, 복구 조회 RPC
- `CHANGELOG.md`, `docs/VERSION_0.21.0.md`: 요청·처리·검사 문서

적용 후 `cargo fmt --all`을 먼저 실행하고 문서의 전체 검사 명령을 실행하세요.

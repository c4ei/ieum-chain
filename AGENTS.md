# IEUM Chain agent guide

작업 전에 `docs/PROJECT_CONTINUITY.md`, `README.md`, 최신 `docs/VERSION_*`, `SECURITY.md`, `CONTRIBUTING.md`를 읽는다.

## 현재 기준

- 패키지: `ieum-chain`
- 소스 버전: `Cargo.toml`의 `1.0.5-1`
- 표시/태그: `1.0.5.1` / `v1.0.5.1`
- 메인넷: Chain ID `21004`, `ieum-mainnet`
- 최대 공급: `210,000,000 IEUM`
- 합의: PoS+BFT. RPC 편의나 UI 요구로 합의 검증을 약화하지 않는다.

## 필수 불변조건

- genesis, chain ID, genesis hash, 프로토콜 버전 변경은 네 노드·Wallet·Manager 공동 전환 없이 배포하지 않는다.
- 운영에서 `--allow-insecure-test-keys`를 사용하지 않는다.
- validator/server node key와 원장을 저장소·로그·AI 대화에 넣지 않는다.
- 거래는 잔액, pending nonce, 수수료, 서명, 영수증, 재시작 보존을 함께 검증한다.
- 동기화는 독립 피어 quorum, 인증된 snapshot/checkpoint, 뒤처진 노드 재합류 안전성을 보존한다.
- 금액은 `u128` 최소 단위와 정밀 JSON 처리를 유지한다.

## 변경 절차

1. 기존 `dev`를 `main`과 동기화하고 작은 브랜치/커밋으로 작업한다.
2. 동작 변경이면 Cargo 버전과 표시 버전을 마지막 자리 +1 한다.
3. `docs/VERSION_<display>_<TOPIC>.md`, README/CHANGELOG, 회귀 테스트를 함께 갱신한다.
4. Draft PR에서 CI를 통과한 뒤에만 `main`에 병합한다.
5. `main` CI 성공 후 `v<display>` annotated tag와 Release를 만든다.

## 필수 검증

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --locked
cargo build --locked
```

합의·P2P·원장·거래 변경은 `.github/workflows/rust-ci.yml`의 실제 4프로세스 BFT 및 재합류 검증도 통과해야 한다. 단위 테스트 성공만으로 운영 안전을 선언하지 않는다.

완료 보고에는 commit SHA, 버전 일치, 테스트, 운영 전환/롤백, 실제 운영 미확인 영역을 적는다.

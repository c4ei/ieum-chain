# IEUM Chain 0.21.8 #14 CI 회귀 방지

작성일: 2026-08-11

## 확인한 최신 상태

- 기준 커밋: `fd360464ee6087200e3ef1733605a79e5d39fd7f`
- GitHub Actions: Rust CI 실행 #65 성공
- 버전: Cargo `0.21.8`

## 반복 실패의 정확한 원인

4프로세스 테스트의 실패는 합의 엔진 실패가 아니었다. 실행 #64에서 네 노드는 높이
2와 같은 상태 루트까지 합의했고 송금도 반영했다. 그러나 고정 수신 주소가 개발
제네시스에서 이미 1 IEUM을 보유했는데 테스트가 송금 후 절대 잔액을 0.1 IEUM으로
가정했다. 실제 정상 잔액 1.1 IEUM을 실패로 판정해 종료 코드 1을 반환했다.

이전 반복 과정에서 함께 수정된 조건은 다음과 같다.

- 유휴 중 만료된 최초 합의 deadline을 거래 도착 시 다시 시작
- P2P 합의 메시지의 `u128` 금액을 십진 문자열로 직렬화
- 네 검증자를 완전 연결망으로 구성해 각 노드가 독립 피어 3개를 보유
- 송금 전 잔액을 기준으로 정확히 0.1 IEUM 증가했는지 검증

## 이번 작업

`tests/ci_regression_guard.sh`를 추가하고 Rust CI의 컴파일 전 단계에서 실행한다.
이 검증은 완전 연결망, 송금 전 잔액 수집, 증가분 기반 기대 잔액 계산과 네 노드
검증이 제거되거나 과거의 고정 절대 잔액 비교가 재도입되는 것을 차단한다.

실제 합의와 송금은 기존 `tests/four_process_network.sh`가 계속 검증한다.

## 검증 명령

```bash
bash tests/ci_regression_guard.sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
```

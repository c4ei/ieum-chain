# IEUM Chain v0.22.7 Sync quorum 작성자 식별 수정

## 확인된 운영 장애

- RPC 8989는 높이 1에서 멈췄습니다.
- RPC 8990·8991·8992는 동일한 높이 2, block hash, state root를 보유했습니다.
- 8989 로그는 주기 sync 응답을 받으면서도 계속 두 번째 독립 피어를 기다렸습니다.

## 원인

Gossipsub 메시지는 원 작성자 `message.source`와 마지막 전달자
`propagation_source`를 구분합니다. 기존 sync quorum은 마지막 전달자를 피어 신원으로
사용했습니다. 여러 노드의 응답이 같은 릴레이를 거치면 모두 한 피어의 투표로
덮어써져 quorum이 만들어지지 않았습니다.

## 수정

- sync 응답의 독립 피어 식별자를 서명된 원 작성자 `message.source`로 변경했습니다.
- v0.22.6의 5초 주기 sync 요청 및 두 피어 이상 동일 tip 검증은 그대로 유지합니다.
- 거래, 합의 및 원장 규칙은 변경하지 않습니다.

## 검증과 배포

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

운영 노드는 한 대씩 롤링 교체합니다. 8989 노드부터 v0.22.7로 재시작한 뒤 10초 안에
높이 2와 `0x18c9ba9e...0895` block hash로 동기화되는지 확인합니다.

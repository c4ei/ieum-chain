# IEUM Chain v0.22.6 주기 동기화·잔액 정합성 수정

## 장애 원인

운영 RPC 노드가 높이 1에 머문 동안 다른 노드와 Explorer는 높이 2를 확정했습니다.
기존 노드는 피어 연결 시점과 동기화 적용 직후에만 sync 요청을 보냈기 때문에, 연결을
유지한 상태에서 새 확정 블록 응답을 놓치면 `syncHighest`도 1로 남아
`readyForTransactions=true`를 잘못 반환할 수 있었습니다.

이 때문에 블록 2에서 송금이 확정되었는데도 해당 RPC의 `eth_getBalance`는 이전
`100 IEUM`을 반환했습니다. 올바른 확정 잔액은 원시값
`99999899999999979000`, 표시값 `99.9999 IEUM`입니다.

## 수정 내용

- 모든 노드가 5초마다 현재 높이 다음부터 sync 응답을 요청합니다.
- 응답은 기존과 동일하게 독립 피어 2개 이상의 height, block hash, state root가
  일치해야 적용합니다.
- 연결 재수립 없이도 놓친 확정 블록을 자동 복구합니다.

## 운영 적용 확인

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

롤링 재시작 후 `irpc.aah.name`에서 `ieum_finalizedBlock.height`가 다른 운영 노드 및
Explorer DB 높이와 일치하는지 확인합니다. 네 노드를 동시에 내리지 않습니다.

# IEUM Chain v0.23.1 — RPC 거래 해시 일관성

## 원인

eth_sendRawTransaction은 raw RLP의 Keccak 해시를 반환했지만 확정 블록과 eth_getTransactionByHash는 Transaction::id를 사용했습니다. 그 결과 Wallet이 보관한 해시로 Explorer와 길드 결제 검증을 조회할 수 없었습니다.

## 변경

- eth_sendRawTransaction 반환값을 원장 거래 ID로 통일했습니다.
- 확정 블록, eth_getTransactionByHash, Receipt, Wallet 최근 전송, Manager 인덱서가 같은 해시를 사용합니다.
- 합의 데이터와 기존 블록은 변경하지 않습니다.

## 운영 적용 순서

1. Chain v0.23.1을 모든 RPC 노드에 배포합니다.
2. Manager v0.3.16을 배포합니다.
3. Wallet v0.0.10.16을 배포합니다.

기존 Wallet이 보관한 raw 해시는 기존 확정 블록의 원장 해시와 다를 수 있습니다. Manager v0.3.16은 보내는 주소, 재단 주소, 금액, 미사용 여부가 정확히 일치하는 단일 결제를 찾아 기존 결제를 안전하게 호환합니다.

## 검증

    cargo fmt --all --check
    cargo test --all-targets --all-features --locked
    cargo clippy --all-targets --all-features --locked -- -D warnings

# IEUM Chain v0.22.2

- 읽기 전용 JSON-RPC `ieum_peerInfo` 추가
- 직접 연결 피어의 `peerId`, 원격 multiaddr/IP, 방향, 연결 수, 연결 시각/지속시간 제공
- 연결·종료 이벤트와 RPC 피어 스냅샷 동기화
- 기존 `net_peerCount`, `ieum_nodeStatus` 호환 유지

```bash
curl -sS -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"ieum_peerInfo","params":[],"id":1}' http://127.0.0.1:8989
```

응답은 `{ version, count, height, peers[] }` 구조이며 개인키나 지갑 정보는 노출하지 않습니다. 각 노드의 직접 연결만 반환하므로 Manager가 여러 노드 결과를 합쳐 전체 토폴로지를 구성합니다. RPC는 LAN 또는 접근제어된 프록시 뒤에서 운영하십시오.

검증: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test --all-targets --all-features --locked`, `cargo build --release --locked`.

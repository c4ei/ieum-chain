# IEUM Chain v0.23.11 — 대기 거래 조회와 진단

## 사용자가 체감한 문제

거래 제출에는 성공했지만 블록 생성이 멈추면 거래가 mempool에 남습니다. 이전 RPC는
확정 원장만 검색해 지갑에 “체인에서 확인되지 않음”으로 표시했습니다.

## 변경

- `eth_getTransactionByHash`가 확정 원장 다음으로 mempool도 검색합니다.
- 대기 거래는 `blockHash`, `blockNumber`, `transactionIndex`가 `null`이고
  `ieumPending`이 `true`입니다.
- `scripts/ieum-doctor.sh`는 운영망 신원, 버전, 동기화, 높이, mempool과 선택한 거래를
  점검합니다. 서버에서는 `--docker`를 붙이면 `ieum-node1`~`ieum-node4`의 최근 BFT
  로그도 읽습니다. 스크립트 자체는 노드를 재시작하지 않습니다.

```bash
./scripts/ieum-doctor.sh --rpc https://irpc.aah.name --tx 0x거래해시
sudo ./scripts/ieum-doctor.sh --rpc http://127.0.0.1:8989 --tx 0x거래해시 --docker
```

Release는 Ubuntu 사전검사가 모두 성공해야 Windows·macOS·Linux 빌드를 시작합니다.
다른 운영체제에서만 발생하는 패키징 문제까지 사전에 완전히 보장할 수는 없지만,
소스·테스트·버전 오류 때문에 긴 빌드를 낭비하는 경우는 먼저 차단합니다.


cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked

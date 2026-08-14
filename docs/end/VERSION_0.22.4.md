# IEUM Chain v0.22.4 변경 내역

## 변경 사항

- 제네시스 블록 높이는 `0`을 유지합니다.
- `config/genesis.json`의 `genesis_time`을 실제 0번 블록 timestamp에 적용합니다.
- 운영망 제네시스 시각을 `2026-08-06 00:00:00 KST` (`1785942000`)로 고정했습니다.
- 블록 생산 통계에서 제네시스→1번 구간을 제외합니다.
- `ieum_blockProductionStatus`가 `--block-time-ms` 설정값을 목표 주기로 사용합니다.

## 필수 적용 순서

이 변경은 제네시스 블록 해시를 바꾸므로 네 검증자에 같은 바이너리를 배포한 뒤 원장을 함께 초기화해야 합니다.

```bash
sudo systemctl stop ieum-chain ieum-node1 ieum-node2 ieum-node3

for dir in /opt/ieum-chain /opt/ieum-node1 /opt/ieum-node2 /opt/ieum-node3; do
  cd "$dir" || exit 1
  sudo -u dev ./ieum-chain --version || exit 1
  sudo -u dev ./ieum-chain node clean --yes || exit 1
done

sudo systemctl start ieum-chain ieum-node1 ieum-node2 ieum-node3
```

각 경로에서 `ieum-chain 0.22.4`가 출력되어야 합니다. 초기화 전 기존 `data/ledger`는 별도 경로에 백업하십시오.

## 검증

```bash
for port in 8989 8990 8991 8992; do
  curl -sS -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["0x0",true],"id":1}' \
    "http://192.168.1.148:$port"
  echo
done
```

네 RPC 모두 `number=0x0`, `timestamp=0x6a734ff0` 및 동일한 블록 해시를 반환해야 합니다.

## 빌드 검증

```bash
cargo fmt --all &&
cargo fmt --all --check &&
cargo clippy --all-targets --all-features --locked -- -D warnings &&
cargo test --all-targets --all-features --locked &&
cargo build --release --locked
```

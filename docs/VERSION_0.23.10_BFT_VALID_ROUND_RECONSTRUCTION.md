# IEUM Chain v0.23.10 — BFT valid-round 증명 복구

## 배경

2026-08-21 운영망에서 거래가 각 노드의 mempool에 들어간 뒤 검증자들이 서로 다른
라운드와 잠금값을 유지하면서 블록 높이가 1에서 진행하지 않는 상황이 관측됐다.
일부 노드는 `잠긴 값과 다른 제안이며 유효한 valid_round 증명이 없습니다`를
반복했고, 네 검증자를 함께 재시작한 뒤 정상 합의와 송금 확정을 확인했다.

## 변경 내용

- 새 라운드의 제안자는 runtime의 보조 캐시만 신뢰하지 않는다.
- BFT 코어가 검증한 prevote 기록에서 해당 높이·라운드·블록의 증명을 재구성한다.
- valid value가 있는데 블록 본문이나 증명을 복구하지 못하면 다른 블록을 제안하지
  않는다.
- 제안 보류 시 제안자가 비운 거래는 mempool에 즉시 되돌린다.
- 운영 로그에 `[BFT 안전 제안 보류]`와 구체적인 원인을 남긴다.

잠금값을 강제로 지우거나 확정 인증서를 삭제하지 않는다. 이는 BFT 안전성을
훼손할 수 있기 때문이다.

## 운영 적용

네 검증자는 반드시 동일한 v0.23.10 바이너리와 설정을 사용해야 한다. 한 서버의
Docker 검증자 네 대라면 이미지 빌드 후 함께 교체한다.

```bash
docker compose build --no-cache
docker compose up -d --force-recreate
```

외부 일반 노드는 검증자로 실행하지 않는다.

```bash
/opt/ieum-chain/ieum-chain --mode client
```

## 배포 후 확인

```bash
for node in ieum-node1 ieum-node2 ieum-node3 ieum-node4; do
  echo "===== $node ====="
  docker logs --since=10m "$node" 2>&1 |
    grep -E 'BFT 안전 제안 보류|BFT 제안 거부|블록 확정|라운드 변경' |
    tail -n 50
done
```

작은 금액을 한 번 전송한 뒤 영수증 `status`가 `0x1`인지 확인한다.

```bash
curl -fsS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_getTransactionReceipt","params":["거래해시"]}' \
  https://irpc.aah.name | jq .
```

## 로컬 검증

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
```

## Git 반영

```bash
git switch dev
git add -- Cargo.toml Cargo.lock CHANGELOG.md README.md \
  src/consensus_runtime.rs src/main.rs \
  docs/VERSION_0.23.10_BFT_VALID_ROUND_RECONSTRUCTION.md
git commit -m "fix: reconstruct BFT valid-round proof"
git push origin dev
```

PR을 `main`에 병합하고 CI 성공을 확인한 뒤 태그를 생성한다.

```bash
git switch main
git pull --ff-only origin main
git tag -a v0.23.10 -m "IEUM Chain v0.23.10"
git push origin v0.23.10
```

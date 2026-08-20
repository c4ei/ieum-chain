# IEUM Chain v0.23.9 — 재단 10% 최초 배분

## 사용자가 알아야 할 내용

- IEUM 최대 발행량은 `210,000,000 IEUM`입니다.
- 재단 주소 `0x356456ff1216b57a6f8891b195b42d296789b67d`에는 최초에
  `21,000,000 IEUM`이 배분됩니다.
- 다른 초기 운영 주소의 합계 `70,100 IEUM`을 포함한 제네시스 총발행량은
  `21,070,100 IEUM`입니다.
- 최대 발행량 대비 아직 발행하지 않은 수량은 `188,929,900 IEUM`입니다.
- 현재 PoS·노드·보유자 보상은 새 IEUM을 발행하지 않고 재단 준비금에서 지급합니다.
  따라서 현재 정책만 유지하면 총발행량은 증가하지 않으며 210,000,000 IEUM에
  자동으로 도달하지 않습니다.
- 과거 코드에 있던 `2026-08-10` 노드 신규 발행 시작값은 새 Genesis 시각인
  `2026-08-21 00:00 KST`로 이동했습니다. 이 신규 발행 모듈은 현재 블록 이벤트에
  연결되어 있지 않으므로 실제 발행은 일어나지 않습니다.
- 검증자 이자·위임 이자·보유 보상은 Genesis 당일 하루치를 선지급하지 않습니다.
  최초 지급 가능 시각은 `2026-08-22 00:00 KST`입니다.

## 재단 내부 관리 기준

| 구분 | 수량 |
| --- | ---: |
| 장기 준비금·PoS 보상 | 15,000,000 IEUM |
| 생태계·노드 운영 | 3,000,000 IEUM |
| 개발·보안·인프라 | 2,000,000 IEUM |
| 마케팅·상장·유동성 | 1,000,000 IEUM |
| 합계 | 21,000,000 IEUM |

내부 구분은 회계·운영 정책이며 온체인에는 재단 주소의 단일 잔액으로 기록됩니다.
용도별 온체인 분리가 필요하면 별도 주소 4개와 공개 배분 정책을 다음 업그레이드에서
도입해야 합니다.

## 왜 기존 원장을 초기화하는가

운영망은 이전 제네시스로 높이 5까지 진행됐습니다. 제네시스 파일만 바꾸면 기존
노드와 신규 노드의 Genesis hash가 달라져 서로 합의할 수 없습니다. v0.23.9를 모든
검증 노드에 동시에 배포하면 기존 원장을 삭제하지 않고
`ledger.pre-foundation-allocation-20260820`으로 옮긴 뒤 새 원장을 만듭니다.

## 4개 검증 노드 전환 순서

> 네 노드를 모두 준비한 뒤 같은 점검 시간에 실행하세요. 한 노드씩 서로 다른
> Genesis로 운영하면 안 됩니다.

1. 기존 노드 4개를 모두 중지합니다.
2. 각 노드의 `config`, `data`, 서비스 파일을 별도 저장장치에 백업합니다.
3. v0.23.9 바이너리와 `config/genesis.json`을 네 노드에 동일하게 배포합니다.
4. 기존 검증자 키와 노드 키는 그대로 유지합니다.
5. 네 노드를 시작합니다. 최초 실행 시 기존 원장은 자동으로 옆 폴더에 보존됩니다.
6. 네 노드에서 아래 값이 모두 같은지 확인합니다.

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"ieum_getNodeIdentity","params":[],"id":1}' \
  http://127.0.0.1:8989
```

- `chainId`: `21004`
- `genesisHash`: `0x82cfc3615112766f3eb151a8677890c1b74ce6bce8463a1a3590991c383650f6`

재단 잔액 확인:

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_getBalance","params":["0x356456ff1216b57a6f8891b195b42d296789b67d","latest"],"id":1}' \
  http://127.0.0.1:8989
```

기대값은 `21,000,000 × 10^18 wei`입니다.

## 검증과 배포

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

검증 후 `dev`에 커밋하고 PR로 `main`에 병합한 다음 태그를 생성합니다.

```bash
git switch dev
git add -- Cargo.toml Cargo.lock README.md CHANGELOG.md \
  config/genesis.json config/genesis_test.json \
  src/genesis.rs src/chain.rs src/installation.rs src/node_emission.rs \
  src/validator_interest.rs src/main.rs src/consensus_runtime.rs \
  docs/VERSION_0.23.9_FOUNDATION_GENESIS.md
git commit -m "feat: allocate foundation genesis supply v0.23.9"
git push origin dev
gh pr create --base main --head dev \
  --title "IEUM Chain v0.23.9 foundation genesis" --draft

# CI 성공 후 PR을 main에 병합하고 실행
git switch main
git pull --ff-only origin main
git tag -a v0.23.9 -m "IEUM Chain v0.23.9"
git push origin v0.23.9
```

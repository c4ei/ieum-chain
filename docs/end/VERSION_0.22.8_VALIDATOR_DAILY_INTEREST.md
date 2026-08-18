# IEUM Chain v0.22.8 PoS 검증자 일일 이자

## 원인 분석

기존 `src/node_emission.rs`에는 하루 단위 예산 계산이 있었지만 이는 일반 네트워크 노드의 트래픽 보상 계산 함수였습니다. 실제 블록 생성 경로에서 `settle_daily_rewards`가 호출되지 않았고, PoS 검증자의 보유 잔액을 기준으로 정산하는 이벤트도 없었습니다. 따라서 전체 공급량이 작아서 지급되지 않은 것이 아니라 **검증자 일일 이자 실행 경로가 연결되어 있지 않은 것**이 직접 원인입니다.

또한 기존 최초 검증자 보상은 검증자당 10 IEUM을 한 번만 지급합니다. 새 기본 최소 보유 기준은 1 IEUM이므로 현재처럼 공급량이 작은 초기 운영망에서도 대상이 됩니다.

## v0.22.8 동작

- KST 날짜를 기준으로 하루에 이벤트 ID 하나만 생성합니다.
- 직전 확정 높이의 잔액 snapshot과 활성 validator 집합을 사용합니다.
- 기본 연 이율은 500bps(5.00% APR), 일 단리 계산은 `잔액 × APR / 10,000 / 365`입니다.
- 최소 보유액 1 IEUM, 하루 전체 지급 상한 1,000 IEUM입니다.
- 지급 재원은 신규 무제한 발행이 아니라 재단 주소 잔액입니다. 따라서 `totalIssued`는 늘지 않으며 재단 잔액 부족 시 블록이 확정되지 않습니다.
- 정책 hash, snapshot 높이, 대상과 금액을 모든 검증자가 재계산해 다르면 블록을 거부합니다.

## 설정 명령

조회:

```bash
./ieum-chain validator-interest show
```

예: APR 7.5%, 최소 1 IEUM, 일일 상한 1,000 IEUM:

```bash
./ieum-chain validator-interest set \
  --annual-rate-bps 750 \
  --minimum-balance-ieum 1 \
  --maximum-daily-total-ieum 1000
```

`config/validator-interest.json`을 네 검증자에 동일하게 배포하고 동시에 재시작해야 합니다. 한 노드만 변경하면 policy hash가 달라져 해당 이자 블록에 합의하지 않습니다. 운영 중 즉시 변경하는 관리자 API는 키 탈취·단독 금리 조작 위험 때문에 넣지 않았습니다. `ieum-manager`에서는 이 명령으로 설정 파일을 생성하고, 4개 노드의 hash 일치 확인 후 일괄 배포·재시작하는 방식으로 연동하십시오.

## 관리자/RPC snapshot

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"ieum_validatorInterestStatus","params":[],"id":1}' \
  http://127.0.0.1:8989
```

응답에는 높이, 조회 시각, APR, 최소 보유액, 정책 hash, 대상 검증자 수, 다음 이벤트 ID, 예상 일 지급 총액과 주소별 금액이 포함됩니다. 기존 `ieum_supplyStatus`와 함께 조회하면 실제 총발행·유통·잠금 상태도 manager 화면에 표시할 수 있습니다.

`ieum_supplyStatus`의 공급량 필드는 관리자와 운영자가 바로 이해할 수 있는 ASCII
콩글리시 이름으로 통일했습니다.

| 필드 | 의미 |
| --- | --- |
| `Bal_All` | 전체 발행량 |
| `Bal_Utong_All` | 실제 유통량 |
| `Bal_Lock_All` | 잠금 물량 |
| `Bal_Genesis_All` | 최초 발행량 |
| `Bal_Ija_Paid_All` | 검증자 이자 지급 누계 |
| `Bal_Ija_Minted_All` | 이자로 신규 발행한 수량(재단 지급형이므로 현재 0) |
| `Bal_NodeReward_Minted_All` | 노드 보상 신규 발행량(현재 재단 지급형이므로 0) |
| `Bal_Foundation` | 현재 재단 주소 잔액 |

한글 식별자는 JSON에서는 가능하지만 SQL, shell, 환경변수, 메트릭 도구와의 호환성을
위해 사용하지 않습니다.

## 배포 전 점검

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

첫 배포일에는 네 노드의 `policyHash`, 체인 높이, validator 목록, 재단 잔액이 같은지 확인하십시오. 이 기능은 실제 stake/unstake 위임 상태가 아니라 현재 활성 validator ID가 보유한 온체인 잔액을 기준으로 합니다.

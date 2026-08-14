# IEUM Chain v0.23.0 — 실제 잠금형 이음 맡기기

## 무엇이 달라졌나

이번 버전부터 “이음 맡기기”는 단순 송금이나 화면 표시가 아닙니다.

- `delegate`: 유동 잔액에서 위임 원장으로 실제 잠금
- `undelegate`: 활성 위임에서 해제 대기 원장으로 이동
- `claim`: 해제 높이가 지난 금액만 유동 잔액으로 반환
- 해제 대기: 합의된 블록 시각 기준 604,800초(7일). 빈 블록을 만들지 않으므로 블록 수 기준으로 계산하지 않습니다.
- 최소 위임: 1 IEUM
- 위임 보상: 활성 위임액 × 설정 APR ÷ 365, 하루 한 번, 소수점 12자리 반올림
- 이중투표: 기존 서명 증거를 블록에 포함한 경우 활성·해제대기 위임액의 5%를 재단으로 이전
- 같은 증거 ID는 한 번만 적용

위임액은 `Bal_All`에서 사라지지 않고 `Bal_Lock_All`에 포함됩니다. 일반 지갑 보유 보상은 유동 잔액만 계산하고, 위임액은 별도 위임 보상으로 계산하여 중복 지급하지 않습니다. 보상은 신규 발행이 아니라 재단 잔액에서 지급됩니다.

## 안전한 프로토콜 v3 전환

이 버전은 거래 해시·상태 루트·스냅샷이 달라지는 합의 변경입니다. 4개 이음지기 모두 v0.23.0 바이너리를 먼저 설치하고, 같은 미래 활성화 높이를 `config/upgrades.json`에 넣어야 합니다. 일부 노드만 적용하거나 현재 높이 이하를 지정하면 체인이 멈출 수 있습니다.

현재 높이를 확인하고 최소 20블록 이후를 정합니다.

```bash
curl -sS https://irpc.aah.name -H 'Content-Type: application/json' \
 --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

예를 들어 공동 활성화 높이가 120이면 모든 노드에 동일하게 저장합니다.

```json
{"upgrades":[{"name":"staking-v1","activation_height":120,"protocol_version":3}]}
```

활성화 전에는 위임 거래를 제출하지 마세요. 기존 원장·키·설정 전체를 백업하고 한 대씩 바이너리를 교체한 뒤, 4대의 버전·높이·제네시스 해시·정책 파일 해시를 비교합니다. 해제 시각은 로컬 PC 시간이 아니라 BFT가 확정한 블록 timestamp로 판정합니다.

## RPC

```bash
# 전체 또는 특정 지갑 위임 상태
curl -sS https://irpc.aah.name -H 'Content-Type: application/json' \
 --data '{"jsonrpc":"2.0","method":"ieum_stakingStatus","params":["0x지갑주소"],"id":1}'

# MetaMask/ethers용 calldata 생성
curl -sS https://irpc.aah.name -H 'Content-Type: application/json' \
 --data '{"jsonrpc":"2.0","method":"ieum_encodeStakingCall","params":["delegate","이음지기64자리hex"],"id":1}'
```

반환된 `to`는 `0x0000000000000000000000000000000000021004`입니다. `delegate`와 `undelegate`는 value에 수량을 넣고, `claim`은 value를 0으로 보냅니다. 외부 지갑은 반드시 반환된 calldata를 포함한 EIP-155 legacy 거래에 서명해야 합니다.

## 거버넌스와 51% 안전 기준

위임액은 보상 회계와 페널티에만 사용하며 v0.23.0에서 합의 투표권을 자동 증가시키지 않습니다. 따라서 돈을 많이 위임받았다는 이유만으로 즉시 이음지기가 되거나 51% 투표권을 얻지 않습니다. 검증자 집합·투표권 변경은 기존 epoch 지연과 별도 거버넌스 승인 대상입니다.

향후 위임액을 투표권에 반영하려면 최대 검증자별 지분 상한, 신규 검증자 대기, 위임 집중도 제한, 거버넌스 타임락, 비상 중지와 외부 보안 감사를 먼저 추가해야 합니다.

## 빌드 검증

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

개인키·keystore·원장·DB·비밀번호 파일은 커밋하거나 압축에 넣지 않습니다.

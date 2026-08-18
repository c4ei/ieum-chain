# IEUM Chain v0.22.9 — 이음마당 보상

## 쉬운 요약

- **이음지기**: 블록을 확인하고 체인을 지키는 검증자입니다.
- **이음 맡기기**: 내 IEUM을 이음지기에게 맡겨 생태계에 참여하는 기능의 사용자 이름입니다. 이번 체인에는 잠금형 위임 거래가 아직 없으므로 화면에서 실제 위임처럼 표시하지 않습니다.
- **들고만 있어도 받는 응원 보상**: 설정된 이벤트 기간에 일반 `0x` 지갑의 스냅샷 잔액을 기준으로 하루 한 번 지급합니다.
- **이음마당**: `https://iem.aah.name`에서 공급량·보상·길드 현황을 보는 친근한 이름입니다.

예시: `99.9999 IEUM × 5% ÷ 365 = 0.013698616438 IEUM` (소수점 12자리 반올림).

## 설정

`config/holder-rewards.json`의 `enabled`, `starts_at`, `ends_at`, `annual_rate_bps`, `minimum_balance`, `maximum_daily_total`을 바꿉니다. 시작/종료는 Unix 초이며, 변경 후 모든 이음지기에 같은 파일을 배포하고 재시작해야 정책 해시가 일치합니다. 재단 주소는 제네시스 설정을 따르며 임의 변경하지 않습니다.

보상은 신규 발행이 아니라 **재단 잔액에서 서명된 합의 이벤트로 이전**됩니다. 서버가 재단 개인키를 보관하거나 임의 출금하지 않습니다. 이벤트 ID·정책 해시·스냅샷 높이·수령자별 계산을 모든 이음지기가 다시 검증합니다.

상태 확인:

```bash
curl -sS https://irpc.aah.name -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"ieum_holderRewardStatus","params":["0x내주소"],"id":1}'
```

## 후속 작업

실제 잠금형 위임은 `delegate`, `undelegate`, 해제 대기, 이중서명 페널티, 위임자별 회계와 거버넌스가 필요합니다. 별도 보안 감사와 합의 버전 업 없이 단순 잔액 이전으로 흉내 내면 안 됩니다.

## 검증

```bash
cargo fmt --check
cargo test --lib
cargo build --release
```

공개 저장소: [Chain](https://github.com/c4ei/ieum-chain) · [Wallet](https://github.com/c4ei/ieum-wallet) · [Manager](https://github.com/c4ei/ieum-manager)

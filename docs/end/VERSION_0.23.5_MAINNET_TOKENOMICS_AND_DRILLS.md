# IEUM Chain v0.23.5 — 메인넷·경제 구조·사고 대응

## 1. 메인넷 신원

- Chain ID: `21004`
- network name: `ieum-mainnet`
- Genesis time: `1787065200` (2026-08-19 00:00:00 KST)
- Genesis hash: `0xc7a4f99b113341db7705117dedb240bb3ea3b0b99c115d134ddf505be1ff8a5a`
- Genesis 공급량: `80,100 IEUM`
- 최대 공급량: `210,000,000 IEUM`
- Genesis 이후 신규 발행 가능 상한: `209,919,900 IEUM`
- 최소 단위: `1 IEUM = 10^18 wei`

`network_name`은 Genesis hash에 포함된다. 기존 `ieum-devnet` 원장을 유지한 채 설정만
바꾸면 새 노드와 기존 노드의 Genesis hash가 달라져 연결·인덱싱·Wallet 신원 검사가
실패한다. v0.23.5는 공개 개발 개인키로 알려진 네 주소의 4 IEUM도 Genesis에서
제거했으므로, 기존 원장을 메인넷으로 승격하는 패치가 아니라 **새 메인넷 Genesis로
동시에 시작하는 전환**이다. 전환 전에 기존 원장을 읽기 전용 보관하고, 네 검증자의
`data/ledger`, `validators.json`, Wallet 예상 hash와 Manager 예상 hash를 함께 맞춘다.
새 바이너리는 번들 Genesis를 `config/genesis.json`에 원자적으로 동기화하고 기존 파일을
`config/genesis.pre-mainnet-20260819.json`으로 한 번 백업한다. 기존 원장·월별 체크포인트의
Genesis hash가 새 메인넷과 다르면 시작을 중단한다.

GitHub Actions의 `--git-action-test`는 메인넷 Genesis를 수정하지 않고
`config/genesis_test.json`과 `config/validators_test.json`을 사용한다. CI Genesis에만 공개 개발키 주소 네 곳의
1 IEUM 송금 잔액이 존재한다. CI Chain ID는 `21005`로 메인넷 `21004`와 분리하며,
`--mainnet-strict` 검증을 통과할 수 없다.

## 2. 최초 배분 현황과 확정 절차

| 주소 | 수량 | 현재 확인 가능한 성격 | 메인넷 공개 전 필요한 조치 |
|---|---:|---|---|
| `0xAda04f6eA65dc31079825e47296d0737A4594696` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0xbcDf32f90E36d8D0883AC5aC8A46A7c575eAF507` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0x356456fF1216B57a6f8891B195b42d296789B67D` | 10,000 | 코드상 재단 수수료 주소 | 재단 준비금·보상 재원 정책 공시 |
| `0x28c1da651c61d88902883adcCc7Df0Ed2Ed8931D` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0x7ea8C617Ad2635fA7bCFbb66056C3280df0987f4` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0xc30de1Af9fF76455ecb6B827384381501EBFDC55` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0x13F3E36F5A1c24215BD910d01c567e6DD62D12b7` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0xc23104A7Dbd6C6616251728018bA4106D57a154b` | 10,000 | 초기 배분 주소 | 실소유자·용도·락업 공시 |
| `0x475e2f4e40Dbd34370e4fce61ddFF5Ff1F2eA817` | 100 | 소액 운영 주소 | 운영 목적과 책임자 공시 |

주소 이름을 코드만 보고 임의로 재단·개발자·마케팅으로 확정하면 허위 공시가 된다.
운영자는 서명으로 각 주소 소유권을 증명한 뒤 아래 권장 구분표를 최종 확정한다.

| 권장 구분 | 권장 원칙 |
|---|---|
| 재단 준비금 | 다중 승인, 월별 잔액·사용내역 공개 |
| 생태계·노드 보상 | 연간 예산과 일일 상한 공개 |
| 신규 사용자·마케팅 | 캠페인별 예산, 중복 수령 방지, 지급 내역 공개 |
| 개발·운영 | 24~48개월 선형 락업 권장 |
| 유동성·시장조성 | 거래소·위탁자·회수 조건 공개, 자전거래 금지 |

전역 최대 공급량은 정확히 `210,000,000 IEUM`이다. 메인넷 Genesis `80,100 IEUM`을
제외한 향후 신규 발행 가능 상한은 `209,919,900 IEUM`이다. 검증자·일반 보유자·위임
보상은 현재 구현상 신규 발행이 아니라 재단 잔액에서 지급되므로 잔여 발행량을 줄이지
않는다. 공급 RPC는 최대 공급량과 현재 총공급량 기준 잔여 발행 가능량을 함께 반환한다.

## 3. 기간형 일반 보유 보상

Manager는 기간이 겹치지 않는 이벤트를 작성·승인·활성화할 수 있다. Chain은 한 번에
하나의 `config/holder-rewards.json` 정책만 읽는다. Manager에서 활성 상태로 바꾼 것만으로
원격 검증자 파일이 자동 변경되지는 않는다. 반환된 JSON을 모든 검증자에 동일하게
배포하고 정책 hash를 확인한 뒤 순차 재시작해야 한다.

확정 지급은 `ieum_holderRewardHistory` RPC로 event ID, 블록 높이/hash, 지급 시각,
snapshot 높이, 주소와 금액을 조회한다. IP와 국가는 합의·원장에 필요하지 않은 개인정보라
Chain에 저장하지 않는다. 별도 참여 신청에서 수집한다면 동의·보존기한·접근권한을 둔다.

## 4. 최초 참여 보상 기본안

| 행동 | 기본 보상 | 검증 방식 |
|---|---:|---|
| 지갑 설치·주소 생성 | 0.01 IEUM | 신규 주소·계정·기기 중복 제한 |
| 첫 온체인 송금 완료 | 0.01 IEUM | 확정 거래 hash와 최초 nonce 확인 |
| 실제 노드 24시간 운영 | 0.01 IEUM | 서명 PeerId, 24시간 관측, 가동률 확인 |
| 검증된 오류 신고 | 난이도별 | 중복 제외 후 영향도·재현성 심사 |
| SNS 체험 후기 | 추첨 또는 소액 | 보상 표기, 중복 문구·봇 계정 심사 |

자동 지급 개인키를 Manager에 넣지 않는다. Faucet 전용 제한 잔액 지갑과 별도 승인
서비스를 사용하고 주소·계정·기기·IP별 cooldown, 일일 예산 상한, 긴급 중지를 적용한다.

## 5. 장애·이중서명·키 탈취 훈련

훈련은 실제 자금이 없는 복제 환경에서 분기별로 실시한다.

1. 장애: 검증자 한 대를 중지하고 3/4 노드로 확정이 계속되는지 확인한다. 복구 후 tip,
   state root, finality certificate와 snapshot hash가 일치하는지 확인한다.
2. 네트워크 분리: 2:2 통신을 차단해 어느 쪽도 잘못 확정하지 않는지 확인한다. 연결 복구
   후 단일 canonical tip으로 수렴하고 동일 높이 이중 확정이 없는지 검사한다.
3. 이중서명: 폐기용 검증자 키로 같은 height/round에 서로 다른 prevote 또는 precommit을
   생성한다. 증거 검증, 5% slash 이벤트, 재실행 방지 ID와 감사 기록을 확인한다.
4. 키 탈취: 한 검증자 키가 유출됐다고 가정해 노드 격리, 후보 제거 승인, 새 키 등록,
   재가입 금지 기간과 공개 사고 공지를 시간 측정하며 수행한다. 원본 키는 삭제하지 말고
   오프라인 증거 보존 후 폐기 절차를 따른다.
5. 릴리스 키: 업데이트 서명키 유출을 가정해 배포 중지, 공개키 교체 릴리스, 이전 manifest
   거부와 사용자 공지를 검증한다.
6. 복구 판정: RTO/RPO, 탐지 시간, 격리 시간, 잘못된 지급·블록 유무를 기록하고 책임자와
   개선 기한을 지정한다.

## 6. 검증과 배포

### Docker 합의 노드 자동 전환

운영 Compose가 `network_mode: host`이고 `/opt/ieum-nodeN:/node`를 bind mount하면
포트 publish 문법은 사용하지 않는다. 네 컨테이너는 호스트에서 직접 UDP
`7001`, `7002`, `7003`, `7004`를 각각 열고 외부에는
`node.ieum.aah.name:7001~7004`로 광고한다. 각 `/node/config/network.json`의
공개 포트와 PeerId는 해당 노드 값이어야 한다.

자동 업데이트가 유지되려면 실행 파일이 읽기 전용 이미지 내부가 아니라 쓰기 가능한
`/node/ieum-chain`이어야 한다. 컨테이너 재시작 시 노드는 서명 manifest를 즉시 확인하고,
새 바이너리를 설치한 뒤 종료한다. `restart: unless-stopped`가 새 바이너리로 다시 시작하면
번들 `config/genesis.json`을 동기화하고 기존 시험 원장을
`data/ledger.pre-mainnet-20260819`로 보존한 뒤 새 메인넷 원장을 만든다. `/node`의 UID/GID가
Compose의 실행 사용자에게 쓰기 가능하지 않으면 안전하게 실패한다.

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
./target/release/ieum-chain --mainnet-strict --version
```

```bash
git switch dev
git add Cargo.toml Cargo.lock config/genesis.json src docs README.md CHANGELOG.md
git commit -m "release: prepare IEUM mainnet v0.23.5"
git push origin dev
gh pr create --base main --head dev --title "IEUM Chain v0.23.5 mainnet" --draft
# CI와 운영 전환 승인이 끝난 뒤 PR을 병합한다.
git switch main && git pull --ff-only origin main
git tag -a v0.23.5 -m "IEUM Chain v0.23.5"
git push origin v0.23.5
```

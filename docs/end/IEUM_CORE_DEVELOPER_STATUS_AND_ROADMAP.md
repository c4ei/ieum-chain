# IEUM Core 1차 완료 판정 및 후속 개발 계획

작성 기준일: 2026-08-04  
검토 기준: `c4ei/ieum-chain` main(`274eb66`, Cargo v0.21.1), `docs/end`, `docs/USER_MANUAL_0.20.9.md`, 현재 소스 구조와 `c4ei/ieum-wallet` main

## 1. 결론

IEUM Core는 **송금 가능한 사설 기능 테스트넷의 1차 개발본**으로 볼 수 있습니다. 그러나 **메인넷 또는 실자산 운영이 끝난 완성품은 아닙니다.**

현재 가능한 핵심 범위는 계정·잔액·수수료·서명 거래, PoS/BFT 합의 코어, QUIC/libp2p P2P, 원장 저장·복구, snapshot·pruning 기반, Ethereum 형식 일부와 JSON-RPC, 검증자 정책, 서명 자동 업데이트입니다.

반면 장기 다노드 실증, 공격·장애 시험, 완전한 스테이킹 경제, 라이트 클라이언트 증명, DID/VC, 외부 보안 감사, 브리지/DEX, WASM 스마트 계약은 완료되지 않았습니다. 따라서 현재 단계 명칭은 다음이 정확합니다.

> IEUM Core 1차 완료본 — 기능 테스트넷/파일럿용, 실자산 메인넷용 아님

## 2. 문서와 코드의 기준 차이

| 항목 | 확인 결과 | 조치 |
|---|---|---|
| 사용자 매뉴얼 | v0.20.9 기준 | 1차 매뉴얼로 다시 정리하되 현재 코드는 v0.21.1임을 명시 |
| Core 패키지 | `Cargo.toml` v0.21.1 | README와 배포 문서 버전 통일 필요 |
| Wallet README | v0.0.7.0 표기 | 실제 npm/Cargo v0.0.9-1과 통일 필요 |
| WASM | 설계 문서에 “추후 추가”만 존재 | 런타임, 상태, 거래, RPC, SDK 모두 신규 구현 필요 |
| 자동 테스트 | 저장소에 단위·통합·4프로세스 시험 존재 | 이번 검토 환경에는 Rust가 없어 재실행하지 못함. CI 성공 SHA를 릴리스 조건으로 고정 필요 |

## 3. 완료된 것으로 분류할 범위

여기서 “완료”는 테스트넷 1차 범위의 코드와 문서가 존재한다는 뜻이며, 메인넷 안전성을 보증한다는 뜻은 아닙니다.

| 영역 | 상태 | 근거와 범위 |
|---|---|---|
| 기본 화폐 | 1차 완료 | 계정, 잔액, nonce, 송금, 수수료, 공급 상태 |
| 거래 서명 | 1차 완료 | Ed25519 검증자 서명, secp256k1/EIP-155 형식 지갑 거래 처리 |
| 합의 | 테스트넷 완료 | 제안, prevote, precommit, 2/3 초과 확정, lock/valid value, timeout, 이중투표 증거 |
| 네트워크 | 테스트넷 완료 | libp2p QUIC, DNS bootstrap, mDNS, DHT, Gossipsub, request-response, relay/AutoNAT 기반 |
| 저장소 | 테스트넷 완료 | SQLite WAL 기반 상태 저장, snapshot, archive/pruning 구조, 백업·복구 기반 |
| 동기화 | 테스트넷 완료 | snapshot chunk, 재개 지점, 다중 피어 tip/state root 교차 확인 기반 |
| RPC | 1차 완료 | 잔액·nonce·거래·블록과 IEUM 네트워크 신원·버전·동기화·확정·복구 조회 |
| 운영 키 | 1차 완료 | 검증자 키 생성·공개키 추출·설정 생성, 외부 signer 경계 |
| 운영 배포 | 1차 완료 | systemd 패키지, 서명 manifest, SHA-256 검증, 이전 바이너리 보존 |
| 사고 복구 정책 | 설계·조회 완료 | 3/4 승인 판정과 감사 기록/RPC. 실제 다중서명 적용기 운영 검증은 별도 필요 |
| 보안 통신 코어 | 기반 완료 | PeerId 대상 신호 전달과 로컬 RPC. 실제 채팅/WebRTC UI는 wallet 책임 |
| 검증자 선발 | 정책 코어 완료 | 1% 지분·국가별 상위·관리자 승인 계산, 소유권/가동률 조건 |

## 4. 앞으로 추가해야 할 작업

### P0 — 메인넷 검토 전에 반드시 완료

1. **버전·문서 단일화**
   - Core README, Cargo, 매뉴얼, CHANGELOG, 배포 파일을 같은 버전으로 맞춥니다.
   - `docs/end`는 과거 기록으로 유지하고 현재 상태는 단일 `STATUS.md`에서 관리합니다.
   - 각 기능에 `설계`, `단위 테스트`, `다프로세스 시험`, `실서버 시험`, `감사 완료` 상태를 따로 표시합니다.

2. **실제 네트워크 장기 시험**
   - 서로 다른 서버·망 사업자의 검증자 4대 이상으로 7일 이상 실행합니다.
   - 중단·재합류, 패킷 지연/유실, 시계 오차, 디스크 부족, 네트워크 분할, Byzantine 메시지를 시험합니다.
   - 100~1,000 일반 노드의 NAT·Sybil·부하 시뮬레이션을 추가합니다.

3. **합의와 상태 전이 검증 강화**
   - property/fuzz 테스트, 악성 proposal/vote/snapshot/state root 거부 시험을 추가합니다.
   - validator set 변경이 epoch 경계에서만 일어나는지 다프로세스로 확인합니다.
   - slashing, jail, unjail, stake/unstake/delegation의 실제 온체인 상태 전이를 완성합니다.

4. **보안·키·복구**
   - 검증자 키를 HSM/KMS/Vault/PKCS#11 중 실제 제품 하나와 연결합니다.
   - 키 교체·분실·유출·검증자 제거 훈련을 문서화하고 실행합니다.
   - 릴리스 서명키는 오프라인 보관하고 복구키·폐기 절차를 둡니다.
   - 독립 보안 감사 전 실제 가치 자산을 넣지 않습니다.

5. **운영 보호 장치**
   - 공개 RPC 앞에 TLS, rate limit, CORS, method allowlist, WAF를 둡니다.
   - metrics HTTP endpoint와 Grafana 경보를 완성합니다.
   - snapshot·SQLite·archive 백업을 다른 서버에 복제하고 정기 복원 훈련을 합니다.
   - 자동 업데이트는 검증자 전부 동시 적용을 금지하고 canary/rolling 배포로 운영합니다.

### P1 — 사용자 생태계에 필요

- 라이트 클라이언트: 확정 헤더, 검증자 집합, Merkle 잔액/거래 증명
- Explorer 전용 인덱서와 안정적인 조회 API
- DID/VC 발급기관·폐기 registry. 개인정보 원문은 체인 밖에 저장
- 거버넌스 제안·서명·활성 높이 관리
- 보상 영수증과 Sybil 방지 검증
- QR 스캔, 하드웨어 지갑/외부 signer, 정식 코드 서명 배포

### P2 — 별도 보안 프로젝트로 진행

- AAH↔IEUM 브리지/교환
- 유동성 풀 DEX
- 원격 제어
- 광고 보상 실지급

브리지와 DEX는 양쪽 체인의 확정 증명, 다중서명/검증자 서명, 한도, 지연 출금, 비상정지와 외부 감사를 먼저 갖춰야 합니다.

## 5. EVM 대신 WASM을 구현하는 방법

### 5.1 권장 방향

IEUM의 경량 목표에는 EVM 전체 호환보다 **제한형 WASM 계약 실행기**가 맞습니다. 첫 버전은 JIT보다 결과가 단순하고 자원 제어가 쉬운 Rust 인터프리터인 `wasmi` 계열을 권장합니다. 성능이 실제 병목으로 확인된 뒤에만 Wasmtime 같은 런타임을 검토합니다.

WASM은 브라우저용 코드를 그대로 실행하는 기능이 아닙니다. 모든 검증자가 같은 입력에 반드시 같은 결과를 내도록 다음을 금지해야 합니다.

- 시스템 시간, 난수, 네트워크, 파일, 스레드 직접 접근
- 부동소수점 연산 또는 결과 차이가 생길 수 있는 기능
- 무제한 메모리, 무한 반복, 재귀 호출
- 계약이 임의 주소나 운영체제 기능을 호출하는 행위

### 5.2 최소 구성

```text
서명 거래
  → 계약 배포/호출 거래 검증
  → WASM 바이트코드 검증
  → 결정론적 런타임 실행
  → gas·메모리·호출 깊이 제한
  → 상태 변경분 생성
  → BFT 확정 때 SQLite 상태와 영수증 원자 저장
```

추가할 Core 모듈 예시:

```text
src/wasm/validator.rs     허용 명령·import·메모리 제한 검사
src/wasm/runtime.rs       wasmi 실행, fuel/gas 계측
src/wasm/host.rs          읽기·쓰기·송금·이벤트 등 제한 host API
src/wasm/state.rs         contract/code/storage namespace
src/wasm/receipt.rs       결과, gas, 로그, 오류
src/wasm/abi.rs           인자와 반환값의 고정 직렬화 규격
```

### 5.3 온체인 자료 구조

- `DeployContract`: 코드, 코드 해시, 배포자, salt, 초기화 인자, gas limit
- `CallContract`: 계약 주소, 함수, 인자, 보낼 IEUM, gas limit
- `ContractReceipt`: 성공 여부, 사용 gas, 반환값 해시, event 목록
- 상태 key: `contract/code/<hash>`, `contract/meta/<address>`, `contract/state/<address>/<key>`
- 계약 주소: `hash(deployer || nonce || code_hash || salt)`로 결정론적 생성

코드 크기, 초기/최대 메모리, 상태 key/value 크기, 이벤트 수, 호출 깊이를 체인 파라미터로 고정해야 합니다. 이 값은 합의 규칙이므로 임의 환경변수로 바꾸면 안 됩니다.

### 5.4 Host API v1

첫 버전에는 다음 정도만 노출합니다.

- `storage_read`, `storage_write`, `storage_remove`
- `caller`, `contract_address`, `attached_value`, `block_height`
- `balance_of`, `transfer`
- `emit_event`
- 검증된 `sha256`, `keccak256`, `ed25519_verify`, `secp256k1_verify`

시간은 로컬 시각 대신 확정 블록의 합의된 값만 사용합니다. 외부 HTTP/API 호출은 계약 안에서 하지 않고 oracle 거래로 별도 제출합니다.

### 5.5 Gas와 수수료

- WASM 명령마다 고정 gas 비용을 부과합니다.
- host 함수와 저장 공간 읽기/쓰기에 별도 비용을 둡니다.
- 실행 전 최대 수수료를 잠그고, 종료 뒤 실제 사용분만 정산합니다.
- out-of-gas와 trap은 상태 변경을 모두 되돌리되 수수료는 정책대로 부과합니다.
- gas 표 변경은 `config/upgrades.json`의 정해진 활성 높이에서만 적용합니다.

### 5.6 구현 순서와 완료 기준

1. ADR과 `IEUM WASM ABI v1` 규격 확정
2. 순수 런타임: 코드 검증, fuel, memory, trap, 결정론 테스트
3. 상태 overlay와 성공 시 commit/실패 시 rollback
4. 배포·호출 거래, mempool 검증, 영수증·이벤트
5. BFT proposal 실행 결과의 state root 검증
6. RPC: estimate, deploy, call, receipt, code 조회
7. Rust SDK와 예제 계약(에스크로·투표·멀티시그)
8. 4노드 동일 결과 시험, differential/property/fuzz 시험
9. 테스트넷 활성 높이 적용
10. 외부 감사 후에만 운영 활성화

완료 판정은 동일 계약을 Linux/Windows와 서로 다른 CPU에서 실행했을 때 gas, 반환값, event, state root가 완전히 같고, 악성 WASM이 노드를 멈추거나 메모리를 고갈시키지 못할 때입니다.

## 6. IEUM Wallet을 개발자 도구로 확장할 것인가

### 결론

**확장은 필요하지만 일반 사용자 지갑 안에 모든 개발 기능을 섞으면 안 됩니다.** 추천 구조는 다음과 같습니다.

| 제품 | 역할 |
|---|---|
| IEUM Wallet | 키 보관, 잔액, 송금, 계약 호출 내용 확인·서명 |
| Wallet 개발자 모드 | 네트워크 상태, raw transaction 미리보기, 계약 호출/영수증, 테스트넷 faucet |
| `ieum-cli` | 노드·계정·계약 배포/호출, script, CI |
| `ieum-sdk` | Rust 우선 SDK, 이후 TypeScript SDK |
| IEUM Studio(선택) | ABI form, 배포, event/receipt, 로컬 테스트 UI |

현재 wallet은 네트워크 신원·프로토콜·동기화·확정 상태를 확인하고 RPC method allowlist를 사용하므로 개발자 모드의 좋은 기반은 이미 있습니다. 다만 arbitrary RPC 콘솔, 개인키 표시, 임의 웹 콘텐츠의 서명 요청은 기본 모드에 넣지 않습니다.

### Wallet 개발자 모드 1차 범위

- 설정에서 여러 번 확인해야 켜지는 테스트넷 전용 고급 모드
- Chain ID, genesis hash, 프로토콜 버전, peer, 동기화, 최종 확정 높이 표시
- 거래 JSON과 해시, nonce, gas/fee, 서명 전 사람이 읽을 수 있는 요약
- WASM ABI를 읽어 함수·인자를 form으로 표시
- dry-run/estimate 결과와 state diff 미리보기
- 계약 배포·호출·영수증·event 조회
- 오류 로그 내보내기 시 seed/private key/token 자동 제거
- mainnet에서는 faucet, 임의 RPC, 위험 기능 강제 비활성화

### 분리해야 하는 기능

- 계약 컴파일러와 패키지 빌드
- unrestricted JSON-RPC 콘솔
- 검증자 키·릴리스 키 관리
- 원장 수정·rollback 실행
- 브리지 관리자·재단 지갑 단독 서명

이 기능은 별도 CLI/Studio와 OS 권한 경계에서 제공하고, wallet은 최종 서명 요청을 사람이 이해할 수 있게 보여주는 역할만 맡습니다.

## 7. 권장 다음 릴리스 순서

1. v0.21.x: 버전 문서 통일, CI 증거, 4대 실서버 장기 시험, 모니터링
2. v0.22.x: 스테이킹/위임/slashing과 epoch validator 변경 운영 완성
3. v0.23.x: 라이트 클라이언트 proof와 Explorer 인덱서
4. v0.24.x: WASM ABI·런타임 MVP를 테스트넷에서 비활성 상태로 탑재
5. v0.25.x: WASM 4노드 결정론 시험, SDK/CLI, wallet 개발자 모드
6. 감사 통과 릴리스: 활성 높이를 정해 WASM 제한 공개 테스트

## 8. 릴리스 판정표

| 판정 | 현재 |
|---|---|
| 로컬 개발/학습 | 가능 |
| 사설 4노드 기능 시험 | 가능 |
| 제한된 파일럿 테스트넷 | 조건부 가능 |
| 공개 테스트넷 | 장기·공격·운영 시험 후 가능 |
| 실자산 메인넷 | 불가 |
| WASM 계약 배포 | 불가 — 신규 구현 필요 |


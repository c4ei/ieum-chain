# IEUM Chain 0.21.7 #8 변경 내역

## 결론

4프로세스 BFT 실패의 직접 원인은 P2P 합의 메시지에 포함된 `u128` 거래 금액을 수신 노드가 JSON에서 역직렬화하지 못한 것이다.

```text
json=u128 is not supported at line 1 column 1719
json=u128 is not supported at line 1 column 2145
```

송신 노드는 `Transaction.amount`와 `fee`를 JSON 숫자로 직렬화했지만, 기본 기능만 활성화한 `serde_json` 수신기는 Rust `u128` 필드를 지원하지 않았다. 이 때문에 거래가 든 `Proposal`, `Block`, 동기화 인증서가 폐기되었다. 제안을 받지 못한 노드에는 투표만 도착했고 다음 현상이 연쇄 발생했다.

```text
제안 메시지 폐기
→ 제안 또는 이전 합의 단계 대기
→ 단계 제한 시간 초과
→ 노드별 라운드 분리
→ 현재 높이/라운드와 다른 투표 거부
→ BFT 합의 실패
```

## #7 수정과의 관계

#7에서 고친 유휴 상태의 만료된 BFT 타이머 문제는 별개의 결함이며 그대로 필요하다. #7의 상세 오류 로그가 적용되면서, 그동안 일반적인 `해석할 수 없는 메시지`로 가려졌던 이번 wire-format 오류가 정확히 확인되었다.

## 다른 블록체인과 비교

geth/Ethereum은 네트워크 프로토콜의 핵심 거래와 블록을 일반 JSON으로 gossip하지 않는다. 실행 계층 P2P에서는 RLP 기반의 명시적인 바이트 인코딩을 사용하고, JSON-RPC의 큰 정수는 `0x` 접두사의 hex quantity 문자열로 표현한다. Tendermint/CometBFT 계열도 합의 wire format에 Protobuf처럼 타입과 크기가 정해진 바이너리 표현을 사용한다.

IEUM은 현재 내부 P2P wire format에도 JSON을 사용한다. JSON 자체가 BFT에 부적합한 것은 아니지만, 모든 노드가 큰 정수 표현을 동일하게 지원하고 직렬화 결과가 버전 간 호환되어야 한다. 이번 실패는 BFT 알고리즘이나 “거래가 없으면 빈 블록을 만들지 않는 정책” 때문이 아니라 그 전 단계인 메시지 디코딩 실패다.

## 변경 사항

### 1. P2P JSON의 u128 역직렬화 지원

- `serde_json`의 `arbitrary_precision` 기능 활성화
- `Transaction.amount`, `Transaction.fee`를 포함한 중첩 메시지를 손실 없이 수신
- 기존 Rust 모델, 거래 서명 바이트, 블록 해시, JSON 필드 이름은 변경하지 않음
- 이전 노드가 보낸 JSON 숫자 형식과 호환 유지

### 2. wire-format 회귀 테스트

- CI 로그와 같은 `0.1 IEUM = 100000000000000000 wei` 거래 생성
- 거래가 포함된 `WireMessage::Proposal` 전체를 직렬화 후 역직렬화
- 금액, 수수료, nonce, 서명이 모두 같은지 검증

이 테스트는 기본 `serde_json` 설정에서는 `u128 is not supported`로 실패하며, 이번 기능 설정이 빠지면 다시 검출한다.

## 영향 범위와 안전성

이번 수정은 정족수, 제안자 선택, prevote/precommit 규칙, 블록 생성 정책을 변경하지 않는다. JSON 숫자 파서의 지원 범위만 Rust 모델이 이미 허용하는 `u128`까지 넓힌다.

큰 정수 정밀도를 잃는 JavaScript 클라이언트에는 기존처럼 JSON-RPC에서 hex quantity 문자열을 사용해야 한다. `arbitrary_precision` 활성화가 외부 RPC에 십진수 숫자를 사용해도 된다는 뜻은 아니다.

## 검증 기준

```bash
git diff --check
bash -n tests/four_process_network.sh
cargo test p2p_proposal_with_u128_transaction_round_trips
cargo test
cargo build --release
bash tests/four_process_network.sh target/release/ieum-chain
```

최종 성공 기준:

```text
4-process BFT passed
```

또한 네 노드 로그에 다음 오류가 없어야 한다.

```text
json=u128 is not supported
```

## 다음에 작업할 큰 항목

### 버전이 고정된 바이너리 합의 wire format 도입

JSON과 Rust 구조체를 직접 결합한 현재 방식은 필드 추가, 숫자 표현, 라이브러리 설정 차이가 곧 네트워크 단절로 이어질 수 있다. 다음 큰 버전에서는 다음을 함께 설계한다.

1. Protobuf, SCALE 또는 명시적 canonical binary codec 중 하나를 선정
2. `protocol_version`과 메시지 종류를 고정 헤더에 포함
3. 정수 크기, 필드 순서, 최대 길이와 알 수 없는 필드 처리 규칙 명시
4. 구버전/신버전 노드 혼합 운용 및 업그레이드 구간 테스트
5. 모든 `WireMessage` variant의 golden vector와 fuzz test 추가
6. `FinalityCertificate` 중심의 확정 블록 전파·복구와 함께 적용

이는 기존 노드와의 P2P 호환성 및 네트워크 업그레이드 절차를 바꾸므로 0.21.7의 작은 수정에 포함하지 않는다.

## 남은 확인 사항

- 실제 GitHub Actions에서 전체 release 빌드와 4프로세스 테스트 통과 확인
- 오류가 사라진 뒤에도 일부 노드만 높이 1을 적용한다면 #7 문서의 `FinalityCertificate` 기반 복구를 우선 구현
- 혼합 버전 노드를 운영하지 말고 네 검증자 모두 같은 빌드로 동시에 검증

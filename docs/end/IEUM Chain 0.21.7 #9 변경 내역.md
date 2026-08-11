# IEUM Chain 0.21.7 #9 변경 내역

## 결론

#8의 `serde_json/arbitrary_precision` 활성화만으로 `u128`의 P2P 표현을 라이브러리 설정과 분리하지 못했다. #9에서는 거래의 `amount`와 `fee`를 십진 문자열로 직렬화하고 기존 숫자 JSON도 읽도록 변경했다. 따라서 거래가 든 `Proposal`을 받는 경로에서 발생한 다음 오류를 wire format 수준에서 제거한다.

```text
json=u128 is not supported
```

큰 P2P JSON은 zstd level 3으로 압축한다. 1,024바이트 미만 메시지와 압축 결과가 원문보다 작지 않은 메시지는 기존 JSON 그대로 전송한다. 투표처럼 작은 메시지에는 압축 지연과 헤더 비용을 추가하지 않는다.

## 실패 원인과 수정

기존 `Transaction`은 `u128`을 JSON 숫자로 직렬화했다. 송신과 수신에 쓰는 JSON 구현 또는 기능 구성이 달라지면 송신에는 성공하고 수신에서만 실패할 수 있었다. 제안이 폐기된 뒤 투표만 처리되어 라운드 불일치와 타임아웃이 연쇄 발생했다.

#9의 새 표현은 다음과 같다.

```json
{"amount":"100000000000000000","fee":"21000"}
```

- 새 노드는 `amount`와 `fee`를 항상 십진 문자열로 기록·전송한다.
- 구버전 데이터의 음수가 아닌 JSON 숫자도 계속 읽는다.
- 거래 서명 바이트와 거래 ID 계산은 기존 `u128` 바이트를 사용하므로 변경되지 않는다.
- 정족수, 제안자 선택, prevote/precommit 및 블록 생성 정책은 변경하지 않는다.

## P2P 압축 형식

압축 프레임은 `IEUMZ`, 형식 버전 1, 압축 해제 후 길이(u32 big-endian), zstd payload 순서다.

- JSON 원문 1,024바이트 이상일 때만 압축 시도
- zstd level 3 사용
- 압축 프레임이 원문보다 작을 때만 압축본 전송
- 압축되지 않은 기존 JSON 수신 지원
- 압축 해제 전에 선언 길이를 `max_message_bytes`와 비교
- 실제 압축 해제 길이가 선언 길이와 다르면 거부
- 손상된 zstd 및 JSON은 피어 오류로 처리

JSON은 반복되는 필드명, 16진 서명과 해시가 많아 거래가 든 Proposal/Block에서는 일반적으로 압축 효과가 있다. 반면 합의 투표 하나처럼 작은 데이터는 그대로 보내는 편이 CPU·지연·바이트 모두 유리하다.

## 호환성과 배포 주의

압축되지 않은 메시지는 기존 wire JSON과 호환된다. 다만 구버전 노드는 `IEUMZ` 압축 프레임을 해석하지 못하므로 큰 메시지가 압축되는 순간 혼합 버전 네트워크가 분리될 수 있다. 검증자 전원을 같은 #9 빌드로 동시에 배포해야 한다.

장기적으로는 libp2p identify의 프로토콜 버전과 합의 토픽 버전을 올리고, 피어 capability 협상 뒤 압축을 켜야 한다. 이번 4노드 격리 테스트에서는 모든 노드가 동일 바이너리를 쓰므로 #9에서 압축 경로를 바로 검증한다.

## 추가한 회귀 검증

1. `u128::MAX`가 JSON 문자열로 직렬화되는지 확인
2. 기존 숫자형 `fee`를 계속 읽는지 확인
3. 실제 거래가 든 Proposal을 압축 후 복원해 동일한지 확인
4. 압축 헤더가 과도한 해제 크기를 선언하면 zstd 실행 전에 거부하는지 확인

## 검증 명령과 성공 기준

```bash
cargo fmt --check
cargo test p2p_proposal_with_u128_transaction_round_trips
cargo test transaction_u128_is_a_decimal_string_and_legacy_number_is_accepted
cargo test compressed_message_declared_size_is_bounded_before_decompression
cargo test
cargo build --release
bash tests/four_process_network.sh target/release/ieum-chain
```

최종 성공 기준은 `4-process BFT passed`이며, 네 노드 어디에도 `u128 is not supported`가 없어야 한다.

## 다음에 작업할 큰 항목

### 버전 협상 가능한 canonical binary wire protocol

현재 압축은 JSON의 트래픽을 줄이지만 JSON 자체의 타입·필드 호환성 문제를 없애지는 않는다. 다음 큰 변경에서는 다음을 함께 구현한다.

1. Protobuf/SCALE 등 canonical binary codec 선정
2. 메시지 헤더에 protocol version, codec, compression, 원문 길이 포함
3. identify 기반 capability 협상과 구버전 fallback
4. 압축률·압축/해제 시간·메시지 종류별 트래픽 계측
5. 최대 중첩 깊이, 압축 비율 제한, fuzz/property test
6. 모든 WireMessage golden vector와 혼합 버전 네트워크 테스트
7. FinalityCertificate 중심의 확정 블록 전파·복구

이는 네트워크 업그레이드 규칙과 혼합 버전 운용을 바꾸므로 별도 버전 작업으로 진행한다.

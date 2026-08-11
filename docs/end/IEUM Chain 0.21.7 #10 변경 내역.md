# IEUM Chain 0.21.7 bugfix #10

## 결론

0.21.8 승격 전에 P2P zstd 프레임 헤더의 버전 바이트 표기를 수정했습니다.
JSON-RPC, 지갑 주소, 거래 서명 바이트, 거래 해시 및 원장 형식은 변경하지 않습니다.

## 원인

#9의 `COMPRESSED_WIRE_MAGIC`이 실제 `0x01` 바이트가 아니라 `\\x01` 네 문자를
포함하도록 작성되어 있었습니다. 선언된 배열 길이는 6바이트지만 리터럴은
9바이트이므로 정상 빌드에서 타입 불일치가 발생합니다.

## 수정

- 압축 wire magic을 정확한 6바이트 `49 45 55 4d 5a 01`로 수정
- 헤더 전체 길이를 10바이트(magic 6 + 원문 길이 4)로 고정 검증
- `u128` 거래가 포함된 Proposal 왕복 테스트 유지
- 큰 압축 가능 메시지의 zstd 프레임 왕복 테스트 유지

## 호환성

- 지갑 및 JSON-RPC: 영향 없음
- 거래 서명/해시: 영향 없음
- 원장 데이터: 영향 없음
- P2P: #9 압축 프레임은 잘못된 헤더 구현이므로 #10 노드로 동시에 교체 권장

## 확인 명령

```bash
cargo test network::connection_log_tests::compressed_wire_magic_has_binary_version_byte
cargo test network::connection_log_tests::p2p_proposal_with_u128_transaction_round_trips
cargo test network::connection_log_tests::large_compressible_wire_message_uses_zstd_frame
bash tests/four_process_network.sh target/release/ieum-chain
```

# IEUM Chain v1.0.9.0 — RPC 이더리움 도구 호환

## 버전과 포함 변경

- Cargo 버전: `1.0.9-0`
- 표시 버전 및 Git 태그: `1.0.9.0`, `v1.0.9.0`
- ethers v6, MetaMask가 블록·거래·영수증 응답을 오류 없이 파싱하도록 Ethereum 표준 필드를 채웁니다.
- `eth_sendRawTransaction`은 keccak256(raw)을 반환하고, 조회 API는 IEUM 거래 ID와 keccak 해시를 모두 받습니다.
- Chain ID, genesis, genesis hash, 프로토콜 버전과 합의 규칙은 변경하지 않습니다.

## 연동 서비스 영향

- IEUM Manager의 상품권·체험 지급은 `isPayoutHash`로 두 해시를 모두 허용하므로 노드 업데이트 전후 모두 동작합니다.
  Manager를 먼저 배포한 뒤 노드를 업데이트합니다.
- 송금 요청의 반환 해시를 IEUM 거래 ID와 직접 비교하던 외부 코드는 keccak 해시도 허용하도록 바꿔야 합니다.

## 빌드와 배포

`v1.0.9.0` 태그 푸시 시 `chain-release.yml`이 세 번째 자리 변경(1.0.8 → 1.0.9)을 감지해
Linux, Windows, macOS 바이너리를 빌드하고 서명된 manifest와 GitHub Release를 게시합니다.

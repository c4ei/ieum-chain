# IEUM Chain v0.22.5 운영 스냅샷·메인넷 안전 보강

## 변경 사항

- 월 변경 또는 활성 원장 100MB 도달로 체크포인트가 만들어지면 현재 확정 높이,
  블록 해시, 상태 루트에 검증자가 `IEUM-SNAPSHOT-V1` 도메인 서명을 전파합니다.
- 등록 검증자 투표권의 2/3를 **초과**한 고유 서명만 인증 스냅샷으로 저장합니다.
- snapshot 내용의 높이, 블록 해시, 상태 루트가 인증서와 모두 일치해야 합니다.
- 서명이 틀리거나 등록되지 않은 검증자 투표를 보낸 피어는 100점 패널티로 임시
  차단합니다.
- 인증 스냅샷은 `data/ledger/certified-snapshots/`에 원자적으로 저장하며 최근 6개만
  유지합니다. Archive의 월별 블록 백업은 계속 전체 보존합니다.
- `ieum_getStorageStatus`에 최신 체크포인트 높이, 인증 스냅샷 수, 최신 인증 높이가
  추가되어 Manager v0.3.5가 누락과 지연을 경보할 수 있습니다.

## 동기화 안전 순서

1. 최소 2개 독립 피어가 동일한 `height/blockHash/stateRoot`를 보고해야 합니다.
2. 내려받은 chunk별 SHA-256과 전체 압축 snapshot ID를 검증합니다.
3. snapshot의 검증자 2/3 초과 인증서를 검증합니다.
4. snapshot 적용 후 해당 높이 다음의 BFT 확정 인증서만 순서대로 적용합니다.
5. 일반 노드는 최근 활성 블록과 인증 스냅샷을 사용하고 Archive 노드는 월별 전체
   이력을 보존합니다.

## 메인넷 릴리스 주의사항

- `config/genesis.json` 변경은 기존 네트워크와 완전히 다른 체인을 만듭니다. 운영망
  전환 전에 총발행량, 재단·보상·락업 물량을 확정하고 genesis SHA-256을 별도 공개한
  뒤에는 수정하지 마세요.
- `--allow-insecure-test-keys`와 `--git-action-test`는 운영 서비스에서 사용하지 마세요.
- 새 운영망 전환 검증에서는 `--mainnet-strict`를 사용하세요. 공개 개발키 주소에 잔액이
  있거나 네트워크 이름에 `test`가 포함되면 노드가 시작을 거부합니다. 현재 기존
  genesis는 개발 잔액이 있어 strict 검사를 통과하지 않으므로, 기존 체인의 genesis를
  조용히 변경하지 말고 별도의 메인넷 genesis·전환 계획을 먼저 확정해야 합니다.
- 네 검증자는 서로 다른 장애 영역에 배치하고 validator key, node key, 원장, 인증
  snapshot을 별도 암호화 저장소에 백업하세요.

## 검증 명령

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

현재 저장소의 genesis는 기존 네트워크 호환을 위해 이번 버전에서 변경하지 않았습니다.

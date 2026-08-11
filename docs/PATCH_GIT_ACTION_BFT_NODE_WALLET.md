# IEUM Chain 0.21.8 변경분

이 압축본은 이전 `0.21.7_git_action_test` 작업본 위에 덮어쓸 변경 파일만 포함합니다.

## 적용 내용

- `consensus.can_make_proposal()` 확인 전에는 거래 큐를 비우지 않음
- 비제안자는 거래를 제거하지 않고 읽기 전용 복사본만 P2P 전파
- `node_reward_signing.key`: 보상 주소 등록 소유권 증명 전용으로 유지
- `node_wallet.keystore`: 실제 보상 자산 주소와 송금 서명용 암호화 지갑
- 최초 무인 실행 시 임의 암호를 생성해 `node_wallet.password`(0600)에 저장
- `reward change-password --new-password-file ...`로 주소를 유지한 원자적 재암호화
- 새 등록 메시지에 `registration_signer`를 추가하되 구버전 메시지는 기존 방식으로 검증

## 암호 변경

새 암호를 10자 이상으로 작성한 소유자 전용 파일을 준비한 뒤 실행합니다.

```bash
chmod 600 /secure/new-node-wallet.password
ieum-chain reward change-password \
  --new-password-file /secure/new-node-wallet.password
```

명령행 인자에 실제 암호를 직접 쓰지 않으므로 프로세스 목록에 노출되지 않습니다.
재암호화된 임시 keystore의 복호화와 주소 일치를 확인한 후 파일을 교체합니다.
암호 파일 교체가 실패하면 keystore를 기존 암호로 되돌립니다.

## 키 역할

- `data/keys/node_reward_signing.key`: 등록 증명용 Ed25519 키
- `data/keys/node_wallet.keystore`: 보상 자산용 암호화 Ed25519 지갑
- `data/keys/node_wallet.password`: 최초 무인 실행용 자동 생성 암호(0600)

HD 월렛은 구현하지 않았습니다. 현재 노드 보상은 주소 하나만 필요하므로 단일 암호화 지갑이 맞으며, 두 키는 자동 변환하거나 공유하지 않습니다.

## 검증 참고

현재 제작 환경에는 Rust 도구체인이 없어 `cargo test`는 실행할 수 없습니다. `git diff --check`, 셸 문법, 참조 위치 및 압축 목록 검사는 로컬에서 수행하며, 실제 컴파일과 4노드 BFT는 GitHub Actions에서 최종 확인해야 합니다.

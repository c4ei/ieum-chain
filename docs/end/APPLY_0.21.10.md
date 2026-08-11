# IEUM Chain 0.21.10 변경 파일 적용

이 묶음은 `c4ei/ieum-chain`의 `v0.21.9` 최종 커밋
`d292e6eea322a2e92240a3cefb23d3b24ae6d737` 위에 덮어쓸 변경 파일만 포함합니다.

```bash
cd /www/ieum-chain
tar -xJf /다운로드경로/ieum-chain-v0.21.10-changed-files.tar.xz
cargo test
cargo build --release
./target/release/ieum-chain --version
```

정상 버전은 `ieum-chain 0.21.10`입니다. 검증자 운영망은 한 대씩 교체하고 RPC,
동기화, 블록 확정을 확인한 뒤 다음 노드로 진행하세요.

업데이트 후 주소 확인:

```bash
./target/release/ieum-chain reward address
```

정상 결과는 `0x`로 시작하는 총 42자 주소입니다. 처음 로드한 구형 보상 keystore는
`data/keys/node_wallet.keystore.ed25519.bak`으로 남습니다. 이 백업은 삭제하지 마세요.
구형 64자리 주소에 잔액이 있다면 새 주소로 자동 이동하지 않으므로
`docs/USER_MANUAL_0.21.10.md`의 구형 잔액 이관 절차를 먼저 수행하세요.

일반 테스트 계정 생성:

```bash
install -m 600 /dev/null /secure/ieum-account.password
vi /secure/ieum-account.password
./target/release/ieum-chain account new \
  --password-file /secure/ieum-account.password
```

상세 명령은 `docs/USER_MANUAL_0.21.10.md`를 확인하세요.

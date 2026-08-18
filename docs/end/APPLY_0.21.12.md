# IEUM Chain 0.21.12 변경분 적용

이 압축은 `0.21.11` 소스 위에 적용합니다. 모든 상대 경로는 빌드 폴더가 아니라
실제로 실행한 바이너리의 폴더를 기준으로 합니다.

```bash
cd ~/www/ieum-chain
tar -xJf /다운로드경로/ieum-chain-v0.21.12-changed-files.tar.xz

cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

노드별로 새 바이너리를 복사한 뒤 각 서비스를 재시작합니다.

```bash
sudo install -m 755 target/release/ieum-chain /opt/ieum-node1/ieum-chain
sudo install -m 755 target/release/ieum-chain /opt/ieum-node2/ieum-chain
sudo install -m 755 target/release/ieum-chain /opt/ieum-node3/ieum-chain
```

`/opt/ieum-node2/ieum-chain account new`는 실행 위치와 관계없이 다음 파일만 사용합니다.

```text
/opt/ieum-node2/secure/ieum-account.password
/opt/ieum-node2/data/keystore/UTC--...--주소
```

자동 업데이트도 `/opt/ieum-node2/config/update.json`만 읽고
`/opt/ieum-node2/ieum-chain`만 교체합니다. 절대경로 옵션은 사용자가 명시한 경로를
그대로 사용합니다.

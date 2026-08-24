# IEUM Chain v1.0.3.1 체크포인트·P2P 자동 복구

표시 버전은 `1.0.3.1`, Cargo 내부 버전은 `1.0.3-1`입니다. 코드 버전의 세 번째 숫자가 바뀌므로 태그 생성 시 Linux·Windows·macOS 바이너리를 빌드합니다.

## 장애 원인과 수정

v1.0.2.1은 원장에 존재하는 연속 블록이 아니라 보관 중인 `FinalityCertificate`만 응답했습니다. 운영 Node 1은 높이 4에서 다음 높이 5를 기대했지만 높이 7 인증서 하나를 받아 `동기화 응답에 블록 높이 공백이 있습니다`로 종료됐습니다.

v1.0.3.1은 Geth의 block/checkpoint sync와 Solana/Agave의 known-validator snapshot 방식을 IEUM BFT에 맞게 최소화합니다.

- Geth 공식 동기화 문서: <https://geth.ethereum.org/docs/fundamentals/sync-modes>
- Agave 검증자·known-validator 문서: <https://docs.anza.xyz/operations/guides/validator-start>

- 기본 12블록(5초 목표 간격 약 1분) 이내이며 인증서가 연속이면 블록 단위 동기화
- 차이가 크거나 인증서가 끊기면 현재 상태 snapshot과 검증자 서명 전송
- 등록 검증자 투표권 2/3 초과가 같은 height/hash/state root에 서명해야 snapshot 설치
- snapshot 다음 높이부터 다시 확정 블록 동기화
- 불완전한 응답은 프로세스를 종료하지 않고 누락 높이부터 재요청
- 직접 동기화 후보 tip은 독립 원격 피어 2개가 일치해야 선택
- 실제 데이터는 블록의 3/4 precommit 또는 snapshot의 3/4 검증자 서명으로 별도 검증
- `--direct-sync-block-limit`으로 기준 조정

### Geth와 IEUM의 차이

Geth는 합의 클라이언트가 제공한 목표 헤더를 기준으로 부모 해시가 이어지는 헤더를
검증한 뒤 block body·receipt·state를 내려받습니다. 가까운 구간은 블록 단위로
실행하고, 먼 구간은 snap/checkpoint 상태에서 시작한 뒤 블록 동기화로 전환합니다.

IEUM은 합의·실행을 한 프로세스에 가진 4검증자 BFT 체인이므로 역할을 다음처럼
분리합니다.

- 같은 tip을 보고한 원격 피어 2개: 다운로드 후보 선택과 단일 악성 피어 배제
- 연속 블록의 3/4 precommit: 블록의 확정성 검증
- 동일 snapshot의 3/4 검증자 서명: checkpoint 상태의 확정성 검증
- 동기화 중인 검증자: 최신 인증 상태가 설치될 때까지 제안·투표 금지

원격 피어 3개를 tip 선택 조건으로 사용하면 검증자 하나가 중단됐을 때 생존 노드가
각각 두 피어밖에 볼 수 없어 진행할 수 없습니다. 이는 BFT 서명 정족수와 네트워크
응답 수를 혼동한 설정이므로 기본값을 2로 유지합니다.

버전은 `Cargo.toml`의 package version 한 곳만 수정합니다. `build.rs`가
`1.0.3-1`을 `1.0.3.1`로 변환하고 CLI·RPC·자동 업데이트가 같은
`IEUM_DISPLAY_VERSION`을 사용합니다.

## 원클릭 복구

```bash
bash scripts/recover-ieum-node.sh -h
sudo bash scripts/recover-ieum-node.sh
```

Docker 4노드에서는 이상 노드와 다수 상태에 속한 정상 원본을 자동 선택합니다. systemd 단일 노드는 config, validator key, server.node.key, `data/keys`를 보존하고 `data/ledger`만 날짜가 붙은 경로로 백업한 뒤 P2P 재동기화합니다.

```bash
sudo bash scripts/recover-ieum-node.sh --docker --node 1 --from-node 2
sudo bash scripts/recover-ieum-node.sh --systemd -d /opt/ieum-chain
```

## Docker 자동 업데이트

```bash
sudo cp deploy/docker-four-node/Dockerfile /opt/ieum-docker-four-node/Dockerfile
sudo cp deploy/docker-four-node/update-four-nodes.sh /opt/ieum-docker-four-node/update-four-nodes.sh
sudo chmod 755 /opt/ieum-docker-four-node/update-four-nodes.sh
sudo /opt/ieum-docker-four-node/update-four-nodes.sh --build-only
```

새 Dockerfile은 `curl`과 `ca-certificates`를 설치합니다. 업데이트 스크립트는 후보 이미지의 버전과 `curl --version`을 모두 확인한 뒤 노드를 교체합니다. 기존 노드 높이가 다르면 최저 높이의 블록 해시가 네 노드에서 같은지도 검증하며, 공통 정규 체인이 아니면 업데이트를 중단합니다.

## CI 최적화와 완료 기준

- PR은 debug 바이너리를 한 번만 빌드해 실제 프로세스 시험이 공유
- 동일 4노드 시험의 중복 반복 제거
- OS별 release 최적화 빌드는 코드 태그에서만 실행
- 태그 workflow는 PR에서 완료한 Clippy·전체 테스트를 반복하지 않음
- Node 1을 3블록 뒤처지게 하고 임계값을 2로 낮춰 실제 snapshot 경로 강제
- 원장 복사 없이 네 노드 height/hash/state root와 `[snapshot 동기화 완료]` 확인

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --locked
bash tests/four_process_network.sh target/debug/ieum-chain
bash tests/four_process_node1_persistent_rejoin.sh target/debug/ieum-chain
bash tests/diagnostic_scripts.sh
bash tests/ci_regression_guard.sh
git diff --check
```

## Commit·Push·PR

```bash
git switch dev
git pull --ff-only origin dev
git status --short

git add -- \
  .github/workflows/chain-release.yml .github/workflows/rust-ci.yml \
  Cargo.toml Cargo.lock CHANGELOG.md \
  build.rs \
  src/consensus_runtime.rs src/lib.rs src/main.rs src/network.rs src/rpc.rs \
  scripts/diagnose-ieum-external.sh scripts/diagnose-ieum-server.sh \
  scripts/ieum-cluster-tool.sh scripts/recover-ieum-node.sh \
  scripts/commit-push-pr-v1.0.3.1-fix.sh \
  deploy/docker-four-node/Dockerfile deploy/docker-four-node/README.md \
  deploy/docker-four-node/update-four-nodes.sh \
  tests/ci_regression_guard.sh tests/diagnostic_scripts.sh tests/four_node_bft.rs \
  tests/four_process_node1_persistent_rejoin.sh tests/release_build_policy.sh \
  docs/README.md docs/IEUM_USER_MANUAL_1.0.1.1.md \
  docs/VERSION_1.0.3.1_CHECKPOINT_P2P_RECOVERY.md

git commit -m "fix: recover lagging nodes with certified snapshots"
git push origin dev

gh pr create \
  --base main \
  --head dev \
  --title "IEUM Chain v1.0.3.1 certified snapshot recovery" \
  --body "연속 블록 및 2/3 인증 snapshot 자동 동기화, 원클릭 복구, Docker curl, CI 중복 빌드 제거"
```

PR CI가 모두 성공한 뒤 병합하고 태그를 만듭니다.

```bash
git switch main
git pull --ff-only origin main
git status --short
git log -1 --oneline
git ls-remote --tags origin 'refs/tags/v1.0.3.1*'

git tag -a v1.0.3.1 -m "IEUM Chain v1.0.3.1"
git push origin v1.0.3.1
```

태그 workflow가 바이너리와 SHA-256을 게시한 뒤 운영 업데이트를 실행합니다.

# IEUM Chain v1.0.2.1 직접 동기화·운영 복구

표시 버전은 `1.0.2.1`, Cargo 내부 버전은 `1.0.2-1`입니다. 코드 버전의 세 번째 숫자가 변경되므로 `v1.0.2.1` 태그는 Linux·Windows 릴리스 빌드를 실행합니다.

## 해결한 운영 장애

운영 4검증자 중 Node 1이 높이 4, Node 2~4가 높이 7인 상태에서 P2P 피어는 3개였지만 Node 1의 `syncHighest`가 4에 머물렀습니다. 공통 4번 블록, Chain ID와 genesis hash는 일치했으므로 체인 분기가 아니라 누락 블록 요청 전파 실패였습니다.

기존 코드는 동기화 요청·응답을 GossipSub `sync` 토픽으로만 발행했습니다. 물리 연결이 있어도 상대 토픽 구독이 아직 보이지 않으면 `NoPeersSubscribedToTopic`으로 요청이 폐기됐습니다.

v1.0.2.1은 다음 두 경로를 함께 사용합니다.

- 직접 경로: 연결된 각 PeerId에 `/ieum-chain/sync/1` request-response 요청
- 보조 경로: 이전 버전과 호환되는 GossipSub `ieum-chain/sync/2`

직접 응답의 `responder`는 실제 인증된 연결 PeerId와 일치해야 합니다. 코어는 기존처럼 서로 다른 두 피어의 동일한 tip/state root를 확인하고 BFT 확정 인증서를 검증한 뒤에만 블록을 적용합니다.

## 자동 테스트

전체 검증:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
bash tests/four_process_node1_persistent_rejoin.sh target/release/ieum-chain
```

Node 1 재현 테스트는 다음을 자동 수행합니다.

1. 테스트 검증자 4개를 완전 연결망으로 시작
2. 합의 후 Node 1 중단
3. Node 2~4만으로 거래 3건을 순서대로 확정
4. Node 1의 기존 원장과 P2P identity key를 그대로 재사용
5. 15초 이내 `syncHighest` 발견
6. 60초 이내 높이·state root 일치
7. 생존 피어의 동기화 요청 수신 로그 확인

각 거래는 대상 노드의 영수증 생성을 확인한 후 다음 거래를 제출합니다. 시스템 이벤트 블록만 증가한 상태를 송금 확정으로 오판하거나 동일 거래를 mempool에 중복 제출하지 않습니다.

운영 포트와 데이터는 사용하지 않으며 임시 디렉터리, UDP `7301~7304`, RPC `9301~9304`를 사용합니다.

## 쉬운 상태관리와 진단

```bash
# 전체 상태
sudo bash scripts/ieum-cluster-tool.sh status

# 종합 진단
sudo bash scripts/ieum-cluster-tool.sh diagnose

# Node 1 핵심 로그
sudo bash scripts/ieum-cluster-tool.sh -s 30m logs 1

# 비밀키·원장 본문을 제외한 장애 자료 수집
sudo bash scripts/ieum-cluster-tool.sh snapshot /tmp/ieum-report

# 데이터 삭제 없이 Node 1만 재시작
sudo bash scripts/ieum-cluster-tool.sh restart 1

# 개발 서버에서 운영 장애 재현
bash scripts/ieum-cluster-tool.sh reproduce target/release/ieum-chain
```

`restart` 외 명령은 읽기 전용입니다. 같은 높이에서 블록 해시가 다르면 재시작·삭제보다 먼저 `snapshot`으로 자료를 보존합니다.

## 내부·외부 P2P 주소 원칙

- 메인 서버 Node 1~4: `192.168.1.148`, UDP `7001~7004` 직접 연결
- 외부 공개: `node.ieum.aah.name`을 공인 IP 확인에 사용
- 외부 노드: DNS가 반환한 공인 IP와 UDP 포트로 QUIC 직접 연결
- 외부 통신은 Cloudflare UDP 프록시를 전제로 하지 않음
- 같은 LAN 직접 연결이 있으면 중첩 `/p2p-circuit` 경로를 학습하지 않음

## 브랜치·PR

변경 적용 후 개발 브랜치에서 검증합니다.

```bash
git switch dev
git status --short

git add -- \
  Cargo.toml Cargo.lock CHANGELOG.md README.md \
  src/lib.rs src/main.rs src/network.rs \
  scripts/diagnose-ieum-server.sh \
  scripts/diagnose-ieum-external.sh \
  scripts/ieum-cluster-tool.sh \
  tests/diagnostic_scripts.sh \
  tests/ci_regression_guard.sh \
  tests/four_process_node1_persistent_rejoin.sh \
  .github/workflows/rust-ci.yml \
  docs/README.md \
  docs/IEUM_USER_MANUAL_1.0.1.1.md \
  docs/VERSION_1.0.2.1_DIRECT_SYNC_RECOVERY.md

git diff --cached --check
git commit -m "fix: recover lagging validators with direct sync"
git push origin dev
```

GitHub에서 `dev`를 `main`으로 보내는 PR을 만들고 다음 필수 CI가 모두 성공한 뒤 병합합니다.

- fmt
- Clippy `-D warnings`
- 전체 테스트
- release build
- 기존 4프로세스 BFT 재합류 2회
- Node 1 영구 원장 다중 블록 재합류

CLI를 사용한다면:

```bash
gh pr create --base main --head dev \
  --title "IEUM Chain v1.0.2.1 direct sync recovery" \
  --body "Node 1 persistent-ledger multi-block rejoin regression and direct sync request-response recovery."
```

## 태그와 릴리스

PR 병합 및 `main` CI 성공 후에만 태그를 생성합니다.

```bash
git switch main
git pull --ff-only origin main
git status --short
git log -1 --oneline
git ls-remote --tags origin 'refs/tags/v1.0.2.1*'

git tag -a v1.0.2.1 -m "IEUM Chain v1.0.2.1"
git push origin v1.0.2.1
```

`-a`는 GPG 키가 필요 없는 annotated tag입니다. 태그 후 GitHub Actions 릴리스 작업에서 실행 파일과 SHA-256 파일 생성을 확인합니다.

## 운영 업데이트 후 확인

```bash
sudo bash scripts/ieum-cluster-tool.sh status
sleep 30
sudo bash scripts/ieum-cluster-tool.sh diagnose
sudo bash scripts/ieum-cluster-tool.sh -s 30m logs 1
```

정상 기준:

- 네 노드 버전 `1.0.2.1`
- Chain ID `21004`와 genesis hash 일치
- Node 1 `syncHighest`가 최고 높이를 발견
- 최종 높이·tip hash·state root 일치
- 로그에 `[동기화 직접 응답 완료]`

데이터 폴더 삭제나 전체 노드 동시 초기화는 이 절차에 포함하지 않습니다.

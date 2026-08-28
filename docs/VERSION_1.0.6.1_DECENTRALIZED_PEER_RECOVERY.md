# IEUM Chain v1.0.6.1 — 메인 서버 장애 후 분산 피어 복구

## 결론

v1.0.5.1은 연결 중인 인터넷 피어와 같은 LAN 피어끼리 메인 서버 없이 통신할 수 있었지만,
프로세스를 재시작하면 학습한 인터넷 피어 주소가 사라졌습니다. 메인 bootstrap DNS까지
중단되면 재시작한 노드가 기존 사용자망에 다시 진입할 보장도 없었습니다.

v1.0.6.1은 메인 bootstrap을 1순위로 유지하면서 다음 독립 경로를 추가합니다.

1. Identify로 검증해 학습한 공개 IPv4·IPv6·DNS·릴레이 주소를 최대 256개 저장
2. 시작 시 `data/network/known-peers.json`의 저장 피어에 자동 접속
3. 메인 bootstrap/DNS 오류가 발생해도 노드 시작을 중단하지 않음
4. 60초마다 Kademlia 라우팅 테이블 복구 및 임의 키 분산 탐색
5. 연결이 0개이면 메인 bootstrap과 저장 피어를 모두 재시도
6. 같은 LAN에서는 기존 mDNS 검색을 계속 사용

이는 Bitcoin의 영구 주소 관리자와 Ethereum/libp2p의 분산 피어 검색 원칙을 IEUM의
QUIC·Kademlia·mDNS·AutoNAT·relay 구조에 맞춘 것입니다.

## 운영 조건과 한계

- 이미 한 번이라도 공개 또는 릴레이 가능한 사용자 피어를 학습해야 완전한 서버 장애 후
  재시작 복구가 가능합니다. 최초 설치자가 세상에 혼자이고 모든 bootstrap도 없다면 어떤
  P2P 체인도 자동으로 상대 주소를 알아낼 수 없습니다.
- 거래 확정은 기존과 동일하게 운영 검증자 정족수가 필요합니다. 일반 사용자 노드만 남아
  있으면 원장·거래를 보관하고 전파할 수 있지만, 검증자 3/4 정족수가 사라지면 새 블록은
  확정되지 않습니다.
- NAT 내부 사용자만 존재하고 공개 포트나 살아 있는 릴레이가 하나도 없으면 인터넷을 통한
  신규 연결은 불가능합니다. 따라서 서로 다른 운영 주체의 공개 노드/릴레이를 권장합니다.
- `known-peers.json`은 개인키가 아니며 삭제해도 원장과 지갑은 손상되지 않지만, 서버 장애 시
  재발견 능력이 낮아집니다.

## 사용자·운영자 사용법

기본 실행은 별도 설정이 필요 없습니다. 노드가 자동으로 아래 파일을 관리합니다.

```text
data/network/known-peers.json
```

노드별 데이터 디렉터리가 다른 4노드 구성에서는 충돌 방지를 위해 각 서비스에 별도 경로를
지정할 수 있습니다.

```bash
ieum-chain server --peer-cache /opt/ieum-node1/data/network/known-peers.json
```

Docker 환경변수는 이번 변경에 추가되지 않았습니다. 기존 command/args 방식이라면
`--peer-cache`만 선택적으로 추가하고, 컨테이너의 `data/` 볼륨이 영구 마운트인지 확인합니다.

## 장애 시험

1. 서로 다른 인터넷망의 공개/릴레이 가능 노드 3개 이상을 실행합니다.
2. 각 노드에서 `known-peers.json` 생성과 `ieum_peerInfo` 피어 수를 확인합니다.
3. 메인 4개 bootstrap과 DNS를 시험 환경에서 차단합니다.
4. 사용자 노드를 완전히 종료 후 재시작합니다.
5. 로그의 `[P2P 저장 피어 접속 실패/시도]`, `[P2P 연결]`과 RPC 피어 수를 확인합니다.
6. 검증자 정족수가 살아 있는 상태에서 실제 서명 거래를 보내 receipt와 잔액을 확인합니다.

## 버전·검증·Git 배포 절차

표시 버전 `1.0.6.1`은 Cargo 버전 `1.0.6-1`과 대응합니다.

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked

git switch -c feat/v1.0.6.1-decentralized-peer-recovery
git add -- Cargo.toml Cargo.lock CHANGELOG.md PATCH_BASE.md README.md \
  src/main.rs src/network.rs docs/README.md \
  docs/IEUM_USER_MANUAL_1.0.1.1.md \
  docs/VERSION_1.0.6.1_DECENTRALIZED_PEER_RECOVERY.md
git commit -m "feat: recover peers without central bootstrap"
git push -u origin feat/v1.0.6.1-decentralized-peer-recovery
gh pr create --base dev --head feat/v1.0.6.1-decentralized-peer-recovery \
  --title "IEUM Chain v1.0.6.1 decentralized peer recovery" --draft
```

PR을 `dev`에서 실제 장애 시험한 뒤 보호된 `main`으로 병합하고 태그를 만듭니다.

```bash
git switch main
git pull --ff-only origin main
test "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "1.0.6-1"
git tag -a v1.0.6.1 -m "IEUM Chain v1.0.6.1"
git push origin v1.0.6.1
```

태그 후 GitHub Actions의 Linux·Windows·macOS 빌드와 서명된 업데이트 manifest 생성을
확인합니다. `download/update-manifest.json`은 릴리스 서명이 필요하므로 소스 패치에서 임의로
수정하지 않습니다.

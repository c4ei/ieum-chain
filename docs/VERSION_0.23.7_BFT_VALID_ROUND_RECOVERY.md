# IEUM Chain v0.23.7 — BFT valid-round 잠금 복구

## 1. 장애 원인

v0.23.6은 더 높은 라운드의 서명된 round-change quorum을 따라갈 수 있었지만,
제안에는 `valid_round` 숫자만 포함했습니다. 서로 다른 블록에 잠긴 검증자는 그 숫자가
실제로 2/3 초과 prevote를 받은 라운드인지 검증할 수 없으므로 제안을 거부했습니다.
그 결과 모든 노드가 같은 높이와 원장 해시를 유지하면서도 라운드만 계속 증가할 수
있었습니다.

## 2. v0.23.7 합의 변경

- `SignedProposal`에 `valid_round_prevotes`를 추가했습니다.
- 각 prevote의 높이, 라운드, 블록 해시, 투표 종류, 검증자 등록 여부와 서명을
  검증합니다.
- 중복 검증자와 검증자 수보다 큰 인증서는 거부하며, 총 투표권의 정확히 2/3는
  부족하고 2/3 초과만 인증서로 인정합니다.
- 인증서의 `valid_round`는 현재 제안 라운드보다 작아야 합니다.
- 다른 블록에 잠긴 검증자는 인증서 라운드가 자기 `locked_round` 이상일 때만 새 값을
  받아들입니다. 증명 없는 잠금 해제는 허용하지 않습니다.
- 제안자 서명 도메인을 `IEUM-PROPOSAL-V3`로 변경하고 인증서의 정렬된 검증자 ID와
  서명 목록을 제안 서명에 포함했습니다.
- v0.23.6 메시지와 혼용되지 않도록 합의 topic을 `ieum-chain/consensus/2`, Identify
  protocol을 `/ieum-chain/1.2.0`으로 올렸습니다.
- 자동 업데이트 설정은 저장소에 남은 과거 manifest가 아니라 서명된 최신 GitHub
  Release의 `update-manifest.json`을 조회합니다.

따라서 합의에 참여하거나 합의 메시지를 관측해야 하는 모든 노드는 v0.23.7로 함께
업데이트해야 합니다. 제네시스, Chain ID 21004, 원장 데이터는 변경하지 않습니다.

## 3. 4검증자와 5노드의 의미

현재 제네시스 검증자 주소는 4개입니다. 화면의 `활성 노드 4/4`는 합의 검증자 수이며,
외부 VM 한 대가 일반 P2P 노드라면 합의권을 갖지 않습니다.

5노드 완전 연결에서 각 노드의 `peers` 값은 자기 자신을 제외한 `4`가 정상입니다.
관리자 피어 상세 화면에서 고유 Peer ID가 5개라면 네트워크 노드 발견도 정상입니다.
외부 VM을 5번째 검증자로 바꾸는 작업은 단순 피어 추가가 아니라 검증자 세트 변경이며,
제네시스 이후에는 별도의 epoch/거버넌스 절차 없이 `validators.json`만 수정하면 안 됩니다.

## 4. LAN 주소와 공개 DNS

- 같은 서버에서 실행되는 Docker 네 노드와 LAN의 Manager RPC는
  `192.168.1.148:7001~7004`, `192.168.1.148:8989~8992`를 사용할 수 있습니다.
- 외부 VM과 신규 사용자가 접속할 bootstrap 주소는 저장소 기준
  `node.ieum.aah.name:7001~7004/UDP`입니다.
- `inode.aah.name`으로 임의 변경하지 않습니다. 실제 운영 DNS 이름과 각 PeerId를 먼저
  확인해야 합니다.
- `node.ieum.aah.name`은 QUIC/UDP이므로 Cloudflare 프록시가 아니라 DNS only여야 합니다.

확인 예시:

```bash
getent ahostsv4 node.ieum.aah.name
grep -nE 'bootstrap|advertise' config/bootstrap.json config/network.json
```

Docker 컨테이너가 `network_mode: host`이면 `192.168.1.148`의 서로 다른 7001~7004
포트를 직접 사용합니다. 이 구성에서는 `7002:7001` 같은 Docker 포트 매핑을 추가하지
않습니다.

## 5. 로컬 검증

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

추가 회귀 범위:

- 다른 값에 잠긴 노드가 더 높은 유효 라운드의 2/3 prevote 인증서를 수락
- 2/3 이하 인증서 거부
- 인증서를 포함한 proposal의 P2P 직렬화 왕복
- 기존 4프로세스 합의 및 블록 확정 테스트

## 6. dev 커밋과 PR

모든 검사가 통과한 뒤 실행합니다.

```bash
git switch dev
git pull --rebase origin dev

git add \
  .github/workflows/chain-release.yml \
  Cargo.toml Cargo.lock CHANGELOG.md README.md config/update.json \
  src/consensus.rs src/consensus_runtime.rs src/network.rs \
  docs/VERSION_0.23.7_BFT_VALID_ROUND_RECOVERY.md

git commit -m "fix: recover certified BFT locks v0.23.7"
git push origin dev
```

GitHub에서 base `main`, compare `dev`로 PR을 만들거나 다음을 실행합니다.

```bash
gh pr create \
  --base main \
  --head dev \
  --title "IEUM Chain v0.23.7 certified BFT lock recovery" \
  --body "Adds verifiable valid-round prevote certificates and safe lock recovery."
```

Rust CI가 모두 통과한 뒤 PR을 main에 병합합니다.

## 7. main 태그와 Release

```bash
git switch main
git pull --ff-only origin main

test "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "0.23.7"

git tag -a v0.23.7 -m "IEUM Chain v0.23.7"
git push origin v0.23.7
```

`Build and release IEUM Chain`에서 Linux, Windows, macOS와 signed Release가 모두
성공한 뒤에만 운영 서버를 업데이트합니다.

## 8. 운영 배포

합의 wire가 변경되므로 외부 VM을 포함한 5개 노드를 모두 v0.23.7로 업데이트합니다.
네 검증 노드의 순차 교체가 끝날 때까지 새 거래를 보내지 않습니다. 원장 디렉터리와
검증자 키, P2P 키는 삭제하거나 교체하지 않습니다.

Docker 서버:

```bash
sudo /opt/ieum-docker-four-node/update-four-nodes.sh

for node in ieum-node1 ieum-node2 ieum-node3 ieum-node4; do
  docker exec "$node" /image/ieum-chain --version
done
```

외부 VM도 해당 서비스 방식에 맞게 v0.23.7 바이너리로 교체하고 재시작합니다.

## 9. 배포 후 판정

```bash
for port in 8989 8990 8991 8992; do
  curl -sS "http://192.168.1.148:$port" \
    -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"ieum_nodeStatus","params":[]}' \
  | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r["version"],r["height"],r["mempoolTransactions"],r["peers"],r["blockHash"])'
done
```

소액 거래 한 건을 보낸 뒤 다음을 확인합니다.

- 네 검증 노드의 높이, 블록 해시, state root가 동일
- 높이가 증가하고 mempool이 0으로 감소
- `peers=4`인 경우 전체 5노드 연결로 해석
- `valid_round` 인증서 오류나 라운드 무한 증가가 없음

```bash
docker compose logs --since=10m --timestamps \
  | grep -E 'BFT 라운드|valid_round|제안 거부|투표 거부|panic|ERROR'
```

높이가 증가하지 않고 라운드만 계속 증가하면 데이터를 삭제하지 말고 네 노드의 상태와
로그를 보존한 뒤 배포를 중단합니다.

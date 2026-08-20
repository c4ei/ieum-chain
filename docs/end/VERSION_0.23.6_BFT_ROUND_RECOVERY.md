# IEUM Chain v0.23.6 — BFT 라운드 자동 복구

## 1. 장애 원인

v0.23.5에서는 검증 노드가 같은 높이에서 서로 다른 라운드에 도달하면 다른 라운드의
제안과 투표를 거부했다. 각 노드는 단계별 timeout으로 자기 라운드만 계속 증가했으므로
다음 로그가 반복되고 재시작 전까지 블록을 확정하지 못할 수 있었다.

```text
[BFT 제안 거부] 현재 높이/라운드와 다른 제안입니다.
[BFT 투표 거부] 현재 높이/라운드와 다른 투표입니다.
[BFT 라운드 변경] 단계별 제한 시간 초과, 새 라운드 ...
```

거래가 일부 노드의 mempool에만 남고 최종 확정 높이와 잔액이 바뀌지 않는 현상도 이
합의 정지의 결과였다.

## 2. v0.23.6 동작

- propose, prevote, precommit은 각각 설정된 제한시간만 기다린다. 무한 대기하지 않는다.
- timeout한 검증자는 블록 prevote/precommit과 도메인이 분리된 `SignedRoundChange`를
  검증자 키로 서명해 합의 topic에 전파한다. 이미 블록에 투표했어도 이중투표가 아니다.
- 서명 payload에는 Chain ID와 Genesis commitment가 포함되어 테스트망·이전
  제네시스의 round-change 재전송 공격을 거부한다.
- 등록 검증자 투표권의 **1/3 초과**가 더 높은 라운드에서 관측되면 뒤처진 노드가 그
  라운드로 catch-up한다. 단일 검증자는 라운드 점프를 강제할 수 없다.
- 같은 라운드의 round-change가 **2/3 초과**이면 그 라운드는 종료된 것으로 검증하고
  다음 라운드로 이동한다.
- 피어 재연결 시 현재 높이에 대해 보관 중인 최근 round-change 투표를 최대 64개만
  재전파한다. 메모리와 네트워크 사용량은 제한된다.
- 현재 라운드보다 4,096을 초과한 비정상 미래 투표는 거부하고, 메모리에는 최대
  128개 라운드만 추적해 등록 검증자 오동작으로 인한 무제한 대기를 막는다.
- RPC를 받은 노드가 현재 제안자인지와 관계없이 대기 거래를 최대 1,000개, 2초
  간격으로 재전파한다. 제안 실패나 라운드 변경이 발생해도 다른 검증자가 거래를
  확보할 수 있다.
- 블록 확정 기준은 기존과 동일하게 등록 검증자 투표권 **2/3 초과 precommit**이다.

이 변경은 Genesis, Chain ID, 원장 형식, 프로토콜 버전을 바꾸지 않는다. 기존 메인넷
원장과 `config/genesis.json`을 그대로 사용한다.

## 3. 회귀 검증

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

테스트는 다음을 확인한다.

1. 1/3 이하의 미래 라운드 투표로는 로컬 라운드가 바뀌지 않는다.
2. 1/3 초과의 서명된 미래 라운드 투표로 catch-up한다.
3. 2/3 초과 round-change이면 다음 라운드로 이동한다.
4. 미등록 검증자의 유효한 서명도 라운드 변경 권한으로 인정하지 않는다.
5. 다른 Chain ID 또는 Genesis commitment의 서명 메시지는 거부한다.
6. deadline 전에는 기다리고 deadline 도달 시 즉시 서명된 round-change를 만든다.
7. 4프로세스 테스트는 한 노드 RPC에만 거래를 제출한 뒤 네 노드의 높이, state root,
   수신 잔액이 동일하게 확정되는지 확인한다.
8. `SignedRoundChange`가 합의 P2P wire encoding을 왕복하고 서명을 다시 검증한다.

## 4. 운영 배포

구버전 노드는 서명 round-change를 전파하지 않으므로 네 검증 노드를 장시간 혼합
운영하지 않는다. 빌드와 CI가 통과한 동일 바이너리를 준비한 뒤 네 컨테이너를 짧은
유지보수 창에서 함께 재생성한다. 데이터 bind mount는 삭제하지 않는다.

```bash
cd /opt/ieum-docker-four-node
sudo ./update-four-nodes.sh

docker compose ps
for node in ieum-node1 ieum-node2 ieum-node3 ieum-node4; do
  docker exec "$node" /node/ieum-chain --version
done
```

네 노드가 모두 `0.23.6`인지 확인하고 원장 신원을 검사한다.

```bash
for port in 8989 8990 8991 8992; do
  curl -sS "http://192.168.1.148:$port" \
    -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"ieum_networkIdentity","params":[]}' \
    | python3 -m json.tool
done
```

모두 Chain ID `21004`, 동일 Genesis hash, 동일 protocol version이어야 한다. 그다음
0.01 IEUM 시험 거래 한 건만 보내고 30초 이내에 네 노드 높이·블록 해시·state root가
일치하며 mempool이 0으로 돌아오는지 확인한다. 실패하면 반복 전송하지 말고 로그와
미확정 nonce를 먼저 확인한다.

## 5. dev 커밋, PR, main 병합 및 태그

압축 파일을 저장소 루트에 덮어쓴 뒤 반드시 변경 범위와 테스트를 확인한다.

```bash
git switch dev
git pull --ff-only origin dev

git status --short
git diff --check

cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked

git add Cargo.toml Cargo.lock CHANGELOG.md README.md \
  src/consensus.rs src/consensus_era.rs src/consensus_runtime.rs \
  src/lib.rs src/main.rs src/network.rs \
  docs/VERSION_0.23.6_BFT_ROUND_RECOVERY.md
git commit -m "fix: recover divergent BFT rounds v0.23.6"
git push origin dev
```

GitHub에서 `base: main`, `compare: dev`로 PR을 생성한다. 모든 Actions가 통과하고 파일
목록에 위 파일만 포함됐는지 확인한 뒤 merge한다. 명령행을 사용한다면:

```bash
gh pr create \
  --base main \
  --head dev \
  --title "IEUM Chain v0.23.6 BFT round recovery" \
  --body "Signed round-change catch-up, bounded timeouts and transaction re-gossip."
```

PR 병합 후 로컬 main을 갱신하고, 서명키가 없는 환경에서는 GPG 태그 `-s`가 아니라
annotated 태그 `-a`를 사용한다.

```bash
git switch main
git pull --ff-only origin main

test "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "0.23.6"

git tag -a v0.23.6 -m "IEUM Chain v0.23.6"
git push origin v0.23.6
```

태그를 먼저 만들지 않는다. 태그는 PR이 병합된 main의 검증 완료 커밋에만 생성한다.

# IEUM Chain v1.0.5.1 — 실제 서명 거래·유휴 후 첫 거래 E2E

## 확인 결과

- v1.0.4.1의 Rust 전체 테스트 147개는 수정 전 상태에서 모두 통과했습니다.
- `eth_sendRawTransaction` 해석·서명자 복구·RPC 재시작 후 잔액·nonce·영수증 보존 단위 테스트가 이미 존재했습니다.
- 기존 4프로세스 BFT 시험의 송금은 노드가 대신 구성하는 `eth_sendTransaction`이어서 Wallet·MetaMask의 실제 서명 raw 거래 경로를 끝까지 검증하지 않았습니다.
- 합의가 유휴 상태에 들어간 뒤 들어온 첫 거래를 실제 프로세스에서 명시적으로 재현하는 시험도 없었습니다.

## v1.0.5.1 변경

1. 고정 CI faucet 키로 서명한 EIP-155 legacy 거래를 `eth_sendRawTransaction`으로 제출합니다.
2. chain ID 21005, nonce 0, gas price 1, gas limit 21000, 0.1 IEUM 송금을 실제 4노드 BFT로 확정합니다.
3. 네 노드에서 수신 잔액, height, state root, 영수증 status와 확정 nonce 1을 모두 확인합니다.
4. 합의 timeout보다 긴 15초 유휴 뒤 첫 거래를 제출하여 재시작 없이 합의가 다시 시작되는지 확인합니다.
5. 실제 하루 유휴 시험은 같은 스크립트를 `IEUM_CI_IDLE_WAIT_SECONDS=86400`으로 실행합니다.

## `--server` 보상 확인

- 승인된 검증자의 일일 이자는 `ValidatorDailyInterest` 합의 이벤트로 연결되어 있습니다.
- 일반 `--server` 노드는 보상 주소가 활성 검증자에게 합계 **100 IEUM 이상을 위임하고 7일이 지난 뒤** 자격을 얻습니다.
- 하루 80% 이상 실제 연결을 서로 다른 활성 검증자 3명 이상이 서명한 경우에만 `NodeServiceDailyReward` 합의 이벤트를 만듭니다.
- 모든 검증자는 증명 서명, 담보 snapshot, epoch, 대상과 금액을 다시 계산하며 결과가 다르면 블록을 거부합니다.
- 등록 시 영구 PeerId 개인키와 보상용 secp256k1 지갑 양쪽의 소유권 서명을 확인합니다.
- 메인 검증 서버의 PeerId는 일반 공개 노드 후보에서 제외합니다.
- 동일 보상 주소는 하루 한 번만 인정하고 IPv4 `/24` 또는 IPv6 `/48` 대역당 최대 2개만 지급해 값싼 대량 노드 생성을 제한합니다.
- 일일 풀은 재단 잔액과 1,000 IEUM 중 작은 값이 상한이며 적격 노드에 결정론적으로 나눕니다. 신규 무제한 발행이나 고정 이율이 아닙니다.
- 연결이 끊기거나 검증자가 재시작되면 연속 활동 관측을 다시 시작하므로 재시작으로 가동 시간을 위조할 수 없습니다.

설계는 Geth처럼 PeerId·피어 제한을 연결 계층의 1차 방어로 유지하면서, 보상 계층에는
Dash의 담보·결정론적 등록·PoSe 개념과 Ethereum PoS의 경제적 담보 원칙을 보수적으로
적용했습니다. 참고: [Geth P2P](https://geth.ethereum.org/docs/fundamentals/peer-to-peer),
[Dash masternode collateral](https://docs.dash.org/en/stable/docs/user/masternodes/setup.html),
[Dash Proof of Service](https://docs.dash.org/en/stable/docs/core/guide/dash-features-proof-of-service.html),
[Ethereum PoS](https://ethereum.org/developers/docs/consensus-mechanisms/pos/).

## 로컬 검증

```bash
cargo fmt --all
cargo fmt --all --check
bash tests/ci_regression_guard.sh
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --locked
bash tests/four_process_network.sh target/debug/ieum-chain
```

하루 유휴 후 첫 거래는 별도 운영 시험에서 실행합니다.

```bash
IEUM_CI_IDLE_WAIT_SECONDS=86400 \
  bash tests/four_process_network.sh target/debug/ieum-chain
```

## dev 커밋·푸시

```bash
git switch dev
git pull --ff-only origin dev

git status --short
git add -- \
  Cargo.toml Cargo.lock CHANGELOG.md PATCH_BASE.md README.md \
  docs/README.md docs/IEUM_USER_MANUAL_1.0.1.1.md \
  docs/VERSION_1.0.5.1_RAW_IDLE_E2E.md \
  src/chain.rs src/consensus_runtime.rs src/lib.rs src/main.rs src/network.rs \
  src/node_emission.rs src/raw_transaction.rs src/scheduled_event.rs src/staking.rs \
  tests/ci_regression_guard.sh tests/four_process_network.sh

git commit -m "feat: secure public node daily rewards"
git push origin dev
```

GitHub에서 `dev`를 head, `main`을 base로 하는 PR을 만들고 CI가 모두 성공한 뒤 병합합니다.

## main 병합 확인·태그

```bash
git switch main
git pull --ff-only origin main

test "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "1.0.5-1" \
  && echo "버전 확인 완료: 1.0.5.1"

git tag -a v1.0.5.1 -m "IEUM Chain v1.0.5.1"
git push origin v1.0.5.1
```

GPG 비밀키가 설정되지 않은 PC에서는 `git tag -s`가 아니라 위의 서명 없는 annotated tag `git tag -a`를 사용합니다.

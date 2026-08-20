# IEUM Chain v0.23.8 — 거래 수수료 표시와 재시작 기본기

## 변경 목적

v0.23.7의 실제 송금과 4검증자 BFT 확정은 정상 동작했지만, Ethereum 조회 응답의
`gasPrice`가 전체 수수료로 표시되어 MetaMask·익스플로러가 수수료를 잘못 해석할
수 있었습니다. 또한 확정 뒤 프로세스를 다시 생성했을 때 잔액, nonce, 영수증이
함께 복원되는 경로를 한 테스트에서 검증하지 않았습니다.

## 수수료 정책

- 송신자는 `전송액 + 수수료`를 지불합니다.
- 수수료의 20%는 재단 주소
  `0x356456ff1216b57a6f8891b195b42d296789b67d`에 적립합니다.
- 나머지 80%와 나눗셈 나머지는 블록 생성자에게 적립합니다.
- Ethereum legacy raw 거래는 원본 `gasPrice × gasLimit`을 수수료로 사용합니다.
- IEUM 자체 서명 거래는 총 수수료만 합의 필드에 저장하므로 RPC에서는
  `gas=1`, `gasPrice=fee`로 표현해 곱이 실제 수수료와 일치하게 합니다.

## 추가된 검증

`tests/v0_23_8_operational_basics.sh`는 다음 항목을 빠르게 검사합니다.

1. 재단 20%·블록 생성자 80% 수수료 배분
2. EIP-155 legacy 거래의 송신자·금액·nonce·gas 복원
3. IEUM 자체 거래의 정확한 총 수수료 표시
4. BFT 확정 상태 설치 후 RPC 재시작
5. 재시작 전후 잔액·nonce·거래 영수증 일치

기존 CI는 이 스크립트에 이어 전체 Rust 테스트, release 빌드와 실제 4프로세스 BFT
송금 테스트를 계속 실행합니다.

## 로컬 검증

```bash
cargo fmt --all
cargo fmt --all --check
bash tests/ci_regression_guard.sh
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
bash tests/v0_23_8_operational_basics.sh
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
```

## dev 커밋과 main PR

```bash
git switch dev
git add -- .github/workflows/rust-ci.yml Cargo.toml Cargo.lock CHANGELOG.md README.md \
  src/raw_transaction.rs src/rpc.rs tests/ci_regression_guard.sh \
  tests/v0_23_8_operational_basics.sh \
  docs/VERSION_0.23.8_TRANSACTION_RESTART_BASICS.md
git commit -m "fix: verify transaction restart basics v0.23.8"
git push origin dev
```

GitHub에서 `dev`를 head, `main`을 base로 PR을 만들고 모든 CI 성공 후 merge합니다.

## 태그

```bash
git switch main
git pull --ff-only origin main
test "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "0.23.8" \
  && echo "버전 확인 완료: 0.23.8"
git tag -a v0.23.8 -m "IEUM Chain v0.23.8"
git push origin v0.23.8
```

GPG 비밀키가 준비되지 않은 운영 PC에서는 `git tag -s`가 아니라 위의 annotated tag
명령인 `git tag -a`를 사용합니다.

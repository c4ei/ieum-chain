# IEUM Chain v1.0.1.1 토픽 재가입·동기화 복구

표시 버전은 `1.0.1.1`, Cargo 내부 버전은 `1.0.1-1`입니다.

## 장애 원인과 수정

Docker/LAN 운영 환경에서 bootstrap 피어와 QUIC 연결은 성공했지만, mDNS로 발견된 피어만 gossipsub explicit peer로 등록되었습니다. 재시작한 Node 1은 peers=3인데도 `NoPeersSubscribedToTopic` 상태가 지속되어 5초 주기의 sync 요청이 전파되지 않았고 높이 4에 머물렀습니다.

- 모든 직접 연결 피어를 sync/consensus/block 토픽 전파 대상으로 즉시 등록
- 마지막 연결이 닫힐 때만 explicit peer 해제
- 활성 합의 중 최근 round-change 투표를 2초 간격으로 제한 재전파하여 한 번의 gossip 유실 뒤에도 생존 검증자 라운드 재정렬
- 동기화 요청·요청 수신·응답 수신에 한국어 진단 로그 추가
- 실서버·외부 PC 진단 쉘과 `-h` 도움말 추가
- CI 재합류 테스트에서 실제 높이·tip hash·state root 일치 검증
- 동일한 4프로세스 장애·재합류 시험을 연속 2회 실행하여 간헐적 라운드 경합 검출
- 릴리스 버전의 네 번째 숫자만 바뀌면 바이너리 빌드를 생략하는 정책 추가

## 진단 도구

실서버:

```bash
bash scripts/diagnose-ieum-server.sh -h
sudo bash scripts/diagnose-ieum-server.sh -H 192.168.1.148
```

외부 PC:

```bash
bash scripts/diagnose-ieum-external.sh -h
bash scripts/diagnose-ieum-external.sh -H 192.168.1.148
```

## 검증

```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
bash tests/diagnostic_scripts.sh
bash tests/release_build_policy.sh
```

## Git·PR·Tag

```bash
git switch -c fix/v1.0.1.1-gossipsub-sync
git add -- Cargo.toml Cargo.lock src/lib.rs src/main.rs src/network.rs \
  scripts/diagnose-ieum-server.sh scripts/diagnose-ieum-external.sh \
  scripts/should-build-release.sh tests docs README.md CHANGELOG.md \
  .github/workflows/chain-release.yml .gitignore
git commit -m "fix: recover gossipsub sync after peer reconnect"
git push -u origin fix/v1.0.1.1-gossipsub-sync
gh pr create --base main --head fix/v1.0.1.1-gossipsub-sync \
  --title "IEUM Chain v1.0.1.1 sync recovery" \
  --body "Bootstrap peer topic registration, diagnostics and rejoin regression tests."
```

PR 병합과 CI 성공 후:

```bash
git switch main
git pull --ff-only origin main
git tag -a v1.0.1.1 -m "IEUM Chain v1.0.1.1"
git push origin v1.0.1.1
```

## 빌드 버전 정책

`vMAJOR.MINOR.PATCH.DOC` 구조에서 앞의 세 숫자가 실행 코드 버전이고 마지막 숫자는 문서·패키징 개정입니다. 직전 태그와 `MAJOR.MINOR.PATCH`가 같으면 CI와 문서 검증은 수행하되 OS별 바이너리 빌드는 생략합니다. 필요하면 workflow 수동 실행에서 `force_build=true`로 강제할 수 있습니다.

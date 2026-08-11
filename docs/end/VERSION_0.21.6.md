# IEUM Chain v0.21.6 변경 내역

## 적용 내용

- 버전을 `0.21.6`으로 올렸다.
- 실제 4프로세스 BFT 테스트는 실행 파일을 절대 경로로 확정한 뒤 각 노드를
  서로 다른 빈 임시 작업 디렉터리에서 실행한다.
- CI 노드는 저장소 루트의 운영용 `config/network.json`과
  `config/update.json`을 읽지 않는다. 따라서 `node.ieum.aah.name` 접속,
  운영 공개 주소 광고 및 자동 업데이트 확인이 테스트망에 섞이지 않는다.
- 테스트 노드는 `--no-default-bootstrap`을 유지하고 노드 1의 localhost QUIC
  주소만 노드 2~4에 명시하여 같은 테스트 실행 안에서만 연결한다.
- 종료·실패 시 네 노드 프로세스와 임시 원장을 정리한다.
- Rust 1.97의 `manual_range_contains` 경고가 오류가 되지 않도록 보상 활성화
  기간 판정을 `RangeInclusive::contains`로 변경했다. 보상 계산 결과는 동일하다.

## 검증 명령

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
```

## 배포 순서

1. 위 검증과 GitHub Actions가 모두 성공한 커밋에 `v0.21.6` 태그를 만든다.
2. GitHub에 `v0.21.6` 태그가 보이는 것을 확인한다.
3. 그 후 IEUM Wallet `0.0.10.5` Actions를 실행한다.

## 다음 작업

- Normal 노드 자동 보상을 실제 발행하기 전, 서명된 중계 영수증의 합의 전파와
  누적 지급량의 원장·스냅샷·상태 해시 영속화를 구현하고 별도 하드포크에서 켠다.
- 메인 4노드의 실제 PeerId를 보상 정책 파일에 등록한다.

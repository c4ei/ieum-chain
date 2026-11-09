# IEUM Chain v1.0.7.2 — 재시작 후 과거 snapshot 서명 보류

## 원인과 변경

저장 상태 DB에서 최신 높이만 복원하면 메모리 canonical 체인에는 과거 블록이 없을 수 있습니다. 이때 더 오래된 로컬 체크포인트가 미인증 상태로 남아 있으면, 주기 인증 작업이 메모리에서 해당 높이의 블록을 찾지 못해 오류를 반환합니다.

v1.0.7.2는 snapshot 서명 전에 체크포인트 높이의 블록이 현재 메모리 canonical 체인에 존재하고 block hash가 일치하는지 확인합니다. 과거 블록이 메모리에 없거나 hash가 다르면 서명을 보류하고 제한 로그를 남깁니다. 서명 검증 자체는 완화하지 않으며, 확인할 수 없는 체크포인트를 임의로 인증하지 않습니다.

현재 canonical 블록에 연결된 체크포인트는 기존과 같이 인증 대상이 됩니다. 과거 체크포인트를 인증해야 하는 경우에는 독립적인 canonical 검증 증거를 제공하는 별도 경로가 필요합니다.

## 검증

저장 tip이 90인 상태에서 높이 82 체크포인트만 있는 경우 서명을 보류하고, 현재 높이 90에 맞는 체크포인트는 인증 대상으로 유지하는 회귀 테스트를 추가했습니다.

```bash
cargo fmt --all --check
cargo test --locked stale_checkpoint_after_state_restore_is_skipped_without_losing_current_checkpoint
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
```

## 버전과 배포 경계

- Cargo 버전: `1.0.7-2`
- 표시 버전: `1.0.7.2`
- 운영 노드, 원장, 키, Docker 이미지 변경은 이 코드 변경에 포함되지 않습니다.
- 브랜치 push만으로 운영 바이너리나 서명된 업데이트 manifest가 배포되지 않습니다. `main` CI와 검토·병합, 릴리스 tag 및 서명 release 절차를 별도로 완료해야 합니다.
- 운영 이미지에 `curl`이 없는 경우 자동 업데이트도 실패하므로, 런타임 이미지 수정은 별도 승인과 검증이 필요합니다.

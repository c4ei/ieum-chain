# v0.23.0 최종 작업·배포 인수인계

## 큰 변경

`TransactionAction`을 기존 거래에 선택 필드로 추가하고 일반 송금은 과거 서명 바이트를 그대로 유지했습니다. 위임·해제·청구만 새 도메인 분리 서명에 포함됩니다. 위임 상태는 canonical state schema 2, 체크포인트, 인증 snapshot, 상태 루트와 공급량 RPC에 포함됩니다. schema 1의 빈 위임 상태는 기존 state root와 호환됩니다.

이중투표 증거는 서명을 다시 검증하고 활성 검증자인지 확인한 뒤 프로토콜 v3 블록 이벤트로만 5% 페널티를 적용합니다. 페널티 금액은 재단으로 이동하므로 공급량이 변하지 않습니다. 위임 보상도 합의 노드가 snapshot·정책 hash·지급 목록을 다시 계산합니다.

## 배포 순서

1. 네 노드의 원장, `config`, 키를 각각 백업한다.
2. CI의 fmt, clippy, 전체 테스트, release build가 모두 성공한 커밋만 사용한다.
3. 4대 모두 v0.23.0 바이너리를 설치하되 아직 프로토콜 v3 높이는 미래로 둔다.
4. 모든 노드가 정상 합의하는지 확인한다.
5. 현재 높이보다 최소 20블록 뒤의 동일 활성화 높이를 합의하고 `config/upgrades.json`을 4대에 똑같이 배포한다.
6. 설정 hash와 서비스 로그를 비교하고 한 대씩 재시작한다.
7. 활성화 뒤 1 IEUM 소액으로 delegate → undelegate를 시험한다. claim은 해제 높이 이후 시험한다.

활성화 높이를 이미 지난 뒤 구버전 노드가 남아 있으면 새 거래를 제출하지 말고 모든 검증자를 같은 버전으로 맞춰야 합니다. 원장을 삭제하거나 다른 노드 원장을 복사하지 않습니다.

프로토콜 v3 블록에서 위임 거래가 한 번이라도 확정된 뒤에는 v0.22.x 바이너리로 롤백할 수 없습니다. 장애 시에는 v0.23.0 코드의 수정 릴리스를 배포하거나, 모든 검증자가 합의한 인증 snapshot 복구 절차를 사용해야 합니다.

## PR·승인

```bash
git switch -c feature/staking-v0.23.0
git add Cargo.toml Cargo.lock README.md src docs
git commit -m "feat: add consensus-safe staking v0.23.0"
git push -u origin feature/staking-v0.23.0
gh pr create --draft --title "IEUM Chain v0.23.0 실제 잠금형 위임" \
 --body "합의 v3 전환, 위임 회계, 합의 시각 기준 7일 해제, 보상, 이중투표 페널티를 포함합니다."
```

체인 개발자, 운영 책임자, 재단 회계 담당자가 각각 코드·활성화 높이·재단 지급 상한을 승인한 뒤 병합합니다. CI가 하나라도 실패하면 태그를 만들지 않습니다.

```bash
git tag -a v0.23.0 -m "IEUM Chain v0.23.0"
git push origin v0.23.0
```

세부 RPC와 보안 기준은 `docs/VERSION_0.23.0.md`를 봅니다.

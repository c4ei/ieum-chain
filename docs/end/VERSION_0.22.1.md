# IEUM Chain 0.22.1 운영 관측 기능

작성일: 2026-08-12

## 작업 내용

v0.22.1은 운영 웹과 모니터링 시스템이 체인 상태를 안전하게 조회하도록 다음 JSON-RPC를 추가합니다.

| 메서드 | 파라미터 | 반환 내용 |
| --- | --- | --- |
| `ieum_supplyStatus` | 없음 | 총발행·유통·잠금 잔액과 기준 높이 |
| `ieum_addressBalances` | `[offset, limit]` | 주소순 전체 잔액 인덱스(최대 1,000건) |
| `ieum_validatorStatus` | `[window]` | validator set과 최근 확정 인증서 서명률 |
| `ieum_blockProductionStatus` | `[window]` | 평균 생성 시간, 지연 구간, 추정 누락 슬롯 |

금액은 JavaScript 정밀도 손실을 막기 위해 10진 문자열 `wei`로 반환합니다. 1 IEUM은 `10^18 wei`입니다.

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"ieum_supplyStatus","params":[]}' \
  http://127.0.0.1:8989
```

## 공급량 정의

- 총발행량은 현재 원장의 모든 주소 잔액 합입니다. 합의 이벤트로 발행된 보상도 포함됩니다.
- 잠금 잔액은 `config/genesis.json`의 `locked_addresses`에 등록된 주소 잔액 합입니다.
- 유통량은 `총발행량 - 잠금 잔액`입니다.
- 잠금 주소는 제네시스 잔액에 존재해야 하며 중복될 수 없습니다.

예제 설정은 오분류를 피하기 위해 빈 배열입니다. 실제 락업 주소는 메인넷 제네시스 확정 전에 입력해야 합니다. 운영 후 목록 변경은 제네시스 해시를 바꾸므로 네트워크 전체 합의 없이 수행하면 안 됩니다.

## 서명률과 블록 누락 해석

서명률은 저장·검증된 확정 인증서의 precommit 기준이며 재시작 때 인증서 기록을 복원합니다. 긴 기간의 빠른 추세 조회는 Prometheus가 담당합니다. 누락은 “없는 블록 높이”가 아니라 목표 3초 대비 긴 시간 간격의 추정 슬롯입니다. 벌점·보상 계산에는 직접 사용하지 마십시오.

## Prometheus/Grafana와 알림

`monitoring/ieum_exporter.py`가 로컬 RPC를 조회해 `/metrics`를 제공합니다. 한 서버의 네 노드는 exporter 포트를 9104~9107처럼 분리합니다. `prometheus.yml`, `alerts.yml`, `grafana-dashboard.json`을 각각 Prometheus와 Grafana에 적용합니다.

```bash
sudo install -d -m 0750 /etc/ieum
sudo tee /etc/ieum/exporter-node1.env >/dev/null <<'EOF'
IEUM_RPC_URL=http://127.0.0.1:8989
IEUM_EXPORTER_LISTEN=127.0.0.1
IEUM_EXPORTER_PORT=9104
EOF
sudo install -m 0644 monitoring/ieum-exporter@.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now ieum-exporter@node1
curl -sS http://127.0.0.1:9104/metrics
```

`alertmanager.yml.example`을 복사하고 SMTP와 Telegram 값을 실제 비밀 파일에 설정합니다. 비밀번호와 bot token은 Git에 커밋하지 않습니다.

## 관리자 작업 감사 로그

계정 생성·가져오기·잠금 해제 및 거래 제출 RPC는 `data/ledger/audit/admin-actions.jsonl`에 시각, 메서드, 성공 여부, PID, 파라미터 SHA-256만 기록합니다. 니모닉·개인키·비밀번호·거래 원문은 저장하지 않습니다. 파일 권한은 서비스 계정 전용으로 제한하고 중앙 로그에 별도 보존하십시오.

## 검증 및 릴리스

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
python3 -m py_compile monitoring/ieum_exporter.py

git add Cargo.toml Cargo.lock CHANGELOG.md README.md config/genesis.json src monitoring docs/VERSION_0.22.1.md
git commit -m "IEUM Chain v0.22.1 관리 모니터링"
git tag -a v0.22.1 -m "IEUM Chain v0.22.1"
git push origin main
git push origin v0.22.1
```

이 변경 묶음에는 새 바이너리가 없습니다. 위 검증을 모두 통과한 커밋에서 기존 서명 릴리스 절차로 바이너리와 update manifest를 생성해야 합니다.

## 이후 추가하면 좋은 내용

1. 확정 인증서 통계를 재시작 후에도 유지하는 영구 시계열 인덱스
2. state root와 cursor를 결합한 대규모 주소 페이지 일관성 보장
3. 락업·베스팅 일정과 해제를 합의 상태로 관리하는 공급량 정책
4. 제안 성공률, 라운드 변경, prevote/precommit 단계별 지연 지표
5. 감사 로그 hash chain, 원격 WORM 보관, 인증 주체·요청 ID 연계
6. Alertmanager 이중화와 장애 대응 runbook
7. 운영 웹 WebAuthn MFA, 역할 권한, 이중 승인 워크플로

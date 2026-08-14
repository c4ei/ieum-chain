# v0.22.9 최종 작업·배포 인수인계

## 큰 이벤트와 핵심 기준

이번 큰 이벤트는 “일반 지갑도 정해진 기간에 매일 응원 보상을 받게 하자”입니다. `holder-rewards.json`을 모든 이음지기에 동일하게 배포하고 KST 하루 단위 스냅샷으로 계산합니다.
연 5% 예시는 `99.9999 × 5% ÷ 365 = 0.013698616438 IEUM`이며 12자리에서 반올림합니다. 재단 잔액에서 지급하므로 신규 발행량에는 더하지 않습니다. 이음지기 보상도 같은 표시 반올림 기준을 사용합니다.

보안 기준은 개인키 서버 미보관, 합의 노드의 정책 해시·스냅샷·금액 재검증, 일일 총액 상한, 이벤트 ID 중복 방지입니다. 잠금형 위임은 이번 범위가 아닙니다.

## 빌드와 테스트

```bash
git clone https://github.com/c4ei/ieum-chain.git
cd ieum-chain
cargo fmt --check
cargo test --lib
cargo build --release
```

## 배포

먼저 테스트넷/한 노드에서 RPC 상태를 확인합니다. 그 다음 같은 바이너리와 같은 `config/holder-rewards.json`을 4개 노드에 배포하되 한 대씩 재시작하고, 
매번 블록 높이와 피어를 확인합니다. 이벤트 활성화 전 재단 잔액·최소 잔액·일일 상한·시작/종료 Unix 초를 재확인합니다. 롤백은 이전 바이너리와 비활성화된 설정 파일로 한 대씩 되돌립니다.

## GitHub PR과 승인

```bash
git switch -c feature/holder-reward-v0.22.9
git add Cargo.toml Cargo.lock src config docs
git commit -m "feat: add scheduled holder rewards v0.22.9"
git push -u origin feature/holder-reward-v0.22.9
gh pr create --draft --title "IEUM Chain v0.22.9 보유 응원 보상" --body "테스트 결과와 배포 순서를 포함합니다."
```

GitHub에서 CI가 모두 통과한 뒤 최소 1명의 코드 리뷰 승인을 받습니다. 재단 지급·합의 변경은 작성자가 혼자 승인하지 말고 별도 운영 책임자가 설정값과 공급량 회계를 확인해야 합니다. 
승인 후 Draft를 Ready로 바꾸고 병합합니다.

세부 설계는 `docs/VERSION_0.22.9.md`를 봅니다.

IEUM Chain 0.21.7 #13 변경 내역

개요

GitHub Actions의 4프로세스 BFT 테스트에서 합의와 송금 상태 반영이 모두 완료됐지만,수신 주소의 최종 잔액을 잘못 판정해 테스트가 실패하던 문제를 수정했다.

원인

고정 수신 주소 0x3252b7b65e50B54508974dB8d634134B0bd6be90는 GitHub Actions용개발 제네시스에서 이미 1 IEUM을 보유한다. 기존 테스트는 0.1 IEUM 송금 후 최종 잔액이정확히 0.1 IEUM이라고 가정했다.

따라서 실제로는 네 노드 모두 다음과 같이 정상 합의했는데도 제한 시간 초과로 처리됐다.

높이: 2 2 2 2

상태 루트: 네 노드 동일

수신 잔액: 1.1 IEUM (1100000000000000000 wei)

변경 사항

송금 전에 네 노드의 수신 주소 잔액을 조회한다.

송금 전 잔액이 네 노드에서 동일한지 검증한다.

송금 후 절대 잔액 대신 송금 전 잔액 + 0.1 IEUM인지 검증한다.

높이, 상태 루트, 네 노드의 최종 잔액 일치 검증은 그대로 유지한다.

GitHub Actions 격리 개발망 옵션 --git_action_test를 그대로 유지한다.

영향 범위

변경 파일: tests/four_process_network.sh

코어 합의, 블록 생성, 거래 처리, RPC 및 운영망 설정은 변경하지 않는다.

테스트 성공 판정만 실제 제네시스 잔액에 맞게 수정한다.

검증 명령

bash -n tests/four_process_network.sh
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
bash tests/four_process_network.sh target/release/ieum-chain

정상 완료 시 다음 형식의 메시지가 출력된다.

4-process BFT passed: heights=2 2 2 2, stateRoot=<동일 상태 루트>, recipientBalance=1100000000000000000

릴리스 태그 생성 및 푸시

GitHub Actions가 성공한 커밋을 로컬에서 확인한 뒤 그 커밋에 v0.21.7 annotated tag를생성하고 원격 저장소에 푸시한다. 아래의 <성공한_커밋_SHA>는 Actions 화면에 표시된성공 커밋의 전체 SHA로 바꾼다.

git fetch origin main --tags
git show --no-patch --oneline <성공한_커밋_SHA>
git tag -a v0.21.7 <성공한_커밋_SHA> -m "IEUM Chain v0.21.7"
git push origin v0.21.7

원격 태그가 성공 커밋을 가리키는지 확인한다.

git ls-remote --tags origin refs/tags/v0.21.7 refs/tags/v0.21.7^{}
git show --no-patch --decorate v0.21.7

v0.21.7 태그가 이미 존재한다면 덮어쓰거나 강제 푸시하지 말고 먼저 대상을 확인한다.

git fetch origin tag v0.21.7
git show --no-patch --decorate v0.21.7
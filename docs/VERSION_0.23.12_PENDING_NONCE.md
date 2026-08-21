# IEUM Chain v0.23.12 — pending nonce 수정

`eth_getTransactionCount(address, "latest")`는 확정 원장의 다음 nonce를 반환합니다.
`"pending"`은 그 뒤에 해당 주소가 제출한 연속된 mempool 거래까지 포함한 다음 nonce를
반환합니다. 다른 주소의 거래나 nonce 중간 공백은 포함하지 않습니다.

이 수정으로 Wallet은 확정되지 않은 이전 거래를 새 거래로 교체하거나 동일 거래를
중복 제출하지 않고 사용자에게 먼저 처리 중인 거래를 확인하도록 안내할 수 있습니다.

네 검증자 모두 동일한 v0.23.12 바이너리로 교체해야 합니다.


cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked

scripts/make-node-package.sh 0.23.12

PR을 `main`에 병합하고 CI 성공을 확인한 뒤 태그를 생성한다.

```bash
git switch main
git pull --ff-only origin main
git tag -a v0.23.12 -m "IEUM Chain v0.23.12"
git push origin v0.23.12
```

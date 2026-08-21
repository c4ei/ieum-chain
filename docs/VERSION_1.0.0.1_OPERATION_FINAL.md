# IEUM Chain v1.0.0.1 운영 안정화

표시 버전은 `1.0.0.1`, Cargo 내부 버전은 표준에 맞춘 `1.0.0-1`입니다.

## 검증

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
```

## Git 반영과 배포 태그

```bash
git switch dev
git pull --ff-only origin dev
git add -- Cargo.toml Cargo.lock src/lib.rs src/main.rs src/rpc.rs src/updater.rs tests/four_process_network.sh \
  .github/workflows/chain-release.yml README.md CHANGELOG.md docs/VERSION_1.0.0.1_OPERATION_FINAL.md
git commit -m "release: IEUM Chain v1.0.0.1 operational final"
git push origin dev
```

GitHub에서 `dev → main` PR을 만들고 CI 성공 후 병합합니다.

```bash
git switch main
git pull --ff-only origin main
git tag -a v1.0.0.1 -m "IEUM Chain v1.0.0.1"
git push origin v1.0.0.1
```

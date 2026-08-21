# IEUM Chain v0.23.11 변경분 적용

이 압축파일은 저장소 전체가 아니라 변경·추가 파일만 포함합니다.

```bash
cd ~/www/ieum-chain
git switch dev
tar -xJf ~/다운로드/ieum-chain-v0.23.11-changed-only.tar.xz
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
git add -A
git commit -m "fix: expose pending transactions and add chain diagnostics v0.23.11"
git push origin dev
```

PR을 main에 병합하고 CI가 성공한 뒤에만 `v0.23.11` 태그를 만드세요. Cargo 버전도
이미 `0.23.11`로 맞춰져 있습니다. 네 검증자 모두 같은 바이너리로 교체해야 합니다.

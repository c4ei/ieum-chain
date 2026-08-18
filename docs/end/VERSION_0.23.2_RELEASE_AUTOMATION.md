# IEUM Chain v0.23.2 다중 운영체제 Release 자동화

## 작업 내용

- 애플리케이션 표시 버전을 `0.23.1`에서 `0.23.2`로 올린다. 프로토콜이나 원장 형식 변경은 없다.
- `vX.Y.Z` 태그가 푸시되면 GitHub Actions가 자동으로 Release를 생성한다.
- 이미 원격에 존재하는 태그는 `workflow_dispatch`의 `release_tag` 입력으로 최초 Release를 만들 수 있다.
- 태그 버전과 `Cargo.toml` 버전이 다르면 배포를 중단한다.
- Linux x86_64, Windows x86_64, macOS Intel, macOS Apple Silicon 바이너리를 각각 네이티브 러너에서 빌드한다.
- 모든 플랫폼에서 테스트 후 `--release --locked` 빌드를 수행한다.
- 각 바이너리의 SHA-256 파일을 생성하고 Release 게시 전에 다시 검증한다.
- 모바일 전체 노드는 배터리, 백그라운드 실행, P2P 포트 및 키 보안 제약 때문에 이번 배포 대상에서 제외한다.

## 생성되는 파일

- `ieum-chain-linux-x86_64`
- `ieum-chain-linux-x86_64.sha256`
- `ieum-chain-windows-x86_64.exe`
- `ieum-chain-windows-x86_64.exe.sha256`
- `ieum-chain-macos-x86_64`
- `ieum-chain-macos-x86_64.sha256`
- `ieum-chain-macos-aarch64`
- `ieum-chain-macos-aarch64.sha256`

## 최초 v0.23.2 Release 실행

압축 파일 적용 후 `Cargo.lock`의 로컬 패키지 버전을 동기화하고 검증한다.

```bash
cd ~/www/ieum-chain
cargo check
cargo fmt --all --check
cargo test --all-targets --all-features --locked
git diff --check
```

`cargo check`가 `Cargo.lock`의 `ieum-chain` 항목을 `0.23.2`로 갱신한다. 변경사항을 `dev`에 커밋하고 PR로 `main`에 병합한 뒤 CI가 성공하면 태그를 생성한다.

```bash
cd ~/www/ieum-chain
git switch main
git pull --ff-only origin main

test "$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)" = "0.23.2"
git tag -s v0.23.2 -m "IEUM Chain v0.23.2"
git push origin v0.23.2
```

서명 태그를 사용할 GPG 키가 없다면 키를 먼저 구성한다. 임시로 서명 없는 태그를 사용하려면 저장소 정책을 확인한 뒤 `git tag v0.23.2`를 사용한다.

## 확인

```bash
gh run list --repo c4ei/ieum-chain --workflow chain-release.yml --limit 3
gh run watch --repo c4ei/ieum-chain
gh release view v0.23.2 --repo c4ei/ieum-chain
```

`v0.23.2` 태그가 이미 원격에 있어 다시 푸시할 수 없다면 다음 명령을 사용한다.

```bash
gh workflow run chain-release.yml \
  --repo c4ei/ieum-chain \
  --ref main \
  -f release_tag=v0.23.2
```

동일한 태그를 다시 사용하지 않는다. 다음 코드 배포는 `Cargo.toml`과 관련 버전 파일을 먼저 올린 후 새 태그(예: `v0.23.3`)로 실행한다.

# IEUM Chain v1.0.1.1 변경분

GitHub `main` 기준 변경된 파일만 포함합니다. 저장소 루트에 적용:

```bash
tar -xJf ieum-chain-v1.0.1.1-changed-only.tar.xz -C /tmp/ieum-v1.0.1.1
bash /tmp/ieum-v1.0.1.1/apply-v1.0.1.1.sh ~/www/ieum-chain
```

적용 스크립트는 기존 `docs/end`와 v1.0.1.1 이전 작업 문서를 먼저 하나의 통합 백업 문서로 합친 뒤, 최신 코드·문서·진단 도구를 덮어씁니다. 사용자 매뉴얼 파일명은 유지됩니다.

검증 환경에는 Rust toolchain이 없어 이 압축본 제작 단계에서는 한국어 진단 쉘, 릴리스 정책, Bash 문법과 diff 검증을 수행했습니다. 적용 서버 또는 GitHub CI에서 전체 Cargo 검증을 실행해야 합니다.

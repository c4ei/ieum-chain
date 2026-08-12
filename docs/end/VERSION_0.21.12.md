# IEUM Chain 0.21.12

멀티 인스턴스 서버에서 파일과 업데이트 대상을 바이너리별로 격리하는 릴리스입니다.

- 기준 폴더: `std::env::current_exe()`의 부모 폴더
- 기본 keystore: `<바이너리 폴더>/data/keystore`
- 기본 계정 암호: `<바이너리 폴더>/secure/ieum-account.password`
- 업데이트 설정: `<바이너리 폴더>/config/update.json`
- 업데이트 대상: 현재 실행 중인 바이너리 자신
- 외부에서 전달한 절대경로: 변경하지 않음

예를 들어 `/opt/ieum-node3/ieum-chain`은 셸의 현재 폴더나 systemd
`WorkingDirectory`와 관계없이 `/opt/ieum-node3`만 기준으로 사용합니다.

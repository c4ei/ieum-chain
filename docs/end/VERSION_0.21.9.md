# IEUM Chain 0.21.9

## 수정 내용

- `scripts/ieum-chain-systemd-update-all.sh`를 추가했다.
- 한 서버에서 서로 다른 경로의 바이너리로 실행되는 여러 IEUM Chain systemd
  서비스를 자동 탐색한다.
- 서비스를 한 개씩 중지하고 서명 manifest와 SHA-256 검증을 거쳐 실행 파일만
  업데이트한 뒤, 해당 RPC가 정상 응답할 때만 다음 서비스로 진행한다.
- RPC 확인 실패 시 그 서비스의 `.previous` 바이너리를 복구하고 전체 작업을
  중단한다.
- 기존 `server --port ... --rpc-port ...` 실행 인자는 변경하지 않는다.

## 실행

```bash
export IEUM_UPDATE_MANIFEST_URL='https://raw.githubusercontent.com/c4ei/ieum-chain/main/download/update-manifest.json'
# export IEUM_RELEASE_PUBLIC_KEY='config/update.json의 release_public_key 값'
export IEUM_RELEASE_PUBLIC_KEY='1b53691ba18362c0729bfba3de94b1d1a5bb630cd132820f4f85c03944e4a53d'
sudo -E ./scripts/ieum-chain-systemd-update-all.sh
```

서비스를 직접 지정할 수도 있다.

```bash
sudo -E ./scripts/ieum-chain-systemd-update-all.sh \
  ieum-chain.service ieum-node1.service ieum-node2.service ieum-node3.service
```

검증자 여러 대를 운영하는 경우에는 이 스크립트를 서버 한 대씩 실행하고 블록
확정과 동기화를 확인한 뒤 다음 서버로 넘어간다.

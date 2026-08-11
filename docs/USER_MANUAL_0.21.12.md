# IEUM Chain 0.21.12 멀티 인스턴스 계정·업데이트 안내

모든 상대경로의 기준은 현재 실행한 `ieum-chain` 바이너리의 폴더입니다.

| 실행 바이너리 | 계정 암호 | keystore | 업데이트 설정 |
| --- | --- | --- | --- |
| `/opt/ieum-node1/ieum-chain` | `/opt/ieum-node1/secure/ieum-account.password` | `/opt/ieum-node1/data/keystore` | `/opt/ieum-node1/config/update.json` |
| `/opt/ieum-node2/ieum-chain` | `/opt/ieum-node2/secure/ieum-account.password` | `/opt/ieum-node2/data/keystore` | `/opt/ieum-node2/config/update.json` |
| `/opt/ieum-node3/ieum-chain` | `/opt/ieum-node3/secure/ieum-account.password` | `/opt/ieum-node3/data/keystore` | `/opt/ieum-node3/config/update.json` |

## 계정 생성과 가져오기

```bash
cd /tmp
/opt/ieum-node2/ieum-chain account new
/opt/ieum-node2/ieum-chain account import secure/private-key.hex
/opt/ieum-node2/ieum-chain account list
```

어느 폴더에서 호출해도 node2의 파일만 사용합니다. `account new` 또는 `import`를
처음 실행할 때 `secure/ieum-account.password`가 없으면 0600 권한의 임의 암호를
자동 생성합니다. 이후 `send`도 같은 파일을 자동 사용합니다. 계정마다
`data/keystore/UTC--timestamp--주소` 파일이 누적됩니다.

외부에서 관리하는 암호를 쓰려면 바이너리 폴더 기준 상대경로나 절대경로를
명시할 수 있습니다.

```bash
/opt/ieum-node2/ieum-chain account new \
  --password-file secure/operator.password
```

## IEUM 전송과 조회

```bash
/opt/ieum-node2/ieum-chain account send \
  --from 0x보내는주소 \
  --to 0x받는주소 \
  --amount 0.1 \
  --fee 0.000001 \
  --rpc-port 8992

/opt/ieum-node2/ieum-chain account balance 0x주소 --rpc-port 8992
/opt/ieum-node2/ieum-chain account transaction 0x거래해시 --rpc-port 8992
/opt/ieum-node2/ieum-chain account receipt 0x거래해시 --rpc-port 8992
```

## 자동 업데이트

각 프로세스는 자기 바이너리 옆 `config/update.json`만 읽습니다. 서명과 SHA-256을
확인한 신규 바이너리는 `current_exe()`가 가리키는 자기 실행 파일만 교체하며,
다른 `/opt/ieum-node*` 폴더에는 접근하지 않습니다. systemd 재시작 후 새 버전이
실행됩니다.

절대경로로 지정한 옵션은 인스턴스 기준으로 다시 붙이지 않고 그대로 사용합니다.

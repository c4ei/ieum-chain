# IEUM Chain 사용자 매뉴얼 1차 완료본

기준 버전: `0.21.13`  
운영체제: Ubuntu Linux  
기본 단위: `1 IEUM = 10^18 wei`

## 1. 인스턴스 원칙

모든 기본 상대경로는 셸의 현재 폴더가 아니라 실행 중인 바이너리 폴더를 기준으로
합니다. `/opt/ieum-node2/ieum-chain`은 node2의 설정, 원장, 계정과 업데이트 대상만
사용합니다.

```text
/opt/ieum-node2/
├── ieum-chain
├── config/
├── data/ledger/
├── data/keystore/
├── logs/
└── secure/ieum-account.password
```

한 서버에서 node1, node2, node3를 함께 실행해도 각 폴더를 분리하십시오. 검증자 키,
P2P node key, keystore와 암호 파일을 인스턴스 사이에서 복사해 공유하면 안 됩니다.

## 2. 설치와 시작 전 확인

```bash
sudo install -m 755 target/release/ieum-chain /opt/ieum-node2/ieum-chain
/opt/ieum-node2/ieum-chain --version
/opt/ieum-node2/ieum-chain node doctor
```

systemd 설정의 `ExecStart`는 해당 인스턴스 바이너리와 고유 P2P/RPC 포트를 사용해야
합니다. RPC는 기본적으로 localhost에만 바인딩하십시오.

```bash
sudo systemctl daemon-reload
sudo systemctl restart ieum-node2.service
sudo systemctl status ieum-node2.service --no-pager
sudo journalctl -u ieum-node2.service -n 100 --no-pager
```

## 3. 계정 생성·가져오기·백업

새 계정을 생성하면 계정별 암호화 keystore 파일이 누적됩니다. 기본 암호 파일이
없으면 인스턴스의 `secure/ieum-account.password`가 권한 `0600`으로 생성됩니다.

```bash
/opt/ieum-node2/ieum-chain account new
/opt/ieum-node2/ieum-chain account list
find /opt/ieum-node2/data/keystore -maxdepth 1 -type f -printf '%f\n'
```

기존 32바이트 secp256k1 개인키를 가져올 때만 import를 사용합니다. 개인키 파일에는
`0x`를 제외한 64자리 hex를 넣고 권한을 `0600`으로 제한합니다.

```bash
/opt/ieum-node2/ieum-chain account import secure/private-key.hex
```

복구에는 keystore와 암호 파일이 모두 필요합니다. 다음 두 경로를 암호화된 별도
저장소에 함께 백업하십시오.

```text
/opt/ieum-node2/data/keystore/
/opt/ieum-node2/secure/ieum-account.password
```

개인키, 암호 파일, `validator.key`, `server.node.key`를 Git이나 메신저에 올리지 마십시오.

## 4. 잔액·송금·거래 조회

```bash
/opt/ieum-node2/ieum-chain account balance 0x주소 --rpc-port 8992

/opt/ieum-node2/ieum-chain account send \
  --from 0x보내는주소 \
  --to 0x받는주소 \
  --amount 0.1 \
  --fee 0.000001 \
  --rpc-port 8992

/opt/ieum-node2/ieum-chain account transaction 0x거래해시 --rpc-port 8992
/opt/ieum-node2/ieum-chain account receipt 0x거래해시 --rpc-port 8992
```

receipt가 `null`이면 아직 확정되지 않았을 수 있습니다. `status: 0x1`과 block number가
표시되면 확정 성공입니다. 전송 전에는 주소, 금액, fee, chain ID를 다시 확인하십시오.

## 5. 블록 익스플로러 연동

익스플로러는 journal 로그가 아니라 JSON-RPC를 사용해야 합니다.

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  http://127.0.0.1:8992

curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["latest",true],"id":2}' \
  http://127.0.0.1:8992
```

권장 인덱싱 순서:

1. `eth_chainId`와 `ieum_networkIdentity`로 대상 네트워크를 확인합니다.
2. `eth_blockNumber`로 최신 높이를 읽습니다.
3. 높이별 `eth_getBlockByNumber`로 블록과 거래를 저장합니다.
4. `eth_getTransactionReceipt`로 거래 확정 상태를 저장합니다.
5. 재시작 시 마지막 저장 높이와 블록 해시를 다시 대조합니다.

지원 핵심 조회는 `eth_getBlockByHash`, `eth_getTransactionByHash`,
`eth_getBlockTransactionCountByNumber`, `eth_getTransactionReceipt`, `eth_getBalance`입니다.
현재 `eth_getLogs`는 빈 배열이며 EVM contract log 인덱싱 용도로 사용할 수 없습니다.

외부 공개가 필요하면 reverse proxy에서 TLS, IP 허용목록, rate limit과 요청 크기 제한을
적용하십시오. `personal_*`, mnemonic, unlock 계열은 localhost 밖에서 사용하지 마십시오.

## 6. 운영 로그 판별

v0.21.13부터 정상 블록 로그는 다음처럼 한 줄로 표시됩니다.

```text
[P2P 블록 수신] PeerId: ..., 높이: 2, 해시: ..., 거래: 1개, 시스템 이벤트: 0개
```

같은 블록을 여러 피어에서 받아도 최초 한 번만 출력하며 raw 거래와 서명은 출력하지
않습니다. 이 변경은 블록 수신·검증·합의나 익스플로러 RPC에 영향을 주지 않습니다.

`AutoNAT DialFailure`, `DialRefused`, `NoServer`는 외부 역접속 판정 실패입니다. P2P
연결·동기화가 정상이고 외부 공개 노드가 아니라면 거래 실패를 뜻하지 않습니다.
panic, state root 불일치, 서명 검증 실패, 지속적인 동기화 정지는 즉시 조사하십시오.

## 7. 업데이트와 운영 점검

자동 업데이트는 각 인스턴스의 `config/update.json`과 현재 실행 바이너리만 사용합니다.
검증자 운영에서는 새 버전을 CI와 별도 노드에서 검증한 뒤 순차 배포하십시오.

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"ieum_syncStatus","params":[],"id":1}' \
  http://127.0.0.1:8992

curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"net_peerCount","params":[],"id":2}' \
  http://127.0.0.1:8992
```

배포 전 원장과 키를 백업하고, 한 번에 모든 검증자를 중지하지 마십시오. 배포 후에는
버전, peer count, sync status, 최신 높이, 시험 거래 receipt를 확인합니다.

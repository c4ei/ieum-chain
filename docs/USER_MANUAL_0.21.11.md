# IEUM Chain 0.21.11 기본 계정·송금 사용자 안내

사용자 자산 주소는 모두 secp256k1 기반 `0x` + 40자리입니다. Ed25519 64자리 값은
검증자·P2P 식별과 과거 호환에만 사용하며 입금 주소로 사용하지 않습니다.

## 암호 파일 준비

```bash
install -m 600 /dev/null /secure/ieum-account.password
vi /secure/ieum-account.password
```

암호는 10자 이상이어야 하며 명령행 인수나 Git에 넣지 않습니다.

## 계정 생성·가져오기·목록

```bash
./ieum-chain account new --password-file /secure/ieum-account.password
./ieum-chain account import /secure/private-key.hex \
  --password-file /secure/ieum-account.password
./ieum-chain account list
```

개인키 파일은 `0x` 접두사가 선택적인 정확히 64자리 hex입니다. 생성·가져오기 결과는
`data/keystore/UTC--timestamp--주소`에 암호화되어 계정별로 누적됩니다. 기존
`data/keystore/주소.json` 파일도 계속 읽습니다.

로컬 RPC에서 같은 저장소에 계정을 생성하거나 가져올 수 있습니다.

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"personal_newAccount","params":["10자이상암호"]}' \
  http://127.0.0.1:8989

curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":2,"method":"personal_importRawKey","params":["64자리개인키","10자이상암호"]}' \
  http://127.0.0.1:8989
```

`personal_*` 요청에는 암호·개인키가 포함되므로 RPC는 localhost에만 바인딩하고
프록시 접근 로그에 요청 본문을 남기지 마세요.

## 잔액·송금·트랜잭션 조회

```bash
./ieum-chain account balance 0x주소 --rpc-port 8989

./ieum-chain account send \
  --from 0x보내는주소 \
  --to 0x받는주소 \
  --amount 0.1 \
  --fee 0.000001 \
  --password-file /secure/ieum-account.password \
  --rpc-port 8989

./ieum-chain account transaction 0x거래해시 --rpc-port 8989
./ieum-chain account receipt 0x거래해시 --rpc-port 8989
```

`account send`가 출력한 `0x` 거래 해시로 조회합니다. 블록 확정 전에는 transaction과
receipt 결과가 `null`일 수 있으며, 확정 후 receipt의 `status`가 `0x1`이면 성공입니다.

기존 JSON-RPC `eth_getBalance`, `eth_getTransactionCount`, `eth_sendTransaction`,
`eth_sendRawTransaction`, `eth_getTransactionByHash`, `eth_getTransactionReceipt`,
`eth_getBlockByNumber`도 유지됩니다.

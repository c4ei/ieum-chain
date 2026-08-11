# geth / EVM 스크립트 호환 범위

## v0.0.4 raw transaction

`eth_sendRawTransaction`은 EIP-155가 적용된 legacy type-0 단순 송금을
지원합니다. 체인 ID가 `--chain-id`와 다르거나 수신 주소가 없거나 data가
비어 있지 않으면 거부합니다. 반환값은 raw transaction의 Keccak-256
해시입니다. EIP-1559 type-2, 컨트랙트 생성과 EVM 실행은 아직 미지원입니다.

## 목적

v0.0.4는 기존 geth/web3 운영 스크립트 가운데 주소 생성, 계정 목록, 잔액,
nonce와 관리형 계정 송금을 먼저 호환합니다. HTTP JSON-RPC 기본 주소는
`http://127.0.0.1:8545`입니다.

사용자 계정은 geth와 같은 secp256k1 개인키를 사용합니다. 주소도 비압축
공개키에서 `0x04`를 제외한 64바이트를 Keccak-256으로 해시하고 마지막
20바이트를 취하는 Ethereum 표준 방식입니다. 같은 개인키를 geth에 가져오면
동일한 주소가 생성됩니다.

seed 문구는 BIP-39 영어 단어를 사용하며 HD 파생 경로는 MetaMask와 널리
쓰이는 `m/44'/60'/0'/0/n`입니다. 같은 seed와 index를 사용하면 동일한
계정 주소를 재현합니다. 검증자 합의 서명과 P2P node key는 사용자 자산
계정과 역할이 다르므로 기존 Ed25519를 유지합니다.

## 지원 메서드

| 메서드 | 상태 | 설명 |
|---|---|---|
| `web3_clientVersion` | 지원 | IEUM 노드 버전 |
| `net_version` | 지원 | 10진수 chain ID |
| `net_listening`, `net_peerCount` | 지원 | 네트워크 기본 상태 |
| `rpc_modules` | 지원 | 활성 namespace 목록 |
| `eth_chainId` | 지원 | hex chain ID |
| `eth_syncing` | 지원 | 현재는 `false` |
| `eth_blockNumber` | 지원 | 최신 확정 높이 |
| `eth_accounts` | 지원 | 노드 관리형 계정 |
| `eth_coinbase` | 지원 | 개발용 faucet 계정 |
| `personal_newAccount` | localhost 지원 | 암호화 keystore에 영구 저장 |
| `personal_importRawKey` | localhost 지원 | raw secp256k1 키를 암호화해 가져오기 |
| `personal_importRawKey` | 개발용 지원 | geth형 32바이트 secp256k1 개인키 가져오기 |
| `ieum_newMnemonic` | IEUM 확장 | BIP-39 12단어 및 0번 계정 생성 |
| `ieum_importMnemonic` | IEUM 확장 | seed와 index로 표준 HD 계정 복원 |
| `personal_unlockAccount` | 개발용 지원 | 관리형 계정 여부만 확인 |
| `eth_getBalance` | 지원 | `latest` 조회 |
| `eth_getTransactionCount` | 지원 | 다음 nonce |
| `eth_gasPrice`, `eth_estimateGas` | 임시 지원 | v0.0.4 고정 개발값 |
| `eth_getCode` | 지원 | 계약 미지원이므로 `0x` |
| `eth_sendTransaction` | 개발용 지원 | 관리형 계정이 서명하고 즉시 소형 블록 생성 |
| `personal_sendTransaction` | 개발용 지원 | `eth_sendTransaction`과 동일 경로 |
| `eth_sendRawTransaction` | 부분 지원 | EIP-155 legacy 단순 송금 |

## curl 사용 예

계정 목록:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_accounts","params":[]}'
```

주소 생성:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":2,"method":"personal_newAccount","params":["개발용암호"]}'
```

geth 개인키 가져오기:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":3,"method":"personal_importRawKey","params":["0x개인키","개발용암호"]}'
```

BIP-39 seed와 첫 주소 생성:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":4,"method":"ieum_newMnemonic","params":[]}'
```

기존 seed의 index 0 주소 복원:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":5,"method":"ieum_importMnemonic","params":["영어 seed 12단어",0]}'
```

잔액 조회:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":3,"method":"eth_getBalance","params":["0x주소","latest"]}'
```

송금:

```bash
curl -s http://127.0.0.1:8545 \
  -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":4,"method":"eth_sendTransaction","params":[{"from":"0x보내는주소","to":"0x받는주소","value":"0x64","gasPrice":"0x1"}]}'
```

모든 수량은 Ethereum JSON-RPC 관례대로 `0x` hex quantity입니다.

## 기존 geth JavaScript 예

```javascript
const Web3 = require("web3");
const web3 = new Web3("http://127.0.0.1:8545");

const accounts = await web3.eth.getAccounts();
const balance = await web3.eth.getBalance(accounts[0]);
console.log(accounts[0], balance);
```

web3.js 버전에 따라 `personal_newAccount`는 provider의 직접 RPC 호출 또는
`web3.eth.personal.newAccount()`를 사용합니다.

## 보안 주의

- RPC 기본 리스닝 주소는 localhost입니다.
- v0.0.4의 관리형 계정은 메모리 기반이라 노드 재시작 후 자동 복구되지 않습니다.
- v0.21.11부터 `personal_newAccount`와 `personal_importRawKey`는 CLI와 같은
  `data/keystore`에 계정별 암호화 파일을 저장합니다. 외부 공개 RPC에서는 차단됩니다.
- `ieum_newMnemonic` 응답의 seed는 RPC 로그나 화면 캡처에 남기지 말고 오프라인에
  안전하게 보관해야 합니다. seed를 잃으면 복구할 수 없고 노출되면 자산을 잃습니다.
- 테스트 코인만 사용하세요.
- 외부 공개 RPC에는 인증, TLS reverse proxy, 요청 속도 제한과 CORS 정책이
  추가되기 전까지 개인키 관리 메서드를 노출하면 안 됩니다.

## 정확한 호환 범위

- 호환: secp256k1 개인키, Ethereum 주소 계산, BIP-39 seed, BIP-44 파생 경로,
  주소·잔액·nonce 관련 JSON-RPC
- IEUM 내부 형식: 현재 코인 거래 서명/직렬화와 블록 형식
- 미지원: geth V3 암호화 keystore, EIP-2718/EIP-1559 typed transaction
  서명 복구, receipt/log, EVM bytecode 및 Solidity 계약

따라서 기존 스크립트 중 `eth_accounts`, `eth_getBalance`,
`eth_getTransactionCount`, 관리형 `eth_sendTransaction`은 사용할 수 있지만,
MetaMask가 만드는 raw transaction을 그대로 보내는 기능은 아직 지원하지 않습니다.

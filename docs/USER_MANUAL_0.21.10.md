# IEUM Chain 0.21.10 사용자 명령 안내

## 주소가 두 종류로 보였던 이유

- `0x` + 40자리: IEUM 잔액을 보유하고 송금·리워드를 받는 secp256k1 계정 주소
- 64자리 hex: 검증자 합의, 노드 등록, P2P 신원 증명에 쓰는 Ed25519 공개키

두 값은 역할이 다르며 서로 변환하거나 잘라 쓰면 안 됩니다. 0.21.10부터
`reward address`도 일반 지갑과 같은 `0x` 계정 주소만 새로 생성합니다. 과거 블록과
등록 기록 검증을 위해 체인 내부에서는 구형 64자리 보상 주소를 읽기 전용으로
허용합니다.

## 일반 계정 만들기

암호를 명령행 인자로 넣지 말고 소유자만 읽을 수 있는 파일로 준비합니다.

```bash
install -m 600 /dev/null /secure/ieum-account.password
vi /secure/ieum-account.password
./ieum-chain account new --password-file /secure/ieum-account.password
```

출력 예시는 `0x7e5f...5bdf` 형식입니다. 기본 저장 위치는 `data/keystore/`입니다.

```bash
./ieum-chain account list
```

## 일반 계정에서 보내기

로컬 노드 RPC가 실행 중이어야 합니다. 기본 포트는 8989입니다.

```bash
./ieum-chain account send \
  --from 0x보내는주소 \
  --to 0x받는주소 \
  --amount 0.1 \
  --fee 0.000001 \
  --password-file /secure/ieum-account.password \
  --rpc-port 8989
```

## 노드 보상 주소와 송금

```bash
./ieum-chain reward address
./ieum-chain reward send \
  --to 0x받는주소 \
  --amount 0.1 \
  --fee 0.000001 \
  --rpc-port 8989
```

수수료를 제외한 전액은 `--amount all`로 보냅니다. 기본 파일은 다음과 같습니다.

```text
data/keys/node_wallet.keystore
data/keys/node_wallet.password
```

구형 0.21.9 Ed25519 보상 keystore가 있으면 최초 실행 때 원본을
`data/keys/node_wallet.keystore.ed25519.bak`으로 백업한 후 0x 계정 형식으로
이관합니다. 백업에 과거 보상 잔액이 연결됐을 수 있으므로 삭제하지 마세요.

구형 64자리 주소에 이미 잔액이 있다면 자동으로 새 주소로 옮겨지지 않습니다. 새
`reward address`를 먼저 기록한 뒤, 보관한 0.21.9 실행 파일로 백업 keystore를
명시해 전액을 새 주소로 한 번 전송합니다.

```bash
./ieum-chain-v0.21.9 reward send \
  --keystore data/keys/node_wallet.keystore.ed25519.bak \
  --password-file data/keys/node_wallet.password \
  --to 0x새로운보상주소 \
  --amount all \
  --fee 0.000001 \
  --rpc-port 8989
```

입금이 확정될 때까지 백업과 0.21.9 실행 파일을 보존하세요.

## 리워드/추첨 상태

체인에는 확정 블록 해시를 seed로 사용하는 결정론적 추첨 함수와 일일 발행 상한,
반감기, 자격 점수 계산이 구현돼 있습니다. 다만 0.21.10에서는 이 계산 결과를
운영 합의 블록에 자동 삽입하는 경로를 활성화하지 않습니다. 검증자마다 후보 목록이
다를 때 원장이 갈라질 위험이 있기 때문입니다. 현재 운영 지급은 서명된 등록을
검증하는 기존 최초 검증자 보상과 100노드 마일스톤 보상만 합의 이벤트로 실행됩니다.

자동 로또형 일일 지급은 후보 영수증 집합을 블록에 포함하고 모든 검증자가 같은
후보·당첨 결과를 재검증하는 프로토콜 업그레이드 뒤 활성화해야 합니다. 단순히 UI나
CLI 버튼으로 켜면 안 됩니다.

## 잘못된 명령 정정

- `reward address` 결과가 64자리인 바이너리는 0.21.9 이하입니다.
- 일반 계정 생성은 `reward address`가 아니라 `account new`입니다.
- `reward send`와 `account send`의 `--rpc-port`, `--fee`, `--amount all`은 지원합니다.
- 월렛의 받는 주소에는 항상 `0x` + 40자리 계정 주소만 입력합니다.

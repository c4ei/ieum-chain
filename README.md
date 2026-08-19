# IEUM Chain v0.23.7 메인넷 실행 안내

v0.23.7은 검증 노드가 서로 다른 블록에 잠긴 뒤 라운드만 증가하던 교착을 해소하도록
서명된 `valid_round` prevote 인증서, 안전한 잠금 전환, 합의 wire v2를 추가합니다.
배포·검증 절차는
[`docs/VERSION_0.23.7_BFT_VALID_ROUND_RECOVERY.md`](docs/VERSION_0.23.7_BFT_VALID_ROUND_RECOVERY.md)를
참고하세요.

v0.23.0은 실제 잠금형 `delegate / undelegate / claim`, 합의 시각 기준 7일 해제 대기,
위임자별 일일 보상, 이중투표 증거 기반 5% 페널티와 상태·스냅샷 회계를 추가합니다.
운영 활성화 전 반드시 [`docs/VERSION_0.23.0.md`](docs/VERSION_0.23.0.md)의
프로토콜 v3 공동 전환 절차를 따르세요.

v0.23.5의 메인넷 Genesis, 최초 배분 공개 절차, 일반 보유·최초 참여 보상 및
장애·이중서명·키 탈취 훈련은
[`docs/VERSION_0.23.5_MAINNET_TOKENOMICS_AND_DRILLS.md`](docs/VERSION_0.23.5_MAINNET_TOKENOMICS_AND_DRILLS.md)를 참고하세요.

v0.22.6은 연결 유지 중 놓친 확정 블록을 5초 주기로 다시 동기화합니다. 자세한 내용은
[`docs/VERSION_0.22.6_PERIODIC_SYNC_BALANCE_FIX.md`](docs/VERSION_0.22.6_PERIODIC_SYNC_BALANCE_FIX.md)를 참고하세요.

v0.22.7은 릴레이를 거친 sync 응답도 실제 작성자별 독립 quorum으로 집계합니다.
[`docs/VERSION_0.22.7_SYNC_QUORUM_ORIGIN_FIX.md`](docs/VERSION_0.22.7_SYNC_QUORUM_ORIGIN_FIX.md)를 참고하세요.

설치, systemd, 계정, 백업, 자동 업데이트와 익스플로러 연동은
[`docs/IEUM_USER_MANUAL_FIRST_RELEASE.md`](docs/IEUM_USER_MANUAL_FIRST_RELEASE.md)에,
이번 운영 관측 변경과 배포·태그 절차는
[`docs/VERSION_0.22.4.md`](docs/VERSION_0.22.4.md)에 정리되어 있습니다.

v0.22.5의 검증자 2/3 인증 스냅샷, 최근 6개 보관, 악성 snapshot 피어 차단과
운영 적용 순서는 [`docs/VERSION_0.22.5.md`](docs/VERSION_0.22.5.md)를 참고하세요.

## Ubuntu 신규 노드 배포본 자동 생성

릴리스 빌드 후 설치 가능한 노드 압축을 한 번에 만듭니다.

```bash
scripts/make-node-package.sh 0.22.6
```

위 명령은 포맷·Clippy·전체 테스트·릴리스 빌드까지 통과한 경우에만 압축을
생성합니다.

대상 서버에 직접 복사한 경우:

```bash
tar -xJf ieum-chain_node_ubuntu_x86_64_v0.20.9.tar.xz
cd ieum-chain-node-v0.20.9
sudo ./install.sh
```

SSH 키와 sudo 권한이 준비된 서버에는 자동 업로드·설치할 수 있습니다.

```bash
scripts/deploy-node-package.sh \
  download/ieum-chain_node_ubuntu_x86_64_v0.20.9.tar.xz \
  dev@192.168.1.148
```

설치기는 `/opt/ieum-chain`에 systemd 서비스를 등록해 부팅 시 자동 실행합니다.
기존 `config/validator.key`, `data/server.node.key`, 원장과 로그는 보존하며,
배포 압축에는 개인키·원장·백업을 포함하지 않습니다.

IEUM Chain은 Ubuntu와 Windows에서 실행할 수 있는 경량 블록체인 노드입니다.
첫 노드는 제네시스 상태에서 단독으로 시작할 수 있습니다. 장애 허용 BFT 검증은
**Ubuntu VM 검증자 3대 + Windows VM 검증자 1대**처럼 4명 이상일 때 활성화합니다.

> IEUM 메인넷은 Chain ID `21004`, 네트워크 이름 `ieum-mainnet`을 사용합니다. 운영자는 `--mainnet-strict`와 독립 백업을 사용해야 합니다.

## 가장 먼저 알아둘 것

- VM 4대는 서로 다른 고정 IP를 사용합니다.
- P2P는 `UDP 7001`, RPC는 기본적으로 자기 PC만 접근 가능한 `127.0.0.1:8989`를 사용합니다.
- 네 서버의 `validator.key`, `server.node.key`, `data/ledger`는 서로 달라야 합니다.
- 처음 실행하면 위 파일과 폴더를 각 서버에서 자동 생성합니다.
- `config/validators.json`이 없으면 번들 제네시스의 검증자 4개가 자동 복원됩니다.
- 신규 서버의 로컬 키가 현재 검증자 집합에 없으면 일반 동기화 노드로 시작합니다.
- 신규 서버는 P2P 연결 뒤 공개키·PeerId 소유권 서명을 후보 등록 메시지로 자동 전송합니다.
- 후보는 현재 검증자 승인과 다음 epoch 반영 전까지 합의 투표를 하지 않습니다.
- 개인키 파일은 메일, 메신저, 클라우드 드라이브로 보내지 마세요.

`--allow-insecure-test-keys`를 사용하는 로컬 개발·CI 환경은 검증자 설정 4개를 자동
생성하므로 `validators.json`을 사람이 만들 필요가 없습니다. 운영망에서는 이 옵션을
절대 사용하지 않습니다.

## 운영 검증자 자동 선발 기준

운영 후보는 다음 자격 중 하나를 충족해야 합니다.

- 확정 유통량의 1% 이상을 보유하거나 온체인으로 위임받음
- 국가가 인증된 계좌 중 해당 국가의 IEUM 보유량 상위 50위
- 관리자 또는 거버넌스의 명시적 승인

세 경로 모두 계좌 소유권 서명, `node.ieum.aah.name` 등록 창구의 실제 UDP/QUIC
접속 확인, 기존 노드의 95% 이상 가동률을 통과해야 합니다. 차단·이중서명 후보는
지분과 승인 여부와 관계없이 제외합니다.

`ping node.ieum.aah.name` 성공은 DNS와 ICMP 연결만 확인하며 검증 노드 자격은
증명하지 못합니다. 최종 자동 등록은 노드가 부트스트랩 서버에 접속한 뒤 임의 nonce에
서명하고, 서버가 후보의 UDP/QUIC 주소로 역접속하는 challenge 방식으로 진행합니다.
v0.19.6은 신규 노드의 P2P 동기화와 서명 후보 등록을 자동화합니다. 운영망 후보는
P2P 접속만으로 합의권을 얻지 않으며 기존 선발 정책과 epoch 변경 절차를 통과해야
합니다.

## Ubuntu에서 실행

처음 한 번만 설치합니다.

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev curl git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

소스를 받은 폴더에서 빌드하고 실행합니다.

```bash
cargo build --release --locked
./target/release/ieum-chain
```

기본 실행은 자동 모드입니다. 승인된 검증자 키가 있으면 검증자 역할을 유지하고,
일반 사용자는 클라이언트로 시작합니다. 실제 외부 UDP 역접속이 확인되면 공개 네트워크
지원 가능 상태로 자동 판정됩니다. 고급 사용자는 역할을 명시할 수 있습니다.

```bash
./target/release/ieum-chain --mode auto
./target/release/ieum-chain --mode client
./target/release/ieum-chain --mode public
./target/release/ieum-chain --mode validator
```

기존 운영 서비스의 `ieum-chain server`와 `ieum-chain client` 명령도 계속 지원합니다.

처음 실행하면 자동으로 다음 항목을 만듭니다.

```text
config/validator.key
data/server.node.key
data/ledger/
config/validators.json
config/events.json
data/.ieum-initialized
```

화면에 표시되는 **검증자 공개키**만 기록합니다. 제네시스 검증자 키를 가진 서버는
합의에 참여하고, 그 외 신규 서버는 원장을 동기화하면서 서명된 후보 등록을 자동으로
전송합니다.

## Windows에서 실행

미리 빌드된 `ieum-chain.exe`가 있으면 원하는 폴더에 아래처럼 둡니다.

```text
C:\ieum-chain\ieum-chain.exe
```

PowerShell에서 실행합니다.

```powershell
cd C:\ieum-chain
.\ieum-chain.exe server
```

직접 빌드하려면 Rustup, Git for Windows, Visual Studio 2022 Build Tools의
`Desktop development with C++`를 설치한 뒤 실행합니다.

```powershell
git clone https://github.com/c4ei/ieum-chain.git
cd ieum-chain
cargo build --release --locked
.\target\release\ieum-chain.exe server
```

관리자 PowerShell에서 P2P 방화벽을 한 번 허용합니다.

```powershell
New-NetFirewallRule -DisplayName "IEUM P2P UDP 7001" -Direction Inbound -Protocol UDP -LocalPort 7001 -Action Allow
```

## 네 검증자 등록

네 서버를 각각 한 번 실행해 공개키 4개를 모읍니다. Ubuntu 1번에서 공통 설정을 만듭니다.

```bash
./target/release/ieum-chain validator-key create-config \
  --public-key <Ubuntu1_공개키> \
  --public-key <Ubuntu2_공개키> \
  --public-key <Ubuntu3_공개키> \
  --public-key <Windows_공개키> \
  --output config/validators.json
```

생성된 `config/validators.json`을 나머지 세 서버의 같은 위치에 복사합니다.
공개키를 넣은 순서가 서버 번호입니다.

```text
Ubuntu 1 = validator-index 1
Ubuntu 2 = validator-index 2
Ubuntu 3 = validator-index 3
Windows  = validator-index 4
```

각 서버는 자기 번호로 실행합니다.

```bash
./target/release/ieum-chain server --validator-index 1
```

```powershell
.\ieum-chain.exe server --validator-index 4
```

다른 네트워크의 서버에 연결할 때만 1번 서버의 IP와 PeerId를 넣습니다.

```bash
./target/release/ieum-chain server --validator-index 2 \
  --peer "/ip4/192.168.1.201/udp/7001/quic-v1/p2p/<1번_PeerId>"
```

## 키나 원장이 없어졌다는 오류가 나오면

처음 설치가 끝난 뒤 `validator.key`, `server.node.key`, `data/ledger`가 없어지면
프로그램은 새 파일을 만들지 않고 중단합니다. 자동으로 다시 만들면 다른 검증자로
바뀌기 때문입니다. 이때는 백업을 복구해야 합니다.

백업할 항목:

```text
config/validator.key
data/server.node.key
data/ledger/
data/.ieum-initialized
```

## 신규 노드 초기화와 기존 노드 확인

원본 서버의 `data/`를 복사한 경로는 신규 노드가 아닙니다. 완전한 신규 노드는
명시적으로 초기화합니다.

```bash
# 완전한 신규 노드
./target/release/ieum-chain node init --new

# 기존 노드 상태·키 일치 확인
./target/release/ieum-chain node verify

# 확인 후 실행
./target/release/ieum-chain server
```

`node init --new`는 기존 `data/` 전체와 `config/validator.key`를
`backups/node-init-<UNIX시각>/`으로 자동 백업 이동한 뒤 새 검증자 키, PeerId,
원장과 초기화 표식을 생성합니다. `config/genesis.json`, `validators.json`,
이벤트·업그레이드 설정은 네트워크 공통 파일이므로 유지합니다. 백업은 자동 삭제되지
않습니다.

`node verify`는 `validator.key`, `server.node.key`, 원장,
`data/.ieum-initialized`의 신원이 모두 일치할 때만 성공합니다.

## 메시지·화상채팅·원격 도움

peer 간 메시지와 화상채팅은 기술적으로 가능합니다. 다만 블록체인 원장에 메시지,
통화 내용, 화면 영상을 기록하면 안 됩니다. 별도의 종단간 암호화 통신 계층으로
구현해야 합니다.

v0.18.1에는 지갑이 화상채팅을 연결할 때 필요한 암호화 통신 신호 전달 코어가
포함되었습니다. 실제 카메라·마이크 화면은 `ieum-wallet`에서 구현해야 합니다.
원격 키보드·마우스 제어는 보안 감사 전까지 포함하지 않습니다. 자세한 조건과 RPC는
[보안 통신 설계 문서](docs/SECURE_PEER_COMMUNICATION.md)를 참고하세요.

## 다른 사용자에게 IEUM 보내기

잔액·서명 거래·BFT 확정은 이미 체인 핵심 기능에 포함되어 있습니다. 실제 사용자는
`ieum-wallet`의 받는 주소, 금액, 수수료 확인 화면에서 전송합니다. 지갑은 로컬 노드의
`eth_sendTransaction` 또는 서명된 거래용 `eth_sendRawTransaction`을 호출합니다.

검증자 콘솔에서 개인키를 붙여 넣어 송금하지 마세요. 송금 개인키는 지갑의 암호화
keystore 또는 운영체제 보안 저장소에만 보관해야 합니다.

v0.21.10부터 일반 계정과 노드 보상 주소는 모두 `0x`로 시작하는 40자리
secp256k1 계정 주소를 사용합니다. v0.21.12부터 상대경로는 항상 실행 중인
바이너리 폴더를 기준으로 하며, 계정 암호 파일은 인스턴스별로 자동 생성됩니다.

```bash
./ieum-chain account new
./ieum-chain account import secure/private-key.hex
./ieum-chain account list
./ieum-chain account send \
  --from 0x보내는주소 \
  --to 0x받는주소 \
  --amount 0.1

./ieum-chain account balance 0x계정주소 --rpc-port 8989
./ieum-chain account transaction 0x거래해시 --rpc-port 8989
./ieum-chain account receipt 0x거래해시 --rpc-port 8989

./ieum-chain reward address
./ieum-chain reward send --to 0x받는지갑주소 --amount 1 --fee 0.000001
```

`reward send`는 실행 중인 로컬 RPC `127.0.0.1:8989`에서 nonce를 조회하고
`data/keys/node_wallet.keystore`로 서명한 뒤 거래를 제출합니다. 다른 RPC 포트는
`--rpc-port`로 지정합니다. `--amount all`은 수수료를 뺀 전액을 보냅니다.

CLI와 `personal_newAccount`/`personal_importRawKey` RPC는 동일한
`data/keystore`를 사용합니다. v0.21.11부터 새 파일은
`UTC--timestamp--주소` 형식으로 계정별 누적되며 기존 `주소.json`도 계속 읽습니다.
기본 암호 파일은 `<바이너리 폴더>/secure/ieum-account.password`이며 최초
`account new` 또는 `account import` 때 0600 권한의 임의 암호로 생성됩니다.

`/opt/ieum-node1/ieum-chain`과 `/opt/ieum-node2/ieum-chain`을 함께 실행해도 각각
자기 폴더의 `config`, `data`, `secure`와 자기 바이너리 업데이트만 사용합니다.
systemd `WorkingDirectory`나 명령을 호출한 셸의 폴더는 기준을 바꾸지 않습니다.

64자리 Ed25519 값은 검증자·합의·P2P 신원용 공개키이며 송금 주소가 아닙니다.
v0.21.9가 만든 구형 Ed25519 보상 keystore는 첫 로드 때
`node_wallet.keystore.ed25519.bak`으로 보존되고, 동일한 32바이트 비밀값으로 새
secp256k1 계정 keystore가 생성됩니다. 백업은 삭제하지 마세요.

개발자용 빌드, 테스트, 구조 설명은 [README_DEV.md](README_DEV.md)를 참고하세요.

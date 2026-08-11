# IEUM Chain v0.20.9 사용자 매뉴얼

## 1. 프로그램 역할

IEUM Chain 노드는 IEUM 거래와 확정 원장을 보관하고 QUIC P2P로 다른 노드와
통신합니다. 기본 포트는 P2P `7001/UDP`, 로컬 JSON-RPC
`127.0.0.1:8989`입니다. RPC 8989를 인터넷에 직접 열지 말고 Caddy 같은 HTTPS
reverse proxy 뒤에 둡니다.

`server`는 외부 연결과 합의 참여를 위한 운영 노드이고, `client` 또는 옵션 없는
실행은 일반 PC·월렛용 노드입니다.

## 2. 가장 간단한 실행

일반 PC:

```bash
./ieum-chain
```

운영 서버:

```bash
./ieum-chain server
```

직접 실행은 터미널을 닫거나 서버를 재부팅하면 종료될 수 있습니다. 운영 서버는
아래 systemd 설치 방식을 사용합니다.

## 3. 배포본 설치와 systemd

`make-node-package.sh`는 압축파일만 만들며 서비스를 설치하지 않습니다.
배포본 안의 `install.sh`를 `sudo`로 실행할 때만 systemd 서비스가 등록됩니다.

```bash
tar -xJf ieum-chain_node_ubuntu_x86_64_v0.20.9.tar.xz
cd ieum-chain-node-v0.20.9
sudo ./install.sh
```

기본 설치 경로는 `/opt/ieum-chain`, 서비스명은 `ieum-chain.service`입니다.

```bash
sudo systemctl status ieum-chain --no-pager
sudo journalctl -u ieum-chain -f
sudo systemctl restart ieum-chain
sudo systemctl stop ieum-chain
```

상태 확인:

```bash
curl -sS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"ieum_nodeStatus","params":[]}' \
  http://127.0.0.1:8989
```

## 4. 방화벽과 공유기

- 서버 인바운드: `7001/UDP`
- 로컬 전용: `8989/TCP`
- 같은 공유기 밖에서 받을 노드는 공유기 UDP 7001 포트포워딩이 필요합니다.
- Cloudflare 프록시는 일반 QUIC/libp2p UDP 포트를 대신 전달하지 않습니다.
  `node.ieum.aah.name`은 DNS only로 운영합니다.

확인:

```bash
sudo ss -lunp | grep ':7001'
pgrep -af '[/]ieum-chain'
```

## 5. 주요 파일과 백업

반드시 백업할 서버 고유 파일:

- `config/validator.key`
- `data/server.node.key`
- `data/reward.key`
- `data/ledger/`
- 운영 중인 `config/validators.json`, `config/events.json`,
  `config/upgrades.json`, `config/bootstrap.json`

개인키는 Git, 배포 압축, 메신저에 넣지 않습니다. 복구 전에는 서비스를 멈추고
전체 디렉터리를 별도 위치에 보존합니다.

```bash
sudo systemctl stop ieum-chain
sudo cp -a /opt/ieum-chain /opt/ieum-chain.backup-$(date +%Y%m%d)
sudo systemctl start ieum-chain
```

노드 일치 검사:

```bash
cd /opt/ieum-chain
./ieum-chain node verify
./ieum-chain node doctor
```

`node doctor`는 원장 삭제 명령이 아닙니다. 현재 키에서 PeerId를 다시 계산하고
초기화 표시, 원장 폴더, 검증자 설정, 자기 광고 주소를 점검·복구합니다.

원장을 초기화해야 할 때만 아래 절차를 사용합니다. `node clean`은 기존 원장을
`backups/ledger-clean-*`으로 옮기고 노드 키와 검증자 키를 보존하지만, 실행 전
서비스 중지와 전체 백업을 권장합니다.

```bash
sudo systemctl stop ieum-chain
cd /opt/ieum-chain
sudo ./ieum-chain node clean --yes
sudo systemctl start ieum-chain
```

## 6. 무인 자동 업데이트

쉘·systemd 노드는 사람의 `y/n` 입력에 의존하면 안 됩니다. IEUM은
`config/update.json`이 활성화된 경우 다음 절차로 비대화형 업데이트합니다.

1. 로컬 HTTPS 주소에서 manifest 다운로드
2. 노드에 고정한 Ed25519 릴리스 공개키로 manifest 서명 확인
3. 현재 플랫폼 바이너리의 SHA-256 확인
4. 기존 바이너리를 `.previous`로 보존하고 원자적으로 교체
5. 노드 종료 후 systemd가 재시작

v0.20.9 설치 패키지는 서명에 사용한 공개키가 들어 있는 활성
`config/update.json`을 자동 설치합니다. 릴리스 생성은 개발 서버에서 실행합니다.

```bash
cd /home/dev/www/ieum-chain
chmod 600 backups/release-private.pem
scripts/make-node-package.sh 0.20.9
git add Cargo.toml Cargo.lock CHANGELOG.md README.md config/update.json \
  download scripts docs src
git commit -m "IEUM v0.20.9 signed release"
git push
```

기본 개인키 경로는
`/home/dev/www/ieum-chain/backups/release-private.pem`입니다. 개인키는
`.gitignore`의 `/backups` 규칙으로 제외되며 패키지·manifest·Git에 포함되지
않습니다. 다른 경로를 쓸 때만 `IEUM_RELEASE_PRIVATE_KEY`를 지정합니다.

스크립트는 설치 압축·SHA-256, 업데이트용 무압축 실행파일, 서명된
`download/update-manifest.json`, 공개키가 고정된 `config/update.json`을
생성합니다. Git push 후 자동 업데이트가 설정된 노드는 5분 이내 확인합니다.
P2P의 새 버전 알림은 확인 시점을 앞당기는 힌트일 뿐이며, 알림을 보낸 피어를
신뢰해 설치하지 않습니다.

활성 `config/update.json` 없이 설치된 기존 노드는 배포 경로를 모르므로
v0.20.9 패키지를 최초 한 번 설치해야 합니다. 이후부터는 수동 설치가
필요하지 않습니다.

검증자 4대 이상은 전부 동시에 자동 업데이트하지 않습니다. 프로토콜 호환
업데이트는 한 대씩 적용하고 RPC·피어·합의 높이를 확인한 뒤 다음 노드로
진행합니다. 합의 규칙이 바뀌는 업데이트는 `config/upgrades.json`의 활성 높이를
검증자들이 사전에 동일하게 합의·배포한 뒤 실행합니다.

긴급 복구:

```bash
sudo systemctl stop ieum-chain
sudo cp --preserve=mode,ownership,timestamps \
  /opt/ieum-chain/ieum-chain.previous /opt/ieum-chain/ieum-chain
sudo systemctl start ieum-chain
```

위 명령은 실패한 **실행 파일 업데이트**를 이전 버전으로 되돌리는 절차이며,
체인 원장을 과거 블록으로 되돌리는 체크포인트 롤백과는 다릅니다.

## 7. 사고 복구 원칙

운영 중 사고가 발생하면 먼저 다음 기준으로 복구 범위를 정합니다.

> 거래 단위 복구: 해킹·오발행처럼 영향 거래가 명확할 때 사용  
> 체크포인트 롤백: 원장 전체가 손상되거나 합의 버그가 발생했을 때만 사용

두 방식 모두 등록 검증자 수의 3/4 이상 또는 전체 검증자 투표권(보유량·스테이크
가중치)의 3/4 이상이 동일한 복구 계획 해시에 서명해야 승인됩니다. 중복 서명은 한
번만 계산하고 미등록 검증자의 서명이 포함되면 계획 전체를 거부합니다.

명령줄에서도 같은 원칙을 확인할 수 있습니다.

```bash
./ieum-chain recovery policy
```

거래 단위 복구에서는 확정된 원본 거래와 블록을 삭제하지 않습니다. 원본 거래는
감사 가능한 상태로 남기고, 새 블록에 사고 ID와 보상 결과를 기록합니다. 현재
`IncidentCompensation` 이벤트는 재단 보유 잔액 안에서 피해 보상 기록을 남길 수
있으며 임의 신규 발행으로 총공급량을 증가시키지 않습니다.

다음 작업은 하지 마세요.

- 특정 노드의 원장 JSON·SQLite 행을 직접 삭제하거나 잔액을 수정
- 한 명의 관리자 판단만으로 확정 거래를 취소
- 정상 거래가 계속 발생하는 상태에서 일부 노드만 과거 체크포인트로 변경
- 원본 거래를 숨기거나 익스플로러에서 복구 이력을 제거

체크포인트 롤백이 꼭 필요한 경우에는 모든 검증자 서비스를 먼저 중지하고, 각 노드
원장과 키를 별도로 백업한 뒤 대상 높이·블록 해시·상태 해시가 일치하는지 확인해야
합니다. 검증자 합의와 복구 후 전체 공급량 검증 없이 재시작하지 않습니다.

## 8. 합의는 y/n 입력으로 결정하지 않음

블록 승인, 검증자 변경, 프로토콜 업그레이드, 보상 지급은 월렛의 팝업이나 서버
콘솔 입력으로 결정하면 안 됩니다. 노드는 서명된 제안·prevote·precommit과
확정 인증서로 자동 합의합니다.

월렛은 다음 관리 화면을 제공할 수 있습니다.

- 노드 버전·피어·동기화·검증자 상태 조회
- 운영자가 업그레이드 제안에 서명하는 거버넌스 UI
- 릴리스 서명과 체크섬 표시
- 보상 내역·중계 기여 내역 표시

월렛이 꺼져 있어도 이미 승인된 합의와 예정된 업그레이드는 노드끼리 진행되어야
합니다. 월렛은 개인키 서명과 상태 표시 UI이지 합의 엔진의 중앙 제어기가 아닙니다.

## 9. 로그 해석

시작 직후 다음 메시지는 피어의 topic 구독 전이면 발생할 수 있습니다.

```text
P2P 메시지 전파 실패: NoPeersSubscribedToTopic
```

피어 연결 뒤에도 계속 누적되면 상대 노드 버전, topic 구독, 방화벽을 확인합니다.
반복 메시지는 최초와 10·100·1000회에만 새 줄로 기록되어 다른
비동기 로그와 섞이지 않습니다.

`Unexpected peer ID`는 주소 끝 PeerId와 실제 상대 노드 키가 다르다는 뜻입니다.
`config/bootstrap.json`과 상대의 `data/server.node.key`를 확인합니다.

`I/O error: timed out` 뒤에 `이 피어의 남은 연결: 1`이 표시되면 동일 피어와
맺은 중복 QUIC 연결 하나만 종료된 것이며 전체 연결 단절은 아닙니다.

`OutboundProbe(Error ... DialError)`는 AutoNAT 역접속 판정 실패입니다. 반복되면
공유기 포트포워딩, 서버 방화벽의 `7001/UDP`, DNS only 설정과 광고 주소를
확인합니다. 피어 연결과 블록 동기화가 정상이라면 즉시 체인 장애로 판단하지
않습니다.

운영 로그의 `고유 연결 피어`는 실제 상대 노드 수이고, `이 피어의 연결`은 같은
PeerId와 맺은 QUIC·AutoNAT 연결 수입니다.

## 9. 검증자 설정과 4노드 CI

v0.20.9는 검증자 자동 등록 결과를 저장할 때 프로세스마다 고유한 임시 파일을
만들고 디스크 동기화 후 `validators.json`으로 원자 교체합니다. 다음 오류는
v0.20.7 이하에서 여러 프로세스가 한 설정 파일을 공유할 때 발생할 수 있습니다.

```text
검증자 설정 교체 실패(.../validators.json): No such file or directory
```

같은 장비에서 여러 테스트 노드를 실행할 때는 원장, 노드 키, 검증자 키뿐 아니라
`validators.json`도 반드시 노드별 경로로 분리합니다. CI·폐쇄형 개발망에서는
운영망 접속 방지를 위해 `--no-default-bootstrap`을 사용합니다.

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
bash tests/four_process_network.sh target/release/ieum-chain
```

`--no-default-bootstrap`은 운영 서버의 일반 실행에 사용하지 않습니다.

## 10. 트래픽 분산 기능의 현재 상태

근거리·국가/ASN 다양성·저부하 슬롯 선택과 이벤트 추첨 코어는 구현되어 있지만
현재 실제 P2P 연결에는 아직 활성화하지 않았습니다. 안전한 활성화에는 다음이
추가로 필요합니다.

- 관측 RTT, 실제 연결 수, 가동률 수집
- 서명되고 만료되는 국가/ASN/수용량 광고
- 자기 신고와 외부 관측의 교차 검증
- NAT 뒤 일반 PC가 중계 가능한지 판정하는 reachability
- Sybil·동일 ASN 군집·담합 시뮬레이션
- 중계 영수증 root에 대한 검증자 합의

국가별 보유 순위만으로 일반 연결을 정하면 소형 노드가 배제되고 위치 위조가
보상을 좌우할 수 있습니다. 국가 순위는 검증자 후보 조건 중 하나로만 사용하고,
일반 트래픽은 실제 품질·다양성·저부하를 함께 사용합니다.

## 11. AAH ↔ IEUM 교환

월렛 화면만 추가해서는 신뢰 없는 DEX가 되지 않습니다. AAH가 EVM 체인이고
IEUM이 별도 체인이므로 양쪽의 확정을 검증하고 자산을 잠그거나 발행·해제하는
브리지 계층이 필요합니다.

권장 구성:

- IEUM 체인: swap/HTLC 또는 검증자 서명 기반 bridge 시스템 모듈
- AAH 체인: 대응 smart contract
- relayer: 양 체인의 확정 이벤트 전달
- IEUM Wallet: 교환 견적·승인·진행 상태 UI
- 선택 사항: 별도 웹/앱 DEX UI

초기에는 소액·일일 한도·다중서명·지연 출금·비상정지를 둔 제한형 swap으로
시작하고, 독립 감사 후 유동성 풀 방식으로 확장합니다. 운영 서버가 양쪽 개인키를
단독 보유하는 단순 교환 API는 중앙화 거래소이며 DEX라고 부르면 안 됩니다.

## 12. 운영 전 필수 점검

- 4개 이상 서로 다른 서버·망 사업자에서 BFT 장시간 테스트
- 노드 한 대 중단·복귀·원장 재동기화 시험
- 이중투표, 잘못된 state root, 손상된 snapshot 거부 시험
- 키 분실·유출·교체 절차와 validator 제거 절차
- 서명 릴리스 키의 오프라인 보관과 복구 키
- 백업 복원 훈련과 체크섬 검증
- RPC rate limit, WAF, CORS, method allowlist
- 디스크 부족, 시간 오차, 메모리, FD, 피어 수 모니터링
- 100~1,000 노드 부하·Sybil·NAT 시뮬레이션
- 보상·브리지·DEX 활성화 전 보안 감사

현재 단계는 기능 테스트넷/파일럿 운영에 적합하며, 대규모 실자산 운영은 위 항목과
트래픽 분산·보상 영수증·브리지 감사를 마친 뒤 진행하는 것이 안전합니다.

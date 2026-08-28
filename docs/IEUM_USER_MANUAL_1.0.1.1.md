# IEUM Chain v1.0.7.1 사용자·운영자 매뉴얼

## 네트워크

- 메인넷 Chain ID: `21004`
- 표시 버전: `1.0.7.1`
- 기본 P2P/RPC: UDP `7001`, TCP `8989`
- 운영 Genesis: `config/genesis.json`

## 일반 공개 노드 보상과 100 IEUM 담보

`--server` 일반 공개 노드는 보상 지갑 주소가 활성 검증자에게 합계 **100 IEUM 이상을
위임한 상태로 7일을 채운 뒤** 보상 대상이 됩니다. 하루(UTC epoch) 80% 이상 연결을
유지하고 서로 다른 활성 검증자 3명 이상이 같은 PeerId·보상 주소·네트워크 대역을
서명해야 합의 지급이 가능합니다.

- 100 IEUM은 지급 수수료가 아니라 보상 자격을 위한 잠금형 위임 담보입니다.
- 담보를 추가하면 안전을 위해 합산 담보 전체의 7일 성숙 기간을 다시 계산합니다.
- 담보를 해제하거나 100 IEUM 아래로 내려가면 보상 자격이 사라집니다.
- 메인 검증 서버의 주소와 PeerId는 일반 공개 노드 보상에서 제외됩니다.
- 동일 보상 주소의 중복 노드와 IPv4 `/24`·IPv6 `/48`당 세 번째 이후 노드는 같은 날 지급하지 않습니다.
- 일일 지급 총액은 최대 1,000 IEUM이며 재단 잔액과 당일 적격 노드 수에 따라 달라지므로 고정 이율을 보장하지 않습니다.
- 프로세스 재시작 시 검증자는 연속 활동 관측을 안전하게 다시 시작합니다. 재시작만으로 가동 시간을 허위 누적하지 않습니다.

즉, `--server`를 실행했다는 사실만으로 보상이 생기지 않습니다. 위 담보와 활동 증명이
확정 블록의 `NodeServiceDailyReward` 합의 이벤트에 포함된 날에 보상 지갑 잔액으로 지급됩니다.

## 노드 상태 확인

```bash
curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"ieum_nodeStatus","params":[]}' \
  http://127.0.0.1:8989 | python3 -m json.tool
```

네 노드의 `chainId`, genesis hash, 높이, tip hash와 state root가 일치해야 합니다. 거래가 없으면 새 블록을 만들지 않으므로 높이가 일정한 것은 정상입니다. 한 노드가 뒤처지면 1분 이내 차이는 확정 블록으로, 큰 차이 또는 인증서 공백은 2/3 인증 snapshot으로 자동 복구합니다.

## 메인 서버 장애 시 자동 복구

노드는 정상 운영 시 메인 bootstrap을 먼저 사용합니다. 연결 과정에서 확인한 공개·IPv6·DNS·릴레이
피어 주소는 `data/network/known-peers.json`에 자동 저장하며, 메인 서버나 DNS가 중단되어도
재시작할 때 저장 피어와 Kademlia 분산 검색으로 사용자망 복구를 시도합니다.

- 이 파일은 노드별 `data/` 영구 볼륨에 보존합니다.
- 4노드가 같은 호스트 경로를 공유한다면 `--peer-cache`로 서로 다른 파일을 지정합니다.
- 파일을 수동 편집할 필요는 없습니다. 삭제해도 키·지갑·원장은 지워지지 않지만 장애 복구 후보가 사라집니다.
- 네트워크가 살아 있어도 검증자 3/4 정족수가 사라지면 새 블록 확정은 멈춥니다.
- 모든 생존 노드가 NAT 내부이고 공개 포트와 릴레이가 전혀 없으면 외부 재접속은 보장할 수 없습니다.

## 휴대폰·가정용 공유기 연결

v1.0.7.1은 다음 순서로 실제 연결을 시도합니다.

1. 공유기가 UPnP를 지원하면 UDP P2P 포트를 자동 매핑합니다.
2. 직접 연결이 안 되면 기존 릴레이 연결을 이용해 DCUtR QUIC 홀펀칭을 시도합니다.
3. 홀펀칭도 안 되면 메인 서버뿐 아니라 연결된 공개 사용자 노드에 Circuit Relay를 예약합니다.

휴대폰 LTE/5G는 통신사 CGNAT 때문에 외부에서 직접 접속할 수 없는 경우가 많습니다. 이때는
다른 공개 사용자 노드나 메인 노드 중 하나가 릴레이로 살아 있어야 합니다. 공유기에서 UPnP를
꺼 둔 경우에도 홀펀칭과 릴레이 경로는 계속 동작합니다.

## 자동 진단

```bash
sudo bash scripts/diagnose-ieum-server.sh -H 192.168.1.148
bash scripts/diagnose-ieum-external.sh -H 192.168.1.148
sudo bash scripts/ieum-cluster-tool.sh status
sudo bash scripts/ieum-cluster-tool.sh logs 1
sudo bash scripts/ieum-cluster-tool.sh recover
```

데이터 디렉터리는 진단과 백업 없이 삭제하지 않습니다. 같은 높이의 block hash가 다르면 즉시 거래를 중지하고 네 노드의 로그·설정·볼륨을 보존합니다.

## Docker 운영

```bash
cd /opt/ieum-docker-four-node
sudo docker compose ps
sudo docker compose restart node1
sudo docker logs --since 10m ieum-node1
```

Compose 명령은 Compose 파일이 있는 디렉터리에서 실행하거나 `docker restart ieum-node1`처럼 컨테이너 이름을 직접 사용합니다.

## 키 보존 복구

```bash
bash scripts/recover-ieum-node.sh -h
sudo bash scripts/recover-ieum-node.sh
```

복구 도구는 config와 노드·검증자 키를 보존하며 기존 원장을 날짜가 붙은 백업으로 남깁니다. 같은 서버 Docker 노드는 다수 정상 상태의 원장을 복제하는 응급복구를 사용하고, 일반 노드는 빈 원장에서 인증 snapshot/P2P 동기화를 시작합니다.

## 빌드와 테스트

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

릴리스와 Git 절차는 [`VERSION_1.0.7.1_NAT_TRAVERSAL.md`](VERSION_1.0.7.1_NAT_TRAVERSAL.md)를 참고합니다.

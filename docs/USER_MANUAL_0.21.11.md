# IEUM Chain v1.0.1.1 사용자·운영자 매뉴얼

## 네트워크

- 메인넷 Chain ID: `21004`
- 표시 버전: `1.0.1.1`
- 기본 P2P/RPC: UDP `7001`, TCP `8989`
- 운영 Genesis: `config/genesis.json`

## 노드 상태 확인

```bash
curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"ieum_nodeStatus","params":[]}' \
  http://127.0.0.1:8989 | python3 -m json.tool
```

네 노드의 `chainId`, genesis hash, 높이, tip hash와 state root가 일치해야 합니다. 거래가 없으면 새 블록을 만들지 않으므로 높이가 일정한 것은 정상입니다. 한 노드만 낮고 `syncHighest`도 자기 높이와 같으면 토픽 동기화를 진단합니다.

## 자동 진단

```bash
sudo bash scripts/diagnose-ieum-server.sh -H 192.168.1.148
bash scripts/diagnose-ieum-external.sh -H 192.168.1.148
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

## 빌드와 테스트

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

릴리스와 Git 절차는 [`VERSION_1.0.1.1_GOSSIPSUB_SYNC_RECOVERY.md`](VERSION_1.0.1.1_GOSSIPSUB_SYNC_RECOVERY.md)를 참고합니다.

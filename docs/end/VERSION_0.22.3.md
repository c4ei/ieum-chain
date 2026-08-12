# IEUM Chain v0.22.3 변경 내역

## 완료

- 실제 제네시스 블록을 높이 `0`으로 영구 저장합니다.
- 재시작 후 메모리 체크포인트가 원본 블록을 가리는 문제를 수정했습니다.
- 블록·거래·영수증 RPC가 활성 및 백업 아카이브의 원본 이력을 조회합니다.
- `--block-time-ms`를 추가했습니다. 허용 범위는 `100`~`15000`, 기본값은 `5000`입니다.
- 거래가 없을 때 빈 블록을 만들지 않는 정책은 유지합니다.
- `/opt/ieum-node1..3`에 로컬 업데이트 설정이 없어도 `/opt/ieum-chain/config/update.json`의 서명 설정을 공유해 각자의 실행 파일을 갱신합니다.

## 새 체인 초기화

기존 테스트 체인은 마이그레이션하지 않고 새 제네시스로 시작합니다. 네 서비스를 모두 중지하고 각 원장을 별도 백업한 뒤 초기화하십시오.

```bash
sudo systemctl stop ieum-chain ieum-node1 ieum-node2 ieum-node3

for dir in /opt/ieum-chain /opt/ieum-node1 /opt/ieum-node2 /opt/ieum-node3; do
  (cd "$dir" && sudo -u dev ./ieum-chain node clean --yes)
done

sudo systemctl start ieum-chain ieum-node1 ieum-node2 ieum-node3
```

한 노드만 먼저 새 체인으로 실행하면 안 됩니다. 초기화 전 각 노드의 키와 원장 백업 경로를 확인하십시오.

## 검증

```bash
curl -sS -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["0x0",true],"id":1}' \
  http://192.168.1.148:8989
```

`number`는 `0x0`, `miner`는 `checkpoint`가 아니어야 하며 네 RPC에서 같은 해시여야 합니다.

## 운영 권장값과 블록 크기

- 서버 검증자: 기본 `--block-time-ms 5000`
- 모바일/LAN 시험: 먼저 `1000` 이상
- `100`은 부하 시험용이며 배터리·네트워크·저장공간 비용 때문에 운영 기본값으로 권장하지 않습니다.
- 현재 블록은 최대 1,000개 거래와 P2P `--max-message-bytes` 한도에 의해 제한됩니다.
- 노드별로 다른 동적 블록 크기는 합의 분기를 만들 수 있어 이번 버전에는 넣지 않았습니다.

## 공개 전 필수 확인

- 4노드 블록 생성 → 전체 재시작 → 0번 및 거래 블록/영수증 동일성
- 월 rollover 이후 백업 블록 RPC 조회
- 서명 manifest 업데이트 뒤 네 설치본 바이너리 해시 동일성
- 모바일은 작업증명 채굴기가 아니라 일반 노드 또는 승인된 PoS 검증자 후보로 시험

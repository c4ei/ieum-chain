# IEUM Docker 4노드 운영 파일

`Dockerfile`은 노드 내장 자동 업데이트가 GitHub Release manifest를 확인할 수 있도록
`curl`과 `ca-certificates`를 포함합니다.

운영 서버 적용:

```bash
sudo cp deploy/docker-four-node/Dockerfile /opt/ieum-docker-four-node/Dockerfile
sudo install -m 755 deploy/docker-four-node/update-four-nodes.sh /opt/ieum-docker-four-node/update-four-nodes.sh
sudo /opt/ieum-docker-four-node/update-four-nodes.sh
```

후보 이미지만 먼저 검증하려면:

```bash
sudo /opt/ieum-docker-four-node/update-four-nodes.sh --build-only
```

이미지 내부 확인:

```bash
sudo docker run --rm --entrypoint curl ieum-chain-local:latest --version
```

노드 장애 복구는 저장소의 `scripts/recover-ieum-node.sh -h`를 참고합니다.

## 공개 HTTPS RPC (MetaMask)

노드가 localhost가 아닌 주소에서 RPC를 열면 CORS 헤더와 `GET /health`가 켜집니다.
계정 RPC(`personal_*`, `ieum_newMnemonic` 등)는 기존대로 localhost 노드에서만 동작합니다.

```bash
sudo cp deploy/docker-four-node/Caddyfile.rpc.example /etc/caddy/Caddyfile.d/ieum-rpc.caddy  # 도메인 수정 후
sudo caddy validate --config /etc/caddy/Caddyfile && sudo systemctl reload caddy
curl -s https://rpc.example.com/health
```

MetaMask 네트워크 추가: RPC URL `https://<도메인>`, Chain ID `21004`, 통화 기호 `IEUM`.

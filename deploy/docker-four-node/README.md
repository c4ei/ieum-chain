# IEUM Docker 4노드 운영 파일

`Dockerfile`은 노드 내장 자동 업데이트가 GitHub Release manifest를 확인할 수 있도록
`curl`과 `ca-certificates`를 포함합니다.

운영 서버 적용:

```bash
sudo cp deploy/docker-four-node/Dockerfile /opt/ieum-docker-four-node/Dockerfile
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

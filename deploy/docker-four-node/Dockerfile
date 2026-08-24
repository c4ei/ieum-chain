FROM ubuntu:24.04

# curl은 노드 내부의 서명 manifest 자동 업데이트에 필요합니다.
# ca-certificates는 GitHub HTTPS 인증서 검증에 사용합니다.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        libssl3t64 \
        libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

COPY ieum-chain-linux-x86_64 /image/ieum-chain
COPY entrypoint.sh /entrypoint.sh

RUN chmod 755 /image/ieum-chain /entrypoint.sh \
    && curl --version >/dev/null

ENTRYPOINT ["/entrypoint.sh"]

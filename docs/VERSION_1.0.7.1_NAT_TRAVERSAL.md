# IEUM Chain v1.0.7.1 — 휴대폰·가정용 공유기 NAT 연결

## 변경 이유

v1.0.6.1에는 AutoNAT, DCUtR, Circuit Relay가 있었지만 공유기 포트를 자동 개방하는 UPnP가
활성화되지 않았고 릴레이 예약이 초기 bootstrap에 집중되어 있었습니다. 따라서 메인 노드가
사라진 뒤 가정용 NAT와 휴대폰 CGNAT 사용자가 서로 새 연결을 만드는 경로가 충분하지 않았습니다.

## 연결 우선순위

1. 메인 bootstrap 또는 v1.0.6.1 저장 피어로 최초 연결
2. 직접 QUIC 연결
3. 가정용 공유기 UPnP UDP 포트 자동 매핑
4. 릴레이를 통한 DCUtR QUIC 홀펀칭 후 직접 연결 승격
5. 직접 연결 실패 시 공개 사용자 노드의 Circuit Relay 사용

모든 IEUM 노드는 기존부터 제한이 적용된 libp2p relay server를 실행합니다. v1.0.7.1은
연결된 공개 피어에도 릴레이 예약을 요청하여 메인 서버가 단독 릴레이가 되지 않게 합니다.

## 실제 환경별 결과

| 환경 | 예상 경로 |
| --- | --- |
| 공인 IP·UDP 허용 | 직접 QUIC |
| 가정용 공유기·UPnP 허용 | UPnP 포트 매핑 후 직접 QUIC |
| 일반 NAT·UPnP 차단 | DCUtR UDP 홀펀칭 |
| 이중 NAT·휴대폰 CGNAT | 공개 사용자 또는 메인 노드 Circuit Relay |
| 같은 Wi-Fi/LAN | mDNS 직접 연결 |
| 모든 노드가 CGNAT이고 공개 릴레이 0개 | 신규 외부 연결 불가 |

Roblox·Minecraft 같은 게임도 중앙 매치메이킹, UPnP/포트포워딩, 홀펀칭 또는 릴레이 중 하나를
사용합니다. IEUM은 중앙 계정 매치메이킹 대신 저장 피어와 Kademlia를 사용하고 나머지 NAT
우회 계층을 동일한 원리로 제공합니다.

## 운영 및 개인정보

- UPnP는 공유기가 지원하고 관리자 설정에서 허용한 경우에만 동작합니다.
- 자동 매핑은 IEUM의 UDP P2P 리스너에만 적용되며 RPC 포트는 외부 공개하지 않습니다.
- 릴레이 노드는 암호화된 P2P 프레임을 전달하며 지갑 개인키를 받지 않습니다.
- 릴레이에는 libp2p 기본 예약·회로 제한이 적용되지만 트래픽이 발생할 수 있으므로 공개 노드는
  네트워크 사용량을 모니터링해야 합니다.
- Docker에서는 호스트 네트워크/NAT 구조에 따라 UPnP가 공유기를 찾지 못할 수 있으며, 이 경우
  DCUtR와 Circuit Relay로 자동 전환합니다. 새 `.env` 변수는 없습니다.

## 실제 시험 절차

1. 서로 다른 가정용 인터넷 두 곳과 LTE/5G 한 곳에서 노드를 실행합니다.
2. 로그에서 `[공유기 UPnP 포트 매핑]`, `[NAT 홀 펀칭]`, `[분산 NAT 릴레이 예약 시도]`를 확인합니다.
3. 공유기 UPnP를 끈 상태, 휴대폰 테더링 상태, 메인 bootstrap 차단 상태를 각각 시험합니다.
4. `ieum_peerInfo`에서 서로 다른 원격 IP와 연결 방향을 확인합니다.
5. 검증자 정족수를 유지한 상태에서 실제 서명 거래의 receipt와 세 노드 잔액을 확인합니다.

## 버전·Git·TAG

- 표시 버전: `1.0.7.1`
- Cargo 버전: `1.0.7-1`
- 추가 환경변수: 없음
- 선택 CLI: 기존 `--peer-cache`
- 빌드 보완: P2P 이벤트 상태를 컨텍스트 구조체로 묶어 `too_many_arguments` Clippy 오류 해결
- 주소 보완: IPv6 ULA·링크로컬·문서용 대역은 인터넷 복구 캐시에서 제외
- 업데이트 보완: `releases/latest/download/update-manifest.json`에서 최신 서명 manifest 확인

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked

git push -u origin feat/v1.0.7.1-nat-traversal
gh pr create --base dev --head feat/v1.0.7.1-nat-traversal \
  --title "IEUM Chain v1.0.7.1 NAT traversal" --draft
```

`dev`에서 서로 다른 실제 NAT 세 곳 시험 후 `main`으로 병합하고 태그를 생성합니다.

```bash
git switch main
git pull --ff-only origin main
test "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')" = "1.0.7-1"
git tag -a v1.0.7.1 -m "IEUM Chain v1.0.7.1"
git push origin v1.0.7.1
```

태그 Actions의 Linux·Windows·macOS 빌드와 서명 manifest를 확인합니다.

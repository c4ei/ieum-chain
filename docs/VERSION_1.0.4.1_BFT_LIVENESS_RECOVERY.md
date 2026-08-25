# IEUM Chain v1.0.4.1 — BFT 잠금 교착 자동 회복

## 장애 재현

높이 9에서 일부 검증자가 이전 제안에 잠긴 뒤 새 제안자가 잠금 해제에 필요한
`valid_round` prevote 인증서를 알지 못해 라운드만 반복되고 블록이 확정되지 않았습니다.

## 수정

- round-change 메시지에 선택적으로 기존 valid block, valid round, 2/3 초과 prevote
  인증서를 포함합니다.
- valid block은 자체 블록 해시, prevote 인증서는 각 검증자의 개별 서명으로 변조를
  차단하며 기존 round-change 서명 형식은 유지합니다.
- 수신 노드는 체인 ID, genesis, 검증자, 높이, 부모 해시, 스케줄 이벤트와 prevote
  서명·중복·투표권을 모두 검증한 뒤에만 valid value를 채택합니다.
- 새 제안자는 전달받은 valid block과 인증서를 다시 제안하여 잠긴 노드가 안전하게
  동일 값으로 합류하도록 합니다.
- valid value가 없는 기존 round-change 메시지의 서명 형식은 유지합니다.

잠금을 시간 경과만으로 강제 해제하지 않으므로 BFT 안전성을 희생하지 않습니다.

## 배포

일반 업데이트는 기존 `deploy/docker-four-node/update-four-nodes.sh`로 한 대씩
교체합니다. 각 노드가 동일 높이·해시로 재합류한 뒤 다음 노드를 교체하므로 네 노드를
동시에 중지할 필요가 없습니다.

```bash
cd /opt/ieum-docker-four-node
sudo ./update-four-nodes.sh
```

## 검증

```bash
cargo fmt --all --check
cargo test --locked
bash tests/ci_regression_guard.sh
bash tests/release_build_policy.sh
bash tests/four_process_network.sh
```

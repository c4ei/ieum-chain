#!/usr/bin/env bash
set -euo pipefail

network_test="tests/four_process_network.sh"
operational_test="tests/v0_23_8_operational_basics.sh"
node1_rejoin_test="tests/four_process_node1_persistent_rejoin.sh"
grep -q 'wait_for_survivor_mesh' "$node1_rejoin_test"
grep -q 'default_value_t = 2, value_parser = parse_sync_quorum_peers' src/main.rs
grep -q 'version = ieum_chain::IEUM_DISPLAY_VERSION' src/main.rs
grep -q 'env!("IEUM_DISPLAY_VERSION")' src/lib.rs
grep -q 'transaction_is_admissible(&state.chain, &transaction)' src/rpc.rs
grep -q 'state.pool.next_nonce(&wallet.address(), finalized_nonce)' src/rpc.rs
grep -q 'rpc.begin_sync(tip.height)' src/main.rs
grep -q '5초 sync tick에 맡깁니다' src/main.rs
grep -q 'pub const SERVICE_BOND: u128 = 100 \* IEUM' src/node_emission.rs
grep -q 'pub const SERVICE_MINIMUM_VALIDATORS: usize = 3' src/node_emission.rs
grep -q 'validator_peer_ids.contains(&attestation.peer_id)' src/node_emission.rs
grep -q 'NodeServiceDailyReward' src/consensus_runtime.rs
grep -q '일반 공개 노드 보상: 100 IEUM 담보 필수' README.md
grep -q '일반 공개 노드 보상과 100 IEUM 담보' docs/IEUM_USER_MANUAL_1.0.1.1.md

bash -n "$network_test"
bash -n "$operational_test"
bash -n "$node1_rejoin_test"

if [[ ! -x "$operational_test" ]]; then
  echo "CI 회귀 방지 실패: v0.23.8 운영 기본기 스크립트에 실행 권한이 필요합니다." >&2
  exit 1
fi

grep -Fq 'syncHighest' "$node1_rejoin_test" || {
  echo "CI 회귀 방지 실패: Node 1 재합류 테스트가 피어 최고 높이를 확인해야 합니다." >&2
  exit 1
}
grep -Fq 'old_peer_id' "$node1_rejoin_test" || {
  echo "CI 회귀 방지 실패: Node 1 재합류 테스트가 영구 PeerId를 확인해야 합니다." >&2
  exit 1
}
grep -Fq 'snapshot 동기화 완료' "$node1_rejoin_test" || {
  echo "CI 회귀 방지 실패: Node 1 재합류 테스트가 인증 snapshot 복구를 확인해야 합니다." >&2
  exit 1
}

require_pattern() {
  local pattern="$1"
  local description="$2"

  if ! grep -Fq -- "$pattern" "$network_test"; then
    echo "CI 회귀 방지 실패: $description" >&2
    exit 1
  fi
}

# 실제 네트워크 검증 전에 과거 실패를 되살리는 판정식 변경을 빠르게 차단한다.
require_pattern 'start_node 4 "$peer_1" "$peer_2" "$peer_3"' \
  "네 번째 노드가 세 피어 모두에 연결되어야 합니다."
require_pattern 'initial_recipient_balances=()' \
  "송금 전 수신 잔액을 수집해야 합니다."
require_pattern 'expected_recipient_balance="$((initial_recipient_balances[0] + transfer_amount_wei))"' \
  "최종 잔액은 송금 전 잔액의 증가분으로 계산해야 합니다."
require_pattern '[[ "${recipient_balances[3]}" == "$expected_recipient_balance" ]]' \
  "네 노드 모두에서 기대 잔액을 확인해야 합니다."
require_pattern 'eth_sendRawTransaction' \
  "실제 EIP-155 서명 raw 거래를 4노드 BFT로 확정해야 합니다."
require_pattern 'IEUM_CI_IDLE_WAIT_SECONDS' \
  "유휴 상태가 끝난 뒤 첫 거래 확정을 검증해야 합니다."
require_pattern 'raw 거래 확정 nonce 불일치' \
  "raw 거래 확정 뒤 네 노드의 nonce 증가를 확인해야 합니다."

if grep -Fq 'recipient_balances[*]}" == "100000000000000000 ' "$network_test"; then
  echo "CI 회귀 방지 실패: 고정 절대 잔액 판정이 다시 추가됐습니다." >&2
  exit 1
fi

echo "Chain CI regression guards passed."

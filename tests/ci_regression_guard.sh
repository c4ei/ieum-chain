#!/usr/bin/env bash
set -euo pipefail

network_test="tests/four_process_network.sh"
operational_test="tests/v0_23_8_operational_basics.sh"

bash -n "$network_test"
bash -n "$operational_test"

if [[ ! -x "$operational_test" ]]; then
  echo "CI 회귀 방지 실패: v0.23.8 운영 기본기 스크립트에 실행 권한이 필요합니다." >&2
  exit 1
fi

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

if grep -Fq 'recipient_balances[*]}" == "100000000000000000 ' "$network_test"; then
  echo "CI 회귀 방지 실패: 고정 절대 잔액 판정이 다시 추가됐습니다." >&2
  exit 1
fi

echo "Chain CI regression guards passed."

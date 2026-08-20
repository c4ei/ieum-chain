#!/usr/bin/env bash
set -euo pipefail

# v0.23.8에서 수정한 수수료/RPC/재시작 경로만 빠르게 재검증합니다.
# 전체 회귀 검증과 실제 4프로세스 BFT 검증은 rust-ci.yml이 이어서 수행합니다.
cargo test --lib --locked block_fee_is_split_between_producer_and_foundation
cargo test --lib --locked eip155_legacy_transfer_recovers_sender_and_fields
cargo test --lib --locked native_transaction_fee_terms_preserve_exact_total_fee
cargo test --lib --locked metamask_raw_transaction_and_receipt_survive_rpc_restart

echo "IEUM Chain v0.23.8 operational basics passed."

use crate::model::Address;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const STAKING_SYSTEM_ADDRESS: &str = "0x0000000000000000000000000000000000021004";
pub const MINIMUM_DELEGATION: u128 = 1_000_000_000_000_000_000;
pub const UNBONDING_SECONDS: u64 = 7 * 24 * 60 * 60;
pub const DOUBLE_VOTE_SLASH_BPS: u32 = 500;
pub const BPS_DENOMINATOR: u128 = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DelegationPosition {
    pub delegator: Address,
    pub validator: Address,
    #[serde(with = "crate::model::decimal_u128")]
    pub amount: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnbondingEntry {
    pub delegator: Address,
    pub validator: Address,
    #[serde(with = "crate::model::decimal_u128")]
    pub amount: u128,
    pub release_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StakingState {
    #[serde(default)]
    pub delegations: Vec<DelegationPosition>,
    #[serde(default)]
    pub unbonding: Vec<UnbondingEntry>,
    #[serde(default)]
    pub applied_slashes: Vec<String>,
}

impl StakingState {
    pub fn delegated_to(&self, delegator: &str, validator: &str) -> u128 {
        self.delegations
            .iter()
            .find(|position| position.delegator == delegator && position.validator == validator)
            .map(|position| position.amount)
            .unwrap_or(0)
    }

    pub fn total_delegated_to(&self, validator: &str) -> u128 {
        self.delegations
            .iter()
            .filter(|position| position.validator == validator)
            .fold(0u128, |sum, position| sum.saturating_add(position.amount))
    }

    pub fn delegate(
        &mut self,
        delegator: &str,
        validator: &str,
        amount: u128,
    ) -> Result<(), String> {
        validate_validator(validator)?;
        if amount < MINIMUM_DELEGATION {
            return Err("최소 위임 수량은 1 IEUM입니다.".into());
        }
        if delegator == validator {
            return Err("자기 자신에게는 위임할 수 없습니다.".into());
        }
        if let Some(position) = self
            .delegations
            .iter_mut()
            .find(|position| position.delegator == delegator && position.validator == validator)
        {
            position.amount = position
                .amount
                .checked_add(amount)
                .ok_or("위임액이 u128 범위를 넘습니다.")?;
        } else {
            self.delegations.push(DelegationPosition {
                delegator: delegator.into(),
                validator: validator.into(),
                amount,
            });
        }
        self.sort();
        Ok(())
    }

    pub fn undelegate(
        &mut self,
        delegator: &str,
        validator: &str,
        amount: u128,
        block_timestamp: u64,
    ) -> Result<u64, String> {
        if amount == 0 {
            return Err("해제 수량은 0보다 커야 합니다.".into());
        }
        let position = self
            .delegations
            .iter_mut()
            .find(|position| position.delegator == delegator && position.validator == validator)
            .ok_or("해제할 위임이 없습니다.")?;
        if position.amount < amount {
            return Err("해제 수량이 위임 잔액보다 큽니다.".into());
        }
        position.amount -= amount;
        self.delegations.retain(|position| position.amount > 0);
        let release_at = block_timestamp
            .checked_add(UNBONDING_SECONDS)
            .ok_or("해제 시각이 범위를 넘습니다.")?;
        self.unbonding.push(UnbondingEntry {
            delegator: delegator.into(),
            validator: validator.into(),
            amount,
            release_at,
        });
        self.sort();
        Ok(release_at)
    }

    pub fn claim(&mut self, delegator: &str, block_timestamp: u64) -> Result<u128, String> {
        let amount = self
            .unbonding
            .iter()
            .filter(|entry| entry.delegator == delegator && entry.release_at <= block_timestamp)
            .try_fold(0u128, |sum, entry| {
                sum.checked_add(entry.amount)
                    .ok_or("청구액이 u128 범위를 넘습니다.")
            })?;
        if amount == 0 {
            return Err("아직 청구할 수 있는 해제 잔액이 없습니다.".into());
        }
        self.unbonding
            .retain(|entry| !(entry.delegator == delegator && entry.release_at <= block_timestamp));
        Ok(amount)
    }

    pub fn slash(
        &mut self,
        evidence_id: &str,
        validator: &str,
        penalty_bps: u32,
    ) -> Result<u128, String> {
        if !(1..=DOUBLE_VOTE_SLASH_BPS).contains(&penalty_bps) {
            return Err("허용되지 않은 페널티 비율입니다.".into());
        }
        if self
            .applied_slashes
            .iter()
            .any(|known| known == evidence_id)
        {
            return Err("이미 적용한 이중투표 페널티입니다.".into());
        }
        let mut total = 0u128;
        for position in self
            .delegations
            .iter_mut()
            .filter(|position| position.validator == validator)
        {
            let cut = position.amount.saturating_mul(u128::from(penalty_bps)) / BPS_DENOMINATOR;
            position.amount -= cut;
            total = total.checked_add(cut).ok_or("페널티 합계가 넘습니다.")?;
        }
        for entry in self
            .unbonding
            .iter_mut()
            .filter(|entry| entry.validator == validator)
        {
            let cut = entry.amount.saturating_mul(u128::from(penalty_bps)) / BPS_DENOMINATOR;
            entry.amount -= cut;
            total = total.checked_add(cut).ok_or("페널티 합계가 넘습니다.")?;
        }
        self.delegations.retain(|position| position.amount > 0);
        self.unbonding.retain(|entry| entry.amount > 0);
        self.applied_slashes.push(evidence_id.into());
        self.applied_slashes.sort();
        Ok(total)
    }

    pub fn state_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("staking serialization")
    }
    fn sort(&mut self) {
        self.delegations.sort_by(|left, right| {
            (&left.delegator, &left.validator).cmp(&(&right.delegator, &right.validator))
        });
        self.unbonding.sort_by(|left, right| {
            (&left.release_at, &left.delegator, &left.validator).cmp(&(
                &right.release_at,
                &right.delegator,
                &right.validator,
            ))
        });
    }
}

pub fn validate_validator(validator: &str) -> Result<(), String> {
    if validator.len() != 64 || !validator.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("이음지기 주소는 32바이트 공개키 hex여야 합니다.".into());
    }
    Ok(())
}

pub fn slash_event_id(evidence: &crate::consensus::DoubleVoteEvidence) -> String {
    format!(
        "ieum-slash-v1-{}",
        hex::encode(Sha256::digest(
            serde_json::to_vec(evidence).expect("evidence serialization")
        ))
    )
}

pub fn reward_event_id(timestamp: u64) -> String {
    format!(
        "ieum-delegation-reward-v1-{}",
        crate::validator_interest::kst_day(timestamp)
    )
}

pub fn calculate_rewards(
    state: &StakingState,
    annual_rate_bps: u32,
    maximum_daily_total: u128,
) -> Vec<crate::scheduled_event::EventPayment> {
    let mut totals = BTreeMap::<String, u128>::new();
    for position in &state.delegations {
        let entry = totals.entry(position.delegator.clone()).or_default();
        *entry = entry.saturating_add(position.amount);
    }
    let mut remaining = maximum_daily_total;
    let mut payments = Vec::new();
    for (address, stake) in totals {
        if remaining == 0 {
            break;
        }
        let raw = stake.saturating_mul(u128::from(annual_rate_bps)) / 10_000 / 365;
        let amount = (raw.saturating_add(500_000) / 1_000_000 * 1_000_000).min(remaining);
        if amount > 0 {
            payments.push(crate::scheduled_event::EventPayment { address, amount });
            remaining -= amount;
        }
    }
    payments
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delegate_wait_and_claim() {
        let mut state = StakingState::default();
        let validator = "11".repeat(32);
        state
            .delegate("alice", &validator, 10 * MINIMUM_DELEGATION)
            .unwrap();
        let release = state
            .undelegate("alice", &validator, 4 * MINIMUM_DELEGATION, 10)
            .unwrap();
        assert!(state.claim("alice", release - 1).is_err());
        assert_eq!(
            state.claim("alice", release).unwrap(),
            4 * MINIMUM_DELEGATION
        );
        assert_eq!(
            state.delegated_to("alice", &validator),
            6 * MINIMUM_DELEGATION
        );
    }
    #[test]
    fn slash_active_and_unbonding() {
        let mut state = StakingState::default();
        let validator = "22".repeat(32);
        state
            .delegate("alice", &validator, 100 * MINIMUM_DELEGATION)
            .unwrap();
        state
            .undelegate("alice", &validator, 20 * MINIMUM_DELEGATION, 1)
            .unwrap();
        assert_eq!(
            state.slash("proof", &validator, 500).unwrap(),
            5 * MINIMUM_DELEGATION
        );
        assert!(state.slash("proof", &validator, 500).is_err());
    }
    #[test]
    fn reward_is_rounded_to_twelve_decimals() {
        let mut state = StakingState::default();
        let validator = "33".repeat(32);
        state
            .delegate(
                "0x1111111111111111111111111111111111111111",
                &validator,
                99_9999u128 * 10u128.pow(14),
            )
            .unwrap();
        let payments = calculate_rewards(&state, 500, 1_000 * MINIMUM_DELEGATION);
        assert_eq!(payments[0].amount, 13_698_616_438_000_000);
    }
}

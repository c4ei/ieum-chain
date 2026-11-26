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
    /// v1.0.5.1 이전 위임은 0으로 읽혀 이미 성숙한 담보로 취급합니다.
    #[serde(default)]
    pub delegated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnbondingEntry {
    pub delegator: Address,
    pub validator: Address,
    #[serde(with = "crate::model::decimal_u128")]
    pub amount: u128,
    pub release_at: u64,
}

/// 담보 전 노드가 쌓은 잠긴 적립 잔액입니다(프로토콜 v5). 보낼 수 없고 같은 주소의 담보로만 인정됩니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedAccrual {
    #[serde(with = "crate::model::decimal_u128")]
    pub amount: u128,
    /// 마지막으로 적립된 보상 epoch(일)입니다. 소멸 판단에 씁니다.
    pub last_epoch: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StakingState {
    #[serde(default)]
    pub delegations: Vec<DelegationPosition>,
    #[serde(default)]
    pub unbonding: Vec<UnbondingEntry>,
    #[serde(default)]
    pub applied_slashes: Vec<String>,
    /// 비어 있으면 직렬화하지 않아 v5 이전 상태 해시가 그대로 유지됩니다.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub accruals: BTreeMap<Address, LockedAccrual>,
    /// 웹 보상 적립(v6) 기록: 주소별 마지막 epoch와 그날 적립 합계입니다. 비어 있으면 직렬화하지 않습니다.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub accrual_days: BTreeMap<Address, AccrualDay>,
    /// 해킹 피해 대응용 계정 동결(v6). 동결된 주소는 거래를 보낼 수 없고 받기만 합니다.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub frozen: BTreeMap<Address, FreezeRecord>,
}

/// 동결 기록입니다. 복구는 frozen_at + 공개 유예 이후, expires_at 이전에만 할 수 있습니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FreezeRecord {
    pub case_id: String,
    pub frozen_at: u64,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccrualDay {
    pub epoch: u64,
    #[serde(with = "crate::model::decimal_u128")]
    pub amount: u128,
}

impl StakingState {
    pub fn locked_accrual_of(&self, address: &str) -> u128 {
        self.accruals
            .get(address)
            .map(|entry| entry.amount)
            .unwrap_or(0)
    }

    pub fn add_accrual(&mut self, address: &str, amount: u128, epoch: u64) -> Result<(), String> {
        let entry = self
            .accruals
            .entry(address.into())
            .or_insert(LockedAccrual {
                amount: 0,
                last_epoch: epoch,
            });
        entry.amount = entry
            .amount
            .checked_add(amount)
            .ok_or("적립 잔액이 u128 범위를 넘습니다.")?;
        entry.last_epoch = entry.last_epoch.max(epoch);
        // 같은 날 웹 적립 기록이 있으면 노드 적립도 합산해 하루 한도를 함께 씁니다.
        if let Some(day) = self.accrual_days.get_mut(address)
            && day.epoch == epoch
        {
            day.amount = day.amount.saturating_add(amount);
        }
        Ok(())
    }

    pub fn is_frozen(&self, address: &str, timestamp: u64) -> bool {
        self.frozen
            .get(address)
            .is_some_and(|record| timestamp < record.expires_at)
    }

    /// 이 epoch에 이미 쓴 하루 적립 한도입니다. 웹 기록 없이 노드 적립만 있었다면 한도를 다 쓴 것으로 봅니다.
    pub fn accrual_used_in(&self, address: &str, epoch: u64, cap: u128) -> u128 {
        match self.accrual_days.get(address) {
            Some(day) if day.epoch == epoch => day.amount,
            _ if self
                .accruals
                .get(address)
                .is_some_and(|entry| entry.last_epoch == epoch) =>
            {
                cap
            }
            _ => 0,
        }
    }

    /// 웹 보상 적립(v6). 보낸 사람 잔액에서 빠진 금액을 받는 주소의 잠긴 적립으로 넣습니다.
    pub fn grant_accrual(
        &mut self,
        address: &str,
        amount: u128,
        epoch: u64,
        cap: u128,
        bond_room: u128,
    ) -> Result<(), String> {
        if amount == 0 {
            return Err("적립액은 0보다 커야 합니다.".into());
        }
        let used = self.accrual_used_in(address, epoch, cap);
        if used.saturating_add(amount) > cap {
            return Err(format!(
                "하루 적립 한도를 넘습니다(오늘 {used}, 한도 {cap})."
            ));
        }
        if amount > bond_room {
            return Err("담보 100 IEUM을 넘는 적립은 할 수 없습니다.".into());
        }
        self.accrual_days.insert(
            address.into(),
            AccrualDay {
                epoch,
                amount: used.saturating_add(amount),
            },
        );
        let entry = self
            .accruals
            .entry(address.into())
            .or_insert(LockedAccrual {
                amount: 0,
                last_epoch: epoch,
            });
        entry.amount = entry
            .amount
            .checked_add(amount)
            .ok_or("적립 잔액이 u128 범위를 넘습니다.")?;
        entry.last_epoch = entry.last_epoch.max(epoch);
        Ok(())
    }

    /// 소멸 대상 적립을 지우고 그 금액을 돌려줍니다. 금액이 다르면 거절합니다.
    pub fn expire_accrual(&mut self, address: &str, amount: u128) -> Result<(), String> {
        match self.accruals.get(address) {
            Some(entry) if entry.amount == amount => {
                self.accruals.remove(address);
                self.accrual_days.remove(address);
                Ok(())
            }
            _ => Err("소멸할 적립 잔액이 현재 상태와 다릅니다.".into()),
        }
    }

    pub fn total_accruals(&self) -> u128 {
        self.accruals
            .values()
            .fold(0u128, |sum, entry| sum.saturating_add(entry.amount))
    }

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
        self.delegate_at(delegator, validator, amount, 0)
    }

    pub fn delegate_at(
        &mut self,
        delegator: &str,
        validator: &str,
        amount: u128,
        delegated_at: u64,
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
            position.delegated_at = position.delegated_at.max(delegated_at);
        } else {
            self.delegations.push(DelegationPosition {
                delegator: delegator.into(),
                validator: validator.into(),
                amount,
                delegated_at,
            });
        }
        self.sort();
        Ok(())
    }

    pub fn mature_delegated_by(
        &self,
        delegator: &str,
        timestamp: u64,
        maturity_seconds: u64,
    ) -> u128 {
        self.delegations
            .iter()
            .filter(|position| {
                position.delegator == delegator
                    && position.delegated_at.saturating_add(maturity_seconds) <= timestamp
            })
            .fold(0u128, |sum, position| sum.saturating_add(position.amount))
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

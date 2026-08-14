use crate::{EventPayment, Validator};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub const BPS_DENOMINATOR: u128 = 10_000;
pub const DAYS_PER_YEAR: u128 = 365;
pub const KST_OFFSET_SECONDS: u64 = 9 * 60 * 60;
pub const DAY_SECONDS: u64 = 24 * 60 * 60;
/// 18자리 원시 수량을 화면 기준 소수점 12자리에서 반올림합니다.
pub const INTEREST_ROUNDING_UNIT: u128 = 1_000_000;

fn round_to_twelve_decimals(value: u128) -> u128 {
    value.saturating_add(INTEREST_ROUNDING_UNIT / 2) / INTEREST_ROUNDING_UNIT
        * INTEREST_ROUNDING_UNIT
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ValidatorInterestPolicy {
    pub enabled: bool,
    /// 연 이율. 500 = 5.00% APR. 일 단리로 하루 한 번 지급합니다.
    pub annual_rate_bps: u32,
    #[serde(with = "crate::model::decimal_u128")]
    pub minimum_balance: u128,
    #[serde(with = "crate::model::decimal_u128")]
    pub maximum_daily_total: u128,
}

impl Default for ValidatorInterestPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            annual_rate_bps: 500,
            minimum_balance: 10u128.pow(18),
            maximum_daily_total: 1_000u128 * 10u128.pow(18),
        }
    }
}

impl ValidatorInterestPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let value: Self = serde_json::from_slice(
            &fs::read(path)
                .map_err(|e| format!("검증자 이자 설정 읽기 실패({}): {e}", path.display()))?,
        )
        .map_err(|e| format!("검증자 이자 설정 JSON 오류({}): {e}", path.display()))?;
        value.validate()?;
        Ok(value)
    }
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temp = path.with_extension("json.tmp");
        fs::write(
            &temp,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        fs::rename(temp, path).map_err(|e| e.to_string())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.annual_rate_bps > 5_000 {
            return Err("검증자 연 이율은 0~50.00%(0~5000bps)여야 합니다.".into());
        }
        if self.minimum_balance == 0 {
            return Err("검증자 최소 보유액은 0보다 커야 합니다.".into());
        }
        if self.maximum_daily_total == 0 {
            return Err("일일 총 지급 상한은 0보다 커야 합니다.".into());
        }
        Ok(())
    }
    pub fn hash(&self) -> String {
        hex::encode(Sha256::digest(
            serde_json::to_vec(self).expect("policy serialization"),
        ))
    }
}

pub fn kst_day(timestamp: u64) -> u64 {
    timestamp.saturating_add(KST_OFFSET_SECONDS) / DAY_SECONDS
}
pub fn event_id(timestamp: u64) -> String {
    format!("ieum-validator-interest-v1-{}", kst_day(timestamp))
}
pub fn execute_at(timestamp: u64) -> u64 {
    kst_day(timestamp)
        .saturating_mul(DAY_SECONDS)
        .saturating_sub(KST_OFFSET_SECONDS)
}

pub fn calculate_payments(
    policy: &ValidatorInterestPolicy,
    validators: &[Validator],
    balances: &std::collections::HashMap<String, u128>,
) -> Vec<EventPayment> {
    if !policy.enabled || policy.annual_rate_bps == 0 {
        return Vec::new();
    }
    let mut validators = validators.to_vec();
    validators.sort_by(|a, b| a.id.cmp(&b.id));
    let mut remaining = policy.maximum_daily_total;
    let mut payments = Vec::new();
    for validator in validators {
        let balance = balances.get(&validator.id).copied().unwrap_or(0);
        if balance < policy.minimum_balance || remaining == 0 {
            continue;
        }
        let amount = round_to_twelve_decimals(
            balance.saturating_mul(u128::from(policy.annual_rate_bps))
                / BPS_DENOMINATOR
                / DAYS_PER_YEAR,
        )
        .min(remaining);
        if amount > 0 {
            payments.push(EventPayment {
                address: validator.id,
                amount,
            });
            remaining -= amount;
        }
    }
    payments
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_supply_validator_is_eligible() {
        let v = Validator::new("11".repeat(32), 100);
        let balances = [(v.id.clone(), 10 * 10u128.pow(18))].into_iter().collect();
        let payments = calculate_payments(&ValidatorInterestPolicy::default(), &[v], &balances);
        assert_eq!(payments.len(), 1);
        // 10 IEUM × 5.00% APR ÷ 365일 = 약 0.001369863 IEUM/일
        assert_eq!(payments[0].amount, 1_369_863_014_000_000);
    }
    #[test]
    fn one_event_id_per_kst_day() {
        assert_eq!(event_id(1_786_287_600), event_id(1_786_287_600 + 86_399));
        assert_ne!(event_id(1_786_287_600), event_id(1_786_287_600 + 86_400));
    }
}

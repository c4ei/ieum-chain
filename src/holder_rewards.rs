use crate::EventPayment;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

const IEUM: u128 = 10u128.pow(18);
const BPS: u128 = 10_000;
const DAYS: u128 = 365;
const DAY_SECONDS: u64 = 86_400;
const KST_OFFSET: u64 = 32_400;
const ROUND_12_DECIMALS: u128 = 1_000_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HolderRewardPolicy {
    pub enabled: bool,
    pub campaign_name: String,
    pub starts_at: u64,
    pub ends_at: u64,
    pub annual_rate_bps: u32,
    #[serde(with = "crate::model::decimal_u128")]
    pub minimum_balance: u128,
    #[serde(with = "crate::model::decimal_u128")]
    pub maximum_daily_total: u128,
}

impl Default for HolderRewardPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            campaign_name: "이음 첫 보유 응원 이벤트".into(),
            starts_at: 0,
            ends_at: 0,
            annual_rate_bps: 500,
            minimum_balance: IEUM,
            maximum_daily_total: 100 * IEUM,
        }
    }
}

impl HolderRewardPolicy {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let policy: Self = serde_json::from_slice(
            &fs::read(path).map_err(|e| format!("보유 보상 설정 읽기 실패: {e}"))?,
        )
        .map_err(|e| format!("보유 보상 설정 JSON 오류: {e}"))?;
        policy.validate()?;
        Ok(policy)
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
        if self.campaign_name.trim().is_empty() || self.campaign_name.len() > 80 {
            return Err("이벤트 이름은 1~80자여야 합니다.".into());
        }
        if self.annual_rate_bps > 5_000 {
            return Err("보유 보상 APR은 0~50.00%여야 합니다.".into());
        }
        if self.minimum_balance == 0 || self.maximum_daily_total == 0 {
            return Err("최소 보유량과 일일 상한은 0보다 커야 합니다.".into());
        }
        if self.enabled && (self.starts_at == 0 || self.ends_at <= self.starts_at) {
            return Err("활성 이벤트의 시작·종료 시각이 올바르지 않습니다.".into());
        }
        Ok(())
    }
    pub fn active(&self, timestamp: u64) -> bool {
        self.enabled && self.starts_at <= timestamp && timestamp <= self.ends_at
    }
    pub fn hash(&self) -> String {
        hex::encode(Sha256::digest(
            serde_json::to_vec(self).expect("policy serialization"),
        ))
    }
}

pub fn event_id(timestamp: u64) -> String {
    format!(
        "ieum-holder-reward-v1-{}",
        (timestamp + KST_OFFSET) / DAY_SECONDS
    )
}
pub fn execute_at(timestamp: u64) -> u64 {
    ((timestamp + KST_OFFSET) / DAY_SECONDS) * DAY_SECONDS - KST_OFFSET
}
pub fn round_12(amount: u128) -> u128 {
    amount.saturating_add(ROUND_12_DECIMALS / 2) / ROUND_12_DECIMALS * ROUND_12_DECIMALS
}

pub fn calculate_payments(
    policy: &HolderRewardPolicy,
    balances: &HashMap<String, u128>,
    excluded: &HashSet<String>,
) -> Vec<EventPayment> {
    if !policy.enabled || policy.annual_rate_bps == 0 {
        return Vec::new();
    }
    let mut accounts: Vec<_> = balances
        .iter()
        .filter(|(address, balance)| {
            address.starts_with("0x")
                && **balance >= policy.minimum_balance
                && !excluded.contains(&address.to_ascii_lowercase())
        })
        .map(|(address, balance)| (address.to_ascii_lowercase(), *balance))
        .collect();
    accounts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut remaining = policy.maximum_daily_total;
    let mut payments = Vec::new();
    for (address, balance) in accounts {
        let amount =
            round_12(balance.saturating_mul(u128::from(policy.annual_rate_bps)) / BPS / DAYS)
                .min(remaining);
        if amount > 0 {
            payments.push(EventPayment { address, amount });
            remaining -= amount;
        }
        if remaining == 0 {
            break;
        }
    }
    payments
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advertised_example_rounds_to_twelve_decimals() {
        let policy = HolderRewardPolicy {
            enabled: true,
            starts_at: 1,
            ends_at: 2,
            ..Default::default()
        };
        let balances = [(
            "0x475e2f4e40dbd34370e4fce61ddff5f1f2ea817".into(),
            99_9999u128 * 10u128.pow(14),
        )]
        .into_iter()
        .collect();
        let payments = calculate_payments(&policy, &balances, &HashSet::new());
        assert_eq!(payments[0].amount, 13_698_616_438_000_000);
    }
}

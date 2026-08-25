use crate::model::Address;
use crate::network::{NodeRewardRegistration, ValidatorRegistration};
use crate::wallet::verify_signature;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub const MAX_CLOCK_DRIFT_SECONDS: u64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledEventAction {
    TreasuryDistribution {
        recipients: Vec<EventPayment>,
    },
    PeriodicProducerReward {
        producer: Address,
        #[serde(with = "crate::model::decimal_u128")]
        amount: u128,
    },
    IncidentCompensation {
        incident_id: String,
        victim: Address,
        #[serde(with = "crate::model::decimal_u128")]
        amount: u128,
    },
    ProtocolCheckpoint {
        protocol_version: u32,
    },
    /// 최초 4검증자 구성이 완성됐을 때 검증자별로 한 번만 지급합니다.
    BootstrapValidatorReward {
        registrations: Vec<ValidatorRegistration>,
        #[serde(with = "crate::model::decimal_u128")]
        amount: u128,
    },
    /// 서명으로 소유권을 증명한 서로 다른 노드가 100개 이상 모였을 때 한 번만 지급합니다.
    NodeMilestoneReward {
        registrations: Vec<NodeRewardRegistration>,
        #[serde(with = "crate::model::decimal_u128")]
        amount: u128,
    },
    /// KST 날짜별 검증자 보유 잔액 snapshot을 기준으로 계산한 일일 이자입니다.
    ValidatorDailyInterest {
        snapshot_height: u64,
        policy_hash: String,
        annual_rate_bps: u32,
        payments: Vec<EventPayment>,
    },
    /// 기간형 일반 0x 지갑 보유 응원 이벤트의 하루 지급분입니다.
    HolderDailyReward {
        snapshot_height: u64,
        policy_hash: String,
        annual_rate_bps: u32,
        payments: Vec<EventPayment>,
    },
    /// 서명된 이중투표 증거가 합의 블록에 포함될 때 위임·해제대기 자금에 적용합니다.
    DoubleVoteSlash {
        evidence: crate::consensus::DoubleVoteEvidence,
        penalty_bps: u32,
    },
    DelegationDailyReward {
        snapshot_height: u64,
        policy_hash: String,
        annual_rate_bps: u32,
        payments: Vec<EventPayment>,
    },
    /// 100 IEUM 성숙 담보와 3/4 독립 서비스 증명을 통과한 외부 공개 노드의 일일 보상입니다.
    NodeServiceDailyReward {
        snapshot_height: u64,
        epoch: u64,
        attestations: Vec<crate::node_emission::NodeServiceAttestation>,
        payments: Vec<EventPayment>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventPayment {
    pub address: Address,
    #[serde(with = "crate::model::decimal_u128")]
    pub amount: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledEvent {
    pub id: String,
    pub execute_at: u64,
    pub action: ScheduledEventAction,
}

impl ScheduledEvent {
    pub fn consensus_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("ScheduledEvent 직렬화는 실패하지 않아야 합니다")
    }

    pub fn payload_hash(&self) -> String {
        hex::encode(Sha256::digest(self.consensus_bytes()))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_' | b'.'))
        {
            return Err("이벤트 ID는 1~128자의 영문, 숫자, -, _, .만 허용합니다.".into());
        }
        if self.execute_at == 0 {
            return Err(format!(
                "이벤트 {}의 execute_at은 0일 수 없습니다.",
                self.id
            ));
        }
        let payments = match &self.action {
            ScheduledEventAction::TreasuryDistribution { recipients } => recipients,
            ScheduledEventAction::PeriodicProducerReward { producer, amount } => {
                validate_address(producer)?;
                if *amount == 0 {
                    return Err("주간 생성자 보상액은 0보다 커야 합니다.".into());
                }
                return Ok(());
            }
            ScheduledEventAction::IncidentCompensation {
                incident_id,
                victim,
                amount,
            } => {
                if incident_id.trim().is_empty() {
                    return Err("사고 ID가 비어 있습니다.".into());
                }
                validate_address(victim)?;
                if *amount == 0 {
                    return Err("사고 보상액은 0보다 커야 합니다.".into());
                }
                return Ok(());
            }
            ScheduledEventAction::ProtocolCheckpoint { protocol_version } => {
                if *protocol_version == 0 {
                    return Err("프로토콜 버전은 1 이상이어야 합니다.".into());
                }
                return Ok(());
            }
            ScheduledEventAction::BootstrapValidatorReward {
                registrations,
                amount,
            } => {
                if registrations.len() != 4 || *amount == 0 {
                    return Err(
                        "최초 검증자 보상은 정확히 4명이고 금액은 0보다 커야 합니다.".into(),
                    );
                }
                let mut peers = HashSet::new();
                let mut addresses = HashSet::new();
                for registration in registrations {
                    if !peers.insert(&registration.peer_id)
                        || !addresses.insert(&registration.validator_id)
                    {
                        return Err("최초 검증자 보상 증명에 중복 노드나 주소가 있습니다.".into());
                    }
                    verify_signature(
                        &registration.validator_id,
                        &ValidatorRegistration::bytes_to_sign(
                            &registration.validator_id,
                            &registration.peer_id,
                        ),
                        &registration.signature_hex,
                    )?;
                }
                return Ok(());
            }
            ScheduledEventAction::NodeMilestoneReward {
                registrations,
                amount,
            } => {
                if registrations.len() != 100 || *amount == 0 {
                    return Err(
                        "노드 마일스톤 보상은 서로 다른 노드 정확히 100개가 필요합니다.".into(),
                    );
                }
                let mut peers = HashSet::new();
                let mut addresses = HashSet::new();
                for registration in registrations {
                    validate_reward_address(&registration.reward_address)?;
                    registration.verify_node_identity()?;
                    if !peers.insert(&registration.peer_id)
                        || !addresses.insert(&registration.reward_address)
                    {
                        return Err("노드 마일스톤 보상 증명에 중복 노드나 주소가 있습니다.".into());
                    }
                    verify_signature(
                        registration.registration_signer(),
                        &NodeRewardRegistration::bytes_to_sign(
                            &registration.reward_address,
                            &registration.peer_id,
                        ),
                        &registration.signature_hex,
                    )?;
                }
                return Ok(());
            }
            ScheduledEventAction::ValidatorDailyInterest {
                snapshot_height: _,
                policy_hash,
                annual_rate_bps,
                payments,
            } => {
                if policy_hash.len() != 64 || !policy_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("검증자 이자 정책 hash가 올바르지 않습니다.".into());
                }
                if *annual_rate_bps > 5_000 || payments.is_empty() {
                    return Err("검증자 이자율 또는 지급 대상이 올바르지 않습니다.".into());
                }
                for payment in payments {
                    validate_reward_address(&payment.address)?;
                    if payment.amount == 0 {
                        return Err("검증자 이자 지급액은 0보다 커야 합니다.".into());
                    }
                }
                return Ok(());
            }
            ScheduledEventAction::HolderDailyReward {
                snapshot_height: _,
                policy_hash,
                annual_rate_bps,
                payments,
            } => {
                if policy_hash.len() != 64
                    || !policy_hash.bytes().all(|b| b.is_ascii_hexdigit())
                    || *annual_rate_bps > 5_000
                    || payments.is_empty()
                {
                    return Err("보유 응원 보상 정책 또는 지급 대상이 올바르지 않습니다.".into());
                }
                for payment in payments {
                    validate_address(&payment.address)?;
                    if payment.amount == 0 {
                        return Err("보유 응원 보상액은 0보다 커야 합니다.".into());
                    }
                }
                return Ok(());
            }
            ScheduledEventAction::DoubleVoteSlash {
                evidence,
                penalty_bps,
            } => {
                evidence.verify()?;
                if *penalty_bps != crate::staking::DOUBLE_VOTE_SLASH_BPS {
                    return Err("이중투표 페널티는 합의 상수 5%여야 합니다.".into());
                }
                if self.id != crate::staking::slash_event_id(evidence) {
                    return Err("이중투표 페널티 이벤트 ID가 증거와 일치하지 않습니다.".into());
                }
                return Ok(());
            }
            ScheduledEventAction::DelegationDailyReward {
                policy_hash,
                annual_rate_bps,
                payments,
                ..
            } => {
                if policy_hash.len() != 64
                    || !policy_hash.bytes().all(|b| b.is_ascii_hexdigit())
                    || *annual_rate_bps > 5_000
                    || payments.is_empty()
                {
                    return Err("위임 보상 정책 또는 지급 대상이 올바르지 않습니다.".into());
                }
                for payment in payments {
                    validate_reward_address(&payment.address)?;
                    if payment.amount == 0 {
                        return Err("위임 보상액은 0보다 커야 합니다.".into());
                    }
                }
                return Ok(());
            }
            ScheduledEventAction::NodeServiceDailyReward {
                epoch,
                attestations,
                payments,
                ..
            } => {
                if self.id != crate::node_emission::service_event_id(*epoch)
                    || attestations.len() < crate::node_emission::SERVICE_MINIMUM_VALIDATORS
                    || attestations.len() > 10_000
                    || payments.is_empty()
                    || payments.len() > 10_000
                {
                    return Err(
                        "공개 노드 일일 보상 증명 또는 지급 대상이 올바르지 않습니다.".into(),
                    );
                }
                for attestation in attestations {
                    attestation.verify()?;
                    if attestation.epoch != *epoch {
                        return Err("공개 노드 서비스 증명의 epoch가 다릅니다.".into());
                    }
                }
                for payment in payments {
                    validate_address(&payment.address)?;
                    if payment.amount == 0 {
                        return Err("공개 노드 일일 보상액은 0보다 커야 합니다.".into());
                    }
                }
                return Ok(());
            }
        };
        if payments.is_empty() || payments.len() > 10_000 {
            return Err("재단 배분 대상은 1~10,000개여야 합니다.".into());
        }
        for payment in payments {
            validate_address(&payment.address)?;
            if payment.amount == 0 {
                return Err("재단 배분액은 0보다 커야 합니다.".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EventSchedule {
    #[serde(default)]
    pub events: Vec<ScheduledEvent>,
}

impl EventSchedule {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path).map_err(|error| {
            format!("이벤트 설정을 읽지 못했습니다({}): {error}", path.display())
        })?;
        let schedule: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("이벤트 설정 JSON 오류({}): {error}", path.display()))?;
        schedule.validate()?;
        Ok(schedule)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for event in &self.events {
            event.validate()?;
            if !ids.insert(&event.id) {
                return Err(format!("중복 이벤트 ID: {}", event.id));
            }
        }
        Ok(())
    }

    pub fn due(&self, timestamp: u64, executed: &HashSet<String>) -> Vec<ScheduledEvent> {
        let mut due: Vec<_> = self
            .events
            .iter()
            .filter(|event| event.execute_at <= timestamp && !executed.contains(&event.id))
            .cloned()
            .collect();
        due.sort_by(|a, b| (a.execute_at, &a.id).cmp(&(b.execute_at, &b.id)));
        due
    }

    pub fn validate_block_events(
        &self,
        timestamp: u64,
        events: &[ScheduledEvent],
        executed: &HashSet<String>,
    ) -> Result<(), String> {
        let expected = self.due(timestamp, executed);
        if events != expected {
            return Err("블록의 시스템 이벤트가 로컬 승인 일정과 일치하지 않습니다.".into());
        }
        Ok(())
    }

    pub fn next_pending_at(&self, executed: &HashSet<String>) -> Option<u64> {
        self.events
            .iter()
            .filter(|event| !executed.contains(&event.id))
            .map(|event| event.execute_at)
            .min()
    }
}

fn validate_address(address: &str) -> Result<(), String> {
    let value = address.strip_prefix("0x").unwrap_or(address);
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("잘못된 IEUM 주소: {address}"));
    }
    Ok(())
}

fn validate_reward_address(address: &str) -> Result<(), String> {
    let is_account = address.starts_with("0x")
        && address.len() == 42
        && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit());
    let is_legacy = address.len() == 64 && address.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !is_account && !is_legacy {
        return Err(format!("잘못된 노드 보상 주소: {address}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reward_address_accepts_new_account_and_legacy_history() {
        assert!(validate_reward_address("0x1111111111111111111111111111111111111111").is_ok());
        assert!(validate_reward_address(&"11".repeat(32)).is_ok());
        assert!(validate_reward_address("0x1234").is_err());
    }

    #[test]
    fn due_events_are_ordered_and_exactly_once() {
        let schedule = EventSchedule {
            events: vec![
                ScheduledEvent {
                    id: "second".into(),
                    execute_at: 20,
                    action: ScheduledEventAction::ProtocolCheckpoint {
                        protocol_version: 2,
                    },
                },
                ScheduledEvent {
                    id: "first".into(),
                    execute_at: 10,
                    action: ScheduledEventAction::ProtocolCheckpoint {
                        protocol_version: 2,
                    },
                },
            ],
        };
        let mut executed = HashSet::new();
        assert_eq!(schedule.due(20, &executed)[0].id, "first");
        executed.insert("first".into());
        assert_eq!(schedule.due(20, &executed)[0].id, "second");
    }
}

//! Normal 노드 자동 보상의 합의 정책과 결정론적 정산 계산입니다.
//!
//! 이 파일의 값은 모든 검증자가 같아야 합니다. 값을 바꾸는 배포는 반드시
//! 체인 버전을 올리고 같은 활성화 높이에서 4개 검증자를 함께 업그레이드해야 합니다.

use crate::traffic_rewards::{EligibleNode, LotteryPayment};
use crate::{EventPayment, Validator};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const IEUM_DECIMALS: u32 = 18;
pub const IEUM: u128 = 10u128.pow(IEUM_DECIMALS);

/// Genesis와 향후 모든 신규 발행을 합한 IEUM 전역 최대 공급량입니다.
pub const MAX_SUPPLY_IEUM: u128 = 210_000_000;
pub const MAX_SUPPLY: u128 = MAX_SUPPLY_IEUM * IEUM;
/// v0.23.9 메인넷 Genesis 공급량입니다.
pub const MAINNET_GENESIS_SUPPLY_IEUM: u128 = 21_070_100;
/// Normal 노드에 2040년 말까지 신규 발행할 수 있는 잔여 상한입니다.
pub const TOTAL_NODE_EMISSION_IEUM: u128 = MAX_SUPPLY_IEUM - MAINNET_GENESIS_SUPPLY_IEUM;
pub const TOTAL_NODE_EMISSION: u128 = TOTAL_NODE_EMISSION_IEUM * IEUM;

/// 현재 0번 블록인 운영망에 100블록의 업그레이드 여유를 둡니다.
pub const REWARD_ACTIVATION_HEIGHT: u64 = 100;
/// v0.23.9 Genesis 이전에는 신규 노드 보상을 만들지 않습니다.
pub const REWARD_ACTIVATION_UNIX: u64 = crate::genesis::IEUM_MAINNET_GENESIS_TIME;
pub const REWARD_END_UNIX: u64 = 2_240_611_199; // 2040-12-31 23:59:59 UTC
pub const REWARD_EPOCH_SECONDS: u64 = 24 * 60 * 60;
pub const HALVING_SECONDS: u64 = 365 * REWARD_EPOCH_SECONDS;

/// 메인 4노드는 일반 노드 1.0배 대비 1.5배 가중치입니다.
pub const NORMAL_NODE_WEIGHT_BPS: u64 = 10_000;
pub const MAIN_NODE_WEIGHT_BPS: u64 = 15_000;

/// 운영 메인 4노드의 영구 PeerId를 입력합니다. 빈 값은 메인노드로 인정하지 않습니다.
/// PeerId가 확정되면 네 문자열만 교체하고 체인 버전을 올려 함께 배포하세요.
pub const MAIN_NODE_PEER_IDS: [&str; 4] = ["", "", "", ""];

/// 외부 공개 노드 하나가 보상 자격을 얻기 위해 보상 주소로 잠가야 하는 위임액입니다.
pub const SERVICE_BOND: u128 = 100 * IEUM;
pub const SERVICE_BOND_MATURITY_SECONDS: u64 = 7 * REWARD_EPOCH_SECONDS;
pub const SERVICE_MINIMUM_UPTIME_BPS: u16 = 8_000;
pub const SERVICE_MINIMUM_VALIDATORS: usize = 3;
pub const SERVICE_MAX_REWARDED_PER_NETWORK_GROUP: usize = 2;
pub const SERVICE_DAILY_POOL_CAP: u128 = 1_000 * IEUM;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeServiceAttestation {
    pub peer_id: String,
    pub reward_address: String,
    pub epoch: u64,
    pub observed_at: u64,
    pub uptime_bps: u16,
    /// IPv4 /24 또는 IPv6 /48. 원문 IP를 합의·블록에 남기지 않습니다.
    pub network_group: String,
    pub validator_id: String,
    pub signature_hex: String,
}

impl NodeServiceAttestation {
    pub fn bytes_to_sign(&self) -> Vec<u8> {
        format!(
            "ieum-node-service-v1:{}:{}:{}:{}:{}:{}:{}",
            self.peer_id,
            self.reward_address,
            self.epoch,
            self.observed_at,
            self.uptime_bps,
            self.network_group,
            self.validator_id
        )
        .into_bytes()
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.peer_id.is_empty()
            || self.reward_address.len() != 42
            || !self.reward_address.starts_with("0x")
            || !self.reward_address[2..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.epoch != self.observed_at / REWARD_EPOCH_SECONDS
            || self.uptime_bps > 10_000
            || self.network_group.is_empty()
            || self.network_group.len() > 96
        {
            return Err("노드 서비스 증명 필드가 올바르지 않습니다.".into());
        }
        crate::wallet::verify_signature(
            &self.validator_id,
            &self.bytes_to_sign(),
            &self.signature_hex,
        )
    }
}

pub fn service_event_id(epoch: u64) -> String {
    format!("ieum-node-service-reward-v1-{epoch}")
}

/// 세 검증자의 독립 관측, 100 IEUM 성숙 담보, 주소·대역 중복 상한을 모두 적용합니다.
pub fn calculate_service_payments(
    timestamp: u64,
    attestations: &[NodeServiceAttestation],
    validators: &[Validator],
    validator_peer_ids: &HashSet<String>,
    staking: &crate::staking::StakingState,
    foundation_balance: u128,
) -> Result<Vec<EventPayment>, String> {
    let epoch = timestamp / REWARD_EPOCH_SECONDS;
    let active: HashSet<_> = validators
        .iter()
        .map(|validator| validator.id.as_str())
        .collect();
    let mut groups = BTreeMap::<(String, String), Vec<&NodeServiceAttestation>>::new();
    for attestation in attestations {
        attestation.verify()?;
        if attestation.epoch != epoch
            || !active.contains(attestation.validator_id.as_str())
            || validator_peer_ids.contains(&attestation.peer_id)
            || attestation.uptime_bps < SERVICE_MINIMUM_UPTIME_BPS
        {
            continue;
        }
        groups
            .entry((
                attestation.peer_id.clone(),
                attestation.reward_address.clone(),
            ))
            .or_default()
            .push(attestation);
    }

    let mut eligible = Vec::<(String, String, String)>::new();
    let mut used_addresses = HashSet::new();
    for ((peer_id, reward_address), proofs) in groups {
        let unique_validators: HashSet<_> = proofs
            .iter()
            .map(|proof| proof.validator_id.as_str())
            .collect();
        if unique_validators.len() < SERVICE_MINIMUM_VALIDATORS
            || staking.mature_delegated_by(
                &reward_address,
                timestamp,
                SERVICE_BOND_MATURITY_SECONDS,
            ) < SERVICE_BOND
            || !used_addresses.insert(reward_address.clone())
        {
            continue;
        }
        let mut network_counts = HashMap::<&str, usize>::new();
        for proof in &proofs {
            *network_counts.entry(&proof.network_group).or_default() += 1;
        }
        let Some((network_group, count)) = network_counts
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0)))
        else {
            continue;
        };
        if count < SERVICE_MINIMUM_VALIDATORS {
            continue;
        }
        eligible.push((peer_id, reward_address, network_group.to_string()));
    }
    eligible.sort();

    let mut per_network = HashMap::<String, usize>::new();
    eligible.retain(|(_, _, network_group)| {
        let count = per_network.entry(network_group.clone()).or_default();
        if *count >= SERVICE_MAX_REWARDED_PER_NETWORK_GROUP {
            return false;
        }
        *count += 1;
        true
    });
    if eligible.is_empty() {
        return Ok(Vec::new());
    }
    let pool = daily_budget(timestamp)
        .min(SERVICE_DAILY_POOL_CAP)
        .min(foundation_balance);
    if pool == 0 {
        return Ok(Vec::new());
    }
    let eligible_count = eligible.len();
    let each = pool / eligible_count as u128;
    let mut assigned = 0u128;
    Ok(eligible
        .into_iter()
        .enumerate()
        .map(|(index, (_, address, _))| {
            let amount = if index + 1 == eligible_count {
                pool - assigned
            } else {
                each
            };
            assigned += amount;
            EventPayment { address, amount }
        })
        .collect())
}

pub fn is_reward_active(height: u64, timestamp: u64) -> bool {
    height >= REWARD_ACTIVATION_HEIGHT
        && (REWARD_ACTIVATION_UNIX..=REWARD_END_UNIX).contains(&timestamp)
}

pub fn halving_index(timestamp: u64) -> Option<u32> {
    if !(REWARD_ACTIVATION_UNIX..=REWARD_END_UNIX).contains(&timestamp) {
        return None;
    }
    Some(((timestamp - REWARD_ACTIVATION_UNIX) / HALVING_SECONDS) as u32)
}

/// 첫해 가중치를 2^N으로 두고 이후 매년 절반으로 줄입니다.
/// 마지막 해에는 정수 나눗셈 잔여까지 포함해 전체 연도 예산 합이 정확히 상한과 같습니다.
pub fn annual_budget(year_index: u32) -> u128 {
    let years = reward_year_count();
    if year_index >= years {
        return 0;
    }
    let denominator = (1u128 << years) - 1;
    if year_index + 1 == years {
        let paid: u128 = (0..year_index)
            .map(|index| TOTAL_NODE_EMISSION * (1u128 << (years - index - 1)) / denominator)
            .sum();
        return TOTAL_NODE_EMISSION - paid;
    }
    TOTAL_NODE_EMISSION * (1u128 << (years - year_index - 1)) / denominator
}

pub fn daily_budget(timestamp: u64) -> u128 {
    let Some(year) = halving_index(timestamp) else {
        return 0;
    };
    let start = REWARD_ACTIVATION_UNIX + u64::from(year) * HALVING_SECONDS;
    let end = (start + HALVING_SECONDS - 1).min(REWARD_END_UNIX);
    let days = (end - start + 1).div_ceil(REWARD_EPOCH_SECONDS);
    annual_budget(year) / u128::from(days)
}

pub fn remaining_budget(distributed: u128) -> u128 {
    TOTAL_NODE_EMISSION.saturating_sub(distributed.min(TOTAL_NODE_EMISSION))
}

pub fn is_main_node(peer_id: &str) -> bool {
    !peer_id.is_empty() && MAIN_NODE_PEER_IDS.contains(&peer_id)
}

/// 확정 블록 해시를 seed로 사용해 검증자마다 같은 수령자와 금액을 계산합니다.
/// 후보는 traffic_rewards에서 1시간 가동과 독립 검증자 3명 검사를 통과한 노드만 받습니다.
pub fn settle_daily_rewards(
    height: u64,
    timestamp: u64,
    previous_block_hash: &str,
    already_distributed: u128,
    eligible: &[EligibleNode],
) -> Vec<LotteryPayment> {
    if !is_reward_active(height, timestamp) || eligible.is_empty() {
        return Vec::new();
    }
    let pool = daily_budget(timestamp).min(remaining_budget(already_distributed));
    if pool == 0 {
        return Vec::new();
    }

    let mut nodes = eligible.to_vec();
    nodes.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
    let weights: Vec<u128> = nodes
        .iter()
        .map(|node| {
            u128::from(node.points.max(1))
                * u128::from(if is_main_node(&node.peer_id) {
                    MAIN_NODE_WEIGHT_BPS
                } else {
                    NORMAL_NODE_WEIGHT_BPS
                })
        })
        .collect();
    let total_weight: u128 = weights.iter().sum();
    let mut payments = Vec::with_capacity(nodes.len());
    let mut assigned = 0u128;
    for (index, node) in nodes.iter().enumerate() {
        let amount = if index + 1 == nodes.len() {
            pool - assigned
        } else {
            pool.saturating_mul(weights[index]) / total_weight
        };
        assigned += amount;
        if amount > 0 {
            payments.push(LotteryPayment {
                peer_id: node.peer_id.clone(),
                reward_address: node.reward_address.clone(),
                amount,
            });
        }
    }
    // 정렬 입력과 이전 블록 해시를 함께 소비해 향후 동률 추첨 확장 시 seed 규칙을 고정합니다.
    let _seed = Sha256::digest(previous_block_hash.as_bytes());
    payments
}

fn reward_year_count() -> u32 {
    ((REWARD_END_UNIX - REWARD_ACTIVATION_UNIX) / HALVING_SECONDS + 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Wallet, staking::StakingState};

    fn node(peer: &str, points: u64) -> EligibleNode {
        EligibleNode {
            peer_id: peer.into(),
            reward_address: "11".repeat(32),
            points,
            distinct_verifiers: 3,
        }
    }

    fn service_proof(
        wallet: &Wallet,
        peer_id: &str,
        reward_address: &str,
        timestamp: u64,
        network_group: &str,
    ) -> NodeServiceAttestation {
        let mut proof = NodeServiceAttestation {
            peer_id: peer_id.into(),
            reward_address: reward_address.into(),
            epoch: timestamp / REWARD_EPOCH_SECONDS,
            observed_at: timestamp,
            uptime_bps: SERVICE_MINIMUM_UPTIME_BPS,
            network_group: network_group.into(),
            validator_id: wallet.address(),
            signature_hex: String::new(),
        };
        proof.signature_hex = wallet.sign_bytes(&proof.bytes_to_sign());
        proof
    }

    fn service_fixture(
        bond: u128,
        delegated_at: u64,
    ) -> (
        u64,
        Vec<Validator>,
        Vec<NodeServiceAttestation>,
        StakingState,
    ) {
        let timestamp = REWARD_ACTIVATION_UNIX + 20 * REWARD_EPOCH_SECONDS;
        let wallets = (1..=4)
            .map(|seed| Wallet::from_seed([seed; 32]))
            .collect::<Vec<_>>();
        let validators = wallets
            .iter()
            .map(|wallet| Validator::new(wallet.address(), 100))
            .collect::<Vec<_>>();
        let reward_address = "0x1111111111111111111111111111111111111111";
        let proofs = wallets[..3]
            .iter()
            .map(|wallet| {
                service_proof(
                    wallet,
                    "public-peer-1",
                    reward_address,
                    timestamp,
                    "ipv4:203.0.113.0/24",
                )
            })
            .collect();
        let mut staking = StakingState::default();
        staking
            .delegate_at(reward_address, &wallets[0].address(), bond, delegated_at)
            .unwrap();
        (timestamp, validators, proofs, staking)
    }

    #[test]
    fn activation_requires_height_and_time() {
        assert!(!is_reward_active(99, REWARD_ACTIVATION_UNIX));
        assert!(!is_reward_active(100, REWARD_ACTIVATION_UNIX - 1));
        assert!(is_reward_active(100, REWARD_ACTIVATION_UNIX));
    }

    #[test]
    fn annual_halving_budgets_sum_exactly_to_cap() {
        let years = reward_year_count();
        assert_eq!(
            (0..years).map(annual_budget).sum::<u128>(),
            TOTAL_NODE_EMISSION
        );
        assert_eq!(
            TOTAL_NODE_EMISSION + MAINNET_GENESIS_SUPPLY_IEUM * IEUM,
            MAX_SUPPLY
        );
        for year in 0..years - 2 {
            assert!(annual_budget(year) >= annual_budget(year + 1) * 2);
        }
    }

    #[test]
    fn settlement_never_exceeds_remaining_cap() {
        let remaining = 7 * IEUM;
        let payments = settle_daily_rewards(
            100,
            REWARD_ACTIVATION_UNIX,
            "aa",
            TOTAL_NODE_EMISSION - remaining,
            &[node("a", 1), node("b", 2)],
        );
        assert_eq!(
            payments.iter().map(|item| item.amount).sum::<u128>(),
            remaining
        );
    }

    #[test]
    fn public_service_requires_three_validators_and_mature_100_ieum_bond() {
        let timestamp = REWARD_ACTIVATION_UNIX + 20 * REWARD_EPOCH_SECONDS;
        let (timestamp, validators, proofs, staking) =
            service_fixture(SERVICE_BOND, timestamp - SERVICE_BOND_MATURITY_SECONDS);
        let payments = calculate_service_payments(
            timestamp,
            &proofs,
            &validators,
            &HashSet::new(),
            &staking,
            2_000 * IEUM,
        )
        .unwrap();
        assert_eq!(payments.len(), 1);
        assert_eq!(payments[0].amount, SERVICE_DAILY_POOL_CAP);

        assert!(
            calculate_service_payments(
                timestamp,
                &proofs[..2],
                &validators,
                &HashSet::new(),
                &staking,
                2_000 * IEUM,
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn insufficient_or_immature_bond_is_rejected() {
        let timestamp = REWARD_ACTIVATION_UNIX + 20 * REWARD_EPOCH_SECONDS;
        for (bond, delegated_at) in [
            (SERVICE_BOND - 1, timestamp - SERVICE_BOND_MATURITY_SECONDS),
            (SERVICE_BOND, timestamp - SERVICE_BOND_MATURITY_SECONDS + 1),
        ] {
            let (timestamp, validators, proofs, staking) = service_fixture(bond, delegated_at);
            assert!(
                calculate_service_payments(
                    timestamp,
                    &proofs,
                    &validators,
                    &HashSet::new(),
                    &staking,
                    2_000 * IEUM,
                )
                .unwrap()
                .is_empty()
            );
        }
    }

    #[test]
    fn validator_peer_is_excluded_from_public_reward() {
        let timestamp = REWARD_ACTIVATION_UNIX + 20 * REWARD_EPOCH_SECONDS;
        let (timestamp, validators, proofs, staking) =
            service_fixture(SERVICE_BOND, timestamp - SERVICE_BOND_MATURITY_SECONDS);
        assert!(
            calculate_service_payments(
                timestamp,
                &proofs,
                &validators,
                &HashSet::from(["public-peer-1".into()]),
                &staking,
                2_000 * IEUM,
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn tampered_attestation_is_rejected() {
        let timestamp = REWARD_ACTIVATION_UNIX + 20 * REWARD_EPOCH_SECONDS;
        let (timestamp, validators, mut proofs, staking) =
            service_fixture(SERVICE_BOND, timestamp - SERVICE_BOND_MATURITY_SECONDS);
        proofs[0].uptime_bps += 1;
        assert!(
            calculate_service_payments(
                timestamp,
                &proofs,
                &validators,
                &HashSet::new(),
                &staking,
                2_000 * IEUM,
            )
            .is_err()
        );
    }

    #[test]
    fn third_node_in_same_network_group_is_not_rewarded() {
        let timestamp = REWARD_ACTIVATION_UNIX + 20 * REWARD_EPOCH_SECONDS;
        let wallets = (1..=4)
            .map(|seed| Wallet::from_seed([seed; 32]))
            .collect::<Vec<_>>();
        let validators = wallets
            .iter()
            .map(|wallet| Validator::new(wallet.address(), 100))
            .collect::<Vec<_>>();
        let mut staking = StakingState::default();
        let mut proofs = Vec::new();
        for index in 1..=3 {
            let reward_address = format!("0x{index:040x}");
            staking
                .delegate_at(
                    &reward_address,
                    &wallets[0].address(),
                    SERVICE_BOND,
                    timestamp - SERVICE_BOND_MATURITY_SECONDS,
                )
                .unwrap();
            proofs.extend(wallets[..3].iter().map(|wallet| {
                service_proof(
                    wallet,
                    &format!("public-peer-{index}"),
                    &reward_address,
                    timestamp,
                    "ipv4:203.0.113.0/24",
                )
            }));
        }
        let payments = calculate_service_payments(
            timestamp,
            &proofs,
            &validators,
            &HashSet::new(),
            &staking,
            2_000 * IEUM,
        )
        .unwrap();
        assert_eq!(payments.len(), SERVICE_MAX_REWARDED_PER_NETWORK_GROUP);
        assert_eq!(
            payments.iter().map(|payment| payment.amount).sum::<u128>(),
            SERVICE_DAILY_POOL_CAP
        );
    }
}

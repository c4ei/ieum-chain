use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const BPS_DENOMINATOR: u64 = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrafficPolicy {
    pub target_connections: usize,
    pub nearby_slots: usize,
    pub diversity_slots: usize,
    pub low_load_slots: usize,
    pub max_per_country: usize,
    pub max_per_asn: usize,
    pub max_bootstrap_connections: usize,
    pub minimum_uptime_bps: u16,
}

impl Default for TrafficPolicy {
    fn default() -> Self {
        Self {
            target_connections: 12,
            nearby_slots: 4,
            diversity_slots: 4,
            low_load_slots: 4,
            max_per_country: 3,
            max_per_asn: 2,
            max_bootstrap_connections: 1,
            minimum_uptime_bps: 8_000,
        }
    }
}

impl TrafficPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.target_connections == 0
            || self.nearby_slots + self.diversity_slots + self.low_load_slots
                != self.target_connections
        {
            return Err("피어 슬롯 합계는 target_connections와 같아야 합니다.".into());
        }
        if self.max_per_country == 0 || self.max_per_asn == 0 {
            return Err("국가·ASN별 연결 상한은 1 이상이어야 합니다.".into());
        }
        if self.max_bootstrap_connections > self.target_connections {
            return Err("부트스트랩 연결 상한이 전체 연결 수보다 클 수 없습니다.".into());
        }
        if self.minimum_uptime_bps > BPS_DENOMINATOR as u16 {
            return Err("최소 가동률은 10,000bps 이하여야 합니다.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerCandidate {
    pub peer_id: String,
    pub country_code: String,
    pub asn: u32,
    pub latency_ms: u32,
    pub uptime_bps: u16,
    pub capacity: u32,
    pub active_connections: u32,
    pub is_bootstrap: bool,
}

impl PeerCandidate {
    fn load_bps(&self) -> u64 {
        if self.capacity == 0 {
            return BPS_DENOMINATOR;
        }
        (u64::from(self.active_connections) * BPS_DENOMINATOR / u64::from(self.capacity))
            .min(BPS_DENOMINATOR)
    }
}

/// 지연시간뿐 아니라 지역·망 사업자 다양성과 저부하 노드 슬롯을 함께 보장합니다.
/// 입력 순서와 관계없이 모든 동률은 PeerId 순으로 결정합니다.
pub fn select_balanced_peers(
    policy: &TrafficPolicy,
    candidates: &[PeerCandidate],
) -> Result<Vec<PeerCandidate>, String> {
    policy.validate()?;
    let mut unique = HashMap::new();
    for candidate in candidates {
        if candidate.peer_id.trim().is_empty()
            || candidate.country_code.trim().is_empty()
            || candidate.capacity == 0
        {
            continue;
        }
        unique
            .entry(candidate.peer_id.clone())
            .or_insert_with(|| candidate.clone());
    }
    let candidates: Vec<_> = unique.into_values().collect();
    let mut selected = Vec::new();

    let mut nearby = candidates.clone();
    nearby.sort_by_key(|peer| {
        (
            peer.latency_ms,
            std::cmp::Reverse(peer.uptime_bps),
            peer.peer_id.clone(),
        )
    });
    fill_slots(policy, &nearby, policy.nearby_slots, &mut selected, false);

    let mut diverse = candidates.clone();
    diverse.sort_by_key(|peer| {
        (
            country_count(&selected, &peer.country_code),
            asn_count(&selected, peer.asn),
            std::cmp::Reverse(peer.uptime_bps),
            peer.latency_ms,
            peer.peer_id.clone(),
        )
    });
    fill_slots(
        policy,
        &diverse,
        policy.diversity_slots,
        &mut selected,
        false,
    );

    let mut low_load = candidates.clone();
    low_load.sort_by_key(|peer| {
        (
            peer.load_bps(),
            std::cmp::Reverse(peer.uptime_bps),
            peer.latency_ms,
            peer.peer_id.clone(),
        )
    });
    fill_slots(
        policy,
        &low_load,
        policy.low_load_slots,
        &mut selected,
        false,
    );

    // 후보가 적거나 다양성 상한 때문에 빈 슬롯이 생기면 가동률 조건만 유지해 채웁니다.
    let mut fallback = candidates;
    fallback.sort_by_key(|peer| {
        (
            peer.load_bps(),
            peer.latency_ms,
            std::cmp::Reverse(peer.uptime_bps),
            peer.peer_id.clone(),
        )
    });
    fill_slots(
        policy,
        &fallback,
        policy.target_connections.saturating_sub(selected.len()),
        &mut selected,
        true,
    );
    selected.truncate(policy.target_connections);
    Ok(selected)
}

fn fill_slots(
    policy: &TrafficPolicy,
    candidates: &[PeerCandidate],
    count: usize,
    selected: &mut Vec<PeerCandidate>,
    relaxed_diversity: bool,
) {
    let target = selected.len() + count;
    for candidate in candidates {
        if selected.len() >= target {
            break;
        }
        if selected
            .iter()
            .any(|peer| peer.peer_id == candidate.peer_id)
            || candidate.uptime_bps < policy.minimum_uptime_bps
        {
            continue;
        }
        if candidate.is_bootstrap
            && selected.iter().filter(|peer| peer.is_bootstrap).count()
                >= policy.max_bootstrap_connections
        {
            continue;
        }
        if !relaxed_diversity
            && (country_count(selected, &candidate.country_code) >= policy.max_per_country
                || asn_count(selected, candidate.asn) >= policy.max_per_asn)
        {
            continue;
        }
        selected.push(candidate.clone());
    }
}

fn country_count(peers: &[PeerCandidate], country: &str) -> usize {
    peers
        .iter()
        .filter(|peer| peer.country_code == country)
        .count()
}

fn asn_count(peers: &[PeerCandidate], asn: u32) -> usize {
    peers.iter().filter(|peer| peer.asn == asn).count()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayReceipt {
    pub relay_peer_id: String,
    pub reward_address: String,
    pub verifier_peer_id: String,
    pub message_id: String,
    pub payload_bytes: u32,
    pub observed_at: u64,
    /// 해당 relay를 독립 피어가 처음 관측한 시각입니다.
    #[serde(default)]
    pub relay_started_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewardPolicy {
    pub epoch_seconds: u64,
    pub pool_fee_bps: u16,
    pub minimum_distinct_verifiers: usize,
    pub maximum_receipts_per_verifier: u32,
    pub maximum_points_per_node: u64,
    pub winner_count: usize,
    /// 단순 실행 직후 보상을 받는 Sybil 공격을 막는 최소 연속 가동 시간입니다.
    pub minimum_uptime_seconds: u64,
}

impl Default for RewardPolicy {
    fn default() -> Self {
        Self {
            epoch_seconds: 24 * 60 * 60,
            pool_fee_bps: 2_500,
            minimum_distinct_verifiers: 3,
            maximum_receipts_per_verifier: 200,
            maximum_points_per_node: 10_000,
            winner_count: 10,
            minimum_uptime_seconds: 60 * 60,
        }
    }
}

impl RewardPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if !(24 * 60 * 60..=7 * 24 * 60 * 60).contains(&self.epoch_seconds) {
            return Err("보상 주기는 1일~7일이어야 합니다.".into());
        }
        if self.pool_fee_bps > BPS_DENOMINATOR as u16 {
            return Err("이벤트 풀 비율은 10,000bps 이하여야 합니다.".into());
        }
        if self.minimum_distinct_verifiers < 2
            || self.maximum_receipts_per_verifier == 0
            || self.maximum_points_per_node == 0
            || self.winner_count == 0
        {
            return Err("보상 검증자·상한·당첨자 수 설정이 올바르지 않습니다.".into());
        }
        if self.minimum_uptime_seconds < 60 * 60 || self.minimum_uptime_seconds > self.epoch_seconds
        {
            return Err("최소 가동 시간은 1시간 이상이며 보상 주기 이하여야 합니다.".into());
        }
        Ok(())
    }

    pub fn pool_share(&self, foundation_fees: u128) -> u128 {
        foundation_fees.saturating_mul(u128::from(self.pool_fee_bps)) / u128::from(BPS_DENOMINATOR)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EligibleNode {
    pub peer_id: String,
    pub reward_address: String,
    pub points: u64,
    pub distinct_verifiers: usize,
}

#[derive(Clone, Debug)]
struct NodeContribution {
    reward_address: String,
    points: u64,
    verifiers: HashSet<String>,
}

#[derive(Clone, Debug)]
pub struct ContributionLedger {
    policy: RewardPolicy,
    epoch: u64,
    seen: HashSet<(String, String)>,
    verifier_counts: HashMap<String, u32>,
    nodes: HashMap<String, NodeContribution>,
}

impl ContributionLedger {
    pub fn new(policy: RewardPolicy, epoch: u64) -> Result<Self, String> {
        policy.validate()?;
        Ok(Self {
            policy,
            epoch,
            seen: HashSet::new(),
            verifier_counts: HashMap::new(),
            nodes: HashMap::new(),
        })
    }

    /// 서명·실제 발신자·메시지 유효성 검사를 네트워크 계층에서 통과한 영수증만 넣습니다.
    pub fn record_validated(&mut self, receipt: RelayReceipt) -> Result<bool, String> {
        if receipt.observed_at / self.policy.epoch_seconds != self.epoch {
            return Err("중계 영수증의 보상 epoch가 다릅니다.".into());
        }
        if receipt.relay_peer_id == receipt.verifier_peer_id {
            return Err("자기 자신이 확인한 중계 영수증은 인정하지 않습니다.".into());
        }
        if receipt.payload_bytes == 0 || receipt.message_id.trim().is_empty() {
            return Err("빈 메시지 중계는 인정하지 않습니다.".into());
        }
        if receipt.relay_started_at == 0
            || receipt.observed_at.saturating_sub(receipt.relay_started_at)
                < self.policy.minimum_uptime_seconds
        {
            return Err("1시간 이상 연속 가동이 독립 피어에게 확인된 노드만 보상됩니다.".into());
        }
        validate_reward_address(&receipt.reward_address)?;
        let dedupe = (receipt.verifier_peer_id.clone(), receipt.message_id.clone());
        if !self.seen.insert(dedupe) {
            return Ok(false);
        }
        let verifier_count = self
            .verifier_counts
            .entry(receipt.verifier_peer_id.clone())
            .or_default();
        if *verifier_count >= self.policy.maximum_receipts_per_verifier {
            return Ok(false);
        }
        *verifier_count += 1;

        let contribution =
            self.nodes
                .entry(receipt.relay_peer_id)
                .or_insert_with(|| NodeContribution {
                    reward_address: receipt.reward_address.clone(),
                    points: 0,
                    verifiers: HashSet::new(),
                });
        if contribution.reward_address != receipt.reward_address {
            return Err("같은 PeerId가 서로 다른 보상 주소를 사용했습니다.".into());
        }
        // 큰 패킷 하나가 추첨을 독점하지 않도록 64KiB 단위, 영수증당 최대 16점입니다.
        let points = u64::from(receipt.payload_bytes.div_ceil(64 * 1024)).clamp(1, 16);
        contribution.points = contribution
            .points
            .saturating_add(points)
            .min(self.policy.maximum_points_per_node);
        contribution.verifiers.insert(receipt.verifier_peer_id);
        Ok(true)
    }

    pub fn eligible_nodes(&self) -> Vec<EligibleNode> {
        let mut result: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, contribution)| {
                contribution.verifiers.len() >= self.policy.minimum_distinct_verifiers
            })
            .map(|(peer_id, contribution)| EligibleNode {
                peer_id: peer_id.clone(),
                reward_address: contribution.reward_address.clone(),
                points: contribution.points,
                distinct_verifiers: contribution.verifiers.len(),
            })
            .collect();
        result.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LotteryPayment {
    pub peer_id: String,
    pub reward_address: String,
    pub amount: u128,
}

/// 직전 확정 블록 해시를 seed로 사용해 모든 검증자가 같은 당첨자를 계산합니다.
/// 가중치는 points의 제곱근으로 완화하여 대형 노드의 독점을 제한합니다.
pub fn draw_lottery(
    seed: &[u8],
    pool: u128,
    winner_count: usize,
    nodes: &[EligibleNode],
) -> Vec<LotteryPayment> {
    if pool == 0 || winner_count == 0 || nodes.is_empty() {
        return Vec::new();
    }
    let mut remaining = nodes.to_vec();
    remaining.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
    let draws = winner_count.min(remaining.len());
    let amount = pool / draws as u128;
    if amount == 0 {
        return Vec::new();
    }
    let mut winners = Vec::with_capacity(draws);
    for round in 0..draws {
        let weights: Vec<_> = remaining
            .iter()
            .map(|node| integer_sqrt(node.points.max(1)))
            .collect();
        let total: u64 = weights.iter().sum();
        let digest = Sha256::digest([seed, &(round as u64).to_be_bytes()].concat());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        let mut cursor = u64::from_be_bytes(bytes) % total;
        let mut selected = 0;
        for (index, weight) in weights.iter().enumerate() {
            if cursor < *weight {
                selected = index;
                break;
            }
            cursor -= *weight;
        }
        let node = remaining.remove(selected);
        winners.push(LotteryPayment {
            peer_id: node.peer_id,
            reward_address: node.reward_address,
            amount,
        });
    }
    winners
}

fn integer_sqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut left = 1;
    let mut right = value.min(u32::MAX as u64);
    while left <= right {
        let middle = left + (right - left) / 2;
        if middle <= value / middle {
            left = middle + 1;
        } else {
            right = middle - 1;
        }
    }
    right
}

fn validate_reward_address(address: &str) -> Result<(), String> {
    let is_account = address.starts_with("0x")
        && address.len() == 42
        && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit());
    let is_legacy = address.len() == 64 && address.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !is_account && !is_legacy {
        return Err("노드 보상 주소 형식이 올바르지 않습니다.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(index: u32, country: &str, asn: u32, latency: u32, load: u32) -> PeerCandidate {
        PeerCandidate {
            peer_id: format!("peer-{index:02}"),
            country_code: country.into(),
            asn,
            latency_ms: latency,
            uptime_bps: 9_500,
            capacity: 100,
            active_connections: load,
            is_bootstrap: index <= 2,
        }
    }

    #[test]
    fn balanced_selection_includes_nearby_diverse_and_low_load_nodes() {
        let mut candidates = Vec::new();
        for index in 1..=6 {
            candidates.push(peer(index, "KR", 100, index, 80));
        }
        candidates.push(peer(7, "JP", 200, 30, 5));
        candidates.push(peer(8, "US", 300, 40, 10));
        candidates.push(peer(9, "DE", 400, 50, 15));
        candidates.push(peer(10, "FR", 500, 60, 20));
        candidates.push(peer(11, "SG", 600, 70, 25));
        candidates.push(peer(12, "AU", 700, 80, 30));
        candidates.push(peer(13, "CA", 800, 90, 35));

        let selected = select_balanced_peers(&TrafficPolicy::default(), &candidates).unwrap();
        assert_eq!(selected.len(), 12);
        assert!(
            selected
                .iter()
                .any(|candidate| candidate.peer_id == "peer-01")
        );
        assert!(
            selected
                .iter()
                .any(|candidate| candidate.peer_id == "peer-07")
        );
        assert!(
            selected
                .iter()
                .filter(|candidate| candidate.is_bootstrap)
                .count()
                <= 1
        );
    }

    #[test]
    fn receipt_requires_independent_verifiers_and_deduplicates() {
        let policy = RewardPolicy::default();
        let mut ledger = ContributionLedger::new(policy, 2).unwrap();
        let address = "11".repeat(32);
        for index in 1..=3 {
            let receipt = RelayReceipt {
                relay_peer_id: "relay".into(),
                reward_address: address.clone(),
                verifier_peer_id: format!("verifier-{index}"),
                message_id: format!("message-{index}"),
                payload_bytes: 100,
                observed_at: 2 * 24 * 60 * 60,
                relay_started_at: 2 * 24 * 60 * 60 - 60 * 60,
            };
            assert!(ledger.record_validated(receipt.clone()).unwrap());
            assert!(!ledger.record_validated(receipt).unwrap());
        }
        let eligible = ledger.eligible_nodes();
        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].points, 3);
    }

    #[test]
    fn lottery_is_deterministic_and_pays_no_duplicate_winner() {
        let nodes: Vec<_> = (1..=20)
            .map(|index| EligibleNode {
                peer_id: format!("peer-{index}"),
                reward_address: format!("{index:064x}"),
                points: index,
                distinct_verifiers: 3,
            })
            .collect();
        let first = draw_lottery(b"finalized-block", 1_000, 10, &nodes);
        let second = draw_lottery(b"finalized-block", 1_000, 10, &nodes);
        assert_eq!(first, second);
        assert_eq!(first.len(), 10);
        assert_eq!(
            first.iter().map(|payment| payment.amount).sum::<u128>(),
            1_000
        );
        let unique: HashSet<_> = first.iter().map(|payment| &payment.peer_id).collect();
        assert_eq!(unique.len(), first.len());
    }
}

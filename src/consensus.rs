use crate::model::Block;
use crate::wallet::{Wallet, verify_signature};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 검증자와 투표권입니다. 현재 예제에서는 stake를 투표 가중치로 사용합니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Validator {
    pub id: String,
    pub voting_power: u64,
}

impl Validator {
    pub fn new(id: impl Into<String>, voting_power: u64) -> Self {
        Self {
            id: id.into(),
            voting_power,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusPhase {
    Waiting,
    Propose,
    Prevote,
    Precommit,
    Finalized,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum VoteType {
    Prevote,
    Precommit,
}

/// 제안자가 서명한 후보 블록입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedProposal {
    pub height: u64,
    pub round: u32,
    pub proposer_id: String,
    /// 제안자가 알고 있는 2/3 초과 prevote 라운드입니다.
    #[serde(default)]
    pub valid_round: Option<u32>,
    /// `valid_round`에서 같은 블록에 서명한 2/3 초과 prevote 증명입니다.
    #[serde(default)]
    pub valid_round_prevotes: Vec<ConsensusMessage>,
    pub block: Block,
    pub signature: String,
}

impl SignedProposal {
    pub fn new(height: u64, round: u32, proposer: &Wallet, block: Block) -> Self {
        Self::with_valid_round_certificate(height, round, proposer, block, None, Vec::new())
    }

    pub fn with_valid_round(
        height: u64,
        round: u32,
        proposer: &Wallet,
        block: Block,
        valid_round: Option<u32>,
    ) -> Self {
        Self::with_valid_round_certificate(height, round, proposer, block, valid_round, Vec::new())
    }

    pub fn with_valid_round_certificate(
        height: u64,
        round: u32,
        proposer: &Wallet,
        block: Block,
        valid_round: Option<u32>,
        valid_round_prevotes: Vec<ConsensusMessage>,
    ) -> Self {
        let proposer_id = proposer.address();
        let signature = proposer.sign_bytes(&Self::unsigned_bytes(
            height,
            round,
            &proposer_id,
            &block.hash,
            valid_round,
            &valid_round_prevotes,
        ));
        Self {
            height,
            round,
            proposer_id,
            valid_round,
            valid_round_prevotes,
            block,
            signature,
        }
    }

    pub fn bytes_to_sign(
        height: u64,
        round: u32,
        proposer_id: &str,
        block_hash: &str,
        valid_round: Option<u32>,
        valid_round_prevotes: &[ConsensusMessage],
    ) -> Vec<u8> {
        Self::unsigned_bytes(
            height,
            round,
            proposer_id,
            block_hash,
            valid_round,
            valid_round_prevotes,
        )
    }

    pub fn from_signature(
        height: u64,
        round: u32,
        proposer_id: String,
        block: Block,
        valid_round: Option<u32>,
        valid_round_prevotes: Vec<ConsensusMessage>,
        signature: String,
    ) -> Result<Self, String> {
        let proposal = Self {
            height,
            round,
            proposer_id,
            valid_round,
            valid_round_prevotes,
            block,
            signature,
        };
        proposal.verify()?;
        Ok(proposal)
    }

    fn unsigned_bytes(
        height: u64,
        round: u32,
        proposer_id: &str,
        block_hash: &str,
        valid_round: Option<u32>,
        valid_round_prevotes: &[ConsensusMessage],
    ) -> Vec<u8> {
        let mut bytes = b"IEUM-PROPOSAL-V3".to_vec();
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&round.to_be_bytes());
        push_text(&mut bytes, proposer_id);
        push_text(&mut bytes, block_hash);
        bytes.extend_from_slice(&valid_round.unwrap_or(u32::MAX).to_be_bytes());
        let mut prevotes = valid_round_prevotes.to_vec();
        prevotes.sort_by(|left, right| left.validator_id.cmp(&right.validator_id));
        bytes.extend_from_slice(&(prevotes.len() as u64).to_be_bytes());
        for vote in prevotes {
            push_text(&mut bytes, &vote.validator_id);
            push_text(&mut bytes, &vote.signature);
        }
        bytes
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.height != self.block.height {
            return Err("제안 높이와 블록 높이가 다릅니다.".into());
        }
        if self.block.hash != self.block.calculate_hash() {
            return Err("제안 블록 해시가 올바르지 않습니다.".into());
        }
        verify_signature(
            &self.proposer_id,
            &Self::unsigned_bytes(
                self.height,
                self.round,
                &self.proposer_id,
                &self.block.hash,
                self.valid_round,
                &self.valid_round_prevotes,
            ),
            &self.signature,
        )
    }
}

/// 네트워크로 전달되는 BFT 합의 메시지입니다.
/// 운영 버전에서는 validator_id가 아니라 검증자 키의 서명을 반드시 검증해야 합니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsensusMessage {
    pub height: u64,
    pub round: u32,
    pub validator_id: String,
    pub vote_type: VoteType,
    pub block_hash: String,
    /// validator_id에 해당하는 Ed25519 개인키로 만든 서명입니다.
    pub signature: String,
}

/// 동일 검증자가 같은 높이·라운드·단계에서 서로 다른 값에 서명한 증거입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoubleVoteEvidence {
    pub first: ConsensusMessage,
    pub second: ConsensusMessage,
}

impl DoubleVoteEvidence {
    pub fn new(first: ConsensusMessage, second: ConsensusMessage) -> Result<Self, String> {
        let evidence = Self { first, second };
        evidence.verify()?;
        Ok(evidence)
    }

    pub fn verify(&self) -> Result<(), String> {
        self.first.verify()?;
        self.second.verify()?;
        if self.first.height != self.second.height
            || self.first.round != self.second.round
            || self.first.vote_type != self.second.vote_type
            || self.first.validator_id != self.second.validator_id
            || self.first.block_hash == self.second.block_hash
        {
            return Err("유효한 이중투표 증거가 아닙니다.".into());
        }
        Ok(())
    }

    pub fn id(&self) -> String {
        format!(
            "{}:{}:{}:{:?}",
            self.first.validator_id, self.first.height, self.first.round, self.first.vote_type
        )
    }
}

/// 확정 블록과 그 블록을 확정한 precommit 증명입니다.
///
/// 새 노드는 단순히 가장 긴 체인을 믿지 않고 등록 검증자 투표권의 2/3 초과
/// 서명을 직접 확인한 뒤에만 블록을 적용합니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FinalityCertificate {
    pub block: Block,
    pub round: u32,
    pub precommits: Vec<ConsensusMessage>,
}

impl FinalityCertificate {
    pub fn verify(&self, validators: &[Validator]) -> Result<(), String> {
        if self.block.hash != self.block.calculate_hash() {
            return Err("확정 인증서의 블록 해시가 올바르지 않습니다.".into());
        }
        let total_power: u128 = validators.iter().map(|v| v.voting_power as u128).sum();
        if total_power == 0 {
            return Err("검증자 총 투표권이 0입니다.".into());
        }
        let powers: HashMap<_, _> = validators
            .iter()
            .map(|v| (v.id.as_str(), v.voting_power as u128))
            .collect();
        let mut voters = HashSet::new();
        let mut signed_power = 0_u128;
        for vote in &self.precommits {
            if vote.height != self.block.height
                || vote.round != self.round
                || vote.vote_type != VoteType::Precommit
                || vote.block_hash != self.block.hash
            {
                return Err("확정 인증서에 다른 높이·라운드·블록 투표가 섞였습니다.".into());
            }
            vote.verify()?;
            let power = powers
                .get(vote.validator_id.as_str())
                .ok_or("등록되지 않은 검증자의 precommit입니다.")?;
            if voters.insert(vote.validator_id.as_str()) {
                signed_power = signed_power
                    .checked_add(*power)
                    .ok_or("확정 투표권 합계가 범위를 넘었습니다.")?;
            }
        }
        if signed_power * 3 <= total_power * 2 {
            return Err("확정에 필요한 2/3 초과 precommit이 없습니다.".into());
        }
        Ok(())
    }
}

impl ConsensusMessage {
    fn unsigned_bytes(
        height: u64,
        round: u32,
        validator_id: &str,
        vote_type: VoteType,
        block_hash: &str,
    ) -> Vec<u8> {
        // 체인 간 재사용 공격을 막기 위한 도메인 구분 문자열입니다.
        let mut bytes = b"IEUM-CONSENSUS-V1".to_vec();
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&round.to_be_bytes());
        bytes.push(match vote_type {
            VoteType::Prevote => 1,
            VoteType::Precommit => 2,
        });
        push_text(&mut bytes, validator_id);
        push_text(&mut bytes, block_hash);
        bytes
    }

    /// 검증자 개인키로 서명된 prevote를 생성합니다.
    pub fn prevote(
        height: u64,
        round: u32,
        validator: &Wallet,
        block_hash: impl Into<String>,
    ) -> Self {
        Self::signed(
            height,
            round,
            validator,
            VoteType::Prevote,
            block_hash.into(),
        )
    }

    /// 검증자 개인키로 서명된 precommit을 생성합니다.
    pub fn precommit(
        height: u64,
        round: u32,
        validator: &Wallet,
        block_hash: impl Into<String>,
    ) -> Self {
        Self::signed(
            height,
            round,
            validator,
            VoteType::Precommit,
            block_hash.into(),
        )
    }

    pub fn bytes_to_sign(
        height: u64,
        round: u32,
        validator_id: &str,
        vote_type: VoteType,
        block_hash: &str,
    ) -> Vec<u8> {
        Self::unsigned_bytes(height, round, validator_id, vote_type, block_hash)
    }

    pub fn from_signature(
        height: u64,
        round: u32,
        validator_id: String,
        vote_type: VoteType,
        block_hash: String,
        signature: String,
    ) -> Result<Self, String> {
        let message = Self {
            height,
            round,
            validator_id,
            vote_type,
            block_hash,
            signature,
        };
        message.verify()?;
        Ok(message)
    }

    fn signed(
        height: u64,
        round: u32,
        validator: &Wallet,
        vote_type: VoteType,
        block_hash: String,
    ) -> Self {
        let validator_id = validator.address();
        let signature = validator.sign_bytes(&Self::unsigned_bytes(
            height,
            round,
            &validator_id,
            vote_type,
            &block_hash,
        ));
        Self {
            height,
            round,
            validator_id,
            vote_type,
            block_hash,
            signature,
        }
    }

    /// 네트워크에서 받은 투표가 실제 등록 검증자의 서명인지 확인합니다.
    pub fn verify(&self) -> Result<(), String> {
        verify_signature(
            &self.validator_id,
            &Self::unsigned_bytes(
                self.height,
                self.round,
                &self.validator_id,
                self.vote_type,
                &self.block_hash,
            ),
            &self.signature,
        )
    }
}

fn push_text(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

/// Tendermint 계열의 propose → prevote → precommit 흐름을 작게 구현한 상태기계입니다.
///
/// 이 코드는 네트워크 학습 및 테스트넷용입니다. 투표 서명, locked/valid value,
/// 라운드 변경과 외부 이중서명 증거를 지원합니다. 자동 slashing과 nil vote는
/// 다음 단계에서 추가합니다.
#[derive(Clone, Debug)]
pub struct BftConsensus {
    validators: HashMap<String, u64>,
    total_power: u64,
    height: u64,
    round: u32,
    phase: ConsensusPhase,
    proposal: Option<String>,
    votes: HashMap<(VoteType, String), HashSet<String>>,
    /// (높이, 라운드, 투표 종류, 검증자)별 첫 투표를 보관해 이중투표를 거부합니다.
    vote_history: HashMap<(u64, u32, VoteType, String), String>,
    vote_messages: HashMap<(u64, u32, VoteType, String), ConsensusMessage>,
    locked_value: Option<String>,
    locked_round: Option<u32>,
    valid_value: Option<String>,
    valid_round: Option<u32>,
    evidence: Vec<DoubleVoteEvidence>,
    finalized: Option<String>,
}

impl BftConsensus {
    pub fn new(validators: Vec<Validator>) -> Result<Self, String> {
        if validators.is_empty() {
            return Err("BFT 검증자는 최소 1개 필요합니다.".into());
        }
        let mut map = HashMap::new();
        for validator in validators {
            if validator.voting_power == 0 || map.contains_key(&validator.id) {
                return Err("검증자 ID는 고유해야 하고 투표권은 1 이상이어야 합니다.".into());
            }
            map.insert(validator.id, validator.voting_power);
        }
        let total_power = map.values().sum();
        Ok(Self {
            validators: map,
            total_power,
            height: 0,
            round: 0,
            phase: ConsensusPhase::Waiting,
            proposal: None,
            votes: HashMap::new(),
            vote_history: HashMap::new(),
            vote_messages: HashMap::new(),
            locked_value: None,
            locked_round: None,
            valid_value: None,
            valid_round: None,
            evidence: Vec::new(),
            finalized: None,
        })
    }

    pub fn start_round(&mut self, height: u64, round: u32) -> Result<(), String> {
        if height < self.height || (height == self.height && round < self.round) {
            return Err("과거 높이 또는 과거 라운드로 돌아갈 수 없습니다.".into());
        }
        if height > self.height {
            self.locked_value = None;
            self.locked_round = None;
            self.valid_value = None;
            self.valid_round = None;
        }
        self.height = height;
        self.round = round;
        self.phase = ConsensusPhase::Propose;
        self.proposal = None;
        // 이전 라운드의 투표는 quorum 계산에서는 제외하지만, 이중투표 증거용 기록은 유지합니다.
        self.votes.clear();
        self.finalized = None;
        Ok(())
    }

    pub fn expected_proposer(&self) -> &str {
        let mut ids: Vec<_> = self.validators.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids[((self.height.saturating_sub(1) + self.round as u64) as usize) % ids.len()]
    }

    pub fn propose(&mut self, proposer: &str, block_hash: &str) -> Result<(), String> {
        if self.phase != ConsensusPhase::Propose {
            return Err("현재 단계에서는 블록을 제안할 수 없습니다.".into());
        }
        if proposer != self.expected_proposer() {
            return Err(format!(
                "이번 라운드의 제안자는 {}입니다.",
                self.expected_proposer()
            ));
        }
        if block_hash.is_empty() {
            return Err("빈 블록 해시는 제안할 수 없습니다.".into());
        }
        self.proposal = Some(block_hash.to_string());
        self.phase = ConsensusPhase::Prevote;
        Ok(())
    }

    /// 네트워크에서 받은 제안자의 서명과 현재 높이/라운드를 확인합니다.
    pub fn handle_proposal(&mut self, proposal: &SignedProposal) -> Result<(), String> {
        if proposal.height != self.height || proposal.round != self.round {
            return Err("현재 높이/라운드와 다른 제안입니다.".into());
        }
        if !self.validators.contains_key(&proposal.proposer_id) {
            return Err("등록되지 않은 검증자의 제안입니다.".into());
        }
        proposal.verify()?;
        if let Some(valid_round) = proposal.valid_round {
            self.verify_valid_round_certificate(proposal, valid_round)?;
        } else if !proposal.valid_round_prevotes.is_empty() {
            return Err("valid_round 없이 prevote 증명만 제공할 수 없습니다.".into());
        }
        if let Some(locked_value) = self.locked_value.as_deref()
            && locked_value != proposal.block.hash
        {
            let unlock_allowed = proposal.valid_round.is_some_and(|valid_round| {
                self.locked_round
                    .is_some_and(|locked_round| valid_round >= locked_round)
            });
            if !unlock_allowed {
                return Err("잠긴 값과 다른 제안이며 유효한 valid_round 증명이 없습니다.".into());
            }
        }
        if let Some(valid_round) = proposal.valid_round {
            self.valid_value = Some(proposal.block.hash.clone());
            self.valid_round = Some(valid_round);
        }
        self.propose(&proposal.proposer_id, &proposal.block.hash)
    }

    fn verify_valid_round_certificate(
        &self,
        proposal: &SignedProposal,
        valid_round: u32,
    ) -> Result<(), String> {
        if valid_round >= proposal.round {
            return Err("valid_round는 제안 라운드보다 과거여야 합니다.".into());
        }
        if proposal.valid_round_prevotes.len() > self.validators.len() {
            return Err("valid_round prevote 수가 검증자 수를 넘었습니다.".into());
        }
        let mut voters = HashSet::new();
        let mut power = 0_u64;
        for vote in &proposal.valid_round_prevotes {
            if vote.height != proposal.height
                || vote.round != valid_round
                || vote.vote_type != VoteType::Prevote
                || vote.block_hash != proposal.block.hash
            {
                return Err("valid_round prevote 증명의 높이·라운드·블록이 다릅니다.".into());
            }
            vote.verify()?;
            let voting_power = self
                .validators
                .get(&vote.validator_id)
                .ok_or("등록되지 않은 검증자의 valid_round prevote입니다.")?;
            if !voters.insert(vote.validator_id.as_str()) {
                return Err("valid_round prevote에 중복 검증자가 있습니다.".into());
            }
            power = power
                .checked_add(*voting_power)
                .ok_or("valid_round 투표권 합계가 범위를 넘었습니다.")?;
        }
        if power.saturating_mul(3) <= self.total_power.saturating_mul(2) {
            return Err("valid_round에 필요한 2/3 초과 prevote 증명이 없습니다.".into());
        }
        Ok(())
    }

    pub fn handle(&mut self, message: ConsensusMessage) -> Result<(), String> {
        if message.height != self.height || message.round != self.round {
            return Err("현재 높이/라운드와 다른 투표입니다.".into());
        }
        if !self.validators.contains_key(&message.validator_id) {
            return Err("등록되지 않은 검증자의 투표입니다.".into());
        }
        message.verify()?;
        match (self.phase, message.vote_type) {
            (ConsensusPhase::Prevote, VoteType::Prevote)
            | (ConsensusPhase::Precommit, VoteType::Prevote)
            | (ConsensusPhase::Precommit, VoteType::Precommit)
            | (ConsensusPhase::Finalized, VoteType::Prevote)
            | (ConsensusPhase::Finalized, VoteType::Precommit) => {}
            _ => return Err("현재 합의 단계와 투표 종류가 일치하지 않습니다.".into()),
        }

        let history_key = (
            message.height,
            message.round,
            message.vote_type,
            message.validator_id.clone(),
        );
        if let Some(previous_hash) = self.vote_history.get(&history_key) {
            if previous_hash != &message.block_hash {
                if let Some(first) = self.vote_messages.get(&history_key)
                    && let Ok(evidence) = DoubleVoteEvidence::new(first.clone(), message.clone())
                    && !self
                        .evidence
                        .iter()
                        .any(|known| known.id() == evidence.id())
                {
                    self.evidence.push(evidence);
                }
                return Err("동일 높이·라운드에서 서로 다른 블록에 이중투표했습니다.".into());
            }
        } else {
            self.vote_history
                .insert(history_key.clone(), message.block_hash.clone());
            self.vote_messages.insert(history_key, message.clone());
        }
        if self.proposal.as_deref() != Some(message.block_hash.as_str()) {
            return Err("현재 제안과 다른 블록에 대한 투표입니다.".into());
        }

        let key = (message.vote_type, message.block_hash.clone());
        self.votes
            .entry(key.clone())
            .or_default()
            .insert(message.validator_id);
        if self.has_quorum(&key) && self.phase != ConsensusPhase::Finalized {
            match message.vote_type {
                VoteType::Prevote if self.phase == ConsensusPhase::Prevote => {
                    self.valid_value = Some(message.block_hash.clone());
                    self.valid_round = Some(self.round);
                    self.locked_value = Some(message.block_hash.clone());
                    self.locked_round = Some(self.round);
                    self.phase = ConsensusPhase::Precommit;
                }
                VoteType::Precommit => {
                    self.phase = ConsensusPhase::Finalized;
                    self.finalized = Some(message.block_hash);
                }
                VoteType::Prevote => {}
            }
        }
        Ok(())
    }

    fn has_quorum(&self, key: &(VoteType, String)) -> bool {
        let power: u64 = self
            .votes
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|id| self.validators.get(id))
            .sum();
        // 정확히 2/3는 부족하고, Byzantine fault tolerance에는 2/3 초과가 필요합니다.
        power.saturating_mul(3) > self.total_power.saturating_mul(2)
    }

    pub fn phase(&self) -> ConsensusPhase {
        self.phase
    }

    pub fn round(&self) -> u32 {
        self.round
    }

    pub fn height(&self) -> u64 {
        self.height
    }

    pub fn locked_value(&self) -> Option<(&str, u32)> {
        self.locked_value.as_deref().zip(self.locked_round)
    }

    pub fn valid_value(&self) -> Option<(&str, u32)> {
        self.valid_value.as_deref().zip(self.valid_round)
    }

    pub fn prevote_certificate(&self, block_hash: &str, round: u32) -> Vec<ConsensusMessage> {
        let mut votes: Vec<_> = self
            .vote_messages
            .values()
            .filter(|vote| {
                vote.height == self.height
                    && vote.round == round
                    && vote.vote_type == VoteType::Prevote
                    && vote.block_hash == block_hash
            })
            .cloned()
            .collect();
        votes.sort_by(|left, right| left.validator_id.cmp(&right.validator_id));
        votes
    }

    /// round-change에 포함된 서명된 2/3 prevote 증명을 채택합니다. 잠금 자체를
    /// 임의로 해제하지 않고, 이후 제안자가 같은 valid value를 재제안할 수 있게 합니다.
    pub fn adopt_valid_value(
        &mut self,
        block_hash: &str,
        valid_round: u32,
        prevotes: &[ConsensusMessage],
    ) -> Result<(), String> {
        if prevotes.len() > self.validators.len() {
            return Err("round-change prevote 수가 검증자 수를 넘었습니다.".into());
        }
        let mut voters = HashSet::new();
        let mut power = 0_u64;
        for vote in prevotes {
            if vote.height != self.height
                || vote.round != valid_round
                || vote.vote_type != VoteType::Prevote
                || vote.block_hash != block_hash
            {
                return Err("round-change valid value의 prevote 증명이 일치하지 않습니다.".into());
            }
            vote.verify()?;
            let voting_power = self
                .validators
                .get(&vote.validator_id)
                .ok_or("등록되지 않은 검증자의 round-change prevote입니다.")?;
            if !voters.insert(vote.validator_id.as_str()) {
                return Err("round-change prevote에 중복 검증자가 있습니다.".into());
            }
            power = power
                .checked_add(*voting_power)
                .ok_or("round-change prevote 투표권 합계가 범위를 넘었습니다.")?;
        }
        if power.saturating_mul(3) <= self.total_power.saturating_mul(2) {
            return Err("round-change valid value에 2/3 초과 prevote가 없습니다.".into());
        }
        if self.valid_round.is_none_or(|known| valid_round > known) {
            self.valid_value = Some(block_hash.to_string());
            self.valid_round = Some(valid_round);
            for vote in prevotes {
                let key = (
                    vote.height,
                    vote.round,
                    vote.vote_type,
                    vote.validator_id.clone(),
                );
                self.vote_history
                    .insert(key.clone(), vote.block_hash.clone());
                self.vote_messages.insert(key, vote.clone());
            }
        }
        Ok(())
    }

    pub fn take_evidence(&mut self) -> Vec<DoubleVoteEvidence> {
        std::mem::take(&mut self.evidence)
    }

    /// 제한 시간 안에 합의하지 못하면 같은 높이의 다음 라운드를 시작합니다.
    pub fn on_timeout(&mut self) -> Result<u32, String> {
        let next_round = self
            .round
            .checked_add(1)
            .ok_or("합의 라운드 번호가 범위를 넘었습니다.")?;
        self.start_round(self.height, next_round)?;
        Ok(next_round)
    }

    pub fn finalized_hash(&self) -> Option<&str> {
        self.finalized.as_deref()
    }
}

#[cfg(test)]
mod valid_round_tests {
    use super::*;

    fn validator_key<'a>(keys: &'a [Wallet], address: &str) -> &'a Wallet {
        keys.iter()
            .find(|key| key.address() == address)
            .expect("expected proposer key")
    }

    #[test]
    fn certified_higher_valid_round_unlocks_a_different_value() {
        let keys: Vec<_> = (1..=4)
            .map(|value| Wallet::from_seed([value; 32]))
            .collect();
        let validators = keys
            .iter()
            .map(|key| Validator::new(key.address(), 100))
            .collect();
        let mut consensus = BftConsensus::new(validators).unwrap();
        consensus.start_round(1, 0).unwrap();

        let first_proposer = validator_key(&keys, consensus.expected_proposer());
        let first_block = Block::new(
            1,
            Block::genesis().hash,
            1_785_942_000,
            first_proposer.address(),
            Vec::new(),
        );
        let first_proposal = SignedProposal::new(1, 0, first_proposer, first_block.clone());
        consensus.handle_proposal(&first_proposal).unwrap();
        for key in keys.iter().take(3) {
            consensus
                .handle(ConsensusMessage::prevote(1, 0, key, &first_block.hash))
                .unwrap();
        }
        assert_eq!(
            consensus.locked_value(),
            Some((first_block.hash.as_str(), 0))
        );

        consensus.start_round(1, 2).unwrap();
        let next_proposer = validator_key(&keys, consensus.expected_proposer());
        let next_block = Block::new(
            1,
            Block::genesis().hash,
            1_785_942_001,
            next_proposer.address(),
            Vec::new(),
        );
        let proof: Vec<_> = keys
            .iter()
            .take(3)
            .map(|key| ConsensusMessage::prevote(1, 1, key, &next_block.hash))
            .collect();
        let proposal = SignedProposal::with_valid_round_certificate(
            1,
            2,
            next_proposer,
            next_block.clone(),
            Some(1),
            proof,
        );

        consensus.handle_proposal(&proposal).unwrap();
        assert_eq!(consensus.valid_value(), Some((next_block.hash.as_str(), 1)));
        assert_eq!(consensus.phase(), ConsensusPhase::Prevote);
    }

    #[test]
    fn valid_round_number_without_two_thirds_proof_is_rejected() {
        let keys: Vec<_> = (1..=4)
            .map(|value| Wallet::from_seed([value; 32]))
            .collect();
        let validators = keys
            .iter()
            .map(|key| Validator::new(key.address(), 100))
            .collect();
        let mut consensus = BftConsensus::new(validators).unwrap();
        consensus.start_round(1, 2).unwrap();
        let proposer = validator_key(&keys, consensus.expected_proposer());
        let block = Block::new(
            1,
            Block::genesis().hash,
            1_785_942_001,
            proposer.address(),
            Vec::new(),
        );
        let proposal = SignedProposal::with_valid_round_certificate(
            1,
            2,
            proposer,
            block.clone(),
            Some(1),
            vec![
                ConsensusMessage::prevote(1, 1, &keys[0], &block.hash),
                ConsensusMessage::prevote(1, 1, &keys[1], &block.hash),
            ],
        );

        assert_eq!(
            consensus.handle_proposal(&proposal).unwrap_err(),
            "valid_round에 필요한 2/3 초과 prevote 증명이 없습니다."
        );
    }
}

use crate::archive::StateSnapshot;
use crate::chain::Blockchain;
use crate::consensus::{
    BftConsensus, ConsensusMessage, ConsensusPhase, DoubleVoteEvidence, FinalityCertificate,
    SignedProposal, Validator, VoteType,
};
use crate::model::Block;
use crate::scheduled_event::{EventSchedule, MAX_CLOCK_DRIFT_SECONDS};
use crate::signer::ValidatorSigner;
use crate::snapshot_sync::SnapshotAttestation;
use crate::wallet::Wallet;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct ConsensusTimeouts {
    pub propose: Duration,
    pub prevote: Duration,
    pub precommit: Duration,
}

impl ConsensusTimeouts {
    pub fn uniform(timeout: Duration) -> Self {
        Self {
            propose: timeout,
            prevote: timeout,
            precommit: timeout,
        }
    }

    fn for_phase(self, phase: ConsensusPhase) -> Duration {
        match phase {
            ConsensusPhase::Waiting | ConsensusPhase::Propose => self.propose,
            ConsensusPhase::Prevote => self.prevote,
            ConsensusPhase::Precommit | ConsensusPhase::Finalized => self.precommit,
        }
    }
}

/// P2P 어댑터와 독립적으로 테스트 가능한 실제 합의 실행 코어입니다.
/// 후보 블록은 메모리에만 보관하고 2/3 초과 precommit 뒤에만 체인에 저장합니다.
pub struct ConsensusRuntime {
    pub chain: Blockchain,
    consensus: BftConsensus,
    validator: ValidatorSigner,
    pending: Option<Block>,
    valid_block: Option<Block>,
    deadline: Instant,
    timeouts: ConsensusTimeouts,
    validators: Vec<Validator>,
    precommits: Vec<ConsensusMessage>,
    deferred_votes: Vec<ConsensusMessage>,
    finalized: Vec<FinalityCertificate>,
    pending_finalized: Vec<FinalityCertificate>,
    event_schedule: EventSchedule,
    validator_interest_policy: crate::validator_interest::ValidatorInterestPolicy,
}

impl ConsensusRuntime {
    pub fn sign_snapshot_attestation(
        &self,
        snapshot: &StateSnapshot,
    ) -> Result<SnapshotAttestation, String> {
        let canonical = self
            .chain
            .block_by_height(snapshot.height)
            .ok_or("snapshot 높이의 확정 블록을 찾을 수 없습니다.")?;
        if snapshot.block_hash != canonical.hash {
            return Err("canonical 확정 블록과 다른 snapshot에는 서명할 수 없습니다.".into());
        }
        SnapshotAttestation::sign(snapshot, &self.validator)
    }

    pub fn new(
        chain: Blockchain,
        validators: Vec<Validator>,
        validator: Wallet,
        timeout: Duration,
    ) -> Result<Self, String> {
        Self::with_timeouts(
            chain,
            validators,
            validator,
            ConsensusTimeouts::uniform(timeout),
        )
    }

    pub fn with_timeouts(
        chain: Blockchain,
        validators: Vec<Validator>,
        validator: Wallet,
        timeouts: ConsensusTimeouts,
    ) -> Result<Self, String> {
        Self::with_signer(chain, validators, validator.into(), timeouts)
    }

    pub fn with_signer(
        chain: Blockchain,
        validators: Vec<Validator>,
        validator: ValidatorSigner,
        timeouts: ConsensusTimeouts,
    ) -> Result<Self, String> {
        let next_height = chain.blocks.last().map(|b| b.height + 1).unwrap_or(1);
        let mut consensus = BftConsensus::new(validators.clone())?;
        consensus.start_round(next_height, 0)?;
        let deadline = Instant::now() + timeouts.propose;
        Ok(Self {
            chain,
            consensus,
            validator,
            pending: None,
            valid_block: None,
            deadline,
            timeouts,
            validators,
            precommits: Vec::new(),
            deferred_votes: Vec::new(),
            finalized: Vec::new(),
            pending_finalized: Vec::new(),
            event_schedule: EventSchedule::default(),
            validator_interest_policy: crate::validator_interest::ValidatorInterestPolicy::default(
            ),
        })
    }

    pub fn set_event_schedule(&mut self, schedule: EventSchedule) -> Result<(), String> {
        schedule.validate()?;
        self.event_schedule = schedule;
        Ok(())
    }

    pub fn set_validator_interest_policy(
        &mut self,
        policy: crate::validator_interest::ValidatorInterestPolicy,
    ) -> Result<(), String> {
        policy.validate()?;
        self.validator_interest_policy = policy;
        Ok(())
    }

    pub fn make_proposal(&self, block: Block) -> Result<SignedProposal, String> {
        if self.consensus.phase() != ConsensusPhase::Propose {
            return Err("현재 단계에서는 새 블록 제안을 만들 수 없습니다.".into());
        }
        if self.validator.address() != self.consensus.expected_proposer() {
            return Err("이 노드는 현재 라운드 제안자가 아닙니다.".into());
        }
        let (block, valid_round) = match (self.valid_block.as_ref(), self.consensus.valid_value()) {
            (Some(valid_block), Some((valid_hash, valid_round)))
                if valid_block.hash == valid_hash =>
            {
                (valid_block.clone(), Some(valid_round))
            }
            _ => (block, None),
        };
        let proposer_id = self.validator.address();
        let signature = self.validator.sign_bytes(&SignedProposal::bytes_to_sign(
            block.height,
            self.consensus.round(),
            &proposer_id,
            &block.hash,
            valid_round,
        ))?;
        SignedProposal::from_signature(
            block.height,
            self.consensus.round(),
            proposer_id,
            block,
            valid_round,
            signature,
        )
    }

    pub fn receive_proposal(
        &mut self,
        proposal: SignedProposal,
    ) -> Result<ConsensusMessage, String> {
        let previous = self
            .chain
            .blocks
            .last()
            .ok_or("제네시스 블록이 없습니다.")?;
        if proposal.block.previous_hash != previous.hash
            || proposal.block.height != previous.height + 1
        {
            return Err("제안 블록이 현재 체인의 다음 블록이 아닙니다.".into());
        }
        self.validate_scheduled_block(&proposal.block)?;
        self.consensus.handle_proposal(&proposal)?;
        self.pending = Some(proposal.block.clone());
        self.reset_deadline();
        self.sign_vote(
            proposal.height,
            proposal.round,
            VoteType::Prevote,
            proposal.block.hash,
        )
    }

    pub fn receive_vote(
        &mut self,
        vote: ConsensusMessage,
    ) -> Result<Option<ConsensusMessage>, String> {
        let previous_phase = self.consensus.phase();
        self.consensus.handle(vote.clone())?;
        if vote.vote_type == VoteType::Precommit
            && !self.precommits.iter().any(|existing| {
                existing.height == vote.height
                    && existing.round == vote.round
                    && existing.validator_id == vote.validator_id
            })
        {
            self.precommits.push(vote);
        }
        if previous_phase != self.consensus.phase() {
            if previous_phase == ConsensusPhase::Prevote
                && self.consensus.phase() == ConsensusPhase::Precommit
            {
                self.valid_block = self.pending.clone();
            }
            self.reset_deadline();
        }
        if self.consensus.phase() == ConsensusPhase::Precommit {
            let block_hash = self
                .pending
                .as_ref()
                .ok_or("후보 블록이 없습니다.")?
                .hash
                .clone();
            return self
                .sign_vote(
                    self.pending.as_ref().unwrap().height,
                    self.consensus.round(),
                    VoteType::Precommit,
                    block_hash,
                )
                .map(Some);
        }
        if self.consensus.phase() == ConsensusPhase::Finalized {
            // 네트워크에서 같은 precommit이 다시 도착해도 이미 적용한 블록을
            // 중복 저장하거나 정상 노드를 오류 상태로 만들지 않습니다.
            let Some(block) = self.pending.take() else {
                return Ok(None);
            };
            if self.consensus.finalized_hash() != Some(block.hash.as_str()) {
                return Err("확정 해시와 후보 블록 해시가 다릅니다.".into());
            }
            let certificate = FinalityCertificate {
                block: block.clone(),
                round: self.consensus.round(),
                precommits: self.precommits.clone(),
            };
            certificate.verify(&self.validators)?;
            self.chain.apply_block(block)?;
            self.finalized.push(certificate.clone());
            self.pending_finalized.push(certificate);
        }
        Ok(None)
    }

    /// GossipSub에서 제안보다 먼저 도착한 정상 투표를 단계 전환 때까지 보관합니다.
    pub fn defer_vote(&mut self, vote: ConsensusMessage) {
        const MAX_DEFERRED_VOTES: usize = 1_024;
        if self.deferred_votes.iter().any(|known| {
            known.height == vote.height
                && known.round == vote.round
                && known.vote_type == vote.vote_type
                && known.validator_id == vote.validator_id
                && known.block_hash == vote.block_hash
        }) {
            return;
        }
        if self.deferred_votes.len() >= MAX_DEFERRED_VOTES {
            self.deferred_votes.remove(0);
        }
        self.deferred_votes.push(vote);
    }

    /// 상태가 전진한 뒤 처리 가능한 보류 투표를 재생합니다.
    pub fn replay_deferred_votes(&mut self) -> Result<Vec<ConsensusMessage>, String> {
        let mut outbound = Vec::new();
        loop {
            let mut progressed = false;
            let mut remaining = Vec::new();
            for vote in std::mem::take(&mut self.deferred_votes) {
                match self.receive_vote(vote.clone()) {
                    Ok(Some(precommit)) => {
                        outbound.push(precommit.clone());
                        self.receive_vote(precommit)?;
                        progressed = true;
                    }
                    Ok(None) => progressed = true,
                    Err(error) if is_deferable_vote_error(&error) => remaining.push(vote),
                    Err(_) => {}
                }
            }
            self.deferred_votes = remaining;
            if !progressed {
                break;
            }
        }
        Ok(outbound)
    }

    pub fn timeout_if_due(&mut self, now: Instant) -> Result<bool, String> {
        if now < self.deadline || self.consensus.phase() == ConsensusPhase::Finalized {
            return Ok(false);
        }
        self.consensus.on_timeout()?;
        self.pending = None;
        self.precommits.clear();
        self.deadline = now + self.timeouts.propose;
        Ok(true)
    }

    /// 거래가 없는 동안 합의 라운드를 쉬었다가 다시 활성화할 때, 노드 시작 시점의
    /// 이미 만료된 deadline을 사용하지 않도록 현재 단계 제한 시간을 새로 시작합니다.
    pub fn restart_phase_timeout(&mut self, now: Instant) {
        self.deadline = now + self.timeouts.for_phase(self.consensus.phase());
    }

    pub fn pending_transactions(&self) -> Vec<crate::model::Transaction> {
        self.pending
            .as_ref()
            .map(|block| block.transactions.clone())
            .unwrap_or_default()
    }

    pub fn force_timeout_for_test(&mut self) -> Result<u32, String> {
        self.pending = None;
        self.consensus.on_timeout()
    }

    pub fn take_evidence(&mut self) -> Vec<DoubleVoteEvidence> {
        self.consensus.take_evidence()
    }

    fn sign_vote(
        &self,
        height: u64,
        round: u32,
        vote_type: VoteType,
        block_hash: String,
    ) -> Result<ConsensusMessage, String> {
        let validator_id = self.validator.address();
        let signature = self.validator.sign_bytes(&ConsensusMessage::bytes_to_sign(
            height,
            round,
            &validator_id,
            vote_type,
            &block_hash,
        ))?;
        ConsensusMessage::from_signature(
            height,
            round,
            validator_id,
            vote_type,
            block_hash,
            signature,
        )
    }

    /// 현재 확정 높이 뒤의 블록만 제공하며 한 응답을 128개로 제한합니다.
    pub fn blocks_from(&self, from_height: u64) -> Vec<Block> {
        self.chain
            .blocks
            .iter()
            .filter(|b| b.height >= from_height)
            .take(128)
            .cloned()
            .collect()
    }

    pub fn certificates_from(&self, from_height: u64) -> Vec<FinalityCertificate> {
        const MAX_SYNC_BYTES: usize = 1_500_000;
        let mut bytes: usize = 0;
        let mut selected = Vec::new();
        for certificate in self
            .finalized
            .iter()
            .filter(|certificate| certificate.block.height >= from_height)
            .take(128)
        {
            let size = serde_json::to_vec(certificate)
                .map(|value| value.len())
                .unwrap_or(MAX_SYNC_BYTES + 1);
            if !selected.is_empty() && bytes.saturating_add(size) > MAX_SYNC_BYTES {
                break;
            }
            bytes = bytes.saturating_add(size);
            selected.push(certificate.clone());
        }
        selected
    }

    pub fn import_certificate_history(
        &mut self,
        certificates: Vec<FinalityCertificate>,
    ) -> Result<usize, String> {
        let mut imported = 0;
        for certificate in certificates {
            certificate.verify(&self.validators)?;
            let Some(block) = self.chain.block_by_height(certificate.block.height) else {
                continue;
            };
            if block.hash != certificate.block.hash {
                return Err("원장 블록과 확정 인증서 해시가 다릅니다.".into());
            }
            if !self
                .finalized
                .iter()
                .any(|known| known.block.height == certificate.block.height)
            {
                self.finalized.push(certificate);
                imported += 1;
            }
        }
        self.finalized
            .sort_by_key(|certificate| certificate.block.height);
        Ok(imported)
    }

    pub fn take_finalized(&mut self) -> Vec<FinalityCertificate> {
        std::mem::take(&mut self.pending_finalized)
    }

    /// 확정 인증서를 검증하고 현재 tip 바로 다음 블록만 순서대로 적용합니다.
    pub fn apply_sync_certificates(
        &mut self,
        certificates: Vec<FinalityCertificate>,
    ) -> Result<usize, String> {
        let mut applied = 0;
        for certificate in certificates {
            certificate.verify(&self.validators)?;
            let next = self.chain.blocks.last().unwrap().height + 1;
            if certificate.block.height < next {
                let canonical = self
                    .chain
                    .block_by_height(certificate.block.height)
                    .ok_or("기존 canonical 블록을 찾을 수 없습니다.")?;
                if canonical.hash != certificate.block.hash {
                    return Err(format!(
                        "확정성 위반: 높이 {}에 서로 다른 2/3 인증서가 존재합니다.",
                        certificate.block.height
                    ));
                }
                continue;
            }
            if certificate.block.height != next {
                return Err("동기화 응답에 블록 높이 공백이 있습니다.".into());
            }
            self.validate_scheduled_block(&certificate.block)?;
            self.chain.apply_block(certificate.block.clone())?;
            self.finalized.push(certificate);
            applied += 1;
        }
        if applied > 0 {
            let next_height = self.chain.blocks.last().unwrap().height + 1;
            self.consensus.start_round(next_height, 0)?;
            self.pending = None;
            self.valid_block = None;
            self.precommits.clear();
            self.reset_deadline();
        }
        Ok(applied)
    }

    /// 확정 블록 저장 후 다음 높이의 0라운드를 시작합니다.
    pub fn advance_after_finalization(&mut self) -> Result<(), String> {
        if self.consensus.phase() != ConsensusPhase::Finalized {
            return Err("확정되지 않은 상태에서는 다음 높이로 이동할 수 없습니다.".into());
        }
        let next_height = self.chain.blocks.last().unwrap().height + 1;
        self.consensus.start_round(next_height, 0)?;
        self.pending = None;
        self.valid_block = None;
        self.precommits.clear();
        self.reset_deadline();
        Ok(())
    }

    /// 제네시스 commitment가 같은 체인의 연속 블록만 적용합니다.
    pub fn apply_sync_batch(&mut self, blocks: Vec<Block>) -> Result<usize, String> {
        let mut applied = 0;
        for block in blocks {
            let next = self.chain.blocks.last().unwrap().height + 1;
            if block.height < next {
                continue;
            }
            if block.height != next {
                return Err("동기화 응답에 블록 높이 공백이 있습니다.".into());
            }
            self.validate_scheduled_block(&block)?;
            self.chain.apply_block(block)?;
            applied += 1;
        }
        Ok(applied)
    }

    pub fn phase(&self) -> ConsensusPhase {
        self.consensus.phase()
    }

    /// 현재 합의 단계와 제안자 순번이 모두 맞을 때만 로컬 제안을 허용합니다.
    pub fn can_make_proposal(&self) -> bool {
        self.consensus.phase() == ConsensusPhase::Propose
            && self.validator.address() == self.consensus.expected_proposer()
    }

    pub fn round(&self) -> u32 {
        self.consensus.round()
    }

    /// 아직 확정 블록이 없는 부트스트랩 구간에서 자동 발견한 공통 검증자 집합을 적용합니다.
    pub fn replace_bootstrap_validators(
        &mut self,
        validators: Vec<Validator>,
    ) -> Result<(), String> {
        if self.chain.tip_height() != 0 || self.pending.is_some() {
            return Err(
                "제네시스 이후에는 epoch 변경 절차 없이 검증자 집합을 바꿀 수 없습니다.".into(),
            );
        }
        let mut consensus = BftConsensus::new(validators.clone())?;
        consensus.start_round(1, 0)?;
        self.consensus = consensus;
        self.validators = validators;
        self.valid_block = None;
        self.precommits.clear();
        self.reset_deadline();
        Ok(())
    }

    pub fn locked_value(&self) -> Option<(&str, u32)> {
        self.consensus.locked_value()
    }

    pub fn valid_value(&self) -> Option<(&str, u32)> {
        self.consensus.valid_value()
    }

    fn reset_deadline(&mut self) {
        self.deadline = Instant::now() + self.timeouts.for_phase(self.consensus.phase());
    }

    fn validate_scheduled_block(&self, block: &Block) -> Result<(), String> {
        let previous = self.chain.blocks.last().ok_or("이전 블록이 없습니다.")?;
        if block.timestamp < previous.timestamp {
            return Err("블록 시각은 이전 블록보다 빠를 수 없습니다.".into());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "시스템 시각이 Unix epoch보다 이전입니다.")?
            .as_secs();
        if block.timestamp > now.saturating_add(MAX_CLOCK_DRIFT_SECONDS) {
            return Err("블록 시각이 허용된 미래 오차를 넘었습니다.".into());
        }
        let configured_events: Vec<_> = block
            .system_events
            .iter()
            .filter(|event| {
                !matches!(
                    &event.action,
                    crate::scheduled_event::ScheduledEventAction::BootstrapValidatorReward { .. }
                        | crate::scheduled_event::ScheduledEventAction::NodeMilestoneReward { .. }
                        | crate::scheduled_event::ScheduledEventAction::ValidatorDailyInterest { .. }
                )
            })
            .cloned()
            .collect();
        self.event_schedule.validate_block_events(
            block.timestamp,
            &configured_events,
            self.chain.executed_events(),
        )?;
        for event in &block.system_events {
            match &event.action {
                crate::scheduled_event::ScheduledEventAction::BootstrapValidatorReward {
                    registrations,
                    ..
                } if event.id == "ieum-bootstrap-validator-reward-v1" => {
                    let mut rewarded: Vec<_> = registrations
                        .iter()
                        .map(|registration| registration.validator_id.clone())
                        .collect();
                    let mut active: Vec<_> = self
                        .validators
                        .iter()
                        .map(|validator| validator.id.clone())
                        .collect();
                    rewarded.sort();
                    active.sort();
                    if rewarded != active {
                        return Err(
                            "최초 검증자 보상 대상이 현재 활성 검증자 집합과 다릅니다.".into()
                        );
                    }
                }
                crate::scheduled_event::ScheduledEventAction::NodeMilestoneReward { .. }
                    if event.id == "ieum-node-100-reward-v1" => {}
                crate::scheduled_event::ScheduledEventAction::ValidatorDailyInterest {
                    snapshot_height,
                    policy_hash,
                    annual_rate_bps,
                    payments,
                } if event.id == crate::validator_interest::event_id(block.timestamp) => {
                    if *snapshot_height != self.chain.tip_height()
                        || policy_hash != &self.validator_interest_policy.hash()
                        || *annual_rate_bps != self.validator_interest_policy.annual_rate_bps
                        || payments
                            != &crate::validator_interest::calculate_payments(
                                &self.validator_interest_policy,
                                &self.validators,
                                &self.chain.balances_snapshot(),
                            )
                    {
                        return Err("검증자 일일 이자가 로컬 정책과 snapshot 계산 결과에 일치하지 않습니다.".into());
                    }
                }
                crate::scheduled_event::ScheduledEventAction::BootstrapValidatorReward {
                    ..
                }
                | crate::scheduled_event::ScheduledEventAction::NodeMilestoneReward { .. } => {
                    return Err("내장 최초 보상 이벤트 ID가 올바르지 않습니다.".into());
                }
                crate::scheduled_event::ScheduledEventAction::ValidatorDailyInterest { .. } => {
                    return Err("검증자 일일 이자 이벤트 ID가 올바르지 않습니다.".into());
                }
                _ => continue,
            }
            event.validate()?;
            if self.chain.executed_events().contains(&event.id) {
                return Err(format!("이벤트 {}는 이미 실행됐습니다.", event.id));
            }
        }
        Ok(())
    }
}

pub fn is_deferable_vote_error(error: &str) -> bool {
    matches!(error, "현재 합의 단계와 투표 종류가 일치하지 않습니다.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::ConsensusMessage;

    #[test]
    fn votes_arriving_before_proposal_are_replayed() {
        let keys: Vec<_> = (1..=4)
            .map(|value| Wallet::from_seed([value; 32]))
            .collect();
        let validators: Vec<_> = keys
            .iter()
            .map(|key| Validator::new(key.address(), 100))
            .collect();
        let chain = Blockchain::new(vec![(keys[0].address(), 1_000)]);
        let block = Block::new(
            1,
            chain.blocks.last().unwrap().hash.clone(),
            1,
            keys[0].address(),
            Vec::new(),
        );
        let mut runtimes: Vec<_> = (1..=4)
            .map(|value| Wallet::from_seed([value; 32]))
            .map(|key| {
                ConsensusRuntime::new(
                    chain.clone(),
                    validators.clone(),
                    key,
                    Duration::from_secs(1),
                )
                .unwrap()
            })
            .collect();
        let proposal = runtimes
            .iter()
            .find(|runtime| runtime.can_make_proposal())
            .unwrap()
            .make_proposal(block)
            .unwrap();
        let block_hash = proposal.block.hash.clone();
        let receiver = &mut runtimes[0];

        for key in keys.iter().take(3) {
            let vote = ConsensusMessage::prevote(1, 0, key, &block_hash);
            let error = receiver.receive_vote(vote.clone()).unwrap_err();
            assert!(is_deferable_vote_error(&error));
            receiver.defer_vote(vote);
        }

        let local_prevote = receiver.receive_proposal(proposal).unwrap();
        let _ = receiver.receive_vote(local_prevote).unwrap();
        let outbound = receiver.replay_deferred_votes().unwrap();

        assert!(matches!(
            receiver.phase(),
            ConsensusPhase::Precommit | ConsensusPhase::Finalized
        ));
        assert!(!outbound.is_empty());
    }

    #[test]
    fn idle_round_timeout_can_be_restarted_when_work_arrives() {
        let keys: Vec<_> = (1..=4)
            .map(|value| Wallet::from_seed([value; 32]))
            .collect();
        let validators = keys
            .iter()
            .map(|key| Validator::new(key.address(), 100))
            .collect();
        let chain = Blockchain::new(vec![(keys[0].address(), 1_000)]);
        let mut runtime = ConsensusRuntime::new(
            chain,
            validators,
            Wallet::from_seed([1; 32]),
            Duration::from_millis(10),
        )
        .unwrap();
        let work_arrived = Instant::now() + Duration::from_secs(1);

        runtime.restart_phase_timeout(work_arrived);

        assert!(
            !runtime
                .timeout_if_due(work_arrived + Duration::from_millis(9))
                .unwrap()
        );
        assert!(
            runtime
                .timeout_if_due(work_arrived + Duration::from_millis(10))
                .unwrap()
        );
    }
}

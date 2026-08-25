use crate::Wallet;
use crate::consensus::{ConsensusMessage, Validator, VoteType};
use crate::model::Block;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// IEUM의 nil 값은 빈 문자열 대신 고정 도메인 값을 사용합니다.
pub const NIL_BLOCK_HASH: &str = "ieum:nil";

/// 블록 prevote/precommit과 분리된 라운드 변경 신호입니다. 같은 라운드의 블록에
/// 이미 투표한 검증자도 이중투표를 만들지 않고 timeout 사실을 알릴 수 있습니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedRoundChange {
    pub chain_id: u64,
    pub genesis_commitment: String,
    pub height: u64,
    pub round: u32,
    pub validator_id: String,
    #[serde(default)]
    pub valid_value: Option<RoundChangeValidValue>,
    pub signature: String,
}

/// 라운드 변경 중 새 제안자가 기존 잠금 값을 안전하게 이어받기 위한 2/3 prevote 증명입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoundChangeValidValue {
    pub block: Block,
    pub valid_round: u32,
    pub prevotes: Vec<ConsensusMessage>,
}

impl SignedRoundChange {
    pub fn new(
        chain_id: u64,
        genesis_commitment: impl Into<String>,
        height: u64,
        round: u32,
        validator: &Wallet,
    ) -> Self {
        let genesis_commitment = genesis_commitment.into();
        let validator_id = validator.address();
        let signature = validator.sign_bytes(&Self::bytes_to_sign(
            chain_id,
            &genesis_commitment,
            height,
            round,
            &validator_id,
            None,
        ));
        Self {
            chain_id,
            genesis_commitment,
            height,
            round,
            validator_id,
            valid_value: None,
            signature,
        }
    }

    pub fn from_signature(
        chain_id: u64,
        genesis_commitment: String,
        height: u64,
        round: u32,
        validator_id: String,
        signature: String,
    ) -> Result<Self, String> {
        let message = Self {
            chain_id,
            genesis_commitment,
            height,
            round,
            validator_id,
            valid_value: None,
            signature,
        };
        message.verify()?;
        Ok(message)
    }

    pub fn bytes_to_sign(
        chain_id: u64,
        genesis_commitment: &str,
        height: u64,
        round: u32,
        validator_id: &str,
        _valid_value: Option<&RoundChangeValidValue>,
    ) -> Vec<u8> {
        let mut bytes = b"IEUM-ROUND-CHANGE-V1".to_vec();
        bytes.extend_from_slice(&chain_id.to_be_bytes());
        push_round_change_text(&mut bytes, genesis_commitment);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&round.to_be_bytes());
        push_round_change_text(&mut bytes, validator_id);
        bytes
    }

    pub fn with_valid_value(
        chain_id: u64,
        genesis_commitment: impl Into<String>,
        height: u64,
        round: u32,
        validator: &Wallet,
        valid_value: Option<RoundChangeValidValue>,
    ) -> Self {
        let genesis_commitment = genesis_commitment.into();
        let validator_id = validator.address();
        let signature = validator.sign_bytes(&Self::bytes_to_sign(
            chain_id,
            &genesis_commitment,
            height,
            round,
            &validator_id,
            valid_value.as_ref(),
        ));
        Self {
            chain_id,
            genesis_commitment,
            height,
            round,
            validator_id,
            valid_value,
            signature,
        }
    }

    pub fn from_signature_with_valid_value(
        chain_id: u64,
        genesis_commitment: String,
        height: u64,
        round: u32,
        validator_id: String,
        valid_value: Option<RoundChangeValidValue>,
        signature: String,
    ) -> Result<Self, String> {
        let message = Self {
            chain_id,
            genesis_commitment,
            height,
            round,
            validator_id,
            valid_value,
            signature,
        };
        message.verify()?;
        Ok(message)
    }

    pub fn verify(&self) -> Result<(), String> {
        crate::wallet::verify_signature(
            &self.validator_id,
            &Self::bytes_to_sign(
                self.chain_id,
                &self.genesis_commitment,
                self.height,
                self.round,
                &self.validator_id,
                self.valid_value.as_ref(),
            ),
            &self.signature,
        )
    }
}

fn push_round_change_text(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NilVote {
    pub message: ConsensusMessage,
}

impl NilVote {
    pub fn prevote(height: u64, round: u32, validator: &Wallet) -> Self {
        Self {
            message: ConsensusMessage::prevote(height, round, validator, NIL_BLOCK_HASH),
        }
    }

    pub fn precommit(height: u64, round: u32, validator: &Wallet) -> Self {
        Self {
            message: ConsensusMessage::precommit(height, round, validator, NIL_BLOCK_HASH),
        }
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.message.block_hash != NIL_BLOCK_HASH {
            return Err("nil vote의 블록 값이 올바르지 않습니다.".into());
        }
        self.message.verify()
    }
}

/// 다음 라운드로 이동해도 된다는 2/3 초과 서명 증명입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoundChangeCertificate {
    pub height: u64,
    pub from_round: u32,
    pub votes: Vec<ConsensusMessage>,
}

impl RoundChangeCertificate {
    pub fn verify(&self, validators: &[Validator]) -> Result<(), String> {
        let total: u128 = validators
            .iter()
            .map(|item| item.voting_power as u128)
            .sum();
        if total == 0 {
            return Err("검증자 총 투표권이 0입니다.".into());
        }
        let powers: HashMap<_, _> = validators
            .iter()
            .map(|item| (item.id.as_str(), item.voting_power as u128))
            .collect();
        let mut voters = HashSet::new();
        let mut signed = 0_u128;
        for vote in &self.votes {
            if vote.height != self.height
                || vote.round != self.from_round
                || vote.vote_type != VoteType::Precommit
                || vote.block_hash != NIL_BLOCK_HASH
            {
                return Err("round-change 인증서에 다른 투표가 섞였습니다.".into());
            }
            vote.verify()?;
            let power = powers
                .get(vote.validator_id.as_str())
                .ok_or("등록되지 않은 검증자의 round-change 투표입니다.")?;
            if voters.insert(vote.validator_id.as_str()) {
                signed = signed
                    .checked_add(*power)
                    .ok_or("round-change 투표권 합계가 범위를 넘었습니다.")?;
            }
        }
        if signed * 3 <= total * 2 {
            return Err("round-change에 필요한 2/3 초과 서명이 없습니다.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EraConfig {
    pub era_length: u64,
    pub activation_delay: u64,
}

impl Default for EraConfig {
    fn default() -> Self {
        Self {
            era_length: 10_000,
            activation_delay: 100,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidatorSetUpdate {
    pub requested_height: u64,
    pub activation_height: u64,
    pub validators: Vec<Validator>,
}

/// Casper의 era 개념을 단순화한 검증자 세트 관리자입니다.
#[derive(Clone, Debug)]
pub struct EraManager {
    config: EraConfig,
    active: Vec<Validator>,
    pending: BTreeMap<u64, ValidatorSetUpdate>,
}

impl EraManager {
    pub fn new(config: EraConfig, validators: Vec<Validator>) -> Result<Self, String> {
        validate_set(&validators)?;
        if config.era_length == 0 || config.activation_delay == 0 {
            return Err("era 길이와 활성화 지연은 1 이상이어야 합니다.".into());
        }
        Ok(Self {
            config,
            active: validators,
            pending: BTreeMap::new(),
        })
    }

    pub fn era_at(&self, height: u64) -> u64 {
        height / self.config.era_length
    }

    pub fn schedule(
        &mut self,
        requested_height: u64,
        validators: Vec<Validator>,
    ) -> Result<u64, String> {
        validate_set(&validators)?;
        let earliest = requested_height
            .checked_add(self.config.activation_delay)
            .ok_or("검증자 세트 활성화 높이가 범위를 넘었습니다.")?;
        let activation = earliest
            .div_ceil(self.config.era_length)
            .saturating_mul(self.config.era_length);
        self.pending.insert(
            activation,
            ValidatorSetUpdate {
                requested_height,
                activation_height: activation,
                validators,
            },
        );
        Ok(activation)
    }

    pub fn apply_height(&mut self, height: u64) -> Option<ValidatorSetUpdate> {
        let activation = self.pending.keys().copied().find(|item| *item <= height)?;
        let update = self.pending.remove(&activation)?;
        self.active = update.validators.clone();
        Some(update)
    }

    pub fn active(&self) -> &[Validator] {
        &self.active
    }
}

fn validate_set(validators: &[Validator]) -> Result<(), String> {
    if validators.len() < 4 {
        return Err("BFT 검증자 세트는 최소 4개여야 합니다.".into());
    }
    let mut ids = HashSet::new();
    if validators
        .iter()
        .any(|item| item.voting_power == 0 || !ids.insert(item.id.as_str()))
    {
        return Err("검증자 ID는 고유해야 하고 투표권은 1 이상이어야 합니다.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validators() -> Vec<Validator> {
        (1..=4)
            .map(|index| {
                let wallet = Wallet::from_seed([index; 32]);
                Validator::new(wallet.address(), 100)
            })
            .collect()
    }

    #[test]
    fn validator_update_waits_for_delayed_era_boundary() {
        let mut manager = EraManager::new(
            EraConfig {
                era_length: 100,
                activation_delay: 25,
            },
            validators(),
        )
        .unwrap();
        assert_eq!(manager.schedule(90, validators()).unwrap(), 200);
        assert!(manager.apply_height(199).is_none());
        assert!(manager.apply_height(200).is_some());
    }

    #[test]
    fn nil_certificate_requires_more_than_two_thirds() {
        let keys: Vec<_> = (1..=4)
            .map(|index| Wallet::from_seed([index; 32]))
            .collect();
        let certificate = RoundChangeCertificate {
            height: 9,
            from_round: 2,
            votes: keys
                .iter()
                .take(3)
                .map(|key| NilVote::precommit(9, 2, key).message)
                .collect(),
        };
        assert!(certificate.verify(&validators()).is_ok());
    }

    #[test]
    fn signed_round_change_carries_independently_signed_valid_value() {
        let keys: Vec<_> = (1..=4)
            .map(|index| Wallet::from_seed([index; 32]))
            .collect();
        let block = Block::new(9, "parent".to_string(), 1, keys[0].address(), Vec::new());
        let valid_value = RoundChangeValidValue {
            block: block.clone(),
            valid_round: 2,
            prevotes: keys
                .iter()
                .take(3)
                .map(|key| ConsensusMessage::prevote(9, 2, key, &block.hash))
                .collect(),
        };
        let message = SignedRoundChange::with_valid_value(
            21_004,
            "genesis",
            9,
            3,
            &keys[0],
            Some(valid_value),
        );
        message.verify().unwrap();
        assert_eq!(message.valid_value.unwrap().prevotes.len(), 3);
    }
}

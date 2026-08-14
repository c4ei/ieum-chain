use crate::model::{Address, Block, Transaction};
use crate::wallet::verify_transaction;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// 운영 초기 재단 수수료 정책입니다.
///
/// 합의 상태를 결정하는 값이므로 노드별 환경변수나 실행 옵션으로 바꾸면 안 됩니다.
/// 주소는 체인 내부 표준인 소문자 Ethereum 주소로 고정합니다.
pub const FOUNDATION_FEE_ADDRESS: &str = "0x356456ff1216b57a6f8891b195b42d296789b67d";
pub const FOUNDATION_FEE_BPS: u128 = 2_000;
pub const FEE_BPS_DENOMINATOR: u128 = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blockchain {
    pub chain_id: u64,
    pub genesis_commitment: String,
    /// 확정된 블록만 저장합니다. 합의 중인 후보 블록은 여기에 넣으면 안 됩니다.
    pub blocks: Vec<Block>,
    pub initial_balances: HashMap<Address, u128>,
    balances: HashMap<Address, u128>,
    next_nonces: HashMap<Address, u64>,
    #[serde(default)]
    executed_events: HashSet<String>,
}

impl Blockchain {
    /// 제네시스 잔액과 0번 블록으로 새 체인을 시작합니다.
    pub fn new(initial_balances: Vec<(Address, u128)>) -> Self {
        Self::with_chain_id(21_004, initial_balances)
    }

    pub fn with_chain_id(chain_id: u64, initial_balances: Vec<(Address, u128)>) -> Self {
        let initial_balances: HashMap<_, _> = initial_balances.into_iter().collect();
        Self {
            chain_id,
            genesis_commitment: Block::genesis().hash,
            blocks: vec![Block::genesis()],
            balances: initial_balances.clone(),
            initial_balances,
            next_nonces: HashMap::new(),
            executed_events: HashSet::new(),
        }
    }

    pub fn from_genesis(genesis: &crate::genesis::GenesisConfig) -> Result<Self, String> {
        genesis.validate()?;
        let balances = genesis
            .initial_balances
            .iter()
            .map(|(address, balance)| (normalize_address(address), *balance))
            .collect();
        let mut chain = Self::with_chain_id(genesis.chain_id, balances);
        chain.blocks = vec![Block::genesis_at(genesis.genesis_time)];
        chain.genesis_commitment = genesis.genesis_hash()?;
        Ok(chain)
    }

    pub fn from_snapshot(
        chain_id: u64,
        genesis_commitment: String,
        height: u64,
        block_hash: String,
        balances: HashMap<Address, u128>,
        next_nonces: HashMap<Address, u64>,
    ) -> Result<Self, String> {
        Self::from_snapshot_with_events(
            chain_id,
            genesis_commitment,
            height,
            block_hash,
            balances,
            next_nonces,
            HashSet::new(),
        )
    }

    pub fn from_snapshot_with_events(
        chain_id: u64,
        genesis_commitment: String,
        height: u64,
        block_hash: String,
        balances: HashMap<Address, u128>,
        next_nonces: HashMap<Address, u64>,
        executed_events: HashSet<String>,
    ) -> Result<Self, String> {
        if block_hash.trim_start_matches("0x").len() != 64 {
            return Err("체크포인트 블록 해시는 32바이트 hex여야 합니다.".into());
        }
        let anchor = Block {
            height,
            previous_hash: String::new(),
            timestamp: 0,
            producer: "checkpoint".into(),
            transactions: vec![],
            system_events: vec![],
            hash: block_hash.trim_start_matches("0x").to_string(),
        };
        Ok(Self {
            chain_id,
            genesis_commitment,
            blocks: vec![anchor],
            initial_balances: balances.clone(),
            balances,
            next_nonces,
            executed_events,
        })
    }

    pub fn balance_of(&self, address: &str) -> u128 {
        self.balances.get(address).copied().unwrap_or(0)
    }

    pub fn tip_height(&self) -> u64 {
        self.blocks.last().map(|block| block.height).unwrap_or(0)
    }

    pub fn tip_hash(&self) -> &str {
        self.blocks
            .last()
            .map(|block| block.hash.as_str())
            .unwrap_or("")
    }

    pub fn balances_snapshot(&self) -> HashMap<Address, u128> {
        self.balances.clone()
    }

    pub fn nonces_snapshot(&self) -> HashMap<Address, u64> {
        self.next_nonces.clone()
    }

    pub fn executed_events(&self) -> &HashSet<String> {
        &self.executed_events
    }

    pub fn block_by_height(&self, height: u64) -> Option<&Block> {
        self.blocks.iter().find(|block| block.height == height)
    }

    pub fn block_by_hash(&self, hash: &str) -> Option<&Block> {
        let hash = hash.trim_start_matches("0x");
        self.blocks.iter().find(|block| block.hash == hash)
    }

    pub fn transaction_by_hash(&self, hash: &str) -> Option<(&Block, usize, &Transaction)> {
        let hash = hash.trim_start_matches("0x");
        self.blocks.iter().find_map(|block| {
            block
                .transactions
                .iter()
                .enumerate()
                .find(|(_, transaction)| transaction.id() == hash)
                .map(|(index, transaction)| (block, index, transaction))
        })
    }

    pub fn next_nonce(&self, address: &str) -> u64 {
        self.next_nonces.get(address).copied().unwrap_or(0)
    }

    /// 거래가 있을 때만 후보 블록을 만들고 즉시 적용하는 학습용 경로입니다.
    /// BFT 노드에서는 후보 생성과 확정을 분리해 확정 후 apply_block을 호출해야 합니다.
    pub fn add_block(
        &mut self,
        transactions: Vec<Transaction>,
        producer: Address,
    ) -> Result<&Block, String> {
        if transactions.is_empty() {
            return Err("거래가 없으므로 빈 블록을 건너뜁니다.".into());
        }
        let previous = self.blocks.last().expect("제네시스 블록이 필요합니다.");
        let block = Block::new(
            previous.height + 1,
            previous.hash.clone(),
            now(),
            producer,
            transactions,
        );
        self.apply_block(block)?;
        Ok(self.blocks.last().unwrap())
    }

    /// 다른 노드에서 합의로 확정된 원본 블록을 검증하고 상태에 반영합니다.
    pub fn apply_block(&mut self, block: Block) -> Result<&Block, String> {
        let previous = self.blocks.last().expect("제네시스 블록이 필요합니다.");
        if block.height != previous.height + 1 || block.previous_hash != previous.hash {
            return Err("새 블록이 현재 체인의 다음 블록이 아닙니다.".into());
        }
        if block.hash != block.calculate_hash() {
            return Err("새 블록 해시가 올바르지 않습니다.".into());
        }
        let mut balances = self.balances.clone();
        let mut nonces = self.next_nonces.clone();
        apply_transactions(
            self.chain_id,
            &block.transactions,
            &block.producer,
            &mut balances,
            &mut nonces,
        )?;
        let mut executed_events = self.executed_events.clone();
        apply_system_events(
            &block.system_events,
            block.timestamp,
            &mut balances,
            &mut executed_events,
        )?;
        self.blocks.push(block);
        self.balances = balances;
        self.next_nonces = nonces;
        self.executed_events = executed_events;
        Ok(self.blocks.last().unwrap())
    }

    /// 저장 파일을 읽은 뒤 제네시스부터 모든 잔액과 nonce를 다시 계산합니다.
    pub fn verify_and_rebuild(&mut self) -> Result<(), String> {
        let mut balances = self.initial_balances.clone();
        let mut nonces = HashMap::new();
        let mut executed_events = HashSet::new();
        let Some(genesis) = self.blocks.first() else {
            return Err("제네시스 블록이 없습니다.".into());
        };
        if genesis.height != 0
            || genesis.previous_hash != "0".repeat(64)
            || genesis.producer != "genesis"
            || !genesis.transactions.is_empty()
            || !genesis.system_events.is_empty()
        {
            return Err("제네시스 블록이 다릅니다.".into());
        }
        for (index, block) in self.blocks.iter().enumerate() {
            if block.hash != block.calculate_hash() {
                return Err(format!("{index}번 블록 해시가 변조되었습니다."));
            }
            if index > 0 {
                let previous = &self.blocks[index - 1];
                if block.height != previous.height + 1 || block.previous_hash != previous.hash {
                    return Err(format!("{index}번 블록 연결이 끊어졌습니다."));
                }
                apply_transactions(
                    self.chain_id,
                    &block.transactions,
                    &block.producer,
                    &mut balances,
                    &mut nonces,
                )?;
                apply_system_events(
                    &block.system_events,
                    block.timestamp,
                    &mut balances,
                    &mut executed_events,
                )?;
            }
        }
        self.balances = balances;
        self.next_nonces = nonces;
        self.executed_events = executed_events;
        Ok(())
    }

    /// 체크포인트와 향후 상태 증명에서 사용할 결정론적 상태 해시입니다.
    pub fn state_hash(&self) -> String {
        let mut entries: Vec<_> = self.balances.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut hasher = Sha256::new();
        for (address, balance) in entries {
            hasher.update(address.as_bytes());
            hasher.update(balance.to_be_bytes());
            hasher.update(self.next_nonce(address).to_be_bytes());
        }
        let mut event_ids: Vec<_> = self.executed_events.iter().collect();
        event_ids.sort();
        for event_id in event_ids {
            hasher.update(b"event:");
            hasher.update(event_id.as_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

fn apply_system_events(
    events: &[crate::scheduled_event::ScheduledEvent],
    block_timestamp: u64,
    balances: &mut HashMap<Address, u128>,
    executed: &mut HashSet<String>,
) -> Result<(), String> {
    use crate::scheduled_event::ScheduledEventAction;
    for event in events {
        event.validate()?;
        if event.execute_at > block_timestamp {
            return Err(format!(
                "이벤트 {}가 실행 시각보다 먼저 포함됐습니다.",
                event.id
            ));
        }
        if !executed.insert(event.id.clone()) {
            return Err(format!("이벤트 {}는 이미 실행됐습니다.", event.id));
        }
        match &event.action {
            ScheduledEventAction::TreasuryDistribution { recipients } => {
                for payment in recipients {
                    transfer_from_foundation(balances, &payment.address, payment.amount)?;
                }
            }
            ScheduledEventAction::PeriodicProducerReward { producer, amount } => {
                transfer_from_foundation(balances, producer, *amount)?;
            }
            ScheduledEventAction::IncidentCompensation { victim, amount, .. } => {
                // 과거 거래를 삭제하지 않고 재단 계정에서 피해 보상 역거래를 기록합니다.
                transfer_from_foundation(balances, victim, *amount)?;
            }
            ScheduledEventAction::ProtocolCheckpoint { .. } => {}
            ScheduledEventAction::BootstrapValidatorReward {
                registrations,
                amount,
            } => {
                for registration in registrations {
                    transfer_from_foundation(balances, &registration.validator_id, *amount)?;
                }
            }
            ScheduledEventAction::NodeMilestoneReward {
                registrations,
                amount,
            } => {
                for registration in registrations {
                    transfer_from_foundation(balances, &registration.reward_address, *amount)?;
                }
            }
            ScheduledEventAction::ValidatorDailyInterest { payments, .. } => {
                for payment in payments {
                    transfer_from_foundation(balances, &payment.address, payment.amount)?;
                }
            }
        }
    }
    Ok(())
}

fn transfer_from_foundation(
    balances: &mut HashMap<Address, u128>,
    receiver: &str,
    amount: u128,
) -> Result<(), String> {
    let foundation = balances.get(FOUNDATION_FEE_ADDRESS).copied().unwrap_or(0);
    if foundation < amount {
        return Err("예약 이벤트를 실행할 재단 잔액이 부족합니다.".into());
    }
    balances.insert(FOUNDATION_FEE_ADDRESS.into(), foundation - amount);
    credit_balance(
        balances,
        &normalize_address(receiver),
        amount,
        "예약 이벤트 수령 잔액이 u128 범위를 넘습니다.",
    )
}

fn apply_transactions(
    chain_id: u64,
    transactions: &[Transaction],
    producer: &str,
    balances: &mut HashMap<Address, u128>,
    nonces: &mut HashMap<Address, u64>,
) -> Result<(), String> {
    // 블록 하나를 원자적으로 처리하기 위해 복제된 상태에 먼저 적용합니다.
    // 하나라도 실패하면 호출자가 원래 balances/nonces를 그대로 유지합니다.
    for tx in transactions {
        if tx.signature.starts_with("ethraw:") {
            crate::raw_transaction::verify_embedded(tx, chain_id)?;
        } else {
            verify_transaction(tx)?;
        }
        if tx.amount == 0 {
            return Err("송금액은 0보다 커야 합니다.".into());
        }
        let expected_nonce = nonces.get(&tx.from).copied().unwrap_or(0);
        if tx.nonce != expected_nonce {
            return Err(format!(
                "nonce 오류: 기대 {expected_nonce}, 입력 {}",
                tx.nonce
            ));
        }
        let total = tx
            .amount
            .checked_add(tx.fee)
            .ok_or("송금액과 수수료 합계가 너무 큽니다.")?;
        let sender = balances.get(&tx.from).copied().unwrap_or(0);
        if sender < total {
            return Err("수수료를 포함한 잔액이 부족합니다.".into());
        }
        balances.insert(tx.from.clone(), sender - total);
        let receiver = balances.get(&tx.to).copied().unwrap_or(0);
        balances.insert(
            tx.to.clone(),
            receiver
                .checked_add(tx.amount)
                .ok_or("받는 계정 잔액이 u128 범위를 넘습니다.")?,
        );
        // 재단 몫을 먼저 내림 계산하고 나머지 전부를 생성자에게 지급합니다.
        // 따라서 아주 작은 수수료의 나머지도 소각되거나 유실되지 않습니다.
        let foundation_fee = tx
            .fee
            .checked_mul(FOUNDATION_FEE_BPS)
            .ok_or("재단 수수료 계산이 u128 범위를 넘습니다.")?
            / FEE_BPS_DENOMINATOR;
        let producer_fee = tx.fee - foundation_fee;
        credit_balance(
            balances,
            producer,
            producer_fee,
            "블록 생성자 보상 잔액이 u128 범위를 넘습니다.",
        )?;
        credit_balance(
            balances,
            FOUNDATION_FEE_ADDRESS,
            foundation_fee,
            "재단 수수료 잔액이 u128 범위를 넘습니다.",
        )?;
        nonces.insert(tx.from.clone(), expected_nonce + 1);
    }
    Ok(())
}

fn credit_balance(
    balances: &mut HashMap<Address, u128>,
    address: &str,
    amount: u128,
    overflow_message: &str,
) -> Result<(), String> {
    if amount == 0 {
        return Ok(());
    }
    let current = balances.get(address).copied().unwrap_or(0);
    balances.insert(
        address.to_string(),
        current.checked_add(amount).ok_or(overflow_message)?,
    );
    Ok(())
}

fn normalize_address(address: &str) -> String {
    if address.starts_with("0x") {
        format!(
            "0x{}",
            address.trim_start_matches("0x").to_ascii_lowercase()
        )
    } else {
        address.to_string()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("시스템 시간이 잘못되었습니다.")
        .as_secs()
}

#[cfg(test)]
mod initial_reward_tests {
    use super::*;
    use crate::network::ValidatorRegistration;
    use crate::scheduled_event::{ScheduledEvent, ScheduledEventAction};
    use crate::wallet::Wallet;

    #[test]
    fn bootstrap_validator_reward_is_paid_exactly_once() {
        let mut chain = Blockchain::new(vec![(
            FOUNDATION_FEE_ADDRESS.into(),
            1_000 * 10u128.pow(18),
        )]);
        let registrations: Vec<_> = (1..=4)
            .map(|index| {
                let wallet = Wallet::from_seed([index; 32]);
                let peer_id = format!("peer-{index}");
                ValidatorRegistration {
                    validator_id: wallet.address(),
                    peer_id: peer_id.clone(),
                    signature_hex: wallet.sign_bytes(&ValidatorRegistration::bytes_to_sign(
                        &wallet.address(),
                        &peer_id,
                    )),
                }
            })
            .collect();
        let event = ScheduledEvent {
            id: "ieum-bootstrap-validator-reward-v1".into(),
            execute_at: 1,
            action: ScheduledEventAction::BootstrapValidatorReward {
                registrations: registrations.clone(),
                amount: 10 * 10u128.pow(18),
            },
        };
        let first = Block::new(
            1,
            chain.tip_hash().into(),
            1,
            registrations[0].validator_id.clone(),
            vec![],
        )
        .with_system_events(vec![event.clone()]);
        chain.apply_block(first).unwrap();
        for registration in &registrations {
            assert_eq!(
                chain.balance_of(&registration.validator_id),
                10 * 10u128.pow(18)
            );
        }
        let second = Block::new(
            2,
            chain.tip_hash().into(),
            2,
            registrations[0].validator_id.clone(),
            vec![],
        )
        .with_system_events(vec![event]);
        assert!(chain.apply_block(second).is_err());
    }
}

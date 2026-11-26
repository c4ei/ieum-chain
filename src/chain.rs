use crate::evm::{EvmReceipt, EvmState};
use crate::model::{Address, Block, Transaction, TransactionAction};
use crate::staking::StakingState;
use crate::wallet::verify_transaction;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// 운영 초기 재단 수수료 정책입니다.
///
/// 합의 상태를 결정하는 값이므로 노드별 환경변수나 실행 옵션으로 바꾸면 안 됩니다.
/// 주소는 체인 내부 표준인 소문자 Ethereum 주소로 고정합니다.
pub const FOUNDATION_FEE_ADDRESS: &str = crate::genesis::IEUM_FOUNDATION_ADDRESS;
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
    #[serde(default)]
    staking: StakingState,
    /// 계약 코드·스토리지(프로토콜 v4). 비어 있으면 기존 형식과 같게 저장합니다.
    #[serde(default, skip_serializing_if = "EvmState::is_empty")]
    evm: EvmState,
    /// 계약 거래 영수증(거래 ID → 결과). 조회용이며 상태 해시에는 넣지 않습니다.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    evm_receipts: HashMap<String, EvmReceipt>,
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
            staking: StakingState::default(),
            evm: EvmState::default(),
            evm_receipts: HashMap::new(),
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
            staking: StakingState::default(),
            evm: EvmState::default(),
            evm_receipts: HashMap::new(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_snapshot_with_staking(
        chain_id: u64,
        genesis_commitment: String,
        height: u64,
        block_hash: String,
        balances: HashMap<Address, u128>,
        next_nonces: HashMap<Address, u64>,
        executed_events: HashSet<String>,
        staking: StakingState,
    ) -> Result<Self, String> {
        let mut chain = Self::from_snapshot_with_events(
            chain_id,
            genesis_commitment,
            height,
            block_hash,
            balances,
            next_nonces,
            executed_events,
        )?;
        chain.staking = staking;
        validate_supply_cap(&chain.balances, &chain.staking)?;
        Ok(chain)
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
    pub fn staking(&self) -> &StakingState {
        &self.staking
    }
    pub fn evm(&self) -> &EvmState {
        &self.evm
    }

    pub fn evm_receipt(&self, transaction_id: &str) -> Option<&EvmReceipt> {
        self.evm_receipts.get(transaction_id)
    }

    pub fn evm_receipts(&self) -> &HashMap<String, EvmReceipt> {
        &self.evm_receipts
    }

    /// snapshot·상태 DB에서 복원한 계약 상태를 넣습니다. 상태 해시 비교 전에 호출해야 합니다.
    pub fn restore_evm(&mut self, evm: EvmState, receipts: HashMap<String, EvmReceipt>) {
        self.evm = evm;
        self.evm_receipts = receipts;
    }

    /// 확정된 상태에서 시뮬레이션(eth_call)할 때 쓰는 읽기 전용 상태입니다.
    pub fn execution_state(&self) -> (&HashMap<Address, u128>, &HashMap<Address, u64>, &EvmState) {
        (&self.balances, &self.next_nonces, &self.evm)
    }

    pub fn staking_snapshot(&self) -> StakingState {
        self.staking.clone()
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
                .find(|(_, transaction)| transaction.matches_hash(hash))
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
        let mut staking = self.staking.clone();
        let mut evm = self.evm.clone();
        let mut receipts = HashMap::new();
        apply_transactions(
            self.chain_id,
            &block,
            &mut balances,
            &mut nonces,
            &mut staking,
            &mut evm,
            &mut receipts,
        )?;
        let mut executed_events = self.executed_events.clone();
        apply_system_events(
            &block.system_events,
            block.timestamp,
            &mut balances,
            &mut executed_events,
            &mut staking,
        )?;
        validate_supply_cap(&balances, &staking)?;
        self.blocks.push(block);
        self.balances = balances;
        self.next_nonces = nonces;
        self.executed_events = executed_events;
        self.staking = staking;
        self.evm = evm;
        self.evm_receipts.extend(receipts);
        Ok(self.blocks.last().unwrap())
    }

    /// 확정 전에 블록을 실제로 적용해 보고, 실패하면 이유를 돌려줍니다(상태는 바꾸지 않음).
    pub fn check_block(&self, block: &Block) -> Result<(), String> {
        let mut balances = self.balances.clone();
        let mut nonces = self.next_nonces.clone();
        let mut staking = self.staking.clone();
        let mut evm = self.evm.clone();
        let mut receipts = HashMap::new();
        apply_transactions(
            self.chain_id,
            block,
            &mut balances,
            &mut nonces,
            &mut staking,
            &mut evm,
            &mut receipts,
        )?;
        let mut executed_events = self.executed_events.clone();
        apply_system_events(
            &block.system_events,
            block.timestamp,
            &mut balances,
            &mut executed_events,
            &mut staking,
        )?;
        validate_supply_cap(&balances, &staking)
    }

    /// 제안자가 블록에 넣을 거래를 고릅니다. 앞 거래부터 차례로 적용해 보고 실패하는 거래는 뺍니다.
    /// 반환값: (넣을 거래, 뺀 거래와 이유)
    pub fn select_executable(
        &self,
        height: u64,
        timestamp: u64,
        producer: &str,
        candidates: Vec<Transaction>,
    ) -> (Vec<Transaction>, Vec<(Transaction, String)>) {
        let mut balances = self.balances.clone();
        let mut nonces = self.next_nonces.clone();
        let mut staking = self.staking.clone();
        let mut evm = self.evm.clone();
        let mut selected = Vec::new();
        let mut rejected = Vec::new();
        let mut gas = 0u64;
        for transaction in candidates {
            let tx_gas = crate::evm::block_gas(std::slice::from_ref(&transaction)).unwrap_or(0);
            if gas.saturating_add(tx_gas) > crate::evm::BLOCK_GAS_LIMIT {
                rejected.push((transaction, "블록 가스 한도 초과(다음 블록으로)".into()));
                continue;
            }
            let probe = Block {
                height,
                previous_hash: String::new(),
                timestamp,
                producer: producer.to_string(),
                transactions: vec![transaction.clone()],
                system_events: vec![],
                hash: String::new(),
            };
            let (mut b, mut n, mut s, mut e) = (
                balances.clone(),
                nonces.clone(),
                staking.clone(),
                evm.clone(),
            );
            let mut receipts = HashMap::new();
            match apply_transactions(
                self.chain_id,
                &probe,
                &mut b,
                &mut n,
                &mut s,
                &mut e,
                &mut receipts,
            ) {
                Ok(()) => {
                    (balances, nonces, staking, evm) = (b, n, s, e);
                    gas += tx_gas;
                    selected.push(transaction);
                }
                Err(error) => rejected.push((transaction, error)),
            }
        }
        (selected, rejected)
    }

    /// 저장 파일을 읽은 뒤 제네시스부터 모든 잔액과 nonce를 다시 계산합니다.
    pub fn verify_and_rebuild(&mut self) -> Result<(), String> {
        let mut balances = self.initial_balances.clone();
        let mut nonces = HashMap::new();
        let mut executed_events = HashSet::new();
        let mut staking = StakingState::default();
        let mut evm = EvmState::default();
        let mut receipts = HashMap::new();
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
                    block,
                    &mut balances,
                    &mut nonces,
                    &mut staking,
                    &mut evm,
                    &mut receipts,
                )?;
                apply_system_events(
                    &block.system_events,
                    block.timestamp,
                    &mut balances,
                    &mut executed_events,
                    &mut staking,
                )?;
            }
        }
        validate_supply_cap(&balances, &staking)?;
        self.balances = balances;
        self.next_nonces = nonces;
        self.executed_events = executed_events;
        self.staking = staking;
        self.evm = evm;
        self.evm_receipts = receipts;
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
        if !self.staking.delegations.is_empty()
            || !self.staking.unbonding.is_empty()
            || !self.staking.applied_slashes.is_empty()
            || !self.staking.accruals.is_empty()
            || !self.staking.accrual_days.is_empty()
            || !self.staking.frozen.is_empty()
        {
            hasher.update(b"staking-v1:");
            hasher.update(self.staking.state_bytes());
        }
        if !self.evm.is_empty() {
            self.evm.hash_into(&mut hasher, &self.next_nonces);
        }
        hex::encode(hasher.finalize())
    }
}

fn validate_supply_cap(
    balances: &HashMap<Address, u128>,
    staking: &StakingState,
) -> Result<(), String> {
    let liquid = balances.values().try_fold(0u128, |sum, balance| {
        sum.checked_add(*balance)
            .ok_or("유동 발행량 합계가 u128 범위를 넘습니다.")
    })?;
    let delegated = staking
        .delegations
        .iter()
        .try_fold(0u128, |sum, position| {
            sum.checked_add(position.amount)
                .ok_or("위임 발행량 합계가 u128 범위를 넘습니다.")
        })?;
    let unbonding = staking.unbonding.iter().try_fold(0u128, |sum, entry| {
        sum.checked_add(entry.amount)
            .ok_or("해제 대기 발행량 합계가 u128 범위를 넘습니다.")
    })?;
    let total = liquid
        .checked_add(delegated)
        .and_then(|value| value.checked_add(unbonding))
        .and_then(|value| value.checked_add(staking.total_accruals()))
        .ok_or("총발행량 합계가 u128 범위를 넘습니다.")?;
    if total > crate::genesis::IEUM_MAX_SUPPLY {
        return Err("총발행량이 IEUM 최대 발행량 210,000,000을 넘습니다.".into());
    }
    Ok(())
}

fn apply_system_events(
    events: &[crate::scheduled_event::ScheduledEvent],
    block_timestamp: u64,
    balances: &mut HashMap<Address, u128>,
    executed: &mut HashSet<String>,
    staking: &mut StakingState,
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
            ScheduledEventAction::HolderDailyReward { payments, .. } => {
                for payment in payments {
                    transfer_from_foundation(balances, &payment.address, payment.amount)?;
                }
            }
            ScheduledEventAction::DoubleVoteSlash {
                evidence,
                penalty_bps,
            } => {
                let slashed =
                    staking.slash(&event.id, &evidence.first.validator_id, *penalty_bps)?;
                credit_balance(
                    balances,
                    FOUNDATION_FEE_ADDRESS,
                    slashed,
                    "재단 페널티 잔액이 u128 범위를 넘습니다.",
                )?;
            }
            ScheduledEventAction::DelegationDailyReward { payments, .. } => {
                for payment in payments {
                    transfer_from_foundation(balances, &payment.address, payment.amount)?;
                }
            }
            ScheduledEventAction::AccountFreeze {
                case_id, addresses, ..
            } => {
                for address in addresses {
                    staking.frozen.insert(
                        normalize_address(address),
                        crate::staking::FreezeRecord {
                            case_id: case_id.clone(),
                            frozen_at: block_timestamp,
                            expires_at: block_timestamp
                                .saturating_add(crate::scheduled_event::FREEZE_MAX_SECONDS),
                        },
                    );
                }
            }
            ScheduledEventAction::AccountUnfreeze { case_id, addresses } => {
                for address in addresses {
                    let address = normalize_address(address);
                    match staking.frozen.get(&address) {
                        Some(record) if &record.case_id == case_id => {
                            staking.frozen.remove(&address);
                        }
                        _ => {
                            return Err(format!(
                                "사건 {case_id}로 동결된 주소가 아닙니다: {address}"
                            ));
                        }
                    }
                }
            }
            ScheduledEventAction::AccountRecovery {
                case_id,
                from,
                payments,
                release,
            } => {
                let from = normalize_address(from);
                let record = staking
                    .frozen
                    .get(&from)
                    .filter(|record| &record.case_id == case_id)
                    .ok_or_else(|| format!("사건 {case_id}로 동결된 주소가 아닙니다: {from}"))?;
                if block_timestamp
                    < record
                        .frozen_at
                        .saturating_add(crate::scheduled_event::RECOVERY_NOTICE_SECONDS)
                {
                    return Err("동결 후 공개 유예(7일)가 지나야 복구할 수 있습니다.".into());
                }
                if block_timestamp >= record.expires_at {
                    return Err("동결 기간이 끝나 복구할 수 없습니다.".into());
                }
                let total = payments.iter().try_fold(0u128, |sum, payment| {
                    sum.checked_add(payment.amount)
                        .ok_or("복구 금액 합계가 u128 범위를 넘습니다.")
                })?;
                let available = balances.get(&from).copied().unwrap_or(0);
                if available < total {
                    return Err("복구 금액이 동결 계정 잔액보다 큽니다.".into());
                }
                balances.insert(from.clone(), available - total);
                for payment in payments {
                    credit_balance(
                        balances,
                        &normalize_address(&payment.address),
                        payment.amount,
                        "복구 수령 잔액이 u128 범위를 넘습니다.",
                    )?;
                }
                if *release {
                    staking.frozen.remove(&from);
                }
            }
            ScheduledEventAction::NodeServiceDailyReward {
                epoch,
                payments,
                accruals,
                expired,
                ..
            } => {
                for payment in payments {
                    transfer_from_foundation(balances, &payment.address, payment.amount)?;
                }
                for entry in expired {
                    let address = normalize_address(&entry.address);
                    staking.expire_accrual(&address, entry.amount)?;
                    credit_balance(
                        balances,
                        FOUNDATION_FEE_ADDRESS,
                        entry.amount,
                        "재단 잔액이 u128 범위를 넘습니다.",
                    )?;
                }
                // 적립은 재단에서 빼서 잠긴 적립 잔액으로만 옮깁니다. 일반 잔액에는 더하지 않습니다.
                for accrual in accruals {
                    let foundation = balances.get(FOUNDATION_FEE_ADDRESS).copied().unwrap_or(0);
                    if foundation < accrual.amount {
                        return Err("노드 적립을 실행할 재단 잔액이 부족합니다.".into());
                    }
                    balances.insert(FOUNDATION_FEE_ADDRESS.into(), foundation - accrual.amount);
                    staking.add_accrual(
                        &normalize_address(&accrual.address),
                        accrual.amount,
                        *epoch,
                    )?;
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

#[allow(clippy::too_many_arguments)]
fn apply_transactions(
    chain_id: u64,
    block: &Block,
    balances: &mut HashMap<Address, u128>,
    nonces: &mut HashMap<Address, u64>,
    staking: &mut StakingState,
    evm: &mut EvmState,
    receipts: &mut HashMap<String, EvmReceipt>,
) -> Result<(), String> {
    let producer = block.producer.as_str();
    let block_timestamp = block.timestamp;
    let mut block_gas = 0u64;
    // 블록 하나를 원자적으로 처리하기 위해 복제된 상태에 먼저 적용합니다.
    // 하나라도 실패하면 호출자가 원래 balances/nonces를 그대로 유지합니다.
    for (index, tx) in block.transactions.iter().enumerate() {
        if tx.signature.starts_with("ethraw:") {
            crate::raw_transaction::verify_embedded(tx, chain_id)?;
        } else {
            verify_transaction(tx)?;
        }
        if staking.is_frozen(&normalize_address(&tx.from), block_timestamp) {
            return Err("동결된 계정은 거래를 보낼 수 없습니다(해킹 피해 대응 중).".into());
        }
        let is_evm = crate::evm::is_evm_transaction(tx, evm);
        let is_staking = matches!(
            &tx.action,
            TransactionAction::Delegate { .. }
                | TransactionAction::Undelegate { .. }
                | TransactionAction::ClaimUnbonded
                | TransactionAction::GrantAccrual { .. }
        );
        if is_staking && normalize_address(&tx.to) != crate::staking::STAKING_SYSTEM_ADDRESS {
            return Err("위임 거래의 수신 주소는 스테이킹 시스템 주소여야 합니다.".into());
        }
        if !is_staking && normalize_address(&tx.to) == crate::staking::STAKING_SYSTEM_ADDRESS {
            return Err(
                "스테이킹 시스템 주소로 일반 송금할 수 없습니다. 위임 calldata가 필요합니다."
                    .into(),
            );
        }
        if matches!(&tx.action, TransactionAction::ClaimUnbonded) && tx.amount != 0 {
            return Err("해제 청구 거래의 amount는 0이어야 합니다.".into());
        }
        if tx.amount == 0 && !is_evm && !matches!(&tx.action, TransactionAction::ClaimUnbonded) {
            return Err("송금액은 0보다 커야 합니다.".into());
        }
        let expected_nonce = nonces.get(&tx.from).copied().unwrap_or(0);
        if tx.nonce != expected_nonce {
            return Err(format!(
                "nonce 오류: 기대 {expected_nonce}, 입력 {}",
                tx.nonce
            ));
        }
        if is_evm {
            let call =
                crate::raw_transaction::evm_call(tx)?.ok_or("EVM 거래 내용을 읽지 못했습니다.")?;
            block_gas = block_gas
                .checked_add(call.gas_limit)
                .filter(|gas| *gas <= crate::evm::BLOCK_GAS_LIMIT)
                .ok_or("블록 가스 한도(30,000,000)를 넘었습니다.")?;
            let context = crate::evm::BlockContext {
                chain_id,
                height: block.height,
                timestamp: block_timestamp,
                producer,
            };
            let (mut receipt, fee) =
                crate::evm::execute_transaction(context, &call, balances, nonces, evm)?;
            receipt.height = block.height;
            receipt.index = index;
            receipts.insert(tx.id(), receipt);
            distribute_fee(balances, producer, fee)?;
            nonces.insert(tx.from.clone(), expected_nonce + 1);
            continue;
        }
        let debit_amount = if matches!(
            &tx.action,
            TransactionAction::Transfer
                | TransactionAction::Delegate { .. }
                | TransactionAction::GrantAccrual { .. }
        ) {
            tx.amount
        } else {
            0
        };
        let total = debit_amount
            .checked_add(tx.fee)
            .ok_or("송금액과 수수료 합계가 너무 큽니다.")?;
        let sender = balances.get(&tx.from).copied().unwrap_or(0);
        if sender < total {
            return Err("수수료를 포함한 잔액이 부족합니다.".into());
        }
        balances.insert(tx.from.clone(), sender - total);
        match &tx.action {
            TransactionAction::Transfer => credit_balance(
                balances,
                &tx.to,
                tx.amount,
                "받는 계정 잔액이 u128 범위를 넘습니다.",
            )?,
            TransactionAction::Delegate { validator } => {
                staking.delegate_at(&tx.from, validator, tx.amount, block_timestamp)?
            }
            TransactionAction::Undelegate { validator } => {
                staking.undelegate(&tx.from, validator, tx.amount, block_timestamp)?;
            }
            TransactionAction::ClaimUnbonded => {
                let claimed = staking.claim(&tx.from, block_timestamp)?;
                credit_balance(
                    balances,
                    &tx.from,
                    claimed,
                    "해제 청구 잔액이 u128 범위를 넘습니다.",
                )?;
            }
            TransactionAction::GrantAccrual { recipient } => {
                let recipient = normalize_address(recipient);
                let bond_room = crate::node_emission::SERVICE_BOND.saturating_sub(
                    crate::node_emission::service_bond_of(staking, &recipient, block_timestamp),
                );
                staking.grant_accrual(
                    &recipient,
                    tx.amount,
                    block_timestamp / crate::node_emission::REWARD_EPOCH_SECONDS,
                    crate::node_emission::ACCRUAL_DAILY_CAP,
                    bond_room,
                )?;
            }
            TransactionAction::Evm => unreachable!("EVM 거래는 위에서 처리합니다."),
        }
        distribute_fee(balances, producer, tx.fee)?;
        nonces.insert(tx.from.clone(), expected_nonce + 1);
    }
    Ok(())
}

/// 재단 몫을 먼저 내림 계산하고 나머지 전부를 생성자에게 지급합니다.
/// 따라서 아주 작은 수수료의 나머지도 소각되거나 유실되지 않습니다.
fn distribute_fee(
    balances: &mut HashMap<Address, u128>,
    producer: &str,
    fee: u128,
) -> Result<(), String> {
    let foundation_fee = fee
        .checked_mul(FOUNDATION_FEE_BPS)
        .ok_or("재단 수수료 계산이 u128 범위를 넘습니다.")?
        / FEE_BPS_DENOMINATOR;
    let producer_fee = fee - foundation_fee;
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
    )
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

#[cfg(test)]
mod staking_transaction_tests {
    use super::*;
    use crate::staking::{MINIMUM_DELEGATION, STAKING_SYSTEM_ADDRESS, UNBONDING_SECONDS};
    use crate::{TransactionAction, Wallet};

    #[test]
    fn delegation_is_locked_then_claimed_after_wait() {
        let alice = Wallet::from_seed([7; 32]);
        let validator = Wallet::from_seed([8; 32]).address();
        let producer = Wallet::from_seed([9; 32]).address();
        let initial = 20 * MINIMUM_DELEGATION;
        let mut chain = Blockchain::new(vec![(alice.address(), initial)]);
        let delegate = alice.sign_action(
            STAKING_SYSTEM_ADDRESS.into(),
            10 * MINIMUM_DELEGATION,
            1,
            0,
            TransactionAction::Delegate {
                validator: validator.clone(),
            },
        );
        chain.add_block(vec![delegate], producer.clone()).unwrap();
        assert_eq!(
            chain.balance_of(&alice.address()),
            10 * MINIMUM_DELEGATION - 1
        );
        assert_eq!(
            chain.staking().delegated_to(&alice.address(), &validator),
            10 * MINIMUM_DELEGATION
        );
        let undelegate = alice.sign_action(
            STAKING_SYSTEM_ADDRESS.into(),
            4 * MINIMUM_DELEGATION,
            1,
            1,
            TransactionAction::Undelegate {
                validator: validator.clone(),
            },
        );
        chain.add_block(vec![undelegate], producer.clone()).unwrap();
        let claim_too_early = alice.sign_action(
            STAKING_SYSTEM_ADDRESS.into(),
            0,
            1,
            2,
            TransactionAction::ClaimUnbonded,
        );
        assert!(
            chain
                .add_block(vec![claim_too_early], producer.clone())
                .is_err()
        );
        let entry = chain.staking().unbonding[0].clone();
        assert!(entry.release_at >= UNBONDING_SECONDS);
        let claim = alice.sign_action(
            STAKING_SYSTEM_ADDRESS.into(),
            0,
            1,
            2,
            TransactionAction::ClaimUnbonded,
        );
        let previous = chain.blocks.last().unwrap();
        let claim_block = Block::new(
            previous.height + 1,
            previous.hash.clone(),
            entry.release_at,
            producer,
            vec![claim],
        );
        chain.apply_block(claim_block).unwrap();
        assert_eq!(
            chain.balance_of(&alice.address()),
            14 * MINIMUM_DELEGATION - 3
        );
    }
}

#[cfg(test)]
mod accrual_tests {
    use super::*;
    use crate::node_emission::{NodeServiceAttestation, REWARD_EPOCH_SECONDS, service_event_id};
    use crate::scheduled_event::{EventPayment, ScheduledEvent, ScheduledEventAction};
    use crate::wallet::Wallet;

    const IEUM: u128 = 10u128.pow(18);
    const ADDRESS: &str = "0x1111111111111111111111111111111111111111";

    fn reward_event(
        epoch: u64,
        accruals: Vec<EventPayment>,
        expired: Vec<EventPayment>,
    ) -> ScheduledEvent {
        let attestations = if accruals.is_empty() {
            Vec::new()
        } else {
            (1..=3)
                .map(|seed| {
                    let wallet = Wallet::from_seed([seed; 32]);
                    let mut proof = NodeServiceAttestation {
                        peer_id: "peer".into(),
                        reward_address: ADDRESS.into(),
                        epoch,
                        observed_at: epoch * REWARD_EPOCH_SECONDS,
                        uptime_bps: 9_000,
                        network_group: "ipv4:203.0.113.0/24".into(),
                        validator_id: wallet.address(),
                        signature_hex: String::new(),
                    };
                    proof.signature_hex = wallet.sign_bytes(&proof.bytes_to_sign());
                    proof
                })
                .collect()
        };
        ScheduledEvent {
            id: service_event_id(epoch),
            execute_at: 1,
            action: ScheduledEventAction::NodeServiceDailyReward {
                snapshot_height: 0,
                epoch,
                attestations,
                payments: vec![],
                accruals,
                expired,
            },
        }
    }

    #[test]
    fn accrual_is_locked_not_spendable_and_expiry_returns_it() {
        let mut chain = Blockchain::new(vec![(FOUNDATION_FEE_ADDRESS.into(), 1_000 * IEUM)]);
        let hash_before = chain.state_hash();
        let amount = 3 * IEUM / 10;
        let event = reward_event(
            10,
            vec![EventPayment {
                address: ADDRESS.into(),
                amount,
            }],
            vec![],
        );
        let block = Block::new(1, chain.tip_hash().into(), 1, "producer".into(), vec![])
            .with_system_events(vec![event]);
        chain.apply_block(block).unwrap();
        assert_eq!(chain.balance_of(ADDRESS), 0);
        assert_eq!(chain.staking().locked_accrual_of(ADDRESS), amount);
        assert_eq!(
            chain.balance_of(FOUNDATION_FEE_ADDRESS),
            1_000 * IEUM - amount
        );
        assert_eq!(chain.staking().accruals[ADDRESS].last_epoch, 10);
        assert_ne!(chain.state_hash(), hash_before);

        let wrong = reward_event(
            80,
            vec![],
            vec![EventPayment {
                address: ADDRESS.into(),
                amount: amount + 1,
            }],
        );
        let block = Block::new(2, chain.tip_hash().into(), 2, "producer".into(), vec![])
            .with_system_events(vec![wrong]);
        assert!(chain.apply_block(block).is_err());

        let expiry = reward_event(
            80,
            vec![],
            vec![EventPayment {
                address: ADDRESS.into(),
                amount,
            }],
        );
        let block = Block::new(2, chain.tip_hash().into(), 2, "producer".into(), vec![])
            .with_system_events(vec![expiry]);
        chain.apply_block(block).unwrap();
        assert_eq!(chain.staking().locked_accrual_of(ADDRESS), 0);
        assert_eq!(chain.balance_of(FOUNDATION_FEE_ADDRESS), 1_000 * IEUM);
    }

    fn event(id: &str, action: ScheduledEventAction) -> ScheduledEvent {
        ScheduledEvent {
            id: id.into(),
            execute_at: 1,
            action,
        }
    }

    fn apply(
        chain: &mut Blockchain,
        timestamp: u64,
        txs: Vec<Transaction>,
        events: Vec<ScheduledEvent>,
    ) -> Result<(), String> {
        let block = Block::new(
            chain.tip_height() + 1,
            chain.tip_hash().into(),
            timestamp,
            "producer".into(),
            txs,
        )
        .with_system_events(events);
        chain.apply_block(block).map(|_| ())
    }

    #[test]
    fn web_grant_is_locked_and_shares_daily_cap_with_node_accrual() {
        let operator = Wallet::from_seed([9; 32]);
        let mut chain = Blockchain::new(vec![
            (FOUNDATION_FEE_ADDRESS.into(), 1_000 * IEUM),
            (operator.address(), 10 * IEUM),
        ]);
        let day = 20 * REWARD_EPOCH_SECONDS;
        let grant = |amount, nonce| {
            operator.sign_action(
                crate::staking::STAKING_SYSTEM_ADDRESS.into(),
                amount,
                0,
                nonce,
                TransactionAction::GrantAccrual {
                    recipient: ADDRESS.into(),
                },
            )
        };
        apply(&mut chain, day + 10, vec![grant(IEUM / 10, 0)], vec![]).unwrap();
        assert_eq!(chain.balance_of(ADDRESS), 0);
        assert_eq!(chain.staking().locked_accrual_of(ADDRESS), IEUM / 10);
        assert_eq!(chain.balance_of(&operator.address()), 10 * IEUM - IEUM / 10);
        // 오늘 남은 한도 0.2를 넘는 웹 적립은 거절됩니다.
        assert!(apply(&mut chain, day + 20, vec![grant(3 * IEUM / 10, 1)], vec![]).is_err());
        // 노드 적립은 같은 날 기록에 합산돼 이후 웹 적립을 막습니다.
        let node = reward_event(
            20,
            vec![EventPayment {
                address: ADDRESS.into(),
                amount: 2 * IEUM / 10,
            }],
            vec![],
        );
        apply(&mut chain, day + 30, vec![], vec![node]).unwrap();
        assert_eq!(
            chain.staking().accrual_used_in(ADDRESS, 20, 3 * IEUM / 10),
            3 * IEUM / 10
        );
        assert!(apply(&mut chain, day + 40, vec![grant(1, 1)], vec![]).is_err());
        // 다음 날은 다시 한도가 생깁니다.
        apply(
            &mut chain,
            day + REWARD_EPOCH_SECONDS,
            vec![grant(3 * IEUM / 10, 1)],
            vec![],
        )
        .unwrap();
        assert_eq!(chain.staking().locked_accrual_of(ADDRESS), 6 * IEUM / 10);
    }

    #[test]
    fn node_accrual_first_blocks_same_day_web_grant() {
        let operator = Wallet::from_seed([8; 32]);
        let mut chain = Blockchain::new(vec![
            (FOUNDATION_FEE_ADDRESS.into(), 1_000 * IEUM),
            (operator.address(), IEUM),
        ]);
        let node = reward_event(
            30,
            vec![EventPayment {
                address: ADDRESS.into(),
                amount: 3 * IEUM / 10,
            }],
            vec![],
        );
        let day = 30 * REWARD_EPOCH_SECONDS;
        apply(&mut chain, day + 1, vec![], vec![node]).unwrap();
        // v5 노드 적립만으로는 웹 기록이 생기지 않아 기존 상태 형식이 유지됩니다.
        assert!(chain.staking().accrual_days.is_empty());
        let tx = operator.sign_action(
            crate::staking::STAKING_SYSTEM_ADDRESS.into(),
            1,
            0,
            0,
            TransactionAction::GrantAccrual {
                recipient: ADDRESS.into(),
            },
        );
        assert!(apply(&mut chain, day + 2, vec![tx], vec![]).is_err());
    }

    #[test]
    fn frozen_account_cannot_send_and_recovery_waits_for_notice() {
        let hacker = Wallet::from_seed([7; 32]);
        let victim = "0x2222222222222222222222222222222222222222";
        let mut chain = Blockchain::new(vec![(hacker.address(), 50 * IEUM)]);
        let hash_before = chain.state_hash();
        let start = 1_000;
        apply(
            &mut chain,
            start,
            vec![],
            vec![event(
                "freeze-case-1",
                ScheduledEventAction::AccountFreeze {
                    case_id: "case-1".into(),
                    addresses: vec![hacker.address()],
                    reason: "test incident".into(),
                },
            )],
        )
        .unwrap();
        assert_ne!(chain.state_hash(), hash_before);
        let send = hacker.sign_transfer(victim.into(), IEUM, 0, 0);
        assert!(apply(&mut chain, start + 1, vec![send.clone()], vec![]).is_err());

        let recovery = |id: &str| {
            event(
                id,
                ScheduledEventAction::AccountRecovery {
                    case_id: "case-1".into(),
                    from: hacker.address(),
                    payments: vec![EventPayment {
                        address: victim.into(),
                        amount: 40 * IEUM,
                    }],
                    release: true,
                },
            )
        };
        assert!(
            apply(
                &mut chain,
                start + 60,
                vec![],
                vec![recovery("recover-early")]
            )
            .is_err()
        );
        let after_notice = start + crate::scheduled_event::RECOVERY_NOTICE_SECONDS;
        apply(
            &mut chain,
            after_notice,
            vec![],
            vec![recovery("recover-case-1")],
        )
        .unwrap();
        assert_eq!(chain.balance_of(victim), 40 * IEUM);
        assert_eq!(chain.balance_of(&hacker.address()), 10 * IEUM);
        // release=true면 동결이 풀려 남은 잔액을 다시 쓸 수 있습니다.
        apply(&mut chain, after_notice + 1, vec![send], vec![]).unwrap();
    }

    #[test]
    fn freeze_expires_and_unfreeze_requires_same_case() {
        let hacker = Wallet::from_seed([6; 32]);
        let mut chain = Blockchain::new(vec![(hacker.address(), 5 * IEUM)]);
        let freeze = event(
            "freeze-a",
            ScheduledEventAction::AccountFreeze {
                case_id: "a".into(),
                addresses: vec![hacker.address()],
                reason: "test".into(),
            },
        );
        apply(&mut chain, 100, vec![], vec![freeze]).unwrap();
        let wrong = event(
            "unfreeze-b",
            ScheduledEventAction::AccountUnfreeze {
                case_id: "b".into(),
                addresses: vec![hacker.address()],
            },
        );
        assert!(apply(&mut chain, 101, vec![], vec![wrong]).is_err());
        let late = 100 + crate::scheduled_event::FREEZE_MAX_SECONDS;
        let send = hacker.sign_transfer(ADDRESS.into(), IEUM, 0, 0);
        apply(&mut chain, late, vec![send], vec![]).unwrap();
        assert_eq!(chain.balance_of(ADDRESS), IEUM);
    }
}

//! EVM(revm) 실행 계층입니다.
//!
//! 합의는 블록 순서만 정하고, 확정 블록을 적용할 때 계약 배포·호출 거래를 여기서 실행합니다.
//! 계정 잔액과 nonce는 기존 `balances`/`next_nonces` 맵을 그대로 쓰고, 코드와 스토리지만
//! [`EvmState`]에 따로 둡니다. 모든 노드가 같은 결과를 내도록 revm 버전은 Cargo.lock으로 고정하고
//! 블록 환경(시각·높이·coinbase·prevrandao)을 블록 내용에서만 만듭니다.

use crate::model::{Address, Transaction, TransactionAction};
use revm::context::{BlockEnv, CfgEnv, TxEnv};
use revm::context_interface::result::{ExecutionResult, Output};
use revm::database_interface::{DBErrorMarker, DatabaseRef, WrapDatabaseRef};
use revm::primitives::hardfork::SpecId;
use revm::primitives::{Address as EvmAddress, B256, Bytes, TxKind, U256, keccak256};
use revm::state::{AccountInfo, Bytecode};
use revm::{Context, ExecuteEvm, MainBuilder, MainContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

/// 계약 실행을 켜는 프로토콜 버전입니다. `upgrades.json`에 이 버전의 활성 높이를 적어야 켜집니다.
pub const EVM_PROTOCOL_VERSION: u32 = 4;
/// 블록 하나에 담을 수 있는 EVM 거래 gasLimit 합계입니다.
pub const BLOCK_GAS_LIMIT: u64 = 30_000_000;
/// eth_call·eth_estimateGas 시뮬레이션 기본 가스입니다.
pub const CALL_GAS_LIMIT: u64 = BLOCK_GAS_LIMIT;

/// 계약 코드와 스토리지입니다. BTreeMap이라 직렬화·해시 순서가 항상 같습니다.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvmState {
    /// 계약 주소(소문자 0x) → 런타임 바이트코드 hex
    #[serde(default)]
    pub code: BTreeMap<String, String>,
    /// 계약 주소 → (슬롯 0x+64hex → 값 0x+64hex). 0 값은 저장하지 않습니다.
    #[serde(default)]
    pub storage: BTreeMap<String, BTreeMap<String, String>>,
}

impl EvmState {
    pub fn is_empty(&self) -> bool {
        self.code.is_empty() && self.storage.is_empty()
    }

    /// 상태 해시에 넣을 결정적 바이트입니다. 계약 nonce도 CREATE 주소를 바꾸므로 함께 넣습니다.
    pub fn hash_into(&self, hasher: &mut Sha256, nonces: &HashMap<Address, u64>) {
        hasher.update(b"evm-v1:");
        for (address, code) in &self.code {
            hasher.update(address.as_bytes());
            hasher.update(code.as_bytes());
            hasher.update(nonces.get(address).copied().unwrap_or(0).to_be_bytes());
        }
        for (address, slots) in &self.storage {
            hasher.update(b"storage:");
            hasher.update(address.as_bytes());
            for (slot, value) in slots {
                hasher.update(slot.as_bytes());
                hasher.update(value.as_bytes());
            }
        }
    }

    pub fn storage_at(&self, address: &str, slot: U256) -> U256 {
        self.storage
            .get(&normalize(address))
            .and_then(|slots| slots.get(&word_hex(slot)))
            .and_then(|value| U256::from_str_radix(value.trim_start_matches("0x"), 16).ok())
            .unwrap_or_default()
    }

    pub fn code_of(&self, address: &str) -> Option<&String> {
        self.code.get(&normalize(address))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvmLog {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
}

/// 계약 거래 실행 결과입니다. 상태가 아니라 조회용이라 상태 해시에는 넣지 않습니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvmReceipt {
    pub height: u64,
    pub index: usize,
    pub status: bool,
    pub gas_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_address: Option<String>,
    #[serde(default)]
    pub logs: Vec<EvmLog>,
    /// 실패 시 revert 데이터, 성공 시 반환값(배포는 빈 값)
    #[serde(default)]
    pub output: String,
}

/// 블록 단위 실행 환경입니다.
#[derive(Clone, Copy, Debug)]
pub struct BlockContext<'a> {
    pub chain_id: u64,
    pub height: u64,
    pub timestamp: u64,
    pub producer: &'a str,
}

/// raw legacy 거래에서 꺼낸 EVM 호출 내용입니다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmCall {
    pub caller: EvmAddress,
    pub to: Option<EvmAddress>,
    pub value: u128,
    pub data: Vec<u8>,
    pub gas_limit: u64,
    pub gas_price: u128,
    pub nonce: u64,
}

/// 이 거래를 EVM으로 실행해야 하는지 판단합니다.
/// 계약 배포·호출 거래와, raw 거래로 코드가 있는 주소에 보내는 송금이 대상입니다.
pub fn is_evm_transaction(tx: &Transaction, evm: &EvmState) -> bool {
    matches!(tx.action, TransactionAction::Evm)
        || (tx.action.is_transfer()
            && tx.signature.starts_with("ethraw:")
            && evm.code.contains_key(&normalize(&tx.to)))
}

/// 블록 하나에 담긴 EVM 거래 gasLimit 합계입니다.
pub fn block_gas(transactions: &[Transaction]) -> Result<u64, String> {
    let mut total = 0u64;
    for tx in transactions {
        if let Some(call) = crate::raw_transaction::evm_call(tx)? {
            total = total
                .checked_add(call.gas_limit)
                .ok_or("블록 가스 합계가 너무 큽니다.")?;
        }
    }
    Ok(total)
}

/// 읽기 전용 상태 뷰입니다. revm은 이 뷰에서 읽기만 하고, 변경분은 [`apply_changes`]가 반영합니다.
struct StateView<'a> {
    balances: &'a HashMap<Address, u128>,
    nonces: &'a HashMap<Address, u64>,
    evm: &'a EvmState,
}

#[derive(Debug)]
struct ViewError(String);

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ViewError {}
impl DBErrorMarker for ViewError {}

impl DatabaseRef for StateView<'_> {
    type Error = ViewError;

    fn basic_ref(&self, address: EvmAddress) -> Result<Option<AccountInfo>, Self::Error> {
        let key = address_key(address);
        let balance = self.balances.get(&key).copied().unwrap_or(0);
        let nonce = self.nonces.get(&key).copied().unwrap_or(0);
        let code = match self.evm.code.get(&key) {
            Some(code) => Some(
                hex::decode(code.trim_start_matches("0x"))
                    .map_err(|_| ViewError(format!("{key} 계약 코드가 손상되었습니다.")))?,
            ),
            None => None,
        };
        if balance == 0 && nonce == 0 && code.is_none() {
            return Ok(None);
        }
        let mut info = AccountInfo {
            balance: U256::from(balance),
            nonce,
            ..Default::default()
        };
        if let Some(code) = code {
            info = info.with_code(Bytecode::new_raw(Bytes::from(code)));
        }
        Ok(Some(info))
    }

    fn code_by_hash_ref(&self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
        // basic_ref가 코드를 함께 돌려주므로 해시로 다시 찾을 일이 없습니다.
        Ok(Bytecode::default())
    }

    fn storage_ref(&self, address: EvmAddress, index: U256) -> Result<U256, Self::Error> {
        Ok(self.evm.storage_at(&address_key(address), index))
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        // BLOCKHASH는 지원하지 않습니다(항상 0). 계약 안 난수 용도를 막기 위한 결정입니다.
        Ok(B256::ZERO)
    }
}

/// 확정 블록 안의 EVM 거래 하나를 실행하고 상태에 반영합니다.
///
/// Err는 거래 자체가 잘못된 경우(nonce·잔액·가스 부족 등)이며 블록 전체를 거부해야 합니다.
/// 계약이 revert·halt한 경우는 Ok(status=false)이며, 수수료만 내고 상태는 되돌립니다.
/// 반환값의 두 번째 값은 실제 낸 수수료(gasUsed × gasPrice)입니다.
pub fn execute_transaction(
    ctx: BlockContext<'_>,
    call: &EvmCall,
    balances: &mut HashMap<Address, u128>,
    nonces: &mut HashMap<Address, u64>,
    evm: &mut EvmState,
) -> Result<(EvmReceipt, u128), String> {
    let beneficiary = producer_address(ctx.producer);
    let beneficiary_key = address_key(beneficiary);
    let beneficiary_existed = balances.contains_key(&beneficiary_key);
    let beneficiary_before = balances.get(&beneficiary_key).copied().unwrap_or(0);
    let (result, state) = run(ctx, call, balances, nonces, evm, false)?;
    apply_changes(&state, balances, nonces, evm)?;
    // revm이 coinbase에 넣어 준 수수료를 다시 빼서, 호출자가 기존 규칙(생산자·재단)으로 나눕니다.
    let beneficiary_after = balances.get(&beneficiary_key).copied().unwrap_or(0);
    let fee = beneficiary_after
        .checked_sub(beneficiary_before)
        .ok_or("EVM 수수료 계산이 잘못되었습니다.")?;
    if beneficiary_existed {
        balances.insert(beneficiary_key, beneficiary_before);
    } else {
        balances.remove(&beneficiary_key);
    }
    let receipt = receipt_from(&result, 0, 0);
    Ok((receipt, fee))
}

/// 상태를 바꾸지 않는 시뮬레이션(eth_call, eth_estimateGas)입니다.
/// 잔액·nonce 검사를 끄므로 아무 주소에서나 읽기 호출을 할 수 있습니다.
pub fn simulate(
    ctx: BlockContext<'_>,
    call: &EvmCall,
    balances: &HashMap<Address, u128>,
    nonces: &HashMap<Address, u64>,
    evm: &EvmState,
) -> Result<SimulationResult, String> {
    let (result, _) = run(ctx, call, balances, nonces, evm, true)?;
    Ok(SimulationResult {
        success: result.is_success(),
        gas_used: result.tx_gas_used(),
        gas_spent: result.gas().total_gas_spent(),
        output: match &result {
            ExecutionResult::Success { output, .. } => output.data().to_vec(),
            ExecutionResult::Revert { output, .. } => output.to_vec(),
            ExecutionResult::Halt { .. } => Vec::new(),
        },
        halted: result.is_halt(),
    })
}

#[derive(Clone, Debug)]
pub struct SimulationResult {
    pub success: bool,
    pub gas_used: u64,
    pub gas_spent: u64,
    pub output: Vec<u8>,
    pub halted: bool,
}

type RunOutput = (ExecutionResult, revm::state::EvmState);

fn run(
    ctx: BlockContext<'_>,
    call: &EvmCall,
    balances: &HashMap<Address, u128>,
    nonces: &HashMap<Address, u64>,
    evm: &EvmState,
    simulation: bool,
) -> Result<RunOutput, String> {
    let view = StateView {
        balances,
        nonces,
        evm,
    };
    let mut cfg = CfgEnv::new_with_spec(SpecId::PRAGUE);
    cfg.chain_id = ctx.chain_id;
    if simulation {
        cfg.disable_nonce_check = true;
        cfg.disable_balance_check = true;
    }
    let block = BlockEnv {
        number: U256::from(ctx.height),
        beneficiary: producer_address(ctx.producer),
        timestamp: U256::from(ctx.timestamp),
        gas_limit: BLOCK_GAS_LIMIT,
        basefee: 0,
        difficulty: U256::ZERO,
        prevrandao: Some(B256::ZERO),
        ..Default::default()
    };
    let tx = TxEnv::builder()
        .tx_type(Some(0))
        .caller(call.caller)
        .gas_limit(call.gas_limit)
        .gas_price(if simulation { 0 } else { call.gas_price })
        .kind(match call.to {
            Some(to) => TxKind::Call(to),
            None => TxKind::Create,
        })
        .value(U256::from(call.value))
        .data(Bytes::from(call.data.clone()))
        .nonce(call.nonce)
        .chain_id(Some(ctx.chain_id))
        .build_fill();
    let mut machine = Context::mainnet()
        .with_db(WrapDatabaseRef(view))
        .with_cfg(cfg)
        .with_block(block)
        .build_mainnet();
    let outcome = machine
        .transact(tx)
        .map_err(|error| format!("EVM 거래가 유효하지 않습니다: {error}"))?;
    Ok((outcome.result, outcome.state))
}

/// revm이 돌려준 계정 변경분을 기존 상태 맵에 반영합니다.
fn apply_changes(
    state: &revm::state::EvmState,
    balances: &mut HashMap<Address, u128>,
    nonces: &mut HashMap<Address, u64>,
    evm: &mut EvmState,
) -> Result<(), String> {
    // 주소 순서로 반영해 해시맵 순회 순서와 무관하게 결과가 같게 합니다.
    let mut accounts: Vec<_> = state.iter().filter(|(_, a)| a.is_touched()).collect();
    accounts.sort_by_key(|(address, _)| **address);
    for (address, account) in accounts {
        let key = address_key(*address);
        if account.is_selfdestructed() {
            evm.code.remove(&key);
            evm.storage.remove(&key);
            nonces.remove(&key);
        }
        let balance = u128::try_from(account.info.balance)
            .map_err(|_| format!("{key} 잔액이 u128 범위를 넘습니다."))?;
        match balances.get(&key) {
            Some(old) if *old != balance => {
                balances.insert(key.clone(), balance);
            }
            None if balance != 0 => {
                balances.insert(key.clone(), balance);
            }
            _ => {}
        }
        if account.is_selfdestructed() {
            continue;
        }
        let old_nonce = nonces.get(&key).copied().unwrap_or(0);
        if account.info.nonce != old_nonce {
            nonces.insert(key.clone(), account.info.nonce);
        }
        if account.is_created()
            && let Some(code) = &account.info.code
            && !code.is_empty()
        {
            evm.code.insert(
                key.clone(),
                format!("0x{}", hex::encode(code.original_bytes())),
            );
        }
        for (slot, value) in account.changed_storage_slots() {
            let slots = evm.storage.entry(key.clone()).or_default();
            if value.present_value.is_zero() {
                slots.remove(&word_hex(*slot));
            } else {
                slots.insert(word_hex(*slot), word_hex(value.present_value));
            }
            if slots.is_empty() {
                evm.storage.remove(&key);
            }
        }
    }
    Ok(())
}

fn receipt_from(result: &ExecutionResult, height: u64, index: usize) -> EvmReceipt {
    let contract_address = match result {
        ExecutionResult::Success {
            output: Output::Create(_, Some(address)),
            ..
        } => Some(address_key(*address)),
        _ => None,
    };
    let output = match result {
        ExecutionResult::Success {
            output: Output::Call(data),
            ..
        } => data.to_vec(),
        ExecutionResult::Revert { output, .. } => output.to_vec(),
        _ => Vec::new(),
    };
    let logs = if result.is_success() {
        result
            .logs()
            .iter()
            .map(|log| EvmLog {
                address: address_key(log.address),
                topics: log
                    .data
                    .topics()
                    .iter()
                    .map(|topic| format!("0x{}", hex::encode(topic)))
                    .collect(),
                data: format!("0x{}", hex::encode(&log.data.data)),
            })
            .collect()
    } else {
        Vec::new()
    };
    EvmReceipt {
        height,
        index,
        status: result.is_success(),
        gas_used: result.tx_gas_used(),
        contract_address,
        logs,
        output: format!("0x{}", hex::encode(output)),
    }
}

/// 검증자 ID(ed25519 공개키 등)를 COINBASE용 20바이트 주소로 바꿉니다. RPC의 miner 표시와 같습니다.
pub fn producer_address(producer: &str) -> EvmAddress {
    let trimmed = producer.trim_start_matches("0x");
    if trimmed.len() == 40
        && let Ok(bytes) = hex::decode(trimmed)
    {
        return EvmAddress::from_slice(&bytes);
    }
    let bytes = hex::decode(trimmed).unwrap_or_else(|_| producer.as_bytes().to_vec());
    EvmAddress::from_slice(&keccak256(bytes)[12..])
}

/// 0x 소문자 20바이트 주소 문자열(상태 맵의 키 형식)입니다.
pub fn address_key(address: EvmAddress) -> String {
    format!("0x{}", hex::encode(address.as_slice()))
}

pub fn parse_address(text: &str) -> Result<EvmAddress, String> {
    let trimmed = text.trim_start_matches("0x");
    let bytes =
        hex::decode(trimmed).map_err(|_| format!("주소가 올바른 hex가 아닙니다: {text}"))?;
    if bytes.len() != 20 {
        return Err(format!("20바이트 주소가 아닙니다: {text}"));
    }
    Ok(EvmAddress::from_slice(&bytes))
}

fn normalize(address: &str) -> String {
    format!(
        "0x{}",
        address.trim_start_matches("0x").to_ascii_lowercase()
    )
}

fn word_hex(value: U256) -> String {
    format!("0x{}", hex::encode(value.to_be_bytes::<32>()))
}

/// 배포 거래가 만들 계약 주소입니다: keccak(rlp([sender, nonce]))[12..]
pub fn create_address(sender: EvmAddress, nonce: u64) -> String {
    address_key(sender.create(nonce))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Blockchain;
    use crate::model::Block;
    use crate::raw_transaction::{decode_legacy, sign_legacy_raw_for_test};

    const CHAIN_ID: u64 = 21_004;
    const ALICE_KEY: [u8; 32] = [7; 32];
    const ONE: u128 = 1_000_000_000_000_000_000;
    const TOKEN: &str = include_str!("../tests/fixtures/evm/TestToken.hex");
    const NFT: &str = include_str!("../tests/fixtures/evm/TestNft.hex");
    const VAULT: &str = include_str!("../tests/fixtures/evm/Vault.hex");

    fn key_address(key: [u8; 32]) -> String {
        let signing = k256::ecdsa::SigningKey::from_bytes((&key).into()).unwrap();
        let point = signing.verifying_key().to_encoded_point(false);
        format!(
            "0x{}",
            hex::encode(&keccak256(&point.as_bytes()[1..])[12..])
        )
    }

    fn selector(signature: &str) -> Vec<u8> {
        keccak256(signature.as_bytes())[..4].to_vec()
    }

    fn word_address(address: &str) -> Vec<u8> {
        let mut word = vec![0u8; 12];
        word.extend(hex::decode(address.trim_start_matches("0x")).unwrap());
        word
    }

    fn word_u256(value: u128) -> Vec<u8> {
        U256::from(value).to_be_bytes::<32>().to_vec()
    }

    struct Harness {
        chain: Blockchain,
        nonce: u64,
        alice: String,
    }

    impl Harness {
        fn new() -> Self {
            let alice = key_address(ALICE_KEY);
            Self {
                chain: Blockchain::with_chain_id(CHAIN_ID, vec![(alice.clone(), 1_000 * ONE)]),
                nonce: 0,
                alice,
            }
        }

        fn raw(&mut self, to: Option<&str>, value: u128, data: &[u8], gas: u64) -> Transaction {
            let to_bytes = to
                .map(|to| hex::decode(to.trim_start_matches("0x")).unwrap())
                .unwrap_or_default();
            let raw = sign_legacy_raw_for_test(
                ALICE_KEY, &to_bytes, value, data, CHAIN_ID, self.nonce, 1_000, gas,
            );
            self.nonce += 1;
            decode_legacy(&raw, CHAIN_ID).unwrap()
        }

        fn block(&mut self, transactions: Vec<Transaction>) -> Result<(), String> {
            let previous = self.chain.blocks.last().unwrap();
            let block = Block::new(
                previous.height + 1,
                previous.hash.clone(),
                previous.timestamp + 5,
                "validator-a".into(),
                transactions,
            );
            self.chain.apply_block(block).map(|_| ())
        }

        fn send(&mut self, to: Option<&str>, value: u128, data: &[u8], gas: u64) -> EvmReceipt {
            let tx = self.raw(to, value, data, gas);
            let id = tx.id();
            self.block(vec![tx]).unwrap();
            self.chain.evm_receipt(&id).cloned().unwrap()
        }

        fn deploy(&mut self, code: &str, args: &[u8]) -> String {
            let mut data = hex::decode(code.trim()).unwrap();
            data.extend_from_slice(args);
            let receipt = self.send(None, 0, &data, 5_000_000);
            assert!(receipt.status, "배포 실패: {}", receipt.output);
            receipt.contract_address.unwrap()
        }

        fn view(&self, to: &str, data: Vec<u8>) -> Vec<u8> {
            let (balances, nonces, evm) = self.chain.execution_state();
            let call = EvmCall {
                caller: parse_address(&self.alice).unwrap(),
                to: Some(parse_address(to).unwrap()),
                value: 0,
                data,
                gas_limit: CALL_GAS_LIMIT,
                gas_price: 0,
                nonce: 0,
            };
            let context = BlockContext {
                chain_id: CHAIN_ID,
                height: self.chain.tip_height() + 1,
                timestamp: 0,
                producer: "validator-a",
            };
            let result = simulate(context, &call, balances, nonces, evm).unwrap();
            assert!(result.success);
            result.output
        }

        fn total_supply(&self) -> u128 {
            self.chain.balances_snapshot().values().sum()
        }
    }

    #[test]
    fn erc20_deploy_transfer_revert_and_create() {
        let mut h = Harness::new();
        let supply = 1_000_000 * ONE;
        let token = h.deploy(TOKEN, &word_u256(supply));
        assert_eq!(token, create_address(parse_address(&h.alice).unwrap(), 0));
        assert!(h.chain.evm().code_of(&token).is_some());

        let bob = "0x00000000000000000000000000000000000000b0";
        let mut data = selector("transfer(address,uint256)");
        data.extend(word_address(bob));
        data.extend(word_u256(250 * ONE));
        let receipt = h.send(Some(&token), 0, &data, 200_000);
        assert!(receipt.status);
        assert_eq!(receipt.logs.len(), 1, "Transfer 이벤트");
        assert_eq!(
            receipt.logs[0].topics[0],
            format!(
                "0x{}",
                hex::encode(keccak256("Transfer(address,address,uint256)"))
            )
        );

        let mut query = selector("balanceOf(address)");
        query.extend(word_address(bob));
        assert_eq!(h.view(&token, query), word_u256(250 * ONE));

        // revert: 상태는 그대로, 수수료와 nonce만 반영
        let before_hash = h.chain.evm().clone();
        let alice_before = h.chain.balance_of(&h.alice);
        let receipt = h.send(Some(&token), 0, &selector("boom()"), 100_000);
        assert!(!receipt.status);
        assert!(receipt.output.contains(&hex::encode("boom")));
        assert_eq!(h.chain.evm(), &before_hash);
        assert_eq!(
            h.chain.balance_of(&h.alice),
            alice_before - u128::from(receipt.gas_used) * 1_000
        );
        assert_eq!(h.chain.next_nonce(&h.alice), h.nonce);

        // 계약 안에서 CREATE: 계약 nonce가 1 → 2로 늘고 자식 코드가 저장됨
        let receipt = h.send(Some(&token), 0, &selector("spawn()"), 300_000);
        assert!(receipt.status);
        let child = create_address(parse_address(&token).unwrap(), 1);
        assert!(h.chain.evm().code_of(&child).is_some());
        assert_eq!(h.chain.next_nonce(&token), 2);

        // 수수료는 생산자·재단으로만 이동하므로 총량은 그대로
        assert_eq!(h.total_supply(), 1_000 * ONE);
    }

    #[test]
    fn erc721_mint_and_owner() {
        let mut h = Harness::new();
        let nft = h.deploy(NFT, &[]);
        let mut data = selector("mint(address,uint256)");
        data.extend(word_address(&h.alice.clone()));
        data.extend(word_u256(42));
        assert!(h.send(Some(&nft), 0, &data, 300_000).status);
        let mut query = selector("ownerOf(uint256)");
        query.extend(word_u256(42));
        assert_eq!(h.view(&nft, query), word_address(&h.alice.clone()));
    }

    #[test]
    fn plain_transfer_to_contract_runs_receive_and_withdraw() {
        let mut h = Harness::new();
        let vault = h.deploy(VAULT, &[]);
        let receipt = h.send(Some(&vault), 5 * ONE, &[], 100_000);
        assert!(receipt.status);
        assert_eq!(receipt.logs.len(), 1, "Deposited 이벤트");
        assert_eq!(h.chain.balance_of(&vault), 5 * ONE);
        assert!(
            h.send(Some(&vault), 0, &selector("withdraw()"), 100_000)
                .status
        );
        assert_eq!(h.chain.balance_of(&vault), 0);
        assert_eq!(h.total_supply(), 1_000 * ONE);
    }

    #[test]
    fn replay_and_snapshot_reproduce_state_hash() {
        let mut h = Harness::new();
        let token = h.deploy(TOKEN, &word_u256(1_000 * ONE));
        let mut data = selector("transfer(address,uint256)");
        data.extend(word_address("0x00000000000000000000000000000000000000b0"));
        data.extend(word_u256(ONE));
        h.send(Some(&token), 0, &data, 200_000);
        let expected = h.chain.state_hash();

        let mut replay = h.chain.clone();
        replay.verify_and_rebuild().unwrap();
        assert_eq!(replay.state_hash(), expected);

        let snapshot = crate::archive::StateSnapshot::from_chain(&h.chain);
        let mut restored = Blockchain::from_snapshot_with_staking(
            snapshot.chain_id,
            snapshot.genesis_commitment.clone(),
            snapshot.height,
            snapshot.block_hash.clone(),
            snapshot.balances.clone(),
            snapshot.next_nonces.clone(),
            snapshot.executed_events.clone(),
            snapshot.staking.clone(),
        )
        .unwrap();
        restored.restore_evm(snapshot.evm.clone(), Default::default());
        assert_eq!(restored.state_hash(), snapshot.state_hash);
    }

    #[test]
    fn block_gas_limit_and_executable_selection() {
        let mut h = Harness::new();
        let code = hex::decode(VAULT.trim()).unwrap();
        let first = h.raw(None, 0, &code, 20_000_000);
        let second = h.raw(None, 0, &code, 20_000_000);
        let error = h.block(vec![first.clone(), second.clone()]).unwrap_err();
        assert!(error.contains("가스 한도"), "{error}");

        // 제안자는 한도를 넘는 거래와 잔액이 모자란 거래를 빼고 블록을 만든다
        let broke = h.raw(None, 0, &code, 25_000_000);
        let (selected, rejected) =
            h.chain
                .select_executable(1, 5, "validator-a", vec![first.clone(), second, broke]);
        assert_eq!(selected, vec![first]);
        assert_eq!(rejected.len(), 2);
    }

    #[test]
    fn legacy_transfer_state_hash_is_unchanged_without_contracts() {
        let mut h = Harness::new();
        let before = h.chain.state_hash();
        let tx = h.raw(
            Some("0x00000000000000000000000000000000000000b0"),
            ONE,
            &[],
            21_000,
        );
        h.block(vec![tx]).unwrap();
        assert!(h.chain.evm().is_empty());
        assert!(h.chain.evm_receipts().is_empty());
        assert_ne!(h.chain.state_hash(), before);
    }
}

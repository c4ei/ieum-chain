use crate::consensus::{FinalityCertificate, Validator};
use crate::model::{Block, Transaction};
use crate::{
    ArchiveStore, Blockchain, CommunicationEnvelope, CommunicationInbox, GenesisConfig, Keystore,
    Mempool, StateStore, account::AccountWallet,
};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

const PROTOCOL_VERSION: &str = "2";
const MIN_COMPATIBLE_PROTOCOL_VERSION: &str = "2";

/// geth/web3 도구가 접속할 HTTP JSON-RPC 설정입니다.
#[derive(Clone, Debug)]
pub struct RpcConfig {
    pub listen_ip: IpAddr,
    pub port: u16,
    pub chain_id: u64,
    pub genesis: Option<GenesisConfig>,
    pub data_dir: PathBuf,
    pub validators: Vec<Validator>,
    pub locked_addresses: Vec<String>,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            // 개발 기본값은 외부에 노출되지 않는 localhost입니다.
            listen_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8545,
            chain_id: 21004,
            genesis: None,
            data_dir: PathBuf::from("data/ledger"),
            validators: Vec::new(),
            locked_addresses: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct RpcState {
    chain: Blockchain,
    pool: Mempool,
    /// Ethereum 표준 0x 주소를 실제 secp256k1 지갑에 연결합니다.
    wallets: HashMap<String, AccountWallet>,
    faucet_alias: String,
    chain_id: u64,
    archive: ArchiveStore,
    keystore: Keystore,
    state_store: StateStore,
    peer_count: usize,
    peers: HashMap<String, RpcPeerInfo>,
    sync_current: u64,
    sync_highest: u64,
    sync_active: bool,
    started_at: std::time::Instant,
    communication_inbox: CommunicationInbox,
    communication_outbox: CommunicationInbox,
    communication_rpc_enabled: bool,
    personal_rpc_enabled: bool,
    data_dir: PathBuf,
    validators: Vec<Validator>,
    locked_addresses: HashSet<String>,
    finality_history: VecDeque<FinalityCertificate>,
    audit_log_path: PathBuf,
}

#[derive(Clone, Debug)]
struct RpcPeerInfo {
    address: String,
    remote_ip: Option<String>,
    direction: String,
    connections: usize,
    connected_at: u64,
}

/// 기존 geth 스크립트에서 자주 쓰는 계정·잔액·송금 API를 제공하는 호환 계층입니다.
///
/// 사용자 계정은 Ethereum과 같은 secp256k1 키와 20바이트 주소를 사용합니다.
/// 합의 검증자 키는 기존 Ed25519를 유지합니다. raw Ethereum transaction과
/// Solidity EVM은 별도 실행 계층이므로 아직 구현하지 않습니다.
pub struct RpcServer {
    config: RpcConfig,
    state: Arc<RwLock<RpcState>>,
}

#[derive(Clone)]
pub struct RpcNodeHandle {
    state: Arc<RwLock<RpcState>>,
}

impl RpcNodeHandle {
    pub fn record_finality(&self, certificate: FinalityCertificate) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        state.finality_history.push_back(certificate);
        while state.finality_history.len() > 10_000 {
            state.finality_history.pop_front();
        }
        Ok(())
    }
    pub fn drain_outbound_communication(&self) -> Result<Vec<CommunicationEnvelope>, String> {
        self.state
            .write()
            .map(|mut state| state.communication_outbox.drain())
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".into())
    }

    pub fn receive_communication(
        &self,
        envelope: CommunicationEnvelope,
        now: u64,
    ) -> Result<(), String> {
        self.state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?
            .communication_inbox
            .push(envelope, now)
    }

    pub fn chain(&self) -> Result<Blockchain, String> {
        self.state
            .read()
            .map(|state| state.chain.clone())
            .map_err(|_| "RPC 상태 읽기 잠금이 손상되었습니다.".into())
    }

    pub fn drain_transactions(&self, limit: usize) -> Result<Vec<Transaction>, String> {
        self.state
            .write()
            .map(|mut state| state.pool.drain(limit))
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".into())
    }

    pub fn has_pending_transactions(&self) -> Result<bool, String> {
        self.state
            .read()
            .map(|state| !state.pool.is_empty())
            .map_err(|_| "RPC 상태 읽기 잠금이 손상되었습니다.".into())
    }

    pub fn pending_transactions_snapshot(&self, limit: usize) -> Result<Vec<Transaction>, String> {
        self.state
            .read()
            .map(|state| state.pool.snapshot(limit))
            .map_err(|_| "RPC 상태 읽기 잠금이 손상되었습니다.".into())
    }

    pub fn restore_transactions(&self, transactions: Vec<Transaction>) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        for transaction in transactions {
            let _ = state.pool.add(transaction);
        }
        Ok(())
    }

    /// 합의 코어에서 검증·확정한 체인만 RPC 조회 원장과 영구 저장소에 반영합니다.
    pub fn install_finalized(
        &self,
        chain_before: &Blockchain,
        chain_after: Blockchain,
        block: &Block,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        state
            .archive
            .append_finalized(block, chain_before, &chain_after)?;
        state.state_store.commit(&chain_after)?;
        state.chain = chain_after;
        state.sync_current = state.chain.tip_height();
        state.sync_active = state.sync_current < state.sync_highest;
        let chain = state.chain.clone();
        state.pool.retain_valid(|transaction| {
            let total = transaction.amount.checked_add(transaction.fee);
            transaction.nonce >= chain.next_nonce(&transaction.from)
                && total.is_some_and(|value| chain.balance_of(&transaction.from) >= value)
                && crate::wallet::verify_transaction(transaction).is_ok()
        });
        Ok(())
    }

    pub fn install_synced_chain(&self, chain: Blockchain) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        state.state_store.commit(&chain)?;
        state.chain = chain;
        state.sync_current = state.chain.tip_height();
        state.sync_active = state.sync_current < state.sync_highest;
        Ok(())
    }

    pub fn set_peer_count(&self, count: usize) -> Result<(), String> {
        self.state
            .write()
            .map(|mut state| state.peer_count = count)
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".into())
    }

    pub fn peer_connected(
        &self,
        peer_id: &str,
        address: &str,
        remote_ip: Option<&str>,
        direction: &str,
        connections: usize,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        state.peers.insert(
            peer_id.to_string(),
            RpcPeerInfo {
                address: address.to_string(),
                remote_ip: remote_ip.map(str::to_owned),
                direction: direction.to_string(),
                connections,
                connected_at: unix_timestamp().unwrap_or_default(),
            },
        );
        state.peer_count = state.peers.len();
        Ok(())
    }

    pub fn peer_disconnected(
        &self,
        peer_id: &str,
        remaining_connections: usize,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        if remaining_connections == 0 {
            state.peers.remove(peer_id);
        } else if let Some(peer) = state.peers.get_mut(peer_id) {
            peer.connections = remaining_connections;
        }
        state.peer_count = state.peers.len();
        Ok(())
    }

    pub fn begin_sync(&self, highest: u64) -> Result<(), String> {
        self.state
            .write()
            .map(|mut state| {
                state.sync_highest = state.sync_highest.max(highest);
                state.sync_current = state.chain.tip_height();
                state.sync_active = state.sync_current < state.sync_highest;
            })
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".into())
    }
}

impl RpcServer {
    pub fn new(config: RpcConfig) -> Self {
        // 첫 번째 계정은 개발용 faucet입니다. 실제 운영망에서는 genesis/config와
        // 암호화된 keystore로 교체해야 합니다.
        // config genesis가 없는 개발망도 모든 프로세스가 같은 genesis/state root로
        // 시작해야 합니다. 이 공개 개발키는 테스트 전용이며 메인넷에서는 genesis와
        // 암호화 keystore를 반드시 명시해야 합니다.
        let faucet = AccountWallet::from_private_key([42; 32])
            .expect("개발 faucet 개인키는 유효해야 합니다.");
        let chain_id = config
            .genesis
            .as_ref()
            .map(|genesis| genesis.chain_id)
            .unwrap_or(config.chain_id);
        let max_active_bytes = config
            .genesis
            .as_ref()
            .map(|genesis| genesis.max_active_block_bytes)
            .unwrap_or(99_000_000);
        let archive = ArchiveStore::new(&config.data_dir, max_active_bytes)
            .expect("활성 블록 저장소를 만들 수 있어야 합니다.");
        // 원장은 data/ledger에, 사용자 계정은 CLI와 동일한 data/keystore에 둡니다.
        // 사용자 지정 원장 경로에서도 그 부모를 계정 루트로 사용합니다.
        let keystore_root = config
            .data_dir
            .parent()
            .unwrap_or(&config.data_dir)
            .join("keystore");
        let keystore =
            Keystore::new(keystore_root).expect("keystore 디렉터리를 만들 수 있어야 합니다.");
        let state_store =
            StateStore::new(&config.data_dir).expect("상태 저장소를 만들 수 있어야 합니다.");
        let mut chain = match config.genesis.as_ref() {
            Some(genesis) => Blockchain::from_genesis(genesis)
                .expect("RpcServer에는 검증된 제네시스 설정을 전달해야 합니다."),
            None => Blockchain::with_chain_id(
                chain_id,
                vec![(faucet.address(), 1_000_000_000_000_000_000)],
            ),
        };
        let stored_state = state_store
            .load()
            .expect("embedded 상태 DB를 읽을 수 있어야 합니다.");
        if let Some(state) = stored_state {
            assert_eq!(state.chain_id, chain_id, "embedded DB chain ID 불일치");
            chain = Blockchain::from_snapshot_with_events(
                chain_id,
                chain.genesis_commitment.clone(),
                state.height,
                state.block_hash,
                state.balances,
                state.nonces,
                state.executed_events,
            )
            .expect("embedded DB 상태를 복원할 수 있어야 합니다.");
            assert_eq!(
                chain.state_hash(),
                state.state_root,
                "embedded DB state root 불일치"
            );
        } else if let Some(snapshot) = archive
            .load_latest_snapshot()
            .expect("상태 체크포인트를 읽을 수 있어야 합니다.")
        {
            assert_eq!(snapshot.chain_id, chain_id, "체크포인트 chain ID 불일치");
            chain = Blockchain::from_snapshot_with_events(
                chain_id,
                chain.genesis_commitment.clone(),
                snapshot.height,
                snapshot.block_hash,
                snapshot.balances,
                snapshot.next_nonces,
                snapshot.executed_events,
            )
            .expect("체크포인트 상태를 복원할 수 있어야 합니다.");
            assert_eq!(
                chain.state_hash(),
                snapshot.state_hash,
                "체크포인트 상태 해시 불일치"
            );
            for block in archive
                .load_active_blocks()
                .expect("활성 블록을 읽을 수 있어야 합니다.")
            {
                chain
                    .apply_block(block)
                    .expect("활성 블록은 체크포인트에 연결되어야 합니다.");
            }
        } else {
            let archived = archive
                .load_all_blocks()
                .expect("저장된 블록 이력을 읽을 수 있어야 합니다.");
            if archived.is_empty() {
                // 제네시스도 실제 0번 블록으로 영구 보존합니다.
                let genesis_block = chain.blocks[0].clone();
                archive
                    .append_finalized(&genesis_block, &chain, &chain)
                    .expect("제네시스 블록을 저장할 수 있어야 합니다.");
            } else {
                // 상태 파일 쓰기 직전 장애가 나도 이미 fsync된 원본 블록으로 복구합니다.
                for block in archived.into_iter().filter(|block| block.height > 0) {
                    chain
                        .apply_block(block)
                        .expect("아카이브 블록은 제네시스부터 연속되어야 합니다.");
                }
            }
            state_store
                .commit(&chain)
                .expect("복구된 체인 상태를 저장할 수 있어야 합니다.");
        }
        let mut wallets = HashMap::new();
        let faucet_address = faucet.address();
        wallets.insert(faucet_address.clone(), faucet);
        let initial_height = chain.tip_height();
        Self {
            state: Arc::new(RwLock::new(RpcState {
                chain,
                pool: Mempool::default(),
                wallets,
                faucet_alias: faucet_address,
                chain_id,
                archive,
                keystore,
                state_store,
                peer_count: 0,
                peers: HashMap::new(),
                sync_current: initial_height,
                sync_highest: initial_height,
                sync_active: false,
                started_at: std::time::Instant::now(),
                communication_inbox: CommunicationInbox::default(),
                communication_outbox: CommunicationInbox::default(),
                communication_rpc_enabled: config.listen_ip.is_loopback(),
                personal_rpc_enabled: config.listen_ip.is_loopback(),
                data_dir: config.data_dir.clone(),
                validators: config.validators.clone(),
                locked_addresses: config
                    .locked_addresses
                    .iter()
                    .map(|v| normalize_address(v))
                    .collect(),
                finality_history: VecDeque::new(),
                audit_log_path: config.data_dir.join("audit/admin-actions.jsonl"),
            })),
            config,
        }
    }

    pub async fn run(self) -> Result<(), String> {
        let address = SocketAddr::new(self.config.listen_ip, self.config.port);
        let app = Router::new()
            .route("/", post(handle_rpc))
            .with_state(self.state);
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| format!("JSON-RPC 포트 열기 실패: {error}"))?;
        crate::log_info!("geth 호환 JSON-RPC 대기: http://{address}");
        axum::serve(listener, app)
            .await
            .map_err(|error| format!("JSON-RPC 서버 오류: {error}"))
    }

    pub fn node_handle(&self) -> RpcNodeHandle {
        RpcNodeHandle {
            state: Arc::clone(&self.state),
        }
    }
}

async fn handle_rpc(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<RpcState>>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    if let Some(requests) = request.as_array() {
        if requests.is_empty() {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32600, "message": "빈 batch 요청은 허용되지 않습니다."}
            }));
        }
        return Json(Value::Array(
            requests
                .iter()
                .map(|request| rpc_response(&state, request))
                .collect(),
        ));
    }
    Json(rpc_response(&state, &request))
}

fn rpc_response(state: &Arc<RwLock<RpcState>>, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request
        .get("params")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let result = dispatch(state, method, &params);
    if is_audited_method(method)
        && let Ok(guard) = state.read()
    {
        let _ = append_audit_log(&guard.audit_log_path, method, result.is_ok(), &params);
    }
    match result {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    }
}

fn dispatch(
    state: &Arc<RwLock<RpcState>>,
    method: &str,
    params: &[Value],
) -> Result<Value, (i64, String)> {
    match method {
        "web3_clientVersion" => Ok(json!(concat!(
            "IEUM-Chain/v",
            env!("CARGO_PKG_VERSION"),
            "/rpc/rust"
        ))),
        "net_version" => {
            let state = read_state(state)?;
            Ok(json!(state.chain_id.to_string()))
        }
        "net_listening" => Ok(json!(true)),
        "net_peerCount" => {
            let state = read_state(state)?;
            Ok(json!(quantity(state.peer_count as u64)))
        }
        "rpc_modules" => Ok(json!({
            "eth": "1.0",
            "net": "1.0",
            "personal": "1.0",
            "web3": "1.0",
            "ieumCommunication": "1.0",
            "ieum": "1.0"
        })),
        "eth_chainId" => {
            let state = read_state(state)?;
            Ok(json!(quantity(state.chain_id)))
        }
        "eth_syncing" => {
            let state = read_state(state)?;
            if !state.sync_active {
                Ok(json!(false))
            } else {
                Ok(json!({
                    "startingBlock": "0x0",
                    "currentBlock": quantity(state.sync_current),
                    "highestBlock": quantity(state.sync_highest)
                }))
            }
        }
        "eth_blockNumber" => {
            let state = read_state(state)?;
            let height = state.chain.tip_height();
            Ok(json!(quantity(height)))
        }
        "eth_accounts" | "personal_listAccounts" => {
            let state = read_state(state)?;
            let mut accounts: Vec<_> = state.wallets.keys().cloned().collect();
            accounts.extend(
                state
                    .keystore
                    .addresses()
                    .map_err(|message| (-32000, message))?,
            );
            accounts.sort();
            accounts.dedup();
            Ok(json!(accounts))
        }
        "eth_coinbase" => {
            let state = read_state(state)?;
            Ok(json!(state.faucet_alias))
        }
        "personal_newAccount" => {
            let password = params
                .first()
                .and_then(Value::as_str)
                .or_else(|| cfg!(test).then_some("test-password"))
                .ok_or_else(|| (-32602, "keystore 비밀번호가 필요합니다.".into()))?;
            let mut state = write_state(state)?;
            if !state.personal_rpc_enabled {
                return Err((
                    -32000,
                    "personal RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            let wallet = AccountWallet::new();
            let address = wallet.address();
            state
                .keystore
                .store(&wallet, password)
                .map_err(|message| (-32000, message))?;
            state.wallets.insert(address.clone(), wallet);
            Ok(json!(address))
        }
        "personal_importRawKey" => {
            let private_key = string_param(params, 0)?;
            let password = string_param(params, 1)?;
            let wallet = AccountWallet::from_private_key_hex(private_key)
                .map_err(|message| (-32602, message))?;
            let address = wallet.address();
            let mut state = write_state(state)?;
            if !state.personal_rpc_enabled {
                return Err((
                    -32000,
                    "personal RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            state
                .keystore
                .store(&wallet, password)
                .map_err(|message| (-32000, message))?;
            state.wallets.insert(address.clone(), wallet);
            Ok(json!(address))
        }
        "ieum_newMnemonic" => {
            if !read_state(state)?.personal_rpc_enabled {
                return Err((
                    -32000,
                    "계정 RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            let password = string_param(params, 0)?;
            let words = AccountWallet::generate_mnemonic().map_err(|message| (-32603, message))?;
            let wallet =
                AccountWallet::from_mnemonic(&words, 0).map_err(|message| (-32603, message))?;
            let address = wallet.address();
            let mut state = write_state(state)?;
            state
                .keystore
                .store(&wallet, password)
                .map_err(|message| (-32000, message))?;
            state.wallets.insert(address.clone(), wallet);
            Ok(json!({"mnemonic": words, "address": address, "path": "m/44'/60'/0'/0/0"}))
        }
        "ieum_importMnemonic" => {
            if !read_state(state)?.personal_rpc_enabled {
                return Err((
                    -32000,
                    "계정 RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            let words = string_param(params, 0)?;
            let index = params.get(1).and_then(Value::as_u64).unwrap_or(0);
            let password = string_param(params, 2)?;
            let index = u32::try_from(index)
                .map_err(|_| (-32602, "계정 index가 u32 범위를 벗어났습니다.".into()))?;
            let wallet =
                AccountWallet::from_mnemonic(words, index).map_err(|message| (-32602, message))?;
            let address = wallet.address();
            let mut state = write_state(state)?;
            state
                .keystore
                .store(&wallet, password)
                .map_err(|message| (-32000, message))?;
            state.wallets.insert(address.clone(), wallet);
            Ok(json!(address))
        }
        "personal_unlockAccount" => {
            let address = string_param(params, 0)?;
            let password = string_param(params, 1)?;
            let mut state = write_state(state)?;
            if !state.personal_rpc_enabled {
                return Err((
                    -32000,
                    "personal RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            let normalized = normalize_address(address);
            let wallet = state
                .keystore
                .load(&normalized, password)
                .map_err(|message| (-32000, message))?;
            state.wallets.insert(normalized, wallet);
            Ok(json!(true))
        }
        "eth_getBalance" => {
            let address = string_param(params, 0)?;
            let state = read_state(state)?;
            let ledger_address = resolve_ledger_address(&state, address);
            Ok(json!(quantity_u128(
                state.chain.balance_of(&ledger_address)
            )))
        }
        "eth_getTransactionCount" => {
            let address = string_param(params, 0)?;
            let state = read_state(state)?;
            let ledger_address = resolve_ledger_address(&state, address);
            Ok(json!(quantity(state.chain.next_nonce(&ledger_address))))
        }
        "eth_gasPrice" => Ok(json!("0x1")),
        "eth_estimateGas" => Ok(json!("0x5208")),
        "eth_getCode" => Ok(json!("0x")),
        "eth_call" => Ok(json!("0x")),
        "eth_getLogs" => Ok(json!([])),
        "eth_getUncleCountByBlockHash" | "eth_getUncleCountByBlockNumber" => Ok(json!("0x0")),
        "eth_getBlockByNumber" => {
            let selector = string_param(params, 0)?;
            let full = params.get(1).and_then(Value::as_bool).unwrap_or(false);
            let state = read_state(state)?;
            let height = if selector == "latest" {
                state.chain.blocks.last().map(|block| block.height)
            } else {
                Some(parse_quantity_u64(selector)?)
            };
            let block = match height {
                Some(height) => state
                    .archive
                    .block_by_height(height)
                    .map_err(|message| (-32603, message))?
                    .or_else(|| state.chain.block_by_height(height).cloned()),
                None => None,
            };
            Ok(block
                .as_ref()
                .map(|block| block_json(block, full))
                .unwrap_or(Value::Null))
        }
        "eth_getBlockByHash" => {
            let hash = string_param(params, 0)?;
            let full = params.get(1).and_then(Value::as_bool).unwrap_or(false);
            let state = read_state(state)?;
            let block = state
                .archive
                .block_by_hash(hash)
                .map_err(|message| (-32603, message))?
                .or_else(|| state.chain.block_by_hash(hash).cloned());
            Ok(block
                .as_ref()
                .map(|block| block_json(block, full))
                .unwrap_or(Value::Null))
        }
        "eth_getTransactionByHash" => {
            let hash = string_param(params, 0)?;
            let state = read_state(state)?;
            let transaction = state
                .archive
                .transaction_by_hash(hash)
                .map_err(|message| (-32603, message))?
                .or_else(|| {
                    state
                        .chain
                        .transaction_by_hash(hash)
                        .map(|(block, index, transaction)| {
                            (block.clone(), index, transaction.clone())
                        })
                });
            Ok(transaction
                .as_ref()
                .map(|(block, index, transaction)| transaction_json(block, *index, transaction))
                .unwrap_or(Value::Null))
        }
        "eth_getBlockTransactionCountByNumber" => {
            let selector = string_param(params, 0)?;
            let state = read_state(state)?;
            let height = if selector == "latest" {
                state.chain.tip_height()
            } else {
                parse_quantity_u64(selector)?
            };
            let block = state
                .archive
                .block_by_height(height)
                .map_err(|message| (-32603, message))?
                .or_else(|| state.chain.block_by_height(height).cloned());
            Ok(block
                .map(|block| json!(quantity(block.transactions.len() as u64)))
                .unwrap_or(Value::Null))
        }
        "eth_getTransactionByBlockNumberAndIndex" => {
            let selector = string_param(params, 0)?;
            let index = parse_quantity_u64(string_param(params, 1)?)? as usize;
            let state = read_state(state)?;
            let height = if selector == "latest" {
                state.chain.tip_height()
            } else {
                parse_quantity_u64(selector)?
            };
            let block = state
                .archive
                .block_by_height(height)
                .map_err(|message| (-32603, message))?
                .or_else(|| state.chain.block_by_height(height).cloned());
            Ok(block
                .as_ref()
                .and_then(|block| {
                    block
                        .transactions
                        .get(index)
                        .map(|transaction| transaction_json(block, index, transaction))
                })
                .unwrap_or(Value::Null))
        }
        "eth_getTransactionReceipt" => {
            let hash = string_param(params, 0)?;
            let state = read_state(state)?;
            let transaction = state
                .archive
                .transaction_by_hash(hash)
                .map_err(|message| (-32603, message))?
                .or_else(|| {
                    state
                        .chain
                        .transaction_by_hash(hash)
                        .map(|(block, index, transaction)| {
                            (block.clone(), index, transaction.clone())
                        })
                });
            Ok(transaction
                .map(|(block, index, transaction)| {
                    json!({
                        "transactionHash": format!("0x{}", transaction.id()),
                        "transactionIndex": quantity(index as u64),
                        "blockHash": format!("0x{}", block.hash),
                        "blockNumber": quantity(block.height),
                        "from": transaction.from,
                        "to": transaction.to,
                        "status": "0x1"
                    })
                })
                .unwrap_or(Value::Null))
        }
        "ieum_getStorageStatus" => {
            let state = read_state(state)?;
            serde_json::to_value(
                state
                    .archive
                    .status()
                    .map_err(|message| (-32603, message))?,
            )
            .map_err(|error| (-32603, error.to_string()))
        }
        "ieum_supplyStatus" => {
            let state = read_state(state)?;
            let balances = state.chain.balances_snapshot();
            let total_issued = balances
                .values()
                .try_fold(0_u128, |sum, value| sum.checked_add(*value))
                .ok_or_else(|| (-32603, "총발행량 합계가 u128 범위를 넘었습니다.".into()))?;
            let locked_balance = state
                .locked_addresses
                .iter()
                .try_fold(0_u128, |sum, address| {
                    sum.checked_add(balances.get(address).copied().unwrap_or(0))
                })
                .ok_or_else(|| (-32603, "잠금 잔액 합계가 u128 범위를 넘었습니다.".into()))?;
            Ok(json!({
                "totalIssued": total_issued.to_string(),
                "circulating": total_issued.saturating_sub(locked_balance).to_string(),
                "locked": locked_balance.to_string(),
                "unit": "wei",
                "decimals": 18,
                "lockedAddressCount": state.locked_addresses.len(),
                "height": state.chain.tip_height()
            }))
        }
        "ieum_addressBalances" => {
            let offset = params.first().and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = params
                .get(1)
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(1_000) as usize;
            let state = read_state(state)?;
            let mut entries: Vec<_> = state
                .chain
                .balances_snapshot()
                .into_iter()
                .filter(|(_, balance)| *balance > 0)
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let total = entries.len();
            let accounts: Vec<_> = entries
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|(address, balance)| {
                    let locked = state.locked_addresses.contains(&address);
                    json!({"address": address, "balance": balance.to_string(), "locked": locked})
                })
                .collect();
            Ok(
                json!({"height": state.chain.tip_height(), "offset": offset, "limit": limit, "total": total, "accounts": accounts}),
            )
        }
        "ieum_validatorStatus" => {
            let state = read_state(state)?;
            let window = params
                .first()
                .and_then(Value::as_u64)
                .unwrap_or(1_000)
                .min(10_000) as usize;
            let history: Vec<_> = state.finality_history.iter().rev().take(window).collect();
            let blocks = history.len() as u64;
            let validators: Vec<_> = state.validators.iter().map(|validator| {
                let signed = history
                    .iter()
                    .filter(|certificate| certificate.precommits.iter().any(|vote| vote.validator_id == validator.id))
                    .count() as u64;
                json!({"id": validator.id, "votingPower": validator.voting_power, "signedBlocks": signed,
                    "eligibleBlocks": blocks, "signingRatePercent": if blocks == 0 { 0.0 } else { signed as f64 * 100.0 / blocks as f64 }})
            }).collect();
            Ok(
                json!({"height": state.chain.tip_height(), "windowBlocks": blocks, "validators": validators}),
            )
        }
        "ieum_blockProductionStatus" => {
            let state = read_state(state)?;
            let window = params
                .first()
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .clamp(2, 10_000) as usize;
            let blocks: Vec<_> = state.chain.blocks.iter().rev().take(window).collect();
            let mut intervals = Vec::new();
            for pair in blocks.windows(2) {
                intervals.push(pair[0].timestamp.saturating_sub(pair[1].timestamp));
            }
            let average = if intervals.is_empty() {
                0.0
            } else {
                intervals.iter().sum::<u64>() as f64 / intervals.len() as f64
            };
            let delayed = intervals.iter().filter(|seconds| **seconds > 6).count();
            let estimated_missed: u64 = intervals
                .iter()
                .map(|seconds| seconds.saturating_sub(1) / 3)
                .sum();
            Ok(
                json!({"height": state.chain.tip_height(), "sampleBlocks": blocks.len(), "averageBlockTimeSeconds": average,
                "delayedIntervalCount": delayed, "estimatedMissedSlots": estimated_missed, "targetBlockTimeSeconds": 3}),
            )
        }
        "txpool_status" => {
            let state = read_state(state)?;
            Ok(json!({
                "pending": quantity(state.pool.len() as u64),
                "queued": "0x0",
                "bytes": quantity(state.pool.total_bytes() as u64)
            }))
        }
        "ieum_nodeStatus" => {
            let state = read_state(state)?;
            Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "chainId": state.chain_id,
                "height": state.chain.tip_height(),
                "blockHash": format!("0x{}", state.chain.tip_hash()),
                "stateRoot": format!("0x{}", state.chain.state_hash()),
                "peers": state.peer_count,
                "mempoolTransactions": state.pool.len(),
                "mempoolBytes": state.pool.total_bytes(),
                "syncing": state.sync_active,
                "syncCurrent": state.sync_current,
                "syncHighest": state.sync_highest,
                "uptimeSeconds": state.started_at.elapsed().as_secs()
            }))
        }
        "ieum_peerInfo" => {
            let state = read_state(state)?;
            let mut peers = state
                .peers
                .iter()
                .map(|(peer_id, peer)| {
                    json!({
                        "peerId": peer_id,
                        "address": peer.address,
                        "remoteIp": peer.remote_ip,
                        "direction": peer.direction,
                        "connections": peer.connections,
                        "connectedAt": peer.connected_at,
                        "connectedSeconds": unix_timestamp().unwrap_or_default().saturating_sub(peer.connected_at)
                    })
                })
                .collect::<Vec<_>>();
            peers.sort_by(|left, right| left["peerId"].as_str().cmp(&right["peerId"].as_str()));
            Ok(json!({
                "version": 1,
                "count": peers.len(),
                "height": state.chain.tip_height(),
                "peers": peers
            }))
        }
        "ieum_syncStatus" => {
            let state = read_state(state)?;
            let progress = if state.sync_highest == 0 {
                100.0
            } else {
                (state.sync_current as f64 / state.sync_highest as f64 * 100.0).min(100.0)
            };
            Ok(json!({
                "syncing": state.sync_active,
                "currentHeight": state.sync_current,
                "highestHeight": state.sync_highest,
                "progressPercent": progress,
                "readyForTransactions": !state.sync_active && state.sync_current >= state.sync_highest
            }))
        }
        "ieum_finalizedBlock" => {
            let state = read_state(state)?;
            let block = state.chain.blocks.last();
            Ok(block
                .map(|block| {
                    json!({
                        "height": block.height,
                        "hash": format!("0x{}", block.hash),
                        "stateRoot": format!("0x{}", state.chain.state_hash()),
                        "transactionCount": block.transactions.len()
                    })
                })
                .unwrap_or_else(|| {
                    json!({
                        "height": 0,
                        "hash": format!("0x{}", state.chain.tip_hash()),
                        "stateRoot": format!("0x{}", state.chain.state_hash()),
                        "transactionCount": 0
                    })
                }))
        }
        "ieum_networkIdentity" => {
            let state = read_state(state)?;
            let genesis_hash = &state.chain.genesis_commitment;
            Ok(json!({
                "chainId": state.chain_id,
                "genesisHash": format!("0x{genesis_hash}"),
                "protocolVersion": PROTOCOL_VERSION
            }))
        }
        "ieum_protocolVersion" => Ok(json!({
            "nodeVersion": env!("CARGO_PKG_VERSION"),
            "protocolVersion": PROTOCOL_VERSION,
            "minimumCompatibleProtocolVersion": MIN_COMPATIBLE_PROTOCOL_VERSION
        })),
        "ieum_recoveryStatus" => {
            let state = read_state(state)?;
            recovery_status(&state)
        }
        "ieum_getRecoveryByTransaction" => {
            let transaction_hash = string_param(params, 0)?;
            let state = read_state(state)?;
            recovery_by_transaction(&state, transaction_hash)
        }
        "ieum_sendCommunication" => {
            if !read_state(state)?.communication_rpc_enabled {
                return Err((
                    -32000,
                    "보안 통신 RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            let value = params
                .first()
                .cloned()
                .ok_or_else(|| (-32602, "암호화 통신 메시지 객체가 필요합니다.".into()))?;
            let envelope: CommunicationEnvelope = serde_json::from_value(value)
                .map_err(|error| (-32602, format!("통신 메시지 형식 오류: {error}")))?;
            let now = unix_timestamp().map_err(|message| (-32603, message))?;
            let id = envelope.id.clone();
            write_state(state)?
                .communication_outbox
                .push(envelope, now)
                .map_err(|message| (-32602, message))?;
            Ok(json!(id))
        }
        "ieum_pollCommunication" => {
            let mut state = write_state(state)?;
            if !state.communication_rpc_enabled {
                return Err((
                    -32000,
                    "보안 통신 RPC는 localhost에서만 사용할 수 있습니다.".into(),
                ));
            }
            let messages = state.communication_inbox.drain();
            serde_json::to_value(messages).map_err(|error| (-32603, error.to_string()))
        }
        "eth_sendTransaction" | "personal_sendTransaction" => send_transaction(state, params),
        "eth_sendRawTransaction" => send_raw_transaction(state, params),
        "ieum_sendSignedTransaction" => {
            let value = params
                .first()
                .cloned()
                .ok_or_else(|| (-32602, "서명 거래 객체가 필요합니다.".into()))?;
            let transaction: Transaction = serde_json::from_value(value)
                .map_err(|error| (-32602, format!("서명 거래 형식 오류: {error}")))?;
            crate::wallet::verify_transaction(&transaction).map_err(|message| (-32000, message))?;
            let transaction_id = transaction.id();
            write_state(state)?
                .pool
                .add(transaction)
                .map_err(|message| (-32000, message))?;
            Ok(json!(format!("0x{transaction_id}")))
        }
        _ => Err((-32601, format!("지원하지 않는 JSON-RPC 메서드: {method}"))),
    }
}

fn is_audited_method(method: &str) -> bool {
    matches!(
        method,
        "personal_newAccount"
            | "personal_importRawKey"
            | "ieum_newMnemonic"
            | "ieum_importMnemonic"
            | "personal_unlockAccount"
            | "eth_sendTransaction"
            | "personal_sendTransaction"
            | "eth_sendRawTransaction"
            | "ieum_sendSignedTransaction"
    )
}

fn append_audit_log(
    path: &std::path::Path,
    method: &str,
    success: bool,
    params: &[Value],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let digest = sha2::Sha256::digest(serde_json::to_vec(params).map_err(|e| e.to_string())?);
    let record = json!({"timestamp": unix_timestamp()?, "method": method, "success": success,
        "parameterSha256": hex::encode(digest), "pid": std::process::id()});
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{record}").map_err(|e| e.to_string())
}

fn send_raw_transaction(
    shared: &Arc<RwLock<RpcState>>,
    params: &[Value],
) -> Result<Value, (i64, String)> {
    let raw = string_param(params, 0)?;
    let mut state = write_state(shared)?;
    let transaction = crate::raw_transaction::decode_legacy(raw, state.chain_id)
        .map_err(|message| (-32000, message))?;
    let transaction_hash =
        crate::raw_transaction::transaction_hash(raw).map_err(|message| (-32602, message))?;
    state
        .pool
        .add(transaction)
        .map_err(|message| (-32000, message))?;
    Ok(json!(transaction_hash))
}

fn recovery_records(state: &RpcState) -> Result<Vec<Value>, (i64, String)> {
    let path = state.data_dir.join("recovery").join("records.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| (-32603, format!("복구 기록을 읽지 못했습니다: {error}")))?;
    serde_json::from_slice::<Vec<Value>>(&bytes)
        .map_err(|error| (-32603, format!("복구 기록 형식이 손상되었습니다: {error}")))
}

fn recovery_status(state: &RpcState) -> Result<Value, (i64, String)> {
    let records = recovery_records(state)?;
    let pending = records
        .iter()
        .filter(|record| record.get("status").and_then(Value::as_str) == Some("pending"))
        .count();
    let applied = records
        .iter()
        .filter(|record| record.get("status").and_then(Value::as_str) == Some("applied"))
        .count();
    Ok(json!({
        "active": pending > 0,
        "pendingPlans": pending,
        "appliedRecords": applied,
        "latest": records.last().cloned().unwrap_or(Value::Null)
    }))
}

fn recovery_by_transaction(
    state: &RpcState,
    transaction_hash: &str,
) -> Result<Value, (i64, String)> {
    let normalized = transaction_hash
        .trim_start_matches("0x")
        .to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err((-32602, "거래 해시는 32바이트 16진수여야 합니다.".into()));
    }
    let record = recovery_records(state)?.into_iter().find(|record| {
        record
            .get("transactionHash")
            .and_then(Value::as_str)
            .map(|value| {
                value
                    .trim_start_matches("0x")
                    .eq_ignore_ascii_case(&normalized)
            })
            .unwrap_or(false)
    });
    Ok(record.unwrap_or(Value::Null))
}

fn send_transaction(
    shared: &Arc<RwLock<RpcState>>,
    params: &[Value],
) -> Result<Value, (i64, String)> {
    let request = params
        .first()
        .and_then(Value::as_object)
        .ok_or_else(|| (-32602, "거래 객체가 필요합니다.".into()))?;
    let from = request
        .get("from")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "from 주소가 필요합니다.".into()))?;
    let to = request
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "to 주소가 필요합니다.".into()))?;
    let amount = parse_quantity_u128(
        request
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("0x0"),
    )?;
    let gas_price = parse_quantity_u128(
        request
            .get("gasPrice")
            .and_then(Value::as_str)
            .unwrap_or("0x1"),
    )?;
    let gas_limit = parse_quantity_u128(
        request
            .get("gas")
            .and_then(Value::as_str)
            .unwrap_or("0x5208"),
    )?;
    let fee = gas_price
        .checked_mul(gas_limit)
        .ok_or_else(|| (-32602, "gasPrice와 gas의 곱이 u128 범위를 넘습니다.".into()))?;

    let mut state = write_state(shared)?;
    let from_alias = normalize_address(from);
    let wallet = state
        .wallets
        .get(&from_alias)
        .cloned()
        .ok_or_else(|| (-32000, "노드가 관리하지 않는 from 계정입니다.".into()))?;
    let to_ledger = resolve_ledger_address(&state, to);
    let nonce = match request.get("nonce").and_then(Value::as_str) {
        Some(value) => parse_quantity_u64(value)?,
        None => state.chain.next_nonce(&wallet.address()),
    };
    let transaction = wallet.sign_transfer(to_ledger, amount, fee, nonce);
    let transaction_id = format!("0x{}", transaction.id());
    state
        .pool
        .add(transaction)
        .map_err(|message| (-32000, message))?;

    Ok(json!(transaction_id))
}

fn unix_timestamp() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("시스템 시각 오류: {error}"))
}

fn resolve_ledger_address(state: &RpcState, address: &str) -> String {
    let normalized = normalize_address(address);
    state
        .wallets
        .get(&normalized)
        .map(AccountWallet::address)
        .unwrap_or(normalized)
}

fn normalize_address(address: &str) -> String {
    format!(
        "0x{}",
        address.trim_start_matches("0x").to_ascii_lowercase()
    )
}

fn quantity(value: u64) -> String {
    format!("0x{value:x}")
}

fn quantity_u128(value: u128) -> String {
    format!("0x{value:x}")
}

fn block_json(block: &Block, full_transactions: bool) -> Value {
    let transactions = if full_transactions {
        block
            .transactions
            .iter()
            .enumerate()
            .map(|(index, transaction)| transaction_json(block, index, transaction))
            .collect::<Vec<_>>()
    } else {
        block
            .transactions
            .iter()
            .map(|transaction| json!(format!("0x{}", transaction.id())))
            .collect::<Vec<_>>()
    };
    json!({
        "number": quantity(block.height),
        "hash": format!("0x{}", block.hash),
        "parentHash": format!("0x{}", block.previous_hash),
        "timestamp": quantity(block.timestamp),
        "miner": block.producer,
        "transactions": transactions,
        "ieumSystemEvents": block.system_events,
        "transactionsRoot": format!("0x{}", block.hash),
        "size": quantity(serde_json::to_vec(block).map(|bytes| bytes.len()).unwrap_or(0) as u64)
    })
}

fn transaction_json(block: &Block, index: usize, transaction: &Transaction) -> Value {
    json!({
        "hash": format!("0x{}", transaction.id()),
        "nonce": quantity(transaction.nonce),
        "blockHash": format!("0x{}", block.hash),
        "blockNumber": quantity(block.height),
        "transactionIndex": quantity(index as u64),
        "from": transaction.from,
        "to": transaction.to,
        "value": quantity_u128(transaction.amount),
        "gasPrice": quantity_u128(transaction.fee),
        "input": "0x"
    })
}

fn parse_quantity_u64(value: &str) -> Result<u64, (i64, String)> {
    let hex = value
        .strip_prefix("0x")
        .ok_or_else(|| (-32602, "수량은 0x 접두사가 있는 hex여야 합니다.".into()))?;
    u64::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16).map_err(|_| {
        (
            -32602,
            "수량이 u64 범위를 벗어났거나 잘못되었습니다.".into(),
        )
    })
}

fn parse_quantity_u128(value: &str) -> Result<u128, (i64, String)> {
    let hex = value
        .strip_prefix("0x")
        .ok_or_else(|| (-32602, "수량은 0x 접두사가 있는 hex여야 합니다.".into()))?;
    u128::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16).map_err(|_| {
        (
            -32602,
            "수량이 u128 범위를 벗어났거나 잘못되었습니다.".into(),
        )
    })
}

fn string_param(params: &[Value], index: usize) -> Result<&str, (i64, String)> {
    params
        .get(index)
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, format!("{index}번 문자열 파라미터가 필요합니다.")))
}

fn read_state(
    state: &Arc<RwLock<RpcState>>,
) -> Result<std::sync::RwLockReadGuard<'_, RpcState>, (i64, String)> {
    state
        .read()
        .map_err(|_| (-32603, "RPC 상태 읽기 잠금이 손상되었습니다.".into()))
}

fn write_state(
    state: &Arc<RwLock<RpcState>>,
) -> Result<std::sync::RwLockWriteGuard<'_, RpcState>, (i64, String)> {
    state
        .write()
        .map_err(|_| (-32603, "RPC 상태 쓰기 잠금이 손상되었습니다.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DATA_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_rpc_config(test_name: &str) -> RpcConfig {
        let sequence = TEST_DATA_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        RpcConfig {
            data_dir: std::env::temp_dir()
                .join(format!(
                    "ieum-rpc-{test_name}-{}-{sequence}",
                    std::process::id()
                ))
                .join("ledger"),
            ..RpcConfig::default()
        }
    }

    #[test]
    fn submitted_transaction_waits_for_bft_finality() {
        let shared =
            RpcServer::new(test_rpc_config("submitted-transaction-waits-for-finality")).state;
        let accounts = dispatch(&shared, "eth_accounts", &[])
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        let faucet = accounts[0].as_str().unwrap();
        let receiver = dispatch(&shared, "personal_newAccount", &[])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let tx = json!({"from": faucet, "to": receiver, "value": "0x64", "gasPrice": "0x1"});
        assert!(dispatch(&shared, "eth_sendTransaction", &[tx]).is_ok());
        assert_eq!(shared.read().unwrap().pool.len(), 1);
        assert_eq!(
            dispatch(&shared, "eth_blockNumber", &[]).unwrap(),
            json!("0x0")
        );
        assert_eq!(
            dispatch(
                &shared,
                "eth_getBalance",
                &[json!(receiver), json!("latest")]
            )
            .unwrap(),
            json!("0x0")
        );
    }

    #[test]
    fn standard_json_rpc_request_dispatches_method() {
        let shared = RpcServer::new(test_rpc_config("standard-json-rpc-request")).state;
        let response = rpc_response(
            &shared,
            &json!({"jsonrpc": "2.0", "method": "eth_blockNumber", "params": [], "id": 7}),
        );
        assert_eq!(
            response,
            json!({"jsonrpc": "2.0", "id": 7, "result": "0x0"})
        );
    }

    #[test]
    fn explorer_block_contract_keeps_standard_fields_and_hides_raw_signature() {
        let transaction = Transaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            amount: 10,
            fee: 21_000,
            nonce: 0,
            signature: "raw-signature-must-not-be-exposed".into(),
        };
        let block = Block::new(
            1,
            "00".repeat(32),
            123,
            "validator".into(),
            vec![transaction],
        );
        let value = block_json(&block, true);

        assert_eq!(value["number"], "0x1");
        assert_eq!(value["transactions"][0]["blockHash"], value["hash"]);
        assert_eq!(value["transactions"][0]["transactionIndex"], "0x0");
        assert!(value["transactions"][0].get("signature").is_none());
    }

    #[test]
    fn malformed_raw_ethereum_transaction_is_rejected() {
        let shared = RpcServer::new(test_rpc_config("malformed-raw-transaction")).state;
        let error = dispatch(&shared, "eth_sendRawTransaction", &[json!("0x00")]).unwrap_err();
        assert_eq!(error.0, -32000);
    }

    #[test]
    fn geth_private_key_import_returns_standard_address() {
        let shared = RpcServer::new(test_rpc_config("geth-private-key-import")).state;
        let private_key = "0000000000000000000000000000000000000000000000000000000000000001";
        let address = dispatch(
            &shared,
            "personal_importRawKey",
            &[json!(private_key), json!("test-password")],
        )
        .unwrap();
        assert_eq!(address, json!("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"));
    }

    #[test]
    fn standard_mnemonic_import_returns_metamask_address() {
        let shared = RpcServer::new(test_rpc_config("standard-mnemonic-import")).state;
        let words = "test test test test test test test test test test test junk";
        let address = dispatch(
            &shared,
            "ieum_importMnemonic",
            &[json!(words), json!(0), json!("test-password")],
        )
        .unwrap();
        assert_eq!(address, json!("0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"));
    }

    #[test]
    fn operational_status_methods_are_available() {
        let shared = RpcServer::new(test_rpc_config("operational-status-methods")).state;
        let identity = dispatch(&shared, "ieum_networkIdentity", &[]).unwrap();
        assert_eq!(identity["chainId"], 21004);
        assert!(identity["genesisHash"].as_str().unwrap().starts_with("0x"));

        let sync = dispatch(&shared, "ieum_syncStatus", &[]).unwrap();
        assert_eq!(sync["readyForTransactions"], true);
        let recovery = dispatch(&shared, "ieum_recoveryStatus", &[]).unwrap();
        assert_eq!(recovery["active"], false);
    }

    #[test]
    fn peer_info_reports_and_removes_live_connections() {
        let server = RpcServer::new(test_rpc_config("peer-info"));
        let handle = RpcNodeHandle {
            state: server.state.clone(),
        };
        handle
            .peer_connected(
                "peer-a",
                "/ip4/10.0.0.2/udp/7001/quic-v1",
                Some("10.0.0.2"),
                "발신",
                1,
            )
            .unwrap();
        let info = dispatch(&server.state, "ieum_peerInfo", &[]).unwrap();
        assert_eq!(info["count"], 1);
        assert_eq!(info["peers"][0]["peerId"], "peer-a");
        assert_eq!(info["peers"][0]["direction"], "발신");
        handle.peer_disconnected("peer-a", 0).unwrap();
        assert_eq!(
            dispatch(&server.state, "ieum_peerInfo", &[]).unwrap()["count"],
            0
        );
    }
}

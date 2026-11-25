use crate::archive::StateSnapshot;
use crate::consensus::{FinalityCertificate, Validator};
use crate::model::{Block, Transaction};
use crate::{
    ArchiveStore, Blockchain, CommunicationEnvelope, CommunicationInbox, GenesisConfig, Keystore,
    Mempool, SnapshotCertificate, StateStore, account::AccountWallet,
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

const PROTOCOL_VERSION: &str = "4";
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
    pub block_time_ms: u64,
    pub validator_interest_policy: crate::validator_interest::ValidatorInterestPolicy,
    pub holder_reward_policy: crate::holder_rewards::HolderRewardPolicy,
    pub upgrade_schedule: crate::upgrade::UpgradeSchedule,
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
            block_time_ms: 5_000,
            validator_interest_policy: crate::validator_interest::ValidatorInterestPolicy::default(
            ),
            holder_reward_policy: crate::holder_rewards::HolderRewardPolicy::default(),
            upgrade_schedule: crate::upgrade::UpgradeSchedule::default(),
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
    block_time_ms: u64,
    validator_interest_policy: crate::validator_interest::ValidatorInterestPolicy,
    holder_reward_policy: crate::holder_rewards::HolderRewardPolicy,
    upgrade_schedule: crate::upgrade::UpgradeSchedule,
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
    pub fn pending_snapshot_certification(&self) -> Result<Option<StateSnapshot>, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "RPC 상태 읽기 잠금이 손상되었습니다.".to_string())?;
        let Some(snapshot) = state.archive.pending_certification()? else {
            return Ok(None);
        };
        let Some(canonical) = state.chain.block_by_height(snapshot.height) else {
            crate::logger::write_repeated_info(&format!(
                "[snapshot 인증 보류] 높이 {} 블록이 현재 canonical 메모리 체인에 없어 서명하지 않습니다 · 현재 높이 {}",
                snapshot.height,
                state.chain.tip_height()
            ));
            return Ok(None);
        };
        if snapshot.block_hash != canonical.hash {
            crate::logger::write_repeated_error(&format!(
                "[snapshot 인증 보류] 높이 {} 체크포인트 hash가 canonical 블록과 달라 서명하지 않습니다",
                snapshot.height
            ));
            return Ok(None);
        }
        Ok(Some(snapshot))
    }

    pub fn certify_snapshot(&self, certificate: SnapshotCertificate) -> Result<(), String> {
        let state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        let snapshot = state
            .archive
            .load_latest_snapshot()?
            .ok_or("인증할 로컬 snapshot이 없습니다.")?;
        certificate.verify_snapshot(&snapshot, &state.validators)?;
        state
            .archive
            .persist_certified_snapshot(snapshot, certificate)?;
        Ok(())
    }
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
            // 확정 직전 다른 피어가 전파한 거래가 뒤늦게 도착할 수 있습니다.
            // 이미 사용된 nonce를 다시 mempool에 넣으면 다음 제안 전체가 무효가 되므로
            // 현재 확정 상태를 기준으로 검증한 거래만 복원합니다.
            if !transaction_is_admissible(&state.chain, &transaction) {
                continue;
            }
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
            let debit = if matches!(
                &transaction.action,
                crate::model::TransactionAction::Transfer
                    | crate::model::TransactionAction::Delegate { .. }
                    | crate::model::TransactionAction::Evm
            ) {
                transaction.amount
            } else {
                0
            };
            let total = debit.checked_add(transaction.fee);
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

    pub fn install_certified_snapshot(
        &self,
        snapshot: StateSnapshot,
        certificate: SnapshotCertificate,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "RPC 상태 쓰기 잠금이 손상되었습니다.".to_string())?;
        certificate.verify_snapshot(&snapshot, &state.validators)?;
        if snapshot.chain_id != state.chain.chain_id
            || snapshot.genesis_commitment != state.chain.genesis_commitment
        {
            return Err("다른 체인의 snapshot을 RPC 상태에 설치할 수 없습니다.".into());
        }
        let mut chain = Blockchain::from_snapshot_with_staking(
            snapshot.chain_id,
            snapshot.genesis_commitment.clone(),
            snapshot.height,
            snapshot.block_hash.clone(),
            snapshot.balances.clone(),
            snapshot.next_nonces.clone(),
            snapshot.executed_events.clone(),
            snapshot.staking.clone(),
        )?;
        chain.restore_evm(snapshot.evm.clone(), Default::default());
        if chain.state_hash() != snapshot.state_hash {
            return Err("snapshot RPC 복원 상태 루트가 인증 값과 다릅니다.".into());
        }
        state
            .archive
            .persist_certified_snapshot(snapshot, certificate)?;
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
            assert_eq!(
                state.genesis_commitment, chain.genesis_commitment,
                "embedded DB Genesis hash 불일치: 기존 원장을 백업한 뒤 새 메인넷 원장으로 시작하세요"
            );
            chain = Blockchain::from_snapshot_with_staking(
                chain_id,
                chain.genesis_commitment.clone(),
                state.height,
                state.block_hash,
                state.balances,
                state.nonces,
                state.executed_events,
                state.staking,
            )
            .expect("embedded DB 상태를 복원할 수 있어야 합니다.");
            chain.restore_evm(state.evm, state.evm_receipts);
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
            assert_eq!(
                snapshot.genesis_commitment, chain.genesis_commitment,
                "체크포인트 Genesis hash 불일치: 다른 네트워크의 체크포인트를 사용할 수 없습니다"
            );
            chain = Blockchain::from_snapshot_with_staking(
                chain_id,
                chain.genesis_commitment.clone(),
                snapshot.height,
                snapshot.block_hash,
                snapshot.balances,
                snapshot.next_nonces,
                snapshot.executed_events,
                snapshot.staking,
            )
            .expect("체크포인트 상태를 복원할 수 있어야 합니다.");
            chain.restore_evm(snapshot.evm, Default::default());
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
                block_time_ms: config.block_time_ms,
                validator_interest_policy: config.validator_interest_policy.clone(),
                holder_reward_policy: config.holder_reward_policy.clone(),
                upgrade_schedule: config.upgrade_schedule.clone(),
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
        // 코드 3(execution reverted)은 "메시지\n0x데이터" 형식으로 revert 데이터를 함께 돌려줍니다.
        Err((3, message)) => {
            let (message, data) = message.split_once('\n').unwrap_or((message.as_str(), "0x"));
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": 3, "message": message, "data": data}})
        }
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
            let finalized_nonce = state.chain.next_nonce(&ledger_address);
            let nonce = if params.get(1).and_then(Value::as_str) == Some("pending") {
                state.pool.next_nonce(&ledger_address, finalized_nonce)
            } else {
                finalized_nonce
            };
            Ok(json!(quantity(nonce)))
        }
        "eth_gasPrice" => Ok(json!("0x1")),
        "eth_estimateGas" => {
            let state = read_state(state)?;
            estimate_gas(&state, params)
        }
        "eth_getCode" => {
            let address = string_param(params, 0)?;
            let state = read_state(state)?;
            Ok(json!(
                state
                    .chain
                    .evm()
                    .code_of(address)
                    .cloned()
                    .unwrap_or_else(|| "0x".into())
            ))
        }
        "eth_getStorageAt" => {
            let address = string_param(params, 0)?;
            let slot = parse_u256(string_param(params, 1)?)?;
            let state = read_state(state)?;
            let value = state.chain.evm().storage_at(address, slot);
            Ok(json!(format!(
                "0x{}",
                hex::encode(value.to_be_bytes::<32>())
            )))
        }
        "eth_call" => {
            let state = read_state(state)?;
            let call = simulation_call(&state, params, crate::evm::CALL_GAS_LIMIT)?;
            let result = simulate_call(&state, &call)?;
            if result.success {
                Ok(json!(format!("0x{}", hex::encode(result.output))))
            } else {
                Err(revert_error(&result))
            }
        }
        "eth_getLogs" => {
            let filter = params.first().cloned().unwrap_or(Value::Null);
            let state = read_state(state)?;
            Ok(Value::Array(logs_matching(&state.chain, &filter)?))
        }
        "eth_getUncleCountByBlockHash" | "eth_getUncleCountByBlockNumber" => Ok(json!("0x0")),
        // EIP-1559가 없는 legacy 체인입니다. 우선 수수료는 0, 기본 수수료는 0으로 응답합니다.
        "eth_maxPriorityFeePerGas" => Ok(json!("0x0")),
        "eth_feeHistory" => {
            let count = params
                .first()
                .and_then(|value| match value {
                    Value::String(text) => parse_quantity_u64(text).ok(),
                    Value::Number(number) => number.as_u64(),
                    _ => None,
                })
                .unwrap_or(1)
                .clamp(1, 1024);
            let percentiles = params
                .get(2)
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let state = read_state(state)?;
            let newest = state.chain.tip_height();
            let count = count.min(newest.saturating_add(1));
            Ok(json!({
                "oldestBlock": quantity(newest + 1 - count),
                "baseFeePerGas": vec!["0x0"; count as usize + 1],
                "gasUsedRatio": vec![0.0; count as usize],
                "reward": vec![vec!["0x0"; percentiles]; if percentiles > 0 { count as usize } else { 0 }]
            }))
        }
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
                .map(|block| block_json(block, full, state.chain_id))
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
                .map(|block| block_json(block, full, state.chain_id))
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
            if let Some((block, index, transaction)) = transaction.as_ref() {
                return Ok(transaction_json(block, *index, transaction, state.chain_id));
            }
            Ok(state
                .pool
                .transaction_by_hash(hash)
                .map(|transaction| pending_transaction_json(transaction, state.chain_id))
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
                    block.transactions.get(index).map(|transaction| {
                        transaction_json(block, index, transaction, state.chain_id)
                    })
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
                    let (gas_price, mut gas_limit) =
                        crate::raw_transaction::ethereum_fee_terms(&transaction);
                    let gas_of = |item: &Transaction| {
                        state
                            .chain
                            .evm_receipt(&item.id())
                            .map(|receipt| u128::from(receipt.gas_used))
                            .unwrap_or_else(|| crate::raw_transaction::ethereum_fee_terms(item).1)
                    };
                    let cumulative_gas = block
                        .transactions
                        .iter()
                        .take(index.saturating_add(1))
                        .map(gas_of)
                        .fold(0_u128, u128::saturating_add);
                    if let Some(receipt) = state.chain.evm_receipt(&transaction.id()) {
                        gas_limit = u128::from(receipt.gas_used);
                        let first_log_index = block
                            .transactions
                            .iter()
                            .take(index)
                            .filter_map(|item| state.chain.evm_receipt(&item.id()))
                            .map(|item| item.logs.len())
                            .sum::<usize>();
                        let logs: Vec<Value> = receipt
                            .logs
                            .iter()
                            .enumerate()
                            .map(|(position, log)| {
                                log_json(log, &block, index, &transaction, first_log_index + position)
                            })
                            .collect();
                        return json!({
                            "transactionHash": format!("0x{}", transaction.id()),
                            "transactionIndex": quantity(index as u64),
                            "blockHash": format!("0x{}", block.hash),
                            "blockNumber": quantity(block.height),
                            "from": transaction.from,
                            "to": if transaction.to.is_empty() { Value::Null } else { json!(transaction.to) },
                            "cumulativeGasUsed": quantity_u128(cumulative_gas),
                            "gasUsed": quantity_u128(gas_limit),
                            "effectiveGasPrice": quantity_u128(gas_price),
                            "logs": logs,
                            "logsBloom": logs_bloom(&receipt.logs),
                            "status": if receipt.status { "0x1" } else { "0x0" },
                            "type": "0x0",
                            "contractAddress": receipt.contract_address,
                            "revertData": if receipt.status { Value::Null } else { json!(receipt.output) },
                            "ethereumHash": crate::raw_transaction::ethereum_hash(&transaction)
                        });
                    }
                    json!({
                        "transactionHash": format!("0x{}", transaction.id()),
                        "transactionIndex": quantity(index as u64),
                        "blockHash": format!("0x{}", block.hash),
                        "blockNumber": quantity(block.height),
                        "from": transaction.from,
                        "to": transaction.to,
                        "cumulativeGasUsed": quantity_u128(cumulative_gas),
                        "gasUsed": quantity_u128(gas_limit),
                        "effectiveGasPrice": quantity_u128(gas_price),
                        "logs": [],
                        "logsBloom": format!("0x{}", "0".repeat(512)),
                        "status": "0x1",
                        "type": "0x0",
                        "contractAddress": Value::Null,
                        "ethereumHash": crate::raw_transaction::ethereum_hash(&transaction)
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
            let liquid_all = balances
                .values()
                .try_fold(0_u128, |sum, value| sum.checked_add(*value))
                .ok_or_else(|| (-32603, "총발행량 합계가 u128 범위를 넘었습니다.".into()))?;
            let genesis_locked = state
                .locked_addresses
                .iter()
                .try_fold(0_u128, |sum, address| {
                    sum.checked_add(balances.get(address).copied().unwrap_or(0))
                })
                .ok_or_else(|| (-32603, "잠금 잔액 합계가 u128 범위를 넘었습니다.".into()))?;
            let staking_locked = state
                .chain
                .staking()
                .delegations
                .iter()
                .map(|p| p.amount)
                .chain(state.chain.staking().unbonding.iter().map(|e| e.amount))
                .try_fold(0u128, |sum, value| sum.checked_add(value))
                .ok_or_else(|| (-32603, "위임 잠금 합계가 u128 범위를 넘었습니다.".into()))?;
            let bal_lock_all = genesis_locked
                .checked_add(staking_locked)
                .ok_or_else(|| (-32603, "전체 잠금 합계가 u128 범위를 넘었습니다.".into()))?;
            let bal_all = liquid_all
                .checked_add(staking_locked)
                .ok_or_else(|| (-32603, "총발행량 합계가 u128 범위를 넘었습니다.".into()))?;
            let bal_genesis_all = state
                .chain
                .initial_balances
                .values()
                .try_fold(0_u128, |sum, value| sum.checked_add(*value))
                .ok_or_else(|| (-32603, "최초 발행량 합계가 u128 범위를 넘었습니다.".into()))?;
            let bal_ija_paid_all = state.chain.blocks.iter().try_fold(
                0_u128,
                |total, block| {
                    block.system_events.iter().try_fold(total, |subtotal, event| {
                        if let crate::scheduled_event::ScheduledEventAction::ValidatorDailyInterest { payments,.. }
                            | crate::scheduled_event::ScheduledEventAction::DelegationDailyReward { payments,.. } = &event.action {
                            payments.iter().try_fold(subtotal, |sum, payment| {
                                sum.checked_add(payment.amount)
                            })
                        } else {
                            Some(subtotal)
                        }
                    })
                },
            ).ok_or_else(|| (-32603, "검증자 이자 누계가 u128 범위를 넘었습니다.".into()))?;
            let bal_foundation = balances
                .get(crate::FOUNDATION_FEE_ADDRESS)
                .copied()
                .unwrap_or(0);
            Ok(json!({
                "Bal_All": bal_all.to_string(),
                "Bal_Utong_All": bal_all.saturating_sub(bal_lock_all).to_string(),
                "Bal_Lock_All": bal_lock_all.to_string(),
                "Bal_Genesis_All": bal_genesis_all.to_string(),
                "Bal_MaxSupply": crate::MAX_SUPPLY.to_string(),
                "Bal_RemainingMintable": crate::MAX_SUPPLY.saturating_sub(bal_all).to_string(),
                "Bal_Ija_Paid_All": bal_ija_paid_all.to_string(),
                "Bal_Ija_Minted_All": "0",
                "Bal_NodeReward_Minted_All": "0",
                "Bal_Foundation": bal_foundation.to_string(),
                "unit": "wei",
                "decimals": 18,
                "lockedAddressCount": state.locked_addresses.len(),
                "stakingLocked": staking_locked.to_string(),
                "height": state.chain.tip_height()
            }))
        }
        "ieum_validatorInterestStatus" => {
            let state = read_state(state)?;
            let policy = &state.validator_interest_policy;
            let balances = state.chain.balances_snapshot();
            let payments =
                crate::validator_interest::calculate_payments(policy, &state.validators, &balances);
            let total: u128 = payments.iter().map(|p| p.amount).sum();
            Ok(json!({
                "height": state.chain.tip_height(),
                "snapshotTimestamp": SystemTime::now().duration_since(UNIX_EPOCH).map(|v| v.as_secs()).unwrap_or(0),
                "enabled": policy.enabled,
                "annualRateBps": policy.annual_rate_bps,
                "annualRatePercent": f64::from(policy.annual_rate_bps) / 100.0,
                "minimumBalance": policy.minimum_balance.to_string(),
                "maximumDailyTotal": policy.maximum_daily_total.to_string(),
                "policyHash": policy.hash(),
                "nextEventId": crate::validator_interest::event_id(SystemTime::now().duration_since(UNIX_EPOCH).map(|v| v.as_secs()).unwrap_or(0)),
                "eligibleValidators": payments.len(),
                "estimatedDailyTotal": total.to_string(),
                "payments": payments.iter().map(|p| json!({"address": p.address, "amount": p.amount.to_string()})).collect::<Vec<_>>()
            }))
        }
        "ieum_holderRewardStatus" => {
            let address = params
                .first()
                .and_then(Value::as_str)
                .map(normalize_address);
            let state = read_state(state)?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|v| v.as_secs())
                .unwrap_or(0);
            let policy = &state.holder_reward_policy;
            let balances = state.chain.balances_snapshot();
            let mut excluded = HashSet::new();
            excluded.insert(crate::FOUNDATION_FEE_ADDRESS.to_string());
            let payments = crate::holder_rewards::calculate_payments(policy, &balances, &excluded);
            let selected = address
                .as_ref()
                .and_then(|wanted| payments.iter().find(|p| &p.address == wanted));
            Ok(json!({
                "campaignName": policy.campaign_name,
                "enabled": policy.enabled,
                "active": policy.active(now),
                "startsAt": policy.starts_at,
                "endsAt": policy.ends_at,
                "annualRateBps": policy.annual_rate_bps,
                "annualRatePercent": f64::from(policy.annual_rate_bps) / 100.0,
                "minimumBalance": policy.minimum_balance.to_string(),
                "policyHash": policy.hash(),
                "eligibleWallets": payments.len(),
                "address": address,
                "eligible": selected.is_some(),
                "estimatedDailyReward": selected.map(|p| p.amount.to_string()).unwrap_or_else(|| "0".into()),
                "friendlyValidatorName": "이음지기",
                "friendlyDelegationName": "이음 맡기기",
                "communityName": "이음마당"
            }))
        }
        "ieum_holderRewardHistory" => {
            let limit = params
                .first()
                .and_then(Value::as_u64)
                .unwrap_or(500)
                .clamp(1, 10_000) as usize;
            let state = read_state(state)?;
            let mut items = Vec::new();
            for block in state.chain.blocks.iter().rev() {
                for event in block.system_events.iter().rev() {
                    if let crate::scheduled_event::ScheduledEventAction::HolderDailyReward {
                        snapshot_height,
                        policy_hash,
                        annual_rate_bps,
                        payments,
                    } = &event.action
                    {
                        for payment in payments.iter().rev() {
                            items.push(json!({
                                "eventId": event.id.clone(),
                                "blockHeight": block.height,
                                "blockHash": block.hash.clone(),
                                "timestamp": block.timestamp,
                                "executeAt": event.execute_at,
                                "snapshotHeight": snapshot_height,
                                "policyHash": policy_hash,
                                "annualRateBps": annual_rate_bps,
                                "address": payment.address.clone(),
                                "amount": payment.amount.to_string(),
                                "ip": Value::Null,
                                "country": Value::Null,
                                "privacyNotice": "온체인 지급 기록에는 IP와 국가를 저장하지 않습니다."
                            }));
                            if items.len() >= limit {
                                return Ok(json!({"items": items, "limited": true}));
                            }
                        }
                    }
                }
            }
            Ok(json!({"items": items, "limited": false}))
        }
        "ieum_stakingStatus" => {
            let wanted = params
                .first()
                .and_then(Value::as_str)
                .map(normalize_address);
            let state = read_state(state)?;
            let staking = state.chain.staking();
            let delegations:Vec<_>=staking.delegations.iter().filter(|p|wanted.as_ref().is_none_or(|a|&p.delegator==a)).map(|p|json!({"delegator":p.delegator,"validator":p.validator,"amount":p.amount.to_string()})).collect();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|v| v.as_secs())
                .unwrap_or(0);
            let unbonding:Vec<_>=staking.unbonding.iter().filter(|e|wanted.as_ref().is_none_or(|a|&e.delegator==a)).map(|e|json!({"delegator":e.delegator,"validator":e.validator,"amount":e.amount.to_string(),"releaseAt":e.release_at,"claimable":e.release_at<=now})).collect();
            let validators:Vec<_>=state.validators.iter().map(|v|json!({"id":v.id,"consensusVotingPower":v.voting_power,"delegated":staking.total_delegated_to(&v.id).to_string()})).collect();
            Ok(
                json!({"height":state.chain.tip_height(),"systemAddress":crate::staking::STAKING_SYSTEM_ADDRESS,"minimumDelegation":crate::staking::MINIMUM_DELEGATION.to_string(),"unbondingSeconds":crate::staking::UNBONDING_SECONDS,"doubleVoteSlashBps":crate::staking::DOUBLE_VOTE_SLASH_BPS,"delegations":delegations,"unbonding":unbonding,"validators":validators,"votingPowerAffected":false}),
            )
        }
        "ieum_encodeStakingCall" => {
            let action = string_param(params, 0)?;
            let payload = match action {
                "claim" => "claim".to_string(),
                "delegate" | "undelegate" => {
                    let validator = string_param(params, 1)?;
                    crate::staking::validate_validator(validator).map_err(|m| (-32602, m))?;
                    format!("{action}:{validator}")
                }
                _ => {
                    return Err((
                        -32602,
                        "action은 delegate, undelegate, claim 중 하나여야 합니다.".into(),
                    ));
                }
            };
            Ok(
                json!({"to":crate::staking::STAKING_SYSTEM_ADDRESS,"data":format!("0x{}",hex::encode(payload)),"amountRule":if action=="claim"{"0"}else{"요청 수량(wei)"}}),
            )
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
                if pair[1].height > 0 {
                    intervals.push(pair[0].timestamp.saturating_sub(pair[1].timestamp));
                }
            }
            let average = if intervals.is_empty() {
                0.0
            } else {
                intervals.iter().sum::<u64>() as f64 / intervals.len() as f64
            };
            let target = state.block_time_ms as f64 / 1_000.0;
            let delayed = intervals
                .iter()
                .filter(|seconds| **seconds as f64 > target * 1.2)
                .count();
            let estimated_missed: u64 = intervals
                .iter()
                .map(|seconds| ((*seconds as f64 / target).floor() as u64).saturating_sub(1))
                .sum();
            Ok(
                json!({"height": state.chain.tip_height(), "sampleBlocks": blocks.len(), "averageBlockTimeSeconds": average,
                "delayedIntervalCount": delayed, "estimatedMissedSlots": estimated_missed, "targetBlockTimeSeconds": target}),
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
                "version": crate::IEUM_DISPLAY_VERSION,
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
            if matches!(transaction.action, crate::model::TransactionAction::Evm) {
                return Err((
                    -32000,
                    "계약 거래는 eth_sendRawTransaction(Ethereum 서명)으로만 보낼 수 있습니다."
                        .into(),
                ));
            }
            {
                let guard = read_state(state)?;
                if !transaction.action.is_transfer()
                    && guard
                        .upgrade_schedule
                        .version_at(guard.chain.tip_height().saturating_add(1))
                        < 3
                {
                    return Err((
                        -32000,
                        "프로토콜 v3 활성화 높이 전에는 위임 거래를 제출할 수 없습니다.".into(),
                    ));
                }
            }
            {
                let guard = read_state(state)?;
                if let crate::model::TransactionAction::Delegate { validator } = &transaction.action
                    && !guard
                        .validators
                        .iter()
                        .any(|active| active.id == validator.as_str())
                {
                    return Err((
                        -32000,
                        "현재 활성 이음지기에게만 새 위임을 할 수 있습니다.".into(),
                    ));
                }
            }
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
    let protocol = state
        .upgrade_schedule
        .version_at(state.chain.tip_height().saturating_add(1));
    if crate::evm::is_evm_transaction(&transaction, state.chain.evm()) {
        if protocol < crate::evm::EVM_PROTOCOL_VERSION {
            return Err((
                -32000,
                "계약 배포·호출은 프로토콜 v4(EVM) 활성화 높이 이후에 제출할 수 있습니다.".into(),
            ));
        }
        let call = crate::raw_transaction::evm_call(&transaction)
            .map_err(|message| (-32000, message))?
            .ok_or_else(|| (-32000, "계약 거래 내용을 읽지 못했습니다.".to_string()))?;
        if call.gas_limit > crate::evm::BLOCK_GAS_LIMIT {
            return Err((
                -32000,
                "gasLimit이 블록 가스 한도(30,000,000)를 넘습니다.".into(),
            ));
        }
        let required = u128::from(call.gas_limit)
            .checked_mul(call.gas_price)
            .and_then(|fee| fee.checked_add(call.value));
        if required.is_none_or(|needed| state.chain.balance_of(&transaction.from) < needed) {
            return Err((
                -32000,
                "잔액이 value + gasLimit × gasPrice보다 적습니다.".into(),
            ));
        }
    }
    let is_staking = matches!(
        &transaction.action,
        crate::model::TransactionAction::Delegate { .. }
            | crate::model::TransactionAction::Undelegate { .. }
            | crate::model::TransactionAction::ClaimUnbonded
    );
    if is_staking && protocol < 3 {
        return Err((
            -32000,
            "프로토콜 v3 활성화 높이 전에는 위임 거래를 제출할 수 없습니다.".into(),
        ));
    }
    if let crate::model::TransactionAction::Delegate { validator } = &transaction.action
        && !state
            .validators
            .iter()
            .any(|active| active.id == validator.as_str())
    {
        return Err((
            -32000,
            "현재 활성 이음지기에게만 새 위임을 할 수 있습니다.".into(),
        ));
    }
    // Ethereum 지갑(ethers, MetaMask)은 keccak256(raw)을 기대하므로 그 값을 돌려줍니다.
    // 조회 API는 이 해시와 IEUM 거래 ID를 모두 받습니다.
    let transaction_hash = crate::raw_transaction::ethereum_hash(&transaction)
        .unwrap_or_else(|| format!("0x{}", transaction.id()));
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
        None => {
            let finalized_nonce = state.chain.next_nonce(&wallet.address());
            state.pool.next_nonce(&wallet.address(), finalized_nonce)
        }
    };
    let transaction = wallet.sign_transfer(to_ledger, amount, fee, nonce);
    let transaction_id = format!("0x{}", transaction.id());
    state
        .pool
        .add(transaction)
        .map_err(|message| (-32000, message))?;

    Ok(json!(transaction_id))
}

fn transaction_is_admissible(chain: &Blockchain, transaction: &Transaction) -> bool {
    if transaction.nonce < chain.next_nonce(&transaction.from)
        || crate::wallet::verify_transaction(transaction).is_err()
    {
        return false;
    }
    let debit = if matches!(
        &transaction.action,
        crate::model::TransactionAction::Transfer
            | crate::model::TransactionAction::Delegate { .. }
    ) {
        transaction.amount
    } else {
        0
    };
    debit
        .checked_add(transaction.fee)
        .is_some_and(|required| chain.balance_of(&transaction.from) >= required)
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

const ZERO_HASH: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";
/// keccak256(rlp([])) - Ethereum 도구가 기대하는 빈 uncle 목록 해시입니다.
const EMPTY_UNCLES_HASH: &str =
    "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347";
/// EVM 실행이 없으므로 표시용 블록 가스 한도입니다.
const DISPLAY_BLOCK_GAS_LIMIT: u64 = 30_000_000;

/// 검증자 ID(Ed25519 공개키 hex 등)를 ethers/MetaMask가 읽을 수 있는 20바이트 주소로 바꿉니다.
/// 이미 0x 주소이면 그대로 쓰고, 아니면 keccak256(ID 바이트)의 뒤 20바이트를 씁니다.
fn evm_address(id: &str) -> String {
    let trimmed = id.trim_start_matches("0x");
    if trimmed.len() == 40 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return format!("0x{}", trimmed.to_ascii_lowercase());
    }
    let bytes = hex::decode(trimmed).unwrap_or_else(|_| id.as_bytes().to_vec());
    let digest = sha3::Keccak256::digest(bytes);
    format!("0x{}", hex::encode(&digest[12..]))
}

fn block_json(block: &Block, full_transactions: bool, chain_id: u64) -> Value {
    let transactions = if full_transactions {
        block
            .transactions
            .iter()
            .enumerate()
            .map(|(index, transaction)| transaction_json(block, index, transaction, chain_id))
            .collect::<Vec<_>>()
    } else {
        block
            .transactions
            .iter()
            .map(|transaction| json!(format!("0x{}", transaction.id())))
            .collect::<Vec<_>>()
    };
    let gas_used = block
        .transactions
        .iter()
        .map(|transaction| crate::raw_transaction::ethereum_fee_terms(transaction).1)
        .fold(0_u128, u128::saturating_add);
    json!({
        "number": quantity(block.height),
        "hash": format!("0x{}", block.hash),
        "parentHash": format!("0x{}", block.previous_hash),
        "timestamp": quantity(block.timestamp),
        "miner": evm_address(&block.producer),
        "ieumProducer": block.producer,
        "difficulty": "0x0",
        "totalDifficulty": "0x0",
        "nonce": "0x0000000000000000",
        "mixHash": ZERO_HASH,
        "sha3Uncles": EMPTY_UNCLES_HASH,
        "uncles": [],
        "extraData": "0x",
        "gasLimit": quantity(DISPLAY_BLOCK_GAS_LIMIT),
        "gasUsed": quantity_u128(gas_used),
        "stateRoot": ZERO_HASH,
        "receiptsRoot": ZERO_HASH,
        "logsBloom": format!("0x{}", "0".repeat(512)),
        "transactions": transactions,
        "ieumSystemEvents": block.system_events,
        "transactionsRoot": format!("0x{}", block.hash),
        "size": quantity(serde_json::to_vec(block).map(|bytes| bytes.len()).unwrap_or(0) as u64)
    })
}

/// legacy(type 0) 서명 필드입니다. IEUM 자체 서명 거래는 v=27, r=s=0으로 표시합니다.
fn ethereum_signature_json(transaction: &Transaction) -> (String, String, String) {
    crate::raw_transaction::ethereum_signature_parts(transaction)
        .unwrap_or_else(|| ("0x1b".into(), ZERO_HASH.into(), ZERO_HASH.into()))
}

/// 거래 조회 응답의 input: raw 거래는 실제 calldata, IEUM 위임 거래는 기존처럼 action JSON입니다.
fn transaction_input(transaction: &Transaction) -> String {
    match &transaction.action {
        crate::model::TransactionAction::Transfer | crate::model::TransactionAction::Evm => {
            match crate::raw_transaction::evm_call(transaction) {
                Ok(Some(call)) => format!("0x{}", hex::encode(call.data)),
                _ => "0x".into(),
            }
        }
        action => format!(
            "0x{}",
            hex::encode(serde_json::to_vec(action).unwrap_or_default())
        ),
    }
}

fn parse_u256(text: &str) -> Result<revm::primitives::U256, (i64, String)> {
    revm::primitives::U256::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| (-32602, format!("256비트 16진 값이 아닙니다: {text}")))
}

/// eth_call·eth_estimateGas의 호출 객체({from,to,data|input,value,gas})를 읽습니다.
fn simulation_call(
    state: &RpcState,
    params: &[Value],
    default_gas: u64,
) -> Result<crate::evm::EvmCall, (i64, String)> {
    let object = params
        .first()
        .and_then(Value::as_object)
        .ok_or_else(|| (-32602, "호출 객체가 필요합니다.".to_string()))?;
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let caller = text("from")
        .and_then(|from| crate::evm::parse_address(from).ok())
        .unwrap_or_default();
    let to = match text("to") {
        Some(to) if !to.is_empty() => {
            Some(crate::evm::parse_address(to).map_err(|message| (-32602, message))?)
        }
        _ => None,
    };
    let data = text("input")
        .or_else(|| text("data"))
        .map(|data| hex::decode(data.trim_start_matches("0x")))
        .transpose()
        .map_err(|_| (-32602, "data가 올바른 hex가 아닙니다.".to_string()))?
        .unwrap_or_default();
    let value = text("value")
        .map(parse_quantity_u128)
        .transpose()?
        .unwrap_or(0);
    let gas_limit = text("gas")
        .map(parse_quantity_u64)
        .transpose()?
        .unwrap_or(default_gas)
        .min(crate::evm::BLOCK_GAS_LIMIT);
    let caller_key = crate::evm::address_key(caller);
    Ok(crate::evm::EvmCall {
        caller,
        to,
        value,
        data,
        gas_limit,
        gas_price: 0,
        nonce: state.chain.next_nonce(&caller_key),
    })
}

fn simulate_call(
    state: &RpcState,
    call: &crate::evm::EvmCall,
) -> Result<crate::evm::SimulationResult, (i64, String)> {
    let (balances, nonces, evm) = state.chain.execution_state();
    let context = crate::evm::BlockContext {
        chain_id: state.chain_id,
        height: state.chain.tip_height().saturating_add(1),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
        producer: "simulation",
    };
    crate::evm::simulate(context, call, balances, nonces, evm).map_err(|message| (-32000, message))
}

fn revert_error(result: &crate::evm::SimulationResult) -> (i64, String) {
    let data = format!("0x{}", hex::encode(&result.output));
    let reason = revert_reason(&result.output)
        .map(|reason| format!("execution reverted: {reason}"))
        .unwrap_or_else(|| {
            if result.halted {
                "execution halted (out of gas 또는 잘못된 명령)".into()
            } else {
                "execution reverted".into()
            }
        });
    (3, format!("{reason}\n{data}"))
}

/// Solidity `Error(string)` revert 데이터에서 이유 문자열을 꺼냅니다.
fn revert_reason(output: &[u8]) -> Option<String> {
    if output.len() < 68 || output[..4] != [0x08, 0xc3, 0x79, 0xa0] {
        return None;
    }
    let length = usize::try_from(revm::primitives::U256::from_be_slice(&output[36..68])).ok()?;
    let bytes = output.get(68..68usize.checked_add(length)?)?;
    String::from_utf8(bytes.to_vec()).ok()
}

/// 성공하는 가장 작은 gasLimit을 이분 탐색으로 찾습니다(환불·63/64 규칙 때문에 gasUsed보다 클 수 있음).
fn estimate_gas(state: &RpcState, params: &[Value]) -> Result<Value, (i64, String)> {
    let mut call = simulation_call(state, params, crate::evm::CALL_GAS_LIMIT)?;
    let upper = call.gas_limit;
    let first = simulate_call(state, &call)?;
    if !first.success {
        return Err(revert_error(&first));
    }
    let (mut low, mut high) = (first.gas_used.saturating_sub(1), upper);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        call.gas_limit = middle;
        match simulate_call(state, &call) {
            Ok(result) if result.success => high = middle,
            _ => low = middle,
        }
    }
    Ok(json!(quantity(high)))
}

fn log_json(
    log: &crate::evm::EvmLog,
    block: &Block,
    index: usize,
    transaction: &Transaction,
    log_index: usize,
) -> Value {
    json!({
        "address": log.address,
        "topics": log.topics,
        "data": log.data,
        "blockNumber": quantity(block.height),
        "blockHash": format!("0x{}", block.hash),
        "transactionHash": format!("0x{}", transaction.id()),
        "transactionIndex": quantity(index as u64),
        "logIndex": quantity(log_index as u64),
        "removed": false
    })
}

/// 2048비트 블룸 필터(EIP 표준: 주소·토픽마다 keccak의 3개 11비트 위치)입니다.
fn logs_bloom(logs: &[crate::evm::EvmLog]) -> String {
    let mut bloom = [0u8; 256];
    let mut add = |item: &[u8]| {
        let hash = sha3::Keccak256::digest(item);
        for pair in [0usize, 2, 4] {
            let bit = (usize::from(hash[pair]) << 8 | usize::from(hash[pair + 1])) & 2047;
            bloom[255 - bit / 8] |= 1 << (bit % 8);
        }
    };
    for log in logs {
        if let Ok(address) = hex::decode(log.address.trim_start_matches("0x")) {
            add(&address);
        }
        for topic in &log.topics {
            if let Ok(topic) = hex::decode(topic.trim_start_matches("0x")) {
                add(&topic);
            }
        }
    }
    format!("0x{}", hex::encode(bloom))
}

/// eth_getLogs 필터(fromBlock, toBlock, blockHash, address, topics)를 활성 블록에 적용합니다.
fn logs_matching(chain: &Blockchain, filter: &Value) -> Result<Vec<Value>, (i64, String)> {
    let tip = chain.tip_height();
    let block_number = |key: &str, default: u64| -> Result<u64, (i64, String)> {
        match filter.get(key).and_then(Value::as_str) {
            None | Some("latest") | Some("pending") | Some("safe") | Some("finalized") => {
                Ok(default)
            }
            Some("earliest") => Ok(0),
            Some(text) => parse_quantity_u64(text),
        }
    };
    let (from, to) = match filter.get("blockHash").and_then(Value::as_str) {
        Some(hash) => {
            let block = chain
                .block_by_hash(hash.trim_start_matches("0x"))
                .ok_or_else(|| (-32602, "블록을 찾을 수 없습니다.".to_string()))?;
            (block.height, block.height)
        }
        None => (
            block_number("fromBlock", tip)?,
            block_number("toBlock", tip)?,
        ),
    };
    if to.saturating_sub(from) > 10_000 {
        return Err((
            -32005,
            "한 번에 10,000블록까지만 조회할 수 있습니다.".into(),
        ));
    }
    let lower = |text: &str| text.to_ascii_lowercase();
    let addresses: Vec<String> = match filter.get("address") {
        Some(Value::String(address)) => vec![lower(address)],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(lower).collect(),
        _ => Vec::new(),
    };
    let topics: Vec<Vec<String>> = filter
        .get("topics")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    Value::String(topic) => vec![lower(topic)],
                    Value::Array(options) => options
                        .iter()
                        .filter_map(Value::as_str)
                        .map(lower)
                        .collect(),
                    _ => Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut found = Vec::new();
    for height in from..=to.min(tip) {
        let Some(block) = chain.block_by_height(height) else {
            continue;
        };
        let mut log_index = 0;
        for (index, transaction) in block.transactions.iter().enumerate() {
            let Some(receipt) = chain.evm_receipt(&transaction.id()) else {
                continue;
            };
            for log in &receipt.logs {
                let address_ok = addresses.is_empty() || addresses.contains(&log.address);
                let topics_ok = topics.iter().enumerate().all(|(position, options)| {
                    options.is_empty()
                        || log
                            .topics
                            .get(position)
                            .is_some_and(|topic| options.contains(&lower(topic)))
                });
                if address_ok && topics_ok {
                    found.push(log_json(log, block, index, transaction, log_index));
                }
                log_index += 1;
            }
        }
    }
    Ok(found)
}

fn transaction_json(
    block: &Block,
    index: usize,
    transaction: &Transaction,
    chain_id: u64,
) -> Value {
    let (gas_price, gas_limit) = crate::raw_transaction::ethereum_fee_terms(transaction);
    let (v, r, s) = ethereum_signature_json(transaction);
    json!({
        "type": "0x0",
        "chainId": quantity(chain_id),
        "v": v,
        "r": r,
        "s": s,
        "ethereumHash": crate::raw_transaction::ethereum_hash(transaction),
        "hash": format!("0x{}", transaction.id()),
        "nonce": quantity(transaction.nonce),
        "blockHash": format!("0x{}", block.hash),
        "blockNumber": quantity(block.height),
        "transactionIndex": quantity(index as u64),
        "from": transaction.from,
        "to": if transaction.to.is_empty() { Value::Null } else { json!(transaction.to) },
        "value": quantity_u128(transaction.amount),
        "gas": quantity_u128(gas_limit),
        "gasPrice": quantity_u128(gas_price),
        "input": transaction_input(transaction),
        "ieumAction": &transaction.action
    })
}

fn pending_transaction_json(transaction: &Transaction, chain_id: u64) -> Value {
    let (gas_price, gas_limit) = crate::raw_transaction::ethereum_fee_terms(transaction);
    let (v, r, s) = ethereum_signature_json(transaction);
    json!({
        "type": "0x0",
        "chainId": quantity(chain_id),
        "v": v,
        "r": r,
        "s": s,
        "ethereumHash": crate::raw_transaction::ethereum_hash(transaction),
        "hash": format!("0x{}", transaction.id()),
        "nonce": quantity(transaction.nonce),
        "blockHash": Value::Null,
        "blockNumber": Value::Null,
        "transactionIndex": Value::Null,
        "from": transaction.from,
        "to": if transaction.to.is_empty() { Value::Null } else { json!(transaction.to) },
        "value": quantity_u128(transaction.amount),
        "gas": quantity_u128(gas_limit),
        "gasPrice": quantity_u128(gas_price),
        "input": transaction_input(transaction),
        "ieumAction": &transaction.action,
        "ieumPending": true
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
    use std::fs;
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
    fn stale_checkpoint_after_state_restore_is_skipped_without_losing_current_checkpoint() {
        let config = test_rpc_config("stale-checkpoint-after-state-restore");
        let base_chain = RpcServer::new(config.clone())
            .state
            .read()
            .unwrap()
            .chain
            .clone();
        let mut stale_checkpoint = StateSnapshot::from_chain(&base_chain);
        stale_checkpoint.height = 82;
        stale_checkpoint.block_hash = "a".repeat(64);

        let tip_chain = Blockchain::from_snapshot_with_staking(
            base_chain.chain_id,
            base_chain.genesis_commitment.clone(),
            90,
            "b".repeat(64),
            base_chain.balances_snapshot(),
            base_chain.nonces_snapshot(),
            base_chain.executed_events().clone(),
            base_chain.staking_snapshot(),
        )
        .unwrap();
        StateStore::new(&config.data_dir)
            .unwrap()
            .commit(&tip_chain)
            .unwrap();

        let checkpoint_dir = config.data_dir.join("checkpoints");
        fs::create_dir_all(&checkpoint_dir).unwrap();
        fs::write(
            checkpoint_dir.join("202610M-h000000000082.json"),
            serde_json::to_vec(&stale_checkpoint).unwrap(),
        )
        .unwrap();

        let server = RpcServer::new(config.clone());
        let handle = RpcNodeHandle {
            state: server.state.clone(),
        };
        assert_eq!(
            handle.pending_snapshot_certification().unwrap(),
            None,
            "a checkpoint whose historical block is absent after state restore must not be signed"
        );
        assert_eq!(server.state.read().unwrap().chain.tip_height(), 90);

        let current_checkpoint = StateSnapshot::from_chain(&tip_chain);
        let current_checkpoint_path = checkpoint_dir.join("202611M-h000000000090.json");
        let mut mismatched_checkpoint = current_checkpoint.clone();
        mismatched_checkpoint.block_hash = "c".repeat(64);
        fs::write(
            &current_checkpoint_path,
            serde_json::to_vec(&mismatched_checkpoint).unwrap(),
        )
        .unwrap();
        assert_eq!(
            handle.pending_snapshot_certification().unwrap(),
            None,
            "a checkpoint with a non-canonical block hash must not be signed"
        );

        fs::write(
            current_checkpoint_path,
            serde_json::to_vec(&current_checkpoint).unwrap(),
        )
        .unwrap();
        assert_eq!(
            handle.pending_snapshot_certification().unwrap(),
            Some(current_checkpoint),
            "a checkpoint anchored to the current canonical block must remain eligible"
        );

        drop(handle);
        drop(server);
        fs::remove_dir_all(config.data_dir.parent().unwrap()).unwrap();
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
    fn submitted_transaction_is_queryable_while_pending() {
        let shared =
            RpcServer::new(test_rpc_config("submitted-transaction-queryable-pending")).state;
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
        let hash = dispatch(&shared, "eth_sendTransaction", &[tx]).unwrap();
        let pending = dispatch(&shared, "eth_getTransactionByHash", &[hash]).unwrap();
        assert_eq!(pending["blockHash"], Value::Null);
        assert_eq!(pending["blockNumber"], Value::Null);
        assert_eq!(pending["ieumPending"], true);
        assert_eq!(pending["to"], receiver);
        assert_eq!(
            dispatch(
                &shared,
                "eth_getTransactionCount",
                &[json!(faucet), json!("latest")]
            )
            .unwrap(),
            json!("0x0")
        );
        assert_eq!(
            dispatch(
                &shared,
                "eth_getTransactionCount",
                &[json!(faucet), json!("pending")]
            )
            .unwrap(),
            json!("0x1")
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
            action: crate::model::TransactionAction::Transfer,
            signature: "raw-signature-must-not-be-exposed".into(),
        };
        let block = Block::new(
            1,
            "00".repeat(32),
            123,
            "validator".into(),
            vec![transaction],
        );
        let value = block_json(&block, true, 21_004);

        assert_eq!(value["number"], "0x1");
        assert_eq!(value["transactions"][0]["blockHash"], value["hash"]);
        assert_eq!(value["transactions"][0]["transactionIndex"], "0x0");
        assert_eq!(value["transactions"][0]["gas"], "0x1");
        assert_eq!(value["transactions"][0]["gasPrice"], "0x5208");
        assert!(value["transactions"][0].get("signature").is_none());
    }

    #[test]
    fn evm_contract_rpc_flow() {
        let config = test_rpc_config("evm-contract-flow");
        let server = RpcServer::new(config);
        let handle = server.node_handle();
        let shared = Arc::clone(&server.state);
        let vault_code =
            hex::decode(include_str!("../tests/fixtures/evm/Vault.hex").trim()).unwrap();
        let sign = |to: &[u8], value: u128, data: &[u8], nonce: u64, gas: u64| {
            crate::raw_transaction::sign_legacy_raw_for_test(
                [42; 32], to, value, data, 21_004, nonce, 1, gas,
            )
        };
        let finalize = |handle: &RpcNodeHandle| {
            let transactions = handle.drain_transactions(10).unwrap();
            let before = handle.chain().unwrap();
            let mut after = before.clone();
            after
                .add_block(
                    transactions,
                    "d475e3a8a10a569c05c3d6406bb37adc681f5372e5855ffd76d24d5df91cad5d".into(),
                )
                .unwrap();
            let finalized = after.blocks.last().unwrap().clone();
            handle
                .install_finalized(&before, after, &finalized)
                .unwrap();
        };

        // 활성 높이 전에는 배포 거래를 받지 않는다
        let deploy = sign(&[], 0, &vault_code, 0, 1_000_000);
        let error = dispatch(&shared, "eth_sendRawTransaction", &[json!(deploy)]).unwrap_err();
        assert!(error.1.contains("v4"), "{}", error.1);

        shared.write().unwrap().upgrade_schedule = crate::upgrade::UpgradeSchedule {
            upgrades: vec![crate::upgrade::ProtocolUpgrade {
                name: "evm-v1".into(),
                activation_height: 0,
                protocol_version: crate::evm::EVM_PROTOCOL_VERSION,
            }],
        };
        let hash = dispatch(&shared, "eth_sendRawTransaction", &[json!(deploy)]).unwrap();
        finalize(&handle);
        let receipt = dispatch(&shared, "eth_getTransactionReceipt", &[hash]).unwrap();
        assert_eq!(receipt["status"], json!("0x1"));
        let vault = receipt["contractAddress"].as_str().unwrap().to_string();
        let code = dispatch(&shared, "eth_getCode", &[json!(vault), json!("latest")]).unwrap();
        assert!(code.as_str().unwrap().len() > 10);

        let gas = dispatch(
            &shared,
            "eth_estimateGas",
            &[json!({"from": receipt["from"], "to": vault, "value": "0x64"})],
        )
        .unwrap();
        let gas = u64::from_str_radix(gas.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
        assert!(gas > 21_000, "receive()가 실행되므로 21000보다 크다: {gas}");

        let to = hex::decode(vault.trim_start_matches("0x")).unwrap();
        dispatch(
            &shared,
            "eth_sendRawTransaction",
            &[json!(sign(&to, 100, &[], 1, gas))],
        )
        .unwrap();
        finalize(&handle);
        let logs = dispatch(
            &shared,
            "eth_getLogs",
            &[json!({"fromBlock": "0x0", "address": vault})],
        )
        .unwrap();
        assert_eq!(logs.as_array().unwrap().len(), 1);

        let mut query = revm::primitives::keccak256("deposits(address)")[..4].to_vec();
        query.extend([0u8; 12]);
        query.extend(
            hex::decode(receipt["from"].as_str().unwrap().trim_start_matches("0x")).unwrap(),
        );
        let deposited = dispatch(
            &shared,
            "eth_call",
            &[
                json!({"to": vault, "data": format!("0x{}", hex::encode(query))}),
                json!("latest"),
            ],
        )
        .unwrap();
        assert_eq!(
            u128::from_str_radix(deposited.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
            100
        );
    }

    #[test]
    fn revert_reason_is_decoded() {
        let mut output = vec![0x08, 0xc3, 0x79, 0xa0];
        output.extend(revm::primitives::U256::from(32).to_be_bytes::<32>());
        output.extend(revm::primitives::U256::from(4).to_be_bytes::<32>());
        let mut text = b"boom".to_vec();
        text.resize(32, 0);
        output.extend(text);
        assert_eq!(revert_reason(&output).as_deref(), Some("boom"));
    }

    #[test]
    fn ethereum_tooling_fields_and_keccak_hash_lookup() {
        let config = test_rpc_config("ethereum-tooling-fields");
        let server = RpcServer::new(config);
        let handle = server.node_handle();
        let shared = Arc::clone(&server.state);
        let receiver: [u8; 20] = [0x22; 20];
        let raw = crate::raw_transaction::sign_legacy_for_test(
            [42; 32],
            receiver,
            100,
            &[],
            21_004,
            0,
            1,
            21_000,
        );
        let returned = dispatch(&shared, "eth_sendRawTransaction", &[json!(raw)]).unwrap();
        let transactions = handle.drain_transactions(10).unwrap();
        let ethereum_hash = crate::raw_transaction::ethereum_hash(&transactions[0]).unwrap();
        let ieum_id = format!("0x{}", transactions[0].id());
        assert_eq!(
            returned,
            json!(ethereum_hash),
            "ethers는 keccak256(raw)을 기대한다"
        );
        let before = handle.chain().unwrap();
        let mut after = before.clone();
        after
            .add_block(
                transactions,
                "d475e3a8a10a569c05c3d6406bb37adc681f5372e5855ffd76d24d5df91cad5d".into(),
            )
            .unwrap();
        let finalized = after.blocks.last().unwrap().clone();
        handle
            .install_finalized(&before, after, &finalized)
            .unwrap();

        let block = dispatch(
            &shared,
            "eth_getBlockByNumber",
            &[json!("latest"), json!(false)],
        )
        .unwrap();
        for field in [
            "difficulty",
            "gasLimit",
            "gasUsed",
            "nonce",
            "extraData",
            "logsBloom",
            "stateRoot",
        ] {
            assert!(block.get(field).is_some(), "블록 필드 누락: {field}");
        }
        let miner = block["miner"].as_str().unwrap();
        assert!(miner.starts_with("0x") && miner.len() == 42);
        assert!(block.get("baseFeePerGas").is_none());

        // IEUM ID와 keccak256(raw) 두 해시 모두로 같은 거래를 찾는다.
        let by_id = dispatch(&shared, "eth_getTransactionByHash", &[json!(ieum_id)]).unwrap();
        let by_eth =
            dispatch(&shared, "eth_getTransactionByHash", &[json!(ethereum_hash)]).unwrap();
        assert_eq!(by_id, by_eth);
        assert_eq!(by_id["type"], "0x0");
        assert_eq!(by_id["chainId"], "0x520c");
        assert_eq!(by_id["ethereumHash"], json!(ethereum_hash));
        let receipt = dispatch(
            &shared,
            "eth_getTransactionReceipt",
            &[json!(ethereum_hash)],
        )
        .unwrap();
        assert_eq!(receipt["status"], "0x1");
        assert_eq!(receipt["type"], "0x0");

        if let Ok(path) = std::env::var("IEUM_RPC_DUMP") {
            let dump = json!({"raw": raw, "block": block, "transaction": by_id, "receipt": receipt,
                "feeHistory": dispatch(&shared, "eth_feeHistory", &[json!("0x2"), json!("latest"), json!([50])]).unwrap()});
            std::fs::write(path, serde_json::to_vec_pretty(&dump).unwrap()).unwrap();
        }
    }

    #[test]
    fn metamask_raw_transaction_and_receipt_survive_rpc_restart() {
        let config = test_rpc_config("finalized-state-survives-restart");
        let server = RpcServer::new(config.clone());
        let handle = server.node_handle();
        let shared = Arc::clone(&server.state);
        let faucet = dispatch(&shared, "eth_coinbase", &[])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let receiver = "0x2222222222222222222222222222222222222222";
        let receiver_bytes: [u8; 20] = hex::decode(receiver.trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap();
        let raw = crate::raw_transaction::sign_legacy_for_test(
            [42; 32],
            receiver_bytes,
            100,
            &[],
            21_004,
            0,
            1,
            21_000,
        );
        let transaction_hash = dispatch(&shared, "eth_sendRawTransaction", &[json!(raw)]).unwrap();
        let transactions = handle.drain_transactions(10).unwrap();
        assert_eq!(transactions.len(), 1);
        let before = handle.chain().unwrap();
        let mut after = before.clone();
        let producer = "0x3333333333333333333333333333333333333333";
        after.add_block(transactions, producer.into()).unwrap();
        let finalized = after.blocks.last().unwrap().clone();
        handle
            .install_finalized(&before, after, &finalized)
            .unwrap();

        let receipt_before = dispatch(
            &shared,
            "eth_getTransactionReceipt",
            std::slice::from_ref(&transaction_hash),
        )
        .unwrap();
        assert_eq!(receipt_before["status"], "0x1");
        assert_eq!(receipt_before["gasUsed"], "0x5208");
        drop(shared);
        drop(server);

        let restarted = RpcServer::new(config);
        let restarted_state = restarted.state;
        assert_eq!(
            dispatch(
                &restarted_state,
                "eth_getBalance",
                &[json!(receiver), json!("latest")],
            )
            .unwrap(),
            json!("0x64")
        );
        assert_eq!(
            dispatch(
                &restarted_state,
                "eth_getTransactionCount",
                &[json!(faucet), json!("latest")],
            )
            .unwrap(),
            json!("0x1")
        );
        let receipt_after = dispatch(
            &restarted_state,
            "eth_getTransactionReceipt",
            &[transaction_hash],
        )
        .unwrap();
        assert_eq!(receipt_after, receipt_before);
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

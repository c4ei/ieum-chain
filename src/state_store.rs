use crate::Blockchain;
use crate::embedded_db::EmbeddedDb;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicalState {
    pub schema_version: u32,
    pub chain_id: u64,
    pub height: u64,
    pub block_hash: String,
    pub state_root: String,
    pub balances: HashMap<String, u128>,
    pub nonces: HashMap<String, u64>,
    #[serde(default)]
    pub executed_events: HashSet<String>,
    #[serde(default)]
    pub staking: crate::staking::StakingState,
    pub height_to_hash: HashMap<u64, String>,
    pub transaction_index: HashMap<String, (u64, usize)>,
}

impl CanonicalState {
    pub fn from_chain(chain: &Blockchain) -> Self {
        let height_to_hash = chain
            .blocks
            .iter()
            .map(|block| (block.height, block.hash.clone()))
            .collect();
        let mut transaction_index = HashMap::new();
        for block in &chain.blocks {
            for (index, transaction) in block.transactions.iter().enumerate() {
                transaction_index.insert(transaction.id(), (block.height, index));
            }
        }
        Self {
            schema_version: 2,
            chain_id: chain.chain_id,
            height: chain.tip_height(),
            block_hash: chain.tip_hash().to_string(),
            state_root: chain.state_hash(),
            balances: chain.balances_snapshot(),
            nonces: chain.nonces_snapshot(),
            executed_events: chain.executed_events().clone(),
            staking: chain.staking_snapshot(),
            height_to_hash,
            transaction_index,
        }
    }
}

/// 확정 상태와 조회 인덱스를 원자적으로 교체하는 저장소입니다.
#[derive(Clone, Debug)]
pub struct StateStore {
    path: PathBuf,
    data_dir: PathBuf,
}

impl StateStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let directory = data_dir.as_ref().join("state");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        Ok(Self {
            path: directory.join("canonical-state.json"),
            data_dir: data_dir.as_ref().to_path_buf(),
        })
    }

    pub fn commit(&self, chain: &Blockchain) -> Result<CanonicalState, String> {
        let state = CanonicalState::from_chain(chain);
        let bytes = serde_json::to_vec(&state).map_err(|error| error.to_string())?;
        let mut db = EmbeddedDb::open(&self.data_dir)?;
        db.put("canonical/current", bytes.clone());
        db.put(
            format!("canonical/height/{:020}", state.height),
            serde_json::to_vec(&(state.block_hash.clone(), state.state_root.clone()))
                .map_err(|error| error.to_string())?,
        );
        for (transaction_id, location) in &state.transaction_index {
            db.put(
                format!("transaction/{transaction_id}"),
                serde_json::to_vec(location).map_err(|error| error.to_string())?,
            );
        }
        db.commit()?;
        // 한 버전 동안 JSON mirror를 유지해 0.9.x 도구와 rollback 호환성을 보장합니다.
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        fs::rename(&temporary, &self.path).map_err(|error| error.to_string())?;
        Ok(state)
    }

    pub fn load(&self) -> Result<Option<CanonicalState>, String> {
        let db = EmbeddedDb::open(&self.data_dir)?;
        if let Some(bytes) = db.get("canonical/current")? {
            let state: CanonicalState =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            validate_schema(&state)?;
            return Ok(Some(state));
        }
        match fs::read(&self.path) {
            Ok(bytes) => {
                let state: CanonicalState =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                validate_schema(&state)?;
                Ok(Some(state))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn validate_schema(state: &CanonicalState) -> Result<(), String> {
    if !matches!(state.schema_version, 1 | 2) {
        return Err("지원하지 않는 상태 저장소 버전입니다.".into());
    }
    Ok(())
}

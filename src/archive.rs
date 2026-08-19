use crate::Blockchain;
use crate::model::Block;
use crate::snapshot_sync::SnapshotCertificate;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

pub const MAX_ACTIVE_BLOCK_BYTES: u64 = 100_000_000;
pub const RETAIN_CERTIFIED_SNAPSHOTS: usize = 6;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateSnapshot {
    pub chain_id: u64,
    #[serde(default)]
    pub genesis_commitment: String,
    pub height: u64,
    pub block_hash: String,
    pub state_hash: String,
    pub balances: HashMap<String, u128>,
    pub next_nonces: HashMap<String, u64>,
    #[serde(default)]
    pub executed_events: HashSet<String>,
    #[serde(default)]
    pub staking: crate::staking::StakingState,
}

impl StateSnapshot {
    pub fn from_chain(chain: &Blockchain) -> Self {
        Self {
            chain_id: chain.chain_id,
            genesis_commitment: chain.genesis_commitment.clone(),
            height: chain.tip_height(),
            block_hash: chain.tip_hash().to_string(),
            state_hash: chain.state_hash(),
            balances: chain.balances_snapshot(),
            next_nonces: chain.nonces_snapshot(),
            executed_events: chain.executed_events().clone(),
            staking: chain.staking_snapshot(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveStatus {
    pub active_bytes: u64,
    pub max_active_bytes: u64,
    pub active_period: String,
    pub latest_checkpoint: Option<PathBuf>,
    pub latest_checkpoint_height: Option<u64>,
    pub certified_snapshot_count: usize,
    pub latest_certified_snapshot_height: Option<u64>,
    pub backups: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CertifiedSnapshot {
    pub snapshot: StateSnapshot,
    pub certificate: SnapshotCertificate,
}

/// 활성 블록의 총합을 제한하고 월별 체크포인트/백업을 만드는 저장소입니다.
///
/// `active/`만 노드 실행에 필요하며 `backup/`은 Explorer·감사용 선택 데이터입니다.
/// 월이 바뀌거나 활성 합계가 한도에 도달하면 현재 상태를 체크포인트로 고정한 뒤
/// 활성 블록을 백업으로 이동합니다.
#[derive(Debug)]
pub struct ArchiveStore {
    root: PathBuf,
    max_active_bytes: u64,
}

impl ArchiveStore {
    pub fn new(root: impl AsRef<Path>, max_active_bytes: u64) -> Result<Self, String> {
        if max_active_bytes == 0 || max_active_bytes > MAX_ACTIVE_BLOCK_BYTES {
            return Err("활성 블록 총합 제한은 1바이트 이상 100MB 이하여야 합니다.".into());
        }
        let store = Self {
            root: root.as_ref().to_path_buf(),
            max_active_bytes,
        };
        fs::create_dir_all(store.active_dir()).map_err(|error| error.to_string())?;
        fs::create_dir_all(store.backup_dir()).map_err(|error| error.to_string())?;
        fs::create_dir_all(store.checkpoint_dir()).map_err(|error| error.to_string())?;
        fs::create_dir_all(store.certified_dir()).map_err(|error| error.to_string())?;
        Ok(store)
    }

    pub fn append_finalized(
        &self,
        block: &Block,
        chain_before: &Blockchain,
        _chain_after: &Blockchain,
    ) -> Result<(), String> {
        let period = period_from_timestamp(block.timestamp);
        let mut record = serde_json::to_vec(block).map_err(|error| error.to_string())?;
        record.push(b'\n');
        if record.len() as u64 > self.max_active_bytes {
            return Err("블록 하나가 활성 블록 총합 제한보다 큽니다.".into());
        }

        let current_period = self.read_active_period()?;
        let active_bytes = directory_bytes(&self.active_dir())?;
        if current_period
            .as_deref()
            .is_some_and(|value| value != period)
            || active_bytes + record.len() as u64 > self.max_active_bytes
        {
            self.rollover(chain_before, current_period.as_deref().unwrap_or(&period))?;
        }

        let path = self.active_dir().join("blocks.jsonl");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        file.write_all(&record).map_err(|error| error.to_string())?;
        file.sync_data().map_err(|error| error.to_string())?;
        fs::write(self.active_dir().join("period"), period).map_err(|error| error.to_string())
    }

    pub fn rollover(&self, chain: &Blockchain, period: &str) -> Result<(), String> {
        let active = self.active_dir().join("blocks.jsonl");
        if !active.exists() {
            return Ok(());
        }
        let snapshot = StateSnapshot::from_chain(chain);
        let checkpoint = self
            .checkpoint_dir()
            .join(format!("{period}-h{:012}.json", snapshot.height));
        atomic_json(&checkpoint, &snapshot)?;

        let backup = unique_backup_path(&self.backup_dir(), period);
        compress_file(&active, &backup)?;
        fs::remove_file(&active).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(self.active_dir().join("period"));
        Ok(())
    }

    pub fn status(&self) -> Result<ArchiveStatus, String> {
        let mut checkpoints = list_files(&self.checkpoint_dir())?;
        checkpoints.sort();
        let mut backups = list_files(&self.backup_dir())?;
        backups.sort();
        let mut certified = list_files(&self.certified_dir())?;
        certified.sort();
        let latest_checkpoint_height = checkpoints.last().and_then(|path| checkpoint_height(path));
        let latest_certified_snapshot_height =
            certified.last().and_then(|path| checkpoint_height(path));
        Ok(ArchiveStatus {
            active_bytes: directory_bytes(&self.active_dir())?,
            max_active_bytes: self.max_active_bytes,
            active_period: self.read_active_period()?.unwrap_or_default(),
            latest_checkpoint: checkpoints.pop(),
            latest_checkpoint_height,
            certified_snapshot_count: certified.len(),
            latest_certified_snapshot_height,
            backups,
        })
    }

    pub fn pending_certification(&self) -> Result<Option<StateSnapshot>, String> {
        let Some(snapshot) = self.load_latest_snapshot()? else {
            return Ok(None);
        };
        let certified_height = self.status()?.latest_certified_snapshot_height;
        Ok((certified_height != Some(snapshot.height)).then_some(snapshot))
    }

    pub fn persist_certified_snapshot(
        &self,
        snapshot: StateSnapshot,
        certificate: SnapshotCertificate,
    ) -> Result<PathBuf, String> {
        let path = self.certified_dir().join(format!(
            "h{:012}-{}.json",
            snapshot.height,
            &snapshot.state_hash[..snapshot.state_hash.len().min(12)]
        ));
        atomic_json(
            &path,
            &CertifiedSnapshot {
                snapshot,
                certificate,
            },
        )?;
        let mut certified = list_files(&self.certified_dir())?;
        certified.sort();
        let remove_count = certified.len().saturating_sub(RETAIN_CERTIFIED_SNAPSHOTS);
        for old in certified.into_iter().take(remove_count) {
            fs::remove_file(old).map_err(|error| error.to_string())?;
        }
        Ok(path)
    }

    pub fn load_latest_snapshot(&self) -> Result<Option<StateSnapshot>, String> {
        let mut checkpoints = list_files(&self.checkpoint_dir())?;
        checkpoints.sort();
        let Some(path) = checkpoints.pop() else {
            return Ok(None);
        };
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    pub fn load_active_blocks(&self) -> Result<Vec<Block>, String> {
        let path = self.active_dir().join("blocks.jsonl");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
            .collect()
    }

    pub fn read_backup_blocks(&self) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();
        for path in list_files(&self.backup_dir())? {
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".jsonl") && !name.ends_with(".jsonl.zst") {
                continue;
            }
            let text = read_backup_text(&path)?;
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                blocks.push(serde_json::from_str(line).map_err(|error| error.to_string())?);
            }
        }
        blocks.sort_by_key(|block: &Block| block.height);
        Ok(blocks)
    }

    /// Explorer/RPC용 전체 블록 이력을 높이 순서로 읽습니다.
    /// 합의 상태 복구에는 체크포인트를 사용하고, 이 경로는 과거 조회에만 사용합니다.
    pub fn load_all_blocks(&self) -> Result<Vec<Block>, String> {
        let mut blocks = self.read_backup_blocks()?;
        blocks.extend(self.load_active_blocks()?);
        blocks.sort_by_key(|block| block.height);
        blocks.dedup_by_key(|block| block.height);
        Ok(blocks)
    }

    pub fn block_by_height(&self, height: u64) -> Result<Option<Block>, String> {
        Ok(self
            .load_all_blocks()?
            .into_iter()
            .find(|block| block.height == height))
    }

    pub fn block_by_hash(&self, hash: &str) -> Result<Option<Block>, String> {
        let hash = hash.trim_start_matches("0x");
        Ok(self
            .load_all_blocks()?
            .into_iter()
            .find(|block| block.hash == hash))
    }

    pub fn transaction_by_hash(
        &self,
        hash: &str,
    ) -> Result<Option<(Block, usize, crate::model::Transaction)>, String> {
        let hash = hash.trim_start_matches("0x");
        for block in self.load_all_blocks()? {
            if let Some((index, transaction)) = block
                .transactions
                .iter()
                .enumerate()
                .find(|(_, transaction)| transaction.id() == hash)
            {
                return Ok(Some((block.clone(), index, transaction.clone())));
            }
        }
        Ok(None)
    }

    /// 전전년도 이전의 월별 파일을 연도별 `YYYY.jsonl.zst` 하나로 합칩니다.
    /// 최근 연도와 직전 연도는 `YYYYMM M` 월 파일을 유지해 Explorer 조회 범위를 좁힙니다.
    pub fn compact_old_backups(&self, current_year: i32) -> Result<Vec<PathBuf>, String> {
        let mut created = Vec::new();
        for year in 1970..=current_year.saturating_sub(2) {
            let prefix = format!("{year:04}");
            let annual = self.backup_dir().join(format!("{prefix}.jsonl.zst"));
            let mut monthly = list_files(&self.backup_dir())?
                .into_iter()
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix) && name.contains('M'))
                })
                .collect::<Vec<_>>();
            monthly.sort();
            if monthly.is_empty() {
                continue;
            }
            let temporary = annual.with_extension("tmp");
            let output = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            let mut output =
                zstd::stream::write::Encoder::new(output, 3).map_err(|error| error.to_string())?;
            for path in &monthly {
                let text = read_backup_text(path)?;
                output
                    .write_all(text.as_bytes())
                    .map_err(|error| error.to_string())?;
            }
            let output = output.finish().map_err(|error| error.to_string())?;
            output.sync_all().map_err(|error| error.to_string())?;
            fs::rename(&temporary, &annual).map_err(|error| error.to_string())?;
            for path in monthly {
                fs::remove_file(path).map_err(|error| error.to_string())?;
            }
            created.push(annual);
        }
        Ok(created)
    }

    fn active_dir(&self) -> PathBuf {
        self.root.join("active")
    }

    fn backup_dir(&self) -> PathBuf {
        self.root.join("backup")
    }

    fn checkpoint_dir(&self) -> PathBuf {
        self.root.join("checkpoints")
    }

    fn certified_dir(&self) -> PathBuf {
        self.root.join("certified-snapshots")
    }

    fn read_active_period(&self) -> Result<Option<String>, String> {
        let path = self.active_dir().join("period");
        match fs::read_to_string(path) {
            Ok(value) => Ok(Some(value.trim().to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn checkpoint_height(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let marker = name.find('h')? + 1;
    let digits = name[marker..]
        .chars()
        .take_while(|value| value.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn unique_backup_path(directory: &Path, period: &str) -> PathBuf {
    let first = directory.join(format!("{period}.jsonl.zst"));
    if !first.exists() {
        return first;
    }
    for part in 2u32.. {
        let candidate = directory.join(format!("{period}-part{part:02}.jsonl.zst"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

fn compress_file(source: &Path, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension("tmp");
    let input = fs::File::open(source).map_err(|error| error.to_string())?;
    let output = fs::File::create(&temporary).map_err(|error| error.to_string())?;
    let mut encoder =
        zstd::stream::write::Encoder::new(output, 3).map_err(|error| error.to_string())?;
    std::io::copy(&mut BufReader::new(input), &mut encoder).map_err(|error| error.to_string())?;
    let output = encoder.finish().map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    fs::rename(temporary, destination).map_err(|error| error.to_string())
}

fn read_backup_text(path: &Path) -> Result<String, String> {
    let input = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut text = String::new();
    if path.extension().and_then(|value| value.to_str()) == Some("zst") {
        zstd::stream::read::Decoder::new(BufReader::new(input))
            .and_then(|mut decoder| decoder.read_to_string(&mut text))
            .map_err(|error| error.to_string())?;
    } else {
        BufReader::new(input)
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
    }
    Ok(text)
}

fn list_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    Ok(files)
}

fn directory_bytes(directory: &Path) -> Result<u64, String> {
    list_files(directory)?
        .into_iter()
        .try_fold(0u64, |sum, path| {
            fs::metadata(path)
                .map(|metadata| sum.saturating_add(metadata.len()))
                .map_err(|error| error.to_string())
        })
}

fn period_from_timestamp(timestamp: u64) -> String {
    // Howard Hinnant의 civil_from_days 알고리즘. 외부 시간 라이브러리 없이 UTC 월을 구합니다.
    let days = (timestamp / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}{month:02}M")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Block;

    #[test]
    fn month_change_creates_checkpoint_and_backup() {
        let root = std::env::temp_dir().join(format!("ieum-archive-{}", std::process::id()));
        let store = ArchiveStore::new(&root, 100_000_000).unwrap();
        let mut chain = Blockchain::new(vec![]);
        let first = Block::new(1, chain.blocks[0].hash.clone(), 1, "p".into(), vec![]);
        chain.apply_block(first.clone()).unwrap();
        let before_second = chain.clone();
        store
            .append_finalized(&first, &Blockchain::new(vec![]), &chain)
            .unwrap();
        let second = Block::new(
            2,
            chain.blocks[1].hash.clone(),
            2_678_400,
            "p".into(),
            vec![],
        );
        chain.apply_block(second.clone()).unwrap();
        store
            .append_finalized(&second, &before_second, &chain)
            .unwrap();
        let status = store.status().unwrap();
        assert_eq!(status.backups.len(), 1);
        assert_eq!(
            status.backups[0].file_name().unwrap().to_string_lossy(),
            "197001M.jsonl.zst"
        );
        assert_eq!(store.read_backup_blocks().unwrap(), vec![first]);
        assert!(status.latest_checkpoint.is_some());
        assert!(status.active_bytes <= status.max_active_bytes);
        let snapshot = store.load_latest_snapshot().unwrap().unwrap();
        let mut restored = Blockchain::from_snapshot(
            snapshot.chain_id,
            chain.genesis_commitment.clone(),
            snapshot.height,
            snapshot.block_hash,
            snapshot.balances,
            snapshot.next_nonces,
        )
        .unwrap();
        assert_eq!(restored.state_hash(), snapshot.state_hash);
        for block in store.load_active_blocks().unwrap() {
            restored.apply_block(block).unwrap();
        }
        assert_eq!(restored.tip_height(), chain.tip_height());
        assert_eq!(restored.tip_hash(), chain.tip_hash());
        let _ = fs::remove_dir_all(root);
    }
}

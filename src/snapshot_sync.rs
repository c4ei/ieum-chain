use crate::archive::StateSnapshot;
use crate::consensus::Validator;
use crate::signer::ValidatorSigner;
use crate::wallet::verify_signature;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_CHUNK_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotAttestation {
    pub validator_id: String,
    pub height: u64,
    pub block_hash: String,
    pub state_root: String,
    pub signature: String,
}

impl SnapshotAttestation {
    pub fn sign(snapshot: &StateSnapshot, signer: &ValidatorSigner) -> Result<Self, String> {
        let validator_id = signer.address();
        let payload =
            snapshot_attestation_bytes(snapshot.height, &snapshot.block_hash, &snapshot.state_hash);
        Ok(Self {
            validator_id,
            height: snapshot.height,
            block_hash: snapshot.block_hash.clone(),
            state_root: snapshot.state_hash.clone(),
            signature: signer.sign_bytes(&payload)?,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        verify_signature(
            &self.validator_id,
            &snapshot_attestation_bytes(self.height, &self.block_hash, &self.state_root),
            &self.signature,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotCertificate {
    pub height: u64,
    pub block_hash: String,
    pub state_root: String,
    pub attestations: Vec<SnapshotAttestation>,
}

impl SnapshotCertificate {
    pub fn from_attestations(attestations: Vec<SnapshotAttestation>) -> Result<Self, String> {
        let first = attestations
            .first()
            .ok_or("snapshot 인증 투표가 없습니다.")?;
        if attestations.iter().any(|vote| {
            vote.height != first.height
                || vote.block_hash != first.block_hash
                || vote.state_root != first.state_root
        }) {
            return Err("서로 다른 snapshot 투표를 하나의 인증서로 묶을 수 없습니다.".into());
        }
        Ok(Self {
            height: first.height,
            block_hash: first.block_hash.clone(),
            state_root: first.state_root.clone(),
            attestations,
        })
    }

    pub fn verify(&self, validators: &[Validator]) -> Result<(), String> {
        let total_power: u128 = validators
            .iter()
            .map(|value| value.voting_power as u128)
            .sum();
        if total_power == 0 {
            return Err("snapshot 검증자 총 투표권이 0입니다.".into());
        }
        let powers = validators
            .iter()
            .map(|value| (value.id.as_str(), value.voting_power as u128))
            .collect::<HashMap<_, _>>();
        let mut voters = HashSet::new();
        let mut signed_power = 0u128;
        for vote in &self.attestations {
            if vote.height != self.height
                || vote.block_hash != self.block_hash
                || vote.state_root != self.state_root
            {
                return Err("snapshot 인증서에 다른 상태 투표가 섞였습니다.".into());
            }
            vote.verify()?;
            let power = powers
                .get(vote.validator_id.as_str())
                .ok_or("등록되지 않은 검증자의 snapshot 투표입니다.")?;
            if voters.insert(vote.validator_id.as_str()) {
                signed_power = signed_power
                    .checked_add(*power)
                    .ok_or("snapshot 투표권 합계가 범위를 넘었습니다.")?;
            }
        }
        if signed_power * 3 <= total_power * 2 {
            return Err("snapshot 인증에 필요한 2/3 초과 서명이 없습니다.".into());
        }
        Ok(())
    }

    pub fn verify_snapshot(
        &self,
        snapshot: &StateSnapshot,
        validators: &[Validator],
    ) -> Result<(), String> {
        self.verify(validators)?;
        if self.height != snapshot.height
            || self.block_hash != snapshot.block_hash
            || self.state_root != snapshot.state_hash
        {
            return Err("snapshot 내용이 2/3 인증서와 일치하지 않습니다.".into());
        }
        Ok(())
    }
}

fn snapshot_attestation_bytes(height: u64, block_hash: &str, state_root: &str) -> Vec<u8> {
    let mut bytes = b"IEUM-SNAPSHOT-V1".to_vec();
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&(block_hash.len() as u32).to_be_bytes());
    bytes.extend_from_slice(block_hash.as_bytes());
    bytes.extend_from_slice(&(state_root.len() as u32).to_be_bytes());
    bytes.extend_from_slice(state_root.as_bytes());
    bytes
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncTip {
    pub height: u64,
    pub block_hash: String,
    pub state_root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotManifest {
    pub snapshot_id: String,
    pub tip: SyncTip,
    pub chunk_count: u32,
    pub compressed_bytes: u64,
    pub chunk_hashes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotChunk {
    pub snapshot_id: String,
    pub index: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ResumeProgress {
    manifest: SnapshotManifest,
    received: HashSet<u32>,
}

/// snapshot chunk를 검증해 디스크에 저장하고 재시작 후 이어받습니다.
pub struct SnapshotDownload {
    root: PathBuf,
    progress: ResumeProgress,
}

impl SnapshotDownload {
    pub fn open(data_dir: impl AsRef<Path>, manifest: SnapshotManifest) -> Result<Self, String> {
        manifest.verify()?;
        let root = data_dir.as_ref().join("sync").join(&manifest.snapshot_id);
        fs::create_dir_all(root.join("chunks")).map_err(|error| error.to_string())?;
        let progress_path = root.join("resume.json");
        let progress = match fs::read(&progress_path) {
            Ok(bytes) => {
                let progress: ResumeProgress = serde_json::from_slice(&bytes)
                    .map_err(|_| "snapshot 재개 파일이 손상되었습니다.")?;
                if progress.manifest != manifest {
                    return Err("재개 중인 snapshot manifest가 피어 응답과 다릅니다.".into());
                }
                progress
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ResumeProgress {
                manifest,
                received: HashSet::new(),
            },
            Err(error) => return Err(error.to_string()),
        };
        let download = Self { root, progress };
        download.persist_progress()?;
        Ok(download)
    }

    pub fn accept(&mut self, chunk: SnapshotChunk) -> Result<bool, String> {
        if chunk.snapshot_id != self.progress.manifest.snapshot_id {
            return Err("다른 snapshot의 chunk입니다.".into());
        }
        let expected = self
            .progress
            .manifest
            .chunk_hashes
            .get(chunk.index as usize)
            .ok_or("snapshot chunk index가 범위를 벗어났습니다.")?;
        if hash(&chunk.bytes) != *expected {
            return Err("snapshot chunk 해시가 일치하지 않습니다.".into());
        }
        atomic_write(&self.chunk_path(chunk.index), &chunk.bytes)?;
        self.progress.received.insert(chunk.index);
        self.persist_progress()?;
        Ok(self.is_complete())
    }

    pub fn missing(&self) -> Vec<u32> {
        (0..self.progress.manifest.chunk_count)
            .filter(|index| !self.progress.received.contains(index))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.progress.received.len() == self.progress.manifest.chunk_count as usize
    }

    pub fn finish(self) -> Result<StateSnapshot, String> {
        if !self.is_complete() {
            return Err("snapshot chunk가 아직 모두 도착하지 않았습니다.".into());
        }
        let mut compressed = Vec::with_capacity(self.progress.manifest.compressed_bytes as usize);
        for index in 0..self.progress.manifest.chunk_count {
            compressed.extend(fs::read(self.chunk_path(index)).map_err(|error| error.to_string())?);
        }
        if compressed.len() as u64 != self.progress.manifest.compressed_bytes {
            return Err("snapshot 압축 크기가 manifest와 다릅니다.".into());
        }
        if hash(&compressed) != self.progress.manifest.snapshot_id {
            return Err("완성된 snapshot ID가 manifest와 다릅니다.".into());
        }
        let bytes =
            zstd::stream::decode_all(compressed.as_slice()).map_err(|error| error.to_string())?;
        let snapshot: StateSnapshot =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if snapshot.height != self.progress.manifest.tip.height
            || snapshot.block_hash != self.progress.manifest.tip.block_hash
            || snapshot.state_hash != self.progress.manifest.tip.state_root
        {
            return Err("완성된 snapshot 상태가 교차검증 tip과 다릅니다.".into());
        }
        Ok(snapshot)
    }

    fn chunk_path(&self, index: u32) -> PathBuf {
        self.root.join("chunks").join(format!("{index:08}.chunk"))
    }

    fn persist_progress(&self) -> Result<(), String> {
        atomic_write(
            &self.root.join("resume.json"),
            &serde_json::to_vec_pretty(&self.progress).map_err(|error| error.to_string())?,
        )
    }
}

impl SnapshotManifest {
    pub fn build(
        snapshot: &StateSnapshot,
        chunk_bytes: usize,
    ) -> Result<(Self, Vec<SnapshotChunk>), String> {
        if chunk_bytes == 0 {
            return Err("snapshot chunk 크기는 0보다 커야 합니다.".into());
        }
        let plain = serde_json::to_vec(snapshot).map_err(|error| error.to_string())?;
        let compressed =
            zstd::stream::encode_all(plain.as_slice(), 3).map_err(|error| error.to_string())?;
        let snapshot_id = hash(&compressed);
        let chunks = compressed
            .chunks(chunk_bytes)
            .enumerate()
            .map(|(index, bytes)| SnapshotChunk {
                snapshot_id: snapshot_id.clone(),
                index: index as u32,
                bytes: bytes.to_vec(),
            })
            .collect::<Vec<_>>();
        let manifest = Self {
            snapshot_id,
            tip: SyncTip {
                height: snapshot.height,
                block_hash: snapshot.block_hash.clone(),
                state_root: snapshot.state_hash.clone(),
            },
            chunk_count: chunks.len() as u32,
            compressed_bytes: compressed.len() as u64,
            chunk_hashes: chunks.iter().map(|chunk| hash(&chunk.bytes)).collect(),
        };
        Ok((manifest, chunks))
    }

    pub fn verify(&self) -> Result<(), String> {
        if self.chunk_count == 0 || self.chunk_count as usize != self.chunk_hashes.len() {
            return Err("snapshot manifest chunk 수가 올바르지 않습니다.".into());
        }
        if self.snapshot_id.len() != 64 || self.chunk_hashes.iter().any(|value| value.len() != 64) {
            return Err("snapshot manifest 해시 길이가 올바르지 않습니다.".into());
        }
        Ok(())
    }
}

/// 최소 quorum 피어가 동일한 height/block hash/state root를 보고해야 선택합니다.
pub struct TipQuorum {
    minimum_peers: usize,
    votes: HashMap<String, SyncTip>,
}

impl TipQuorum {
    pub fn new(minimum_peers: usize) -> Result<Self, String> {
        if !(2..=3).contains(&minimum_peers) {
            return Err("동기화 교차검증 피어 수는 2 또는 3이어야 합니다.".into());
        }
        Ok(Self {
            minimum_peers,
            votes: HashMap::new(),
        })
    }

    pub fn observe(&mut self, peer_id: impl Into<String>, tip: SyncTip) -> Option<SyncTip> {
        self.votes.insert(peer_id.into(), tip);
        let mut counts: BTreeMap<(u64, String, String), usize> = BTreeMap::new();
        for value in self.votes.values() {
            *counts
                .entry((
                    value.height,
                    value.block_hash.clone(),
                    value.state_root.clone(),
                ))
                .or_default() += 1;
        }
        counts
            .into_iter()
            .filter(|(_, count)| *count >= self.minimum_peers)
            .max_by_key(|((height, _, _), _)| *height)
            .map(|((height, block_hash, state_root), _)| SyncTip {
                height,
                block_hash,
                state_root,
            })
    }
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StateSnapshot {
        StateSnapshot {
            chain_id: 21_004,
            height: 7,
            block_hash: "block".into(),
            state_hash: "state".into(),
            balances: HashMap::from([("alice".into(), 10)]),
            next_nonces: HashMap::new(),
            executed_events: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn resumes_verified_chunks() {
        let root = std::env::temp_dir().join(format!("ieum-snapshot-{}", std::process::id()));
        let (manifest, chunks) = SnapshotManifest::build(&sample(), 8).unwrap();
        let mut first = SnapshotDownload::open(&root, manifest.clone()).unwrap();
        first.accept(chunks[0].clone()).unwrap();
        drop(first);
        let mut resumed = SnapshotDownload::open(&root, manifest).unwrap();
        assert!(!resumed.missing().contains(&0));
        for chunk in chunks.into_iter().skip(1) {
            resumed.accept(chunk).unwrap();
        }
        assert_eq!(resumed.finish().unwrap(), sample());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn requires_two_matching_independent_peers() {
        let tip = SyncTip {
            height: 10,
            block_hash: "b".into(),
            state_root: "s".into(),
        };
        let mut quorum = TipQuorum::new(2).unwrap();
        assert!(quorum.observe("peer-a", tip.clone()).is_none());
        assert_eq!(quorum.observe("peer-b", tip.clone()), Some(tip));
    }

    #[test]
    fn certified_snapshot_requires_more_than_two_thirds_voting_power() {
        use crate::signer::ValidatorSigner;
        use crate::wallet::Wallet;

        let wallets = (1u8..=4)
            .map(|seed| Wallet::from_seed([seed; 32]))
            .collect::<Vec<_>>();
        let validators = wallets
            .iter()
            .map(|wallet| Validator::new(wallet.address(), 1))
            .collect::<Vec<_>>();
        let snapshot = sample();
        let mut votes = Vec::new();
        for wallet in wallets.into_iter().take(3) {
            votes.push(
                SnapshotAttestation::sign(&snapshot, &ValidatorSigner::from(wallet)).unwrap(),
            );
        }
        let certificate = SnapshotCertificate::from_attestations(votes).unwrap();
        certificate.verify_snapshot(&snapshot, &validators).unwrap();
    }
}

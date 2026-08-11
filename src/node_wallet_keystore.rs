use crate::account::AccountWallet;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const KDF_ROUNDS: u32 = 200_000;

#[derive(Debug, Serialize, Deserialize)]
struct Document {
    version: u32,
    address: String,
    salt: String,
    nonce: String,
    ciphertext: String,
    mac: String,
    kdf_rounds: u32,
}

/// 실제 노드 보상 자산용 secp256k1 지갑입니다.
/// 합의/등록 증명용 Ed25519 키와 분리하며 주소는 사용자 지갑과 같은 0x+40자리입니다.
pub struct NodeWalletKeystore;

impl NodeWalletKeystore {
    pub fn load_or_create_default(
        path: &Path,
        password_path: &Path,
    ) -> Result<AccountWallet, String> {
        if path.exists() {
            let password = read_password(password_path)?;
            return match Self::load(path, password.trim()) {
                Ok(wallet) => Ok(wallet),
                Err(error) if error == "legacy-ed25519-keystore" => {
                    Self::migrate_legacy(path, password.trim())
                }
                Err(error) => Err(error),
            };
        }
        if password_path.exists() {
            return Err(
                "node_wallet.keystore는 없지만 비밀번호 파일이 남아 있어 자동 생성을 중단합니다."
                    .into(),
            );
        }
        let wallet = AccountWallet::new();
        let password = random_password();
        write_private_new(password_path, password.as_bytes())?;
        if let Err(error) = Self::store(path, &wallet, &password) {
            let _ = fs::remove_file(password_path);
            return Err(error);
        }
        Ok(wallet)
    }

    pub fn load(path: &Path, password: &str) -> Result<AccountWallet, String> {
        let bytes = fs::read(path).map_err(|e| format!("노드 지갑 읽기 실패: {e}"))?;
        let doc: Document = serde_json::from_slice(&bytes)
            .map_err(|_| "node_wallet.keystore가 손상되었습니다.".to_string())?;
        if !matches!(doc.version, 1 | 2) || doc.kdf_rounds < KDF_ROUNDS {
            return Err("지원하지 않거나 너무 약한 node_wallet.keystore입니다.".into());
        }
        let salt = decode_32(&doc.salt)?;
        let nonce = decode_32(&doc.nonce)?;
        let ciphertext = hex::decode(&doc.ciphertext).map_err(|_| "노드 지갑 암호문 오류")?;
        let key = derive_key(password.as_bytes(), &salt, doc.kdf_rounds);
        if mac(&key, &nonce, &ciphertext) != decode_32(&doc.mac)? {
            return Err("노드 지갑 비밀번호가 틀렸거나 파일이 변조되었습니다.".into());
        }
        let private_key: [u8; 32] = crypt(&ciphertext, &key, &nonce)
            .try_into()
            .map_err(|_| "노드 지갑 개인키 길이 오류")?;
        if doc.version == 1 && !doc.address.starts_with("0x") {
            return Err("legacy-ed25519-keystore".into());
        }
        let wallet = AccountWallet::from_private_key(private_key)?;
        if wallet.address() != doc.address {
            return Err("노드 지갑 주소 검증 실패".into());
        }
        Ok(wallet)
    }

    pub fn store(path: &Path, wallet: &AccountWallet, password: &str) -> Result<(), String> {
        if password.len() < 10 {
            return Err("노드 지갑 비밀번호는 10자 이상이어야 합니다.".into());
        }
        let mut salt = [0; 32];
        let mut nonce = [0; 32];
        OsRng.fill_bytes(&mut salt);
        OsRng.fill_bytes(&mut nonce);
        let key = derive_key(password.as_bytes(), &salt, KDF_ROUNDS);
        let ciphertext = crypt(&wallet.private_key_bytes(), &key, &nonce);
        let doc = Document {
            version: 2,
            address: wallet.address(),
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(&ciphertext),
            mac: hex::encode(mac(&key, &nonce, &ciphertext)),
            kdf_rounds: KDF_ROUNDS,
        };
        let bytes = serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?;
        atomic_verified_replace(path, &bytes, password, &wallet.address())
    }

    pub fn change_password(path: &Path, old: &str, new: &str) -> Result<(), String> {
        let wallet = Self::load(path, old)?;
        Self::store(path, &wallet, new)?;
        if Self::load(path, new)?.address() != wallet.address() {
            return Err("재암호화 후 주소 불일치".into());
        }
        Ok(())
    }

    fn migrate_legacy(path: &Path, password: &str) -> Result<AccountWallet, String> {
        let bytes = fs::read(path).map_err(|e| format!("노드 지갑 읽기 실패: {e}"))?;
        let doc: Document = serde_json::from_slice(&bytes)
            .map_err(|_| "node_wallet.keystore가 손상되었습니다.".to_string())?;
        let salt = decode_32(&doc.salt)?;
        let nonce = decode_32(&doc.nonce)?;
        let ciphertext = hex::decode(&doc.ciphertext).map_err(|_| "노드 지갑 암호문 오류")?;
        let key = derive_key(password.as_bytes(), &salt, doc.kdf_rounds);
        if mac(&key, &nonce, &ciphertext) != decode_32(&doc.mac)? {
            return Err("노드 지갑 비밀번호가 틀렸거나 파일이 변조되었습니다.".into());
        }
        let private_key: [u8; 32] = crypt(&ciphertext, &key, &nonce)
            .try_into()
            .map_err(|_| "노드 지갑 개인키 길이 오류")?;
        let backup = PathBuf::from(format!("{}.ed25519.bak", path.display()));
        if !backup.exists() {
            fs::copy(path, &backup).map_err(|e| format!("구형 보상 지갑 백업 실패: {e}"))?;
            set_private_permissions(&backup)?;
        }
        let wallet = AccountWallet::from_private_key(private_key)?;
        Self::store(path, &wallet, password)?;
        eprintln!(
            "[지갑 이관] 구형 Ed25519 보상 keystore를 {}에 백업하고 0x 계정으로 전환했습니다: {}",
            backup.display(),
            wallet.address()
        );
        Ok(wallet)
    }
}

fn read_password(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("노드 지갑 비밀번호 파일 읽기 실패: {e}"))
}
fn random_password() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
fn derive_key(password: &[u8], salt: &[u8; 32], rounds: u32) -> [u8; 32] {
    let mut d = Sha256::new();
    d.update(b"IEUM-NODE-WALLET-KDF-V1");
    d.update(salt);
    d.update(password);
    let mut k: [u8; 32] = d.finalize().into();
    for r in 1..rounds {
        let mut d = Sha256::new();
        d.update(k);
        d.update(salt);
        d.update(r.to_be_bytes());
        k = d.finalize().into();
    }
    k
}
fn crypt(input: &[u8], key: &[u8; 32], nonce: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for (i, c) in input.chunks(32).enumerate() {
        let mut d = Sha256::new();
        d.update(b"IEUM-NODE-WALLET-STREAM-V1");
        d.update(key);
        d.update(nonce);
        d.update((i as u64).to_be_bytes());
        let s = d.finalize();
        out.extend(c.iter().zip(s.iter()).map(|(a, b)| *a ^ *b));
    }
    out
}
fn mac(key: &[u8; 32], nonce: &[u8; 32], ciphertext: &[u8]) -> [u8; 32] {
    let mut d = Sha256::new();
    d.update(b"IEUM-NODE-WALLET-MAC-V1");
    d.update(key);
    d.update(nonce);
    d.update(ciphertext);
    d.finalize().into()
}
fn decode_32(v: &str) -> Result<[u8; 32], String> {
    hex::decode(v)
        .map_err(|_| "노드 지갑 hex 오류".to_string())?
        .try_into()
        .map_err(|_| "노드 지갑 필드 길이 오류".to_string())
}
fn write_private_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o
        .open(path)
        .map_err(|e| format!("비밀번호 파일 생성 실패: {e}"))?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

fn set_private_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("개인 파일 권한 설정 실패: {e}"))?;
    }
    Ok(())
}
fn atomic_verified_replace(
    path: &Path,
    bytes: &[u8],
    password: &str,
    address: &str,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    if NodeWalletKeystore::load(&temporary, password)?.address() != address {
        let _ = fs::remove_file(&temporary);
        return Err("임시 node_wallet.keystore 검증 실패".into());
    }
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_password_change() {
        let root = std::env::temp_dir().join(format!("ieum-node-wallet-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("node_wallet.keystore");
        let w = AccountWallet::from_private_key([9; 32]).unwrap();
        NodeWalletKeystore::store(&path, &w, "initial-password").unwrap();
        NodeWalletKeystore::change_password(&path, "initial-password", "replacement-password")
            .unwrap();
        assert_eq!(
            NodeWalletKeystore::load(&path, "replacement-password")
                .unwrap()
                .address(),
            w.address()
        );
        assert!(w.address().starts_with("0x"));
        assert_eq!(w.address().len(), 42);
        assert!(NodeWalletKeystore::load(&path, "initial-password").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_ed25519_document_is_backed_up_and_migrated() {
        let root =
            std::env::temp_dir().join(format!("ieum-node-wallet-legacy-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("node_wallet.keystore");
        let password_path = root.join("node_wallet.password");
        let password = "legacy-test-password";
        fs::write(&password_path, password).unwrap();
        let legacy_seed = [7; 32];
        let legacy = crate::Wallet::from_seed(legacy_seed);
        let salt = [1; 32];
        let nonce = [2; 32];
        let key = derive_key(password.as_bytes(), &salt, KDF_ROUNDS);
        let ciphertext = crypt(&legacy_seed, &key, &nonce);
        let doc = Document {
            version: 1,
            address: legacy.address(),
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(&ciphertext),
            mac: hex::encode(mac(&key, &nonce, &ciphertext)),
            kdf_rounds: KDF_ROUNDS,
        };
        fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

        let migrated = NodeWalletKeystore::load_or_create_default(&path, &password_path).unwrap();
        assert!(migrated.address().starts_with("0x"));
        assert!(PathBuf::from(format!("{}.ed25519.bak", path.display())).exists());
        assert_eq!(
            NodeWalletKeystore::load(&path, password).unwrap().address(),
            migrated.address()
        );
        let _ = fs::remove_dir_all(root);
    }
}

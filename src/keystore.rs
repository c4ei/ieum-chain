use crate::account::AccountWallet;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const KDF_ROUNDS: u32 = 200_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeystoreFile {
    version: u32,
    address: String,
    salt: String,
    nonce: String,
    ciphertext: String,
    mac: String,
    kdf_rounds: u32,
}

/// 개인키를 평문으로 저장하지 않는 IEUM 로컬 keystore입니다.
///
/// 외부 노출 RPC에서는 사용하지 말고 로컬 지갑 프로세스에서만 잠금 해제합니다.
/// 파일 교체는 임시 파일과 rename으로 원자적으로 수행합니다.
#[derive(Clone, Debug)]
pub struct Keystore {
    root: PathBuf,
}

impl Keystore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        Ok(Self { root })
    }

    pub fn store(&self, wallet: &AccountWallet, password: &str) -> Result<String, String> {
        validate_password(password)?;
        let mut salt = [0_u8; 32];
        let mut nonce = [0_u8; 32];
        OsRng.fill_bytes(&mut salt);
        OsRng.fill_bytes(&mut nonce);
        let key = derive_key(password.as_bytes(), &salt, KDF_ROUNDS);
        let plaintext = wallet.private_key_bytes();
        let ciphertext = crypt(&plaintext, &key, &nonce);
        let mac = calculate_mac(&key, &nonce, &ciphertext);
        let address = wallet.address();
        let document = KeystoreFile {
            version: 1,
            address: address.clone(),
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
            mac: hex::encode(mac),
            kdf_rounds: KDF_ROUNDS,
        };
        let path = self.new_path_for(&address)?;
        atomic_write(
            &path,
            &serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?,
        )?;
        Ok(address)
    }

    pub fn load(&self, address: &str, password: &str) -> Result<AccountWallet, String> {
        let path = self.find_path(address)?;
        let bytes = fs::read(path).map_err(|_| "keystore 파일이 없습니다.")?;
        let document: KeystoreFile =
            serde_json::from_slice(&bytes).map_err(|_| "keystore 파일이 손상되었습니다.")?;
        if document.version != 1 || document.kdf_rounds < KDF_ROUNDS {
            return Err("지원하지 않거나 너무 약한 keystore 버전입니다.".into());
        }
        let salt = decode_32(&document.salt)?;
        let nonce = decode_32(&document.nonce)?;
        let ciphertext = hex::decode(&document.ciphertext)
            .map_err(|_| "keystore 암호문이 올바르지 않습니다.")?;
        let expected_mac = decode_32(&document.mac)?;
        let key = derive_key(password.as_bytes(), &salt, document.kdf_rounds);
        if calculate_mac(&key, &nonce, &ciphertext) != expected_mac {
            return Err("keystore 비밀번호가 틀렸거나 파일이 변조되었습니다.".into());
        }
        let plaintext = crypt(&ciphertext, &key, &nonce);
        let private_key: [u8; 32] = plaintext
            .try_into()
            .map_err(|_| "keystore 개인키 길이가 잘못되었습니다.")?;
        let wallet = AccountWallet::from_private_key(private_key)?;
        if wallet.address() != document.address {
            return Err("keystore 주소 검증에 실패했습니다.".into());
        }
        Ok(wallet)
    }

    pub fn addresses(&self) -> Result<Vec<String>, String> {
        let mut values = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if !path.is_file() {
                continue;
            }
            if let Ok(bytes) = fs::read(&path)
                && let Ok(document) = serde_json::from_slice::<KeystoreFile>(&bytes)
                && is_address(&document.address)
            {
                values.push(normalize_address(&document.address));
            }
        }
        values.sort();
        values.dedup();
        Ok(values)
    }

    fn find_path(&self, address: &str) -> Result<PathBuf, String> {
        let normalized = normalize_address(address);
        let legacy = self.root.join(format!("{}.json", &normalized[2..]));
        if legacy.exists() {
            return Ok(legacy);
        }
        for entry in fs::read_dir(&self.root).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if !path.is_file() {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            let Ok(document) = serde_json::from_slice::<KeystoreFile>(&bytes) else {
                continue;
            };
            if normalize_address(&document.address) == normalized {
                return Ok(path);
            }
        }
        Err("keystore 파일이 없습니다.".into())
    }

    fn new_path_for(&self, address: &str) -> Result<PathBuf, String> {
        if self.find_path(address).is_ok() {
            return Err("같은 주소의 keystore가 이미 존재합니다.".into());
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "시스템 시간이 UNIX epoch 이전입니다.".to_string())?
            .as_millis();
        Ok(self.root.join(format!(
            "UTC--{timestamp}--{}",
            address.trim_start_matches("0x").to_ascii_lowercase()
        )))
    }
}

fn normalize_address(address: &str) -> String {
    format!(
        "0x{}",
        address.trim_start_matches("0x").to_ascii_lowercase()
    )
}

fn is_address(address: &str) -> bool {
    address.starts_with("0x")
        && address.len() == 42
        && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.len() < 10 {
        return Err("keystore 비밀번호는 10자 이상이어야 합니다.".into());
    }
    Ok(())
}

fn derive_key(password: &[u8], salt: &[u8; 32], rounds: u32) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"IEUM-KEYSTORE-KDF-V1");
    digest.update(salt);
    digest.update(password);
    let mut key: [u8; 32] = digest.finalize().into();
    for round in 1..rounds {
        let mut digest = Sha256::new();
        digest.update(key);
        digest.update(salt);
        digest.update(round.to_be_bytes());
        key = digest.finalize().into();
    }
    key
}

fn crypt(input: &[u8], key: &[u8; 32], nonce: &[u8; 32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    for (counter, chunk) in input.chunks(32).enumerate() {
        let mut digest = Sha256::new();
        digest.update(b"IEUM-KEYSTORE-STREAM-V1");
        digest.update(key);
        digest.update(nonce);
        digest.update((counter as u64).to_be_bytes());
        let stream = digest.finalize();
        output.extend(
            chunk
                .iter()
                .zip(stream.iter())
                .map(|(left, right)| *left ^ *right),
        );
    }
    output
}

fn calculate_mac(key: &[u8; 32], nonce: &[u8; 32], ciphertext: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"IEUM-KEYSTORE-MAC-V1");
    digest.update(key);
    digest.update(nonce);
    digest.update(ciphertext);
    digest.finalize().into()
}

fn decode_32(value: &str) -> Result<[u8; 32], String> {
    hex::decode(value)
        .map_err(|_| "keystore hex 필드가 잘못되었습니다.".to_string())?
        .try_into()
        .map_err(|_| "keystore 필드는 32바이트여야 합니다.".to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_keystore_round_trip_and_wrong_password() {
        let root = std::env::temp_dir().join(format!("ieum-keystore-{}", std::process::id()));
        let store = Keystore::new(&root).unwrap();
        let wallet = AccountWallet::from_private_key([7; 32]).unwrap();
        let address = store.store(&wallet, "correct-password").unwrap();
        assert_eq!(
            store.load(&address, "correct-password").unwrap().address(),
            address
        );
        assert!(store.load(&address, "wrong-password").is_err());
        let files: Vec<_> = fs::read_dir(&root).unwrap().collect();
        assert_eq!(files.len(), 1);
        assert!(
            files[0]
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("UTC--")
        );
        let _ = fs::remove_dir_all(root);
    }
}

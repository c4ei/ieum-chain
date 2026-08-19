use crate::{Validator, Wallet};
use rand_core::{OsRng, RngCore};
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const CHAIN_ID: &str = "21004";

#[derive(Serialize)]
struct ValidatorConfig<'a> {
    chain_id: &'a str,
    validators: Vec<Validator>,
}

pub fn generate_key_file(path: &Path) -> Result<String, String> {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    write_new_file(path, format!("{}\n", hex::encode(seed)).as_bytes(), true)?;
    Ok(Wallet::from_seed(seed).address())
}

pub fn public_key_from_file(path: &Path) -> Result<String, String> {
    Ok(wallet_from_file(path)?.address())
}

pub fn wallet_from_file(path: &Path) -> Result<Wallet, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("검증자 키 파일 읽기 실패({}): {error}", path.display()))?;
    let seed = parse_seed(&text)?;
    Ok(Wallet::from_seed(seed))
}

pub fn create_validators_config(
    path: &Path,
    public_keys: &[String],
    voting_power: u64,
) -> Result<(), String> {
    create_validators_config_for_chain(path, CHAIN_ID, public_keys, voting_power)
}

pub fn create_validators_config_for_chain(
    path: &Path,
    chain_id: &str,
    public_keys: &[String],
    voting_power: u64,
) -> Result<(), String> {
    if public_keys.is_empty() {
        return Err("검증자 공개키가 최소 1개 필요합니다.".into());
    }
    if voting_power == 0 {
        return Err("검증자 voting power는 0보다 커야 합니다.".into());
    }

    let mut seen = HashSet::new();
    let mut validators = Vec::with_capacity(public_keys.len());
    for value in public_keys {
        let id = normalize_public_key(value)?;
        if !seen.insert(id.clone()) {
            return Err(format!("중복된 검증자 공개키입니다: {id}"));
        }
        validators.push(Validator { id, voting_power });
    }

    let config = ValidatorConfig {
        chain_id,
        validators,
    };
    let mut json = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("검증자 설정 직렬화 실패: {error}"))?;
    json.push('\n');
    write_new_file(path, json.as_bytes(), false)
}

fn parse_seed(value: &str) -> Result<[u8; 32], String> {
    hex::decode(value.trim().trim_start_matches("0x"))
        .map_err(|_| "검증자 키 파일은 32바이트 hex여야 합니다.".to_string())?
        .try_into()
        .map_err(|_| "검증자 키 파일은 정확히 32바이트여야 합니다.".to_string())
}

fn normalize_public_key(value: &str) -> Result<String, String> {
    let normalized = value.trim().trim_start_matches("0x").to_ascii_lowercase();
    let bytes = hex::decode(&normalized)
        .map_err(|_| "검증자 공개키는 32바이트 hex여야 합니다.".to_string())?;
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "검증자 공개키는 정확히 32바이트여야 합니다.".to_string())?;
    ed25519_dalek::VerifyingKey::from_bytes(&key)
        .map_err(|_| "유효하지 않은 Ed25519 공개키입니다.".to_string())?;
    Ok(normalized)
}

fn write_new_file(path: &Path, contents: &[u8], private: bool) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("디렉터리 생성 실패({}): {error}", parent.display()))?;
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    let mut file = options.open(path).map_err(|error| {
        format!(
            "파일 생성 실패({}): {error}. 기존 파일은 안전을 위해 덮어쓰지 않습니다.",
            path.display()
        )
    })?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("파일 저장 실패({}): {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ieum-validator-key-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn generated_key_public_value_matches_file() {
        let path = temp_path("validator.key");
        let generated = generate_key_file(&path).unwrap();
        assert_eq!(generated, public_key_from_file(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap().trim().len(), 64);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn key_generation_never_overwrites_existing_file() {
        let path = temp_path("validator.key");
        fs::write(&path, "keep-me").unwrap();
        assert!(generate_key_file(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep-me");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn config_rejects_duplicate_public_keys() {
        let path = temp_path("validators.json");
        let key = Wallet::from_seed([1; 32]).address();
        assert!(
            create_validators_config(&path, &[key.clone(), key.clone(), key.clone(), key], 100)
                .is_err()
        );
        assert!(!path.exists());
    }

    #[test]
    fn config_contains_ordered_public_keys() {
        let path = temp_path("validators.json");
        let keys: Vec<_> = (1..=4)
            .map(|index| Wallet::from_seed([index; 32]).address())
            .collect();
        create_validators_config(&path, &keys, 100).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["chain_id"], CHAIN_ID);
        assert_eq!(config["validators"].as_array().unwrap().len(), 4);
        assert_eq!(config["validators"][0]["id"], keys[0]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn config_allows_one_genesis_validator_during_bootstrap() {
        let path = temp_path("single-validator.json");
        let key = Wallet::from_seed([9; 32]).address();
        create_validators_config(&path, std::slice::from_ref(&key), 100).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["validators"].as_array().unwrap().len(), 1);
        assert_eq!(config["validators"][0]["id"], key);
        fs::remove_file(path).unwrap();
    }
}

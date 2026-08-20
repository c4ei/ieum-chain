use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MARKER_NAME: &str = ".ieum-initialized";
const MAINNET_GENESIS_MARKER_VERSION: &str = "v0.23.9-foundation-allocation";

/// 서명된 바이너리에 포함된 메인넷 Genesis를 실행 디렉터리의 설정 파일과 맞춥니다.
/// 기존 파일은 최초 전환 시 한 번 보존하고, 임시 파일을 rename해 부분 쓰기를 막습니다.
pub fn synchronize_bundled_mainnet_genesis(path: &Path) -> Result<(), String> {
    let bundled = include_str!("../config/genesis.json");
    let genesis: ieum_chain::genesis::GenesisConfig = serde_json::from_str(bundled)
        .map_err(|error| format!("번들 메인넷 Genesis 읽기 실패: {error}"))?;
    genesis.validate_production_safety()?;
    if genesis.chain_id != 21_004
        || genesis.network_name != "ieum-mainnet"
        || genesis.genesis_time != ieum_chain::genesis::IEUM_MAINNET_GENESIS_TIME
    {
        return Err("번들 메인넷 Genesis 신원이 v0.23.9 기준과 다릅니다.".into());
    }
    if fs::read_to_string(path).ok().as_deref() == Some(bundled) {
        return Ok(());
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Genesis 설정 폴더 생성 실패: {error}"))?;
    }
    if path.exists() {
        let backup = path.with_file_name("genesis.pre-foundation-allocation-20260820.json");
        if !backup.exists() {
            fs::copy(path, &backup).map_err(|error| {
                format!("기존 Genesis 백업 실패({}): {error}", backup.display())
            })?;
        }
    }
    let temporary = path.with_file_name(".genesis.json.new");
    fs::write(&temporary, bundled)
        .map_err(|error| format!("새 Genesis 임시 저장 실패: {error}"))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("새 Genesis 원자적 교체 실패: {error}"))?;
    println!("[메인넷 Genesis 동기화] {}", path.display());
    Ok(())
}

/// v0.23.5 새 메인넷을 처음 실행할 때 기존 시험 원장을 삭제하지 않고 옆으로 보존합니다.
/// 같은 Genesis marker가 있으면 이후 Docker 재시작에서는 아무 작업도 하지 않습니다.
pub fn prepare_mainnet_ledger_transition(
    ledger_dir: &Path,
    genesis_commitment: &str,
) -> Result<Option<PathBuf>, String> {
    let data_dir = ledger_dir.parent().unwrap_or(ledger_dir);
    let ledger_name = ledger_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("ledger");
    let marker = data_dir.join(format!(
        ".ieum-mainnet-genesis-{MAINNET_GENESIS_MARKER_VERSION}-{ledger_name}"
    ));
    if marker.exists() {
        let recorded = fs::read_to_string(&marker).map_err(|error| error.to_string())?;
        if recorded.trim() != genesis_commitment {
            return Err("메인넷 원장 marker의 Genesis hash가 현재 바이너리와 다릅니다.".into());
        }
        return Ok(None);
    }
    let has_data = ledger_dir
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    let backup = data_dir.join(format!("{ledger_name}.pre-foundation-allocation-20260820"));
    let moved = if has_data {
        if backup.exists() {
            return Err(format!(
                "기존 메인넷 전환 백업이 이미 있습니다: {}",
                backup.display()
            ));
        }
        fs::rename(ledger_dir, &backup)
            .map_err(|error| format!("기존 시험 원장 백업 실패: {error}"))?;
        Some(backup)
    } else {
        None
    };
    fs::create_dir_all(ledger_dir)
        .map_err(|error| format!("새 메인넷 원장 폴더 생성 실패: {error}"))?;
    fs::write(&marker, format!("{genesis_commitment}\n"))
        .map_err(|error| format!("메인넷 Genesis marker 저장 실패: {error}"))?;
    Ok(moved)
}

/// 서버 최초 실행에 필요한 서버별 비밀키와 로컬 원장을 준비합니다.
///
/// 초기화 표시가 생긴 뒤 핵심 파일이 사라진 경우에는 새 신원을 자동 생성하지 않습니다.
/// marker와 키 파일의 신원이 다르면 자동 채택하지 않고 실행을 중단합니다.
pub fn prepare_server_files(
    validator_key: &Path,
    node_key: &Path,
    ledger_dir: &Path,
    validators_config: &Path,
    events_config: &Path,
    upgrades_config: &Path,
    allow_insecure_test_keys: bool,
) -> Result<(), String> {
    let marker = marker_path(ledger_dir);
    if marker.exists() {
        require_existing("validator.key", validator_key)?;
        require_existing("server.node.key", node_key)?;
        if !ledger_dir.exists() {
            fs::create_dir_all(ledger_dir).map_err(|error| {
                format!("원장 자동 복구 실패({}): {error}", ledger_dir.display())
            })?;
            println!(
                "[자동 복구] 원장 경로가 없어 다시 만들었습니다: {}",
                ledger_dir.display()
            );
        }
        let validator_public_key = ieum_chain::validator_key::public_key_from_file(validator_key)?;
        verify_marker_identity(&marker, &validator_public_key, node_key)?;
        return prepare_validators_config(
            validators_config,
            &validator_public_key,
            allow_insecure_test_keys,
        );
    }

    let ledger_has_data = ledger_dir
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if ledger_has_data && (!validator_key.exists() || !node_key.exists()) {
        return Err(format!(
            "기존 원장({})은 있지만 서버 키가 없습니다. 기존 노드일 수 있어 \
             새 키를 자동 생성하지 않습니다. 백업을 복구해 주세요.",
            ledger_dir.display()
        ));
    }

    fs::create_dir_all(ledger_dir)
        .map_err(|error| format!("원장 폴더 생성 실패({}): {error}", ledger_dir.display()))?;

    let validator_public_key = if validator_key.exists() {
        ieum_chain::validator_key::public_key_from_file(validator_key)?
    } else {
        let public_key = ieum_chain::validator_key::generate_key_file(validator_key)?;
        println!("[자동 생성] {}", validator_key.display());
        public_key
    };

    let identity = if node_key.exists() {
        ieum_chain::node_key::load_or_create_node_key(node_key)?
    } else {
        let key = ieum_chain::node_key::load_or_create_node_key(node_key)?;
        println!("[자동 생성] {}", node_key.display());
        key
    };
    create_if_missing(
        events_config,
        "{\n  \"events\": []\n}\n",
        "예약 이벤트 설정",
    )?;
    create_if_missing(
        upgrades_config,
        "{\n  \"upgrades\": []\n}\n",
        "업그레이드 설정",
    )?;
    write_marker(
        &marker,
        &validator_public_key,
        &libp2p::PeerId::from(identity.public()).to_string(),
    )?;

    println!("[자동 생성] {}", ledger_dir.display());
    println!("[초기 설정 완료] 검증자 공개키: {validator_public_key}");
    println!(
        "[초기 설정 완료] PeerId: {}",
        libp2p::PeerId::from(identity.public())
    );
    println!("위 공개키만 관리자에게 전달하세요. validator.key 내용은 절대 공유하지 마세요.");

    prepare_validators_config(
        validators_config,
        &validator_public_key,
        allow_insecure_test_keys,
    )
}

/// 검증자/노드 신원은 그대로 두고 손상되었을 수 있는 원장만 백업합니다.
/// 백업 폴더로 rename하므로 사용자가 필요하면 되돌릴 수 있습니다.
pub fn clean_ledger_preserving_identity(ledger_dir: &Path) -> Result<PathBuf, String> {
    let data_dir = ledger_dir.parent().unwrap_or(ledger_dir);
    let project_root = data_dir.parent().unwrap_or_else(|| Path::new("."));
    let backups_dir = project_root.join("backups");
    fs::create_dir_all(&backups_dir)
        .map_err(|error| format!("백업 폴더 생성 실패({}): {error}", backups_dir.display()))?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("백업 시각 생성 실패: {error}"))?
        .as_secs();
    let backup = (0..=999)
        .map(|suffix| {
            let name = if suffix == 0 {
                format!("ledger-clean-{timestamp}")
            } else {
                format!("ledger-clean-{timestamp}-{suffix}")
            };
            backups_dir.join(name)
        })
        .find(|path| !path.exists())
        .ok_or("원장 백업 폴더 이름을 만들 수 없습니다.")?;
    if ledger_dir.exists() {
        fs::rename(ledger_dir, &backup).map_err(|error| {
            format!(
                "원장 백업 이동 실패({} -> {}): {error}",
                ledger_dir.display(),
                backup.display()
            )
        })?;
    } else {
        fs::create_dir_all(&backup)
            .map_err(|error| format!("빈 원장 백업 폴더 생성 실패: {error}"))?;
    }
    fs::create_dir_all(ledger_dir)
        .map_err(|error| format!("새 원장 폴더 생성 실패({}): {error}", ledger_dir.display()))?;
    Ok(backup)
}

fn verify_marker_identity(
    marker: &Path,
    validator_public_key: &str,
    node_key: &Path,
) -> Result<(), String> {
    let contents = fs::read_to_string(marker)
        .map_err(|error| format!("초기화 표시 파일 읽기 실패({}): {error}", marker.display()))?;
    let expected_validator = marker_value(&contents, "validator_public_key")
        .ok_or("초기화 표시 파일에 validator_public_key가 없습니다.")?;
    let expected_peer =
        marker_value(&contents, "peer_id").ok_or("초기화 표시 파일에 peer_id가 없습니다.")?;
    let identity = ieum_chain::node_key::load_or_create_node_key(node_key)?;
    let actual_peer = libp2p::PeerId::from(identity.public()).to_string();
    if expected_validator != validator_public_key {
        return Err(
            "validator.key가 최초 초기화 때의 키와 다릅니다. 자동 생성하지 않고 실행을 중단합니다."
                .into(),
        );
    }
    if expected_peer != actual_peer {
        return Err(format!(
            "server.node.key의 PeerId가 최초 초기화 기록과 다릅니다. \
             자동으로 교체하지 않고 실행을 중단합니다(기록: {expected_peer}, 현재: {actual_peer})."
        ));
    }
    Ok(())
}

fn marker_value<'a>(contents: &'a str, name: &str) -> Option<&'a str> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
}

fn prepare_validators_config(
    path: &Path,
    _local_validator_public_key: &str,
    allow_insecure_test_keys: bool,
) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if allow_insecure_test_keys {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../config/validators_test.json"))
                .map_err(|error| format!("번들 CI 검증자 설정 읽기 실패: {error}"))?;
        if config["chain_id"] != "21005" {
            return Err("CI 검증자 설정 chain_id는 21005여야 합니다.".into());
        }
        let public_keys = config["validators"]
            .as_array()
            .ok_or("CI 검증자 배열이 없습니다.")?
            .iter()
            .map(|validator| {
                validator["id"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "CI 검증자 ID 형식이 올바르지 않습니다.".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        ieum_chain::validator_key::create_validators_config_for_chain(
            path,
            "21005",
            &public_keys,
            100,
        )?;
        println!("[CI 검증자 설정 자동 생성] {}", path.display());
        return Ok(());
    }
    let genesis: ieum_chain::genesis::GenesisConfig =
        serde_json::from_str(include_str!("../config/genesis.json"))
            .map_err(|error| format!("번들 제네시스 읽기 실패: {error}"))?;
    genesis.validate()?;
    let public_keys = genesis
        .validators
        .iter()
        .map(|validator| validator.id.clone())
        .collect::<Vec<_>>();
    ieum_chain::validator_key::create_validators_config_for_chain(
        path,
        &genesis.chain_id.to_string(),
        &public_keys,
        100,
    )?;
    println!("[제네시스 검증자 설정 복원] {}", path.display());
    println!(
        "[신규 노드 모드] 제네시스 검증자 집합으로 동기화를 시작합니다. \
         로컬 키는 서명 후보로 자동 전송되며 승인 전에는 일반 동기화 노드로 동작합니다."
    );
    Ok(())
}

fn marker_path(ledger_dir: &Path) -> PathBuf {
    ledger_dir.parent().unwrap_or(ledger_dir).join(MARKER_NAME)
}

/// 기존 서버 신원과 로컬 상태를 자동 백업한 뒤 신규 서버 신원을 만듭니다.
pub fn initialize_new_server_node(
    validator_key: &Path,
    node_key: &Path,
    ledger_dir: &Path,
    validators_config: &Path,
    events_config: &Path,
    upgrades_config: &Path,
) -> Result<Option<PathBuf>, String> {
    let backup =
        backup_existing_node_state(validator_key, node_key, ledger_dir, validators_config)?;
    prepare_server_files(
        validator_key,
        node_key,
        ledger_dir,
        validators_config,
        events_config,
        upgrades_config,
        false,
    )?;
    Ok(backup)
}

fn backup_existing_node_state(
    validator_key: &Path,
    node_key: &Path,
    ledger_dir: &Path,
    validators_config: &Path,
) -> Result<Option<PathBuf>, String> {
    let data_dir = ledger_dir.parent().unwrap_or(ledger_dir);
    if !data_dir.exists()
        && !validator_key.exists()
        && !node_key.exists()
        && !validators_config.exists()
    {
        return Ok(None);
    }

    let project_root = data_dir.parent().unwrap_or_else(|| Path::new("."));
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("백업 시각 생성 실패: {error}"))?
        .as_secs();
    let backups_dir = project_root.join("backups");
    fs::create_dir_all(&backups_dir).map_err(|error| {
        format!(
            "신규 노드 백업 상위 폴더 생성 실패({}): {error}",
            backups_dir.display()
        )
    })?;
    let backup_root = (0..=999)
        .map(|suffix| {
            if suffix == 0 {
                backups_dir.join(format!("node-init-{timestamp}"))
            } else {
                backups_dir.join(format!("node-init-{timestamp}-{suffix}"))
            }
        })
        .find(|path| !path.exists())
        .ok_or("신규 노드 백업 폴더 이름을 만들 수 없습니다.")?;
    let backup_config = backup_root.join("config");
    fs::create_dir_all(&backup_config).map_err(|error| {
        format!(
            "신규 노드 백업 폴더 생성 실패({}): {error}",
            backup_root.display()
        )
    })?;

    let backup_data = backup_root.join("data");
    let moved_data = data_dir.exists();
    if moved_data {
        fs::rename(data_dir, &backup_data).map_err(|error| {
            format!(
                "기존 data 백업 이동 실패({} -> {}): {error}",
                data_dir.display(),
                backup_data.display()
            )
        })?;
    }

    if validator_key.exists() {
        let backup_validator = backup_config.join(
            validator_key
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("validator.key")),
        );
        if let Err(error) = fs::rename(validator_key, &backup_validator) {
            if moved_data {
                fs::rename(&backup_data, data_dir).map_err(|rollback_error| {
                    format!(
                        "validator.key 백업 이동 실패({error}), data 원상복구도 실패했습니다 \
                         ({} -> {}): {rollback_error}. 백업 상태를 직접 확인하세요.",
                        backup_data.display(),
                        data_dir.display()
                    )
                })?;
            }
            return Err(format!(
                "validator.key 백업 이동 실패({} -> {}): {error}. \
                 data 이동은 원상복구했습니다.",
                validator_key.display(),
                backup_validator.display()
            ));
        }
    }

    if validators_config.exists() {
        let backup_validators = backup_config.join("validators.json");
        fs::rename(validators_config, &backup_validators).map_err(|error| {
            format!(
                "기존 validators.json 백업 이동 실패({} -> {}): {error}. \
                 신규 초기화를 중단했으므로 백업 상태를 확인하세요.",
                validators_config.display(),
                backup_validators.display()
            )
        })?;
    }

    Ok(Some(backup_root))
}

/// 기존 서버의 두 키와 최초 초기화 marker가 정확히 일치하는지 검사합니다.
pub fn verify_server_node(
    validator_key: &Path,
    node_key: &Path,
    ledger_dir: &Path,
) -> Result<(String, String), String> {
    let marker = marker_path(ledger_dir);
    require_existing(".ieum-initialized", &marker)?;
    require_existing("validator.key", validator_key)?;
    require_existing("server.node.key", node_key)?;
    if !ledger_dir.is_dir() {
        return Err(format!(
            "기존 원장 경로가 없습니다: {}",
            ledger_dir.display()
        ));
    }
    let validator_public_key = ieum_chain::validator_key::public_key_from_file(validator_key)?;
    verify_marker_identity(&marker, &validator_public_key, node_key)?;
    let identity = ieum_chain::node_key::load_or_create_node_key(node_key)?;
    let peer_id = libp2p::PeerId::from(identity.public()).to_string();
    Ok((validator_public_key, peer_id))
}

fn require_existing(name: &str, path: &Path) -> Result<(), String> {
    if path.exists() {
        Ok(())
    } else {
        Err(format!(
            "기존 IEUM 설치에서 {name} 파일이 없어 실행을 중단합니다: {}. \
             새 키를 자동 생성하면 다른 노드가 되므로 백업 파일을 복구해 주세요.",
            path.display()
        ))
    }
}

fn create_if_missing(path: &Path, contents: &str, description: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{description} 폴더 생성 실패: {error}"))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("{description} 생성 실패({}): {error}", path.display()))?;
    file.write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("{description} 저장 실패({}): {error}", path.display()))?;
    println!("[자동 생성] {}", path.display());
    Ok(())
}

fn write_marker(path: &Path, validator_public_key: &str, peer_id: &str) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("초기화 표시 파일 생성 실패({}): {error}", path.display()))?;
    writeln!(
        file,
        "version=1\nvalidator_public_key={validator_public_key}\npeer_id={peer_id}"
    )
    .and_then(|_| file.sync_all())
    .map_err(|error| format!("초기화 표시 파일 저장 실패({}): {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ieum-installation-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn first_run_restores_bundled_genesis_validators_and_starts() {
        let root = temp_root("first-run");
        prepare_server_files(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
            false,
        )
        .unwrap();
        assert!(root.join("config/validator.key").exists());
        assert!(root.join("data/server.node.key").exists());
        assert!(root.join("data/.ieum-initialized").exists());
        assert!(root.join("data/ledger").is_dir());
        assert!(root.join("config/events.json").exists());
        assert!(root.join("config/upgrades.json").exists());
        let config = fs::read_to_string(root.join("config/validators.json")).unwrap();
        assert_eq!(config.matches("\"id\"").count(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_mainnet_genesis_is_atomically_synchronized_and_backed_up() {
        let root = temp_root("mainnet-genesis-sync");
        let path = root.join("config/genesis.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"network_name\":\"ieum-devnet\"}\n").unwrap();
        synchronize_bundled_mainnet_genesis(&path).unwrap();
        let installed: ieum_chain::genesis::GenesisConfig =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(installed.chain_id, 21_004);
        assert_eq!(installed.network_name, "ieum-mainnet");
        assert_eq!(
            installed.genesis_time,
            ieum_chain::genesis::IEUM_MAINNET_GENESIS_TIME
        );
        assert!(
            root.join("config/genesis.pre-foundation-allocation-20260820.json")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mainnet_transition_preserves_old_ledger_once() {
        let root = temp_root("mainnet-ledger-transition");
        let ledger = root.join("data/ledger");
        fs::create_dir_all(&ledger).unwrap();
        fs::write(ledger.join("old-block"), "test-chain").unwrap();
        let backup = prepare_mainnet_ledger_transition(&ledger, "new-genesis")
            .unwrap()
            .unwrap();
        assert!(backup.join("old-block").exists());
        assert!(ledger.is_dir());
        assert!(
            prepare_mainnet_ledger_transition(&ledger, "new-genesis")
                .unwrap()
                .is_none()
        );
        assert!(prepare_mainnet_ledger_transition(&ledger, "other-genesis").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initialized_node_never_regenerates_missing_validator_key() {
        let root = temp_root("missing-key");
        fs::create_dir_all(root.join("data/ledger")).unwrap();
        fs::write(root.join("data/.ieum-initialized"), "version=1\n").unwrap();
        fs::write(root.join("data/server.node.key"), "keep").unwrap();
        let key = root.join("config/validator.key");
        let error = prepare_server_files(
            &key,
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
            false,
        )
        .unwrap_err();
        assert!(error.contains("자동 생성하면 다른 노드"));
        assert!(!key.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn initialized_node_rejects_replaced_node_key() {
        let root = temp_root("replaced-node-key");
        prepare_server_files(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
            false,
        )
        .unwrap();
        fs::remove_file(root.join("data/server.node.key")).unwrap();
        ieum_chain::node_key::load_or_create_node_key(root.join("data/server.node.key")).unwrap();
        let error = prepare_server_files(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
            false,
        )
        .unwrap_err();
        assert!(error.contains("자동으로 교체하지 않고"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_new_node_backs_up_copied_state() {
        let root = temp_root("explicit-new-backup");
        fs::create_dir_all(root.join("data/ledger")).unwrap();
        fs::write(root.join("data/ledger/copied.db"), "old").unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/validator.key"), "old-key").unwrap();
        fs::write(
            root.join("config/validators.json"),
            "{\"chain_id\":\"21004\",\"validators\":[]}\n",
        )
        .unwrap();
        let backup = initialize_new_server_node(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            fs::read_to_string(backup.join("data/ledger/copied.db")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(backup.join("config/validator.key")).unwrap(),
            "old-key"
        );
        assert!(backup.join("config/validators.json").exists());
        let validators = fs::read_to_string(root.join("config/validators.json")).unwrap();
        assert_eq!(validators.matches("\"id\"").count(), 4);
        assert!(root.join("config/validator.key").exists());
        assert!(root.join("data/server.node.key").exists());
        assert!(root.join("data/.ieum-initialized").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_new_node_then_verify_succeeds() {
        let root = temp_root("explicit-new-verify");
        initialize_new_server_node(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
        )
        .unwrap();
        verify_server_node(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
        )
        .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn insecure_testnet_creates_shared_validator_config_automatically() {
        let root = temp_root("testnet-auto-config");
        prepare_server_files(
            &root.join("config/validator.key"),
            &root.join("data/server.node.key"),
            &root.join("data/ledger"),
            &root.join("config/validators.json"),
            &root.join("config/events.json"),
            &root.join("config/upgrades.json"),
            true,
        )
        .unwrap();
        let config = fs::read_to_string(root.join("config/validators.json")).unwrap();
        assert_eq!(config.matches("\"id\"").count(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_ledger_preserves_identity_files_and_is_recoverable() {
        let root = temp_root("clean-ledger");
        fs::create_dir_all(root.join("data/ledger")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("data/ledger/state.db"), "state").unwrap();
        fs::write(root.join("data/server.node.key"), "node-key").unwrap();
        fs::write(root.join("config/validator.key"), "validator-key").unwrap();

        let backup = clean_ledger_preserving_identity(&root.join("data/ledger")).unwrap();

        assert_eq!(
            fs::read_to_string(backup.join("state.db")).unwrap(),
            "state"
        );
        assert!(root.join("data/ledger").is_dir());
        assert_eq!(
            fs::read_to_string(root.join("data/server.node.key")).unwrap(),
            "node-key"
        );
        assert_eq!(
            fs::read_to_string(root.join("config/validator.key")).unwrap(),
            "validator-key"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

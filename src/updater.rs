use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateArtifact {
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub protocol_version: u32,
    pub mandatory: bool,
    pub published_at: u64,
    #[serde(default)]
    pub notes: String,
    pub artifacts: BTreeMap<String, UpdateArtifact>,
    pub signature: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateResult {
    Current,
    Installed,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AutoUpdateConfig {
    #[serde(default)]
    pub enabled: bool,
    pub manifest_url: String,
    pub release_public_key: String,
    #[serde(default = "default_check_interval_secs")]
    pub check_interval_secs: u64,
}

fn default_check_interval_secs() -> u64 {
    6 * 60 * 60
}

impl AutoUpdateConfig {
    pub fn load_if_enabled(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(path).map_err(|error| {
            format!("자동 업데이트 설정 읽기 실패({}): {error}", path.display())
        })?;
        let config: Self = serde_json::from_str(&text).map_err(|error| {
            format!("자동 업데이트 설정 형식 오류({}): {error}", path.display())
        })?;
        if !config.enabled {
            return Ok(None);
        }
        if config.check_interval_secs < 60 {
            return Err("자동 업데이트 확인 간격은 최소 60초여야 합니다.".into());
        }
        if !config.manifest_url.starts_with("https://") {
            return Err("자동 업데이트 manifest 주소는 HTTPS여야 합니다.".into());
        }
        decode_fixed::<32>(&config.release_public_key, "릴리스 공개키")?;
        Ok(Some(config))
    }

    /// 각 인스턴스는 자기 실행 파일 옆의 설정만 신뢰합니다. 한 서버에 여러
    /// ieum-node 디렉터리가 있어도 다른 인스턴스의 업데이트 설정을 읽지 않습니다.
    pub fn discover() -> Result<Option<(PathBuf, Self)>, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("현재 실행 바이너리 경로 확인 실패: {error}"))?;
        let directory = executable
            .parent()
            .ok_or("현재 실행 바이너리의 상위 폴더를 확인할 수 없습니다.")?;
        let path = directory.join("config/update.json");
        Ok(Self::load_if_enabled(&path)?.map(|config| (path, config)))
    }
}

#[derive(Serialize)]
struct UnsignedManifest<'a> {
    version: &'a str,
    protocol_version: u32,
    mandatory: bool,
    published_at: u64,
    notes: &'a str,
    artifacts: &'a BTreeMap<String, UpdateArtifact>,
}

impl UpdateManifest {
    pub fn verify(&self, release_public_key: &str) -> Result<(), String> {
        let public_bytes: [u8; 32] = decode_fixed(release_public_key, "릴리스 공개키")?;
        let signature_bytes: [u8; 64] = decode_fixed(&self.signature, "manifest 서명")?;
        let key = VerifyingKey::from_bytes(&public_bytes)
            .map_err(|_| "릴리스 공개키가 올바른 Ed25519 키가 아닙니다.")?;
        let signature = Signature::from_bytes(&signature_bytes);
        let payload = serde_json::to_vec(&UnsignedManifest {
            version: &self.version,
            protocol_version: self.protocol_version,
            mandatory: self.mandatory,
            published_at: self.published_at,
            notes: &self.notes,
            artifacts: &self.artifacts,
        })
        .map_err(|error| error.to_string())?;
        key.verify(&payload, &signature)
            .map_err(|_| "업데이트 manifest 서명이 일치하지 않습니다.".into())
    }
}

pub fn check_and_prompt(
    manifest_url: &str,
    release_public_key: &str,
    allow_install: bool,
) -> Result<(), String> {
    let manifest = fetch_manifest(manifest_url)?;
    manifest.verify(release_public_key)?;
    if !is_newer(env!("CARGO_PKG_VERSION"), &manifest.version)? {
        return Ok(());
    }
    eprintln!(
        "IEUM {} 업데이트가 있습니다(현재 {}).",
        manifest.version,
        env!("CARGO_PKG_VERSION")
    );
    if !manifest.notes.trim().is_empty() {
        eprintln!("{}", manifest.notes.trim());
    }
    if !allow_install {
        eprintln!("검증자 서버는 자동 교체하지 않습니다. 운영자가 릴리스를 검증한 뒤 배포하세요.");
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        eprintln!("대화형 터미널이 아니므로 업데이트 설치를 건너뜁니다.");
        return Ok(());
    }
    eprint!("서명된 업데이트를 다운로드하고 설치할까요? [y/N] ");
    io::stderr().flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Ok(());
    }
    install_current_platform(&manifest)
}

/// systemd의 ExecStartPre처럼 외부 서비스 관리자가 호출하는 비대화형 업데이트입니다.
/// 실행 중인 서버의 키·설정·원장은 건드리지 않고 현재 실행 파일만 교체합니다.
pub fn install_non_interactive(
    manifest_url: &str,
    release_public_key: &str,
) -> Result<UpdateResult, String> {
    let manifest = fetch_manifest(manifest_url)?;
    manifest.verify(release_public_key)?;
    if !is_newer(env!("CARGO_PKG_VERSION"), &manifest.version)? {
        return Ok(UpdateResult::Current);
    }
    install_current_platform(&manifest)?;
    Ok(UpdateResult::Installed)
}

/// P2P 알림은 새 버전 확인을 앞당기는 힌트일 뿐입니다. 실제 설치 여부는 로컬에
/// 고정된 URL과 공개키로 manifest 서명을 다시 검증해서 결정합니다.
pub fn install_if_newer(
    manifest_url: &str,
    release_public_key: &str,
) -> Result<UpdateResult, String> {
    install_non_interactive(manifest_url, release_public_key)
}

fn fetch_manifest(url: &str) -> Result<UpdateManifest, String> {
    // GitHub raw/CDN이 push 직후 이전 manifest를 캐시해 반환하지 않도록 매 검사마다
    // 고유 query를 붙입니다. 서명 검증 대상은 응답 본문이므로 신뢰 경계는 바뀌지 않습니다.
    let cache_busted = cache_busted_url(url, unix_timestamp_millis());
    let bytes = download_https(&cache_busted)?;
    serde_json::from_slice(&bytes).map_err(|error| format!("업데이트 manifest JSON 오류: {error}"))
}

fn cache_busted_url(url: &str, nonce: u128) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}ieum_update_check={nonce}")
}

fn unix_timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn install_current_platform(manifest: &UpdateManifest) -> Result<(), String> {
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let artifact = manifest
        .artifacts
        .get(&platform)
        .ok_or_else(|| format!("{platform}용 업데이트 파일이 없습니다."))?;
    let bytes = download_https(&artifact.url)?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(artifact.sha256.trim_start_matches("0x")) {
        return Err("업데이트 파일 SHA-256이 manifest와 다릅니다.".into());
    }
    validate_executable_bytes(&bytes)?;
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    stage_or_replace(&current, &bytes)
}

fn validate_executable_bytes(bytes: &[u8]) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if !bytes.starts_with(b"\x7fELF") {
        return Err(
            "Linux 업데이트 파일이 ELF 실행파일이 아닙니다. tar.xz URL이 아닌 무압축 바이너리 raw URL을 사용하세요."
                .into(),
        );
    }
    #[cfg(target_os = "windows")]
    if !bytes.starts_with(b"MZ") {
        return Err("Windows 업데이트 파일이 PE 실행파일이 아닙니다.".into());
    }
    Ok(())
}

#[cfg(unix)]
fn stage_or_replace(current: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let next = sibling(current, "new")?;
    let backup = sibling(current, "previous")?;
    fs::write(&next, bytes).map_err(|error| error.to_string())?;
    fs::set_permissions(&next, fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    if backup.exists() {
        fs::remove_file(&backup).map_err(|error| error.to_string())?;
    }
    fs::rename(current, &backup).map_err(|error| format!("기존 실행 파일 백업 실패: {error}"))?;
    if let Err(error) = fs::rename(&next, current) {
        let _ = fs::rename(&backup, current);
        return Err(format!("새 실행 파일 교체 실패(기존 파일 복구): {error}"));
    }
    eprintln!(
        "업데이트 설치 완료. 프로그램을 다시 실행하세요. 이전 파일: {}",
        backup.display()
    );
    Ok(())
}

#[cfg(not(unix))]
fn stage_or_replace(current: &Path, bytes: &[u8]) -> Result<(), String> {
    let next = sibling(current, "new.exe")?;
    fs::write(&next, bytes).map_err(|error| error.to_string())?;
    eprintln!(
        "업데이트를 {}에 저장했습니다. 종료 후 실행 파일을 교체하세요.",
        next.display()
    );
    Ok(())
}

fn sibling(current: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = current
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("현재 실행 파일 이름을 확인할 수 없습니다.")?;
    Ok(current.with_file_name(format!("{name}.{suffix}")))
}

pub fn is_newer(current: &str, candidate: &str) -> Result<bool, String> {
    fn parts(value: &str) -> Result<Vec<u64>, String> {
        value
            .trim_start_matches('v')
            .split('.')
            .map(|part| {
                part.parse::<u64>()
                    .map_err(|_| format!("잘못된 버전: {value}"))
            })
            .collect()
    }
    let mut current = parts(current)?;
    let mut candidate = parts(candidate)?;
    let length = current.len().max(candidate.len());
    current.resize(length, 0);
    candidate.resize(length, 0);
    Ok(candidate > current)
}

fn decode_fixed<const N: usize>(value: &str, name: &str) -> Result<[u8; N], String> {
    hex::decode(value.trim().trim_start_matches("0x"))
        .map_err(|_| format!("{name}은 hex여야 합니다."))?
        .try_into()
        .map_err(|_| format!("{name}은 {N}바이트여야 합니다."))
}

fn download_https(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("업데이트 주소는 HTTPS만 허용합니다.".into());
    }
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--header",
            "Cache-Control: no-cache",
            "--header",
            "Pragma: no-cache",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "120",
            url,
        ])
        .output()
        .map_err(|error| format!("curl 실행 실패: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "업데이트 다운로드 실패: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::{AutoUpdateConfig, is_newer, validate_executable_bytes};
    use std::fs;

    #[test]
    fn semantic_numeric_version_comparison() {
        assert!(is_newer("0.16.2", "0.17.0").unwrap());
        assert!(!is_newer("0.17.0", "0.17.0").unwrap());
        assert!(!is_newer("0.17.1", "0.17.0").unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn archive_cannot_replace_linux_executable() {
        let error = validate_executable_bytes(b"\xfd7zXZ\0").unwrap_err();
        assert!(error.contains("tar.xz"));
        assert!(validate_executable_bytes(b"\x7fELFrest").is_ok());
    }

    #[test]
    fn disabled_auto_update_config_is_ignored() {
        let path =
            std::env::temp_dir().join(format!("ieum-update-disabled-{}.json", std::process::id()));
        fs::write(
            &path,
            r#"{"enabled":false,"manifest_url":"","release_public_key":""}"#,
        )
        .unwrap();
        assert!(AutoUpdateConfig::load_if_enabled(&path).unwrap().is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn manifest_cache_buster_uses_existing_query_separator() {
        assert_eq!(
            super::cache_busted_url("https://example.com/update.json", 123),
            "https://example.com/update.json?ieum_update_check=123"
        );
        assert_eq!(
            super::cache_busted_url("https://example.com/update.json?ref=main", 123),
            "https://example.com/update.json?ref=main&ieum_update_check=123"
        );
    }
}

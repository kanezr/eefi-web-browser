use std::{path::PathBuf, time::Duration};

use futures_util::StreamExt;
use reqwest::Client;
use rusqlite::Connection;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tokio::{fs, io::AsyncWriteExt};

use crate::storage::{get_setting, set_setting, StorageService};

const RELEASES_API: &str =
    "https://api.github.com/repos/kanezr/eefi-web-browser/releases?per_page=100";
const DOWNLOADS_BASE: &str = "https://github.com/kanezr/eefi-web-browser/releases/download/";
const MAX_UPDATE_SIZE: u64 = 512 * 1024 * 1024;
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Stable,
    Beta,
}

impl UpdateChannel {
    fn as_setting(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterSettings {
    pub channel: UpdateChannel,
    pub automatic_updates: bool,
    pub check_on_startup: bool,
}

impl Default for UpdaterSettings {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::Stable,
            automatic_updates: false,
            check_on_startup: true,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug)]
struct UpdateCandidate {
    version: Version,
    asset_url: String,
    asset_size: u64,
    digest: String,
}

#[tauri::command]
pub async fn updater_settings_get(
    storage: State<'_, StorageService>,
) -> Result<UpdaterSettings, String> {
    storage.with_connection(read_settings).await
}

#[tauri::command]
pub async fn updater_settings_set(
    settings: UpdaterSettings,
    storage: State<'_, StorageService>,
) -> Result<UpdaterSettings, String> {
    storage
        .with_connection(move |connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|error| format!("could not begin updater settings update: {error}"))?;
            set_setting(
                &transaction,
                "updater.channel",
                settings.channel.as_setting(),
            )?;
            set_setting(
                &transaction,
                "updater.automatic_updates",
                if settings.automatic_updates {
                    "true"
                } else {
                    "false"
                },
            )?;
            set_setting(
                &transaction,
                "updater.check_on_startup",
                if settings.check_on_startup {
                    "true"
                } else {
                    "false"
                },
            )?;
            transaction
                .commit()
                .map_err(|error| format!("could not save updater settings: {error}"))?;
            Ok(settings)
        })
        .await
}

#[tauri::command]
pub fn updater_current_version() -> String {
    CURRENT_VERSION.to_string()
}

#[tauri::command]
pub async fn updater_check(storage: State<'_, StorageService>) -> Result<UpdateCheck, String> {
    let settings = storage.with_connection(read_settings).await?;
    let current_version = Version::parse(CURRENT_VERSION)
        .map_err(|error| format!("invalid application version: {error}"))?;
    let candidate = fetch_candidate(settings.channel, current_version.clone()).await?;
    Ok(UpdateCheck {
        current_version: current_version.to_string(),
        latest_version: candidate
            .as_ref()
            .map(|release| release.version.to_string()),
        update_available: candidate.is_some(),
    })
}

#[tauri::command]
pub async fn updater_install(
    version: String,
    app: AppHandle,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    let requested_version = Version::parse(&version)
        .map_err(|_| "The requested update version is invalid.".to_string())?;
    let settings = storage.with_connection(read_settings).await?;
    let current_version = Version::parse(CURRENT_VERSION)
        .map_err(|error| format!("invalid application version: {error}"))?;
    let candidate = fetch_candidate(settings.channel, current_version)
        .await?
        .ok_or_else(|| "No newer update is available on the selected channel.".to_string())?;
    if candidate.version != requested_version {
        return Err("The available release changed. Check for updates again.".to_string());
    }

    let target = std::env::current_exe()
        .map_err(|error| format!("could not locate the running browser: {error}"))?;
    let parent = target
        .parent()
        .ok_or_else(|| "the browser executable has no parent directory".to_string())?;
    let staged = parent.join(format!(
        ".eefi-browser-update-{}-{}.exe",
        candidate.version,
        std::process::id()
    ));
    download_update(&candidate, &staged).await?;
    crate::update_apply::launch_helper(&app, &target, &staged).await
}

fn read_settings(connection: &Connection) -> Result<UpdaterSettings, String> {
    let channel = match get_setting(connection, "updater.channel")?.as_deref() {
        None | Some("stable") => UpdateChannel::Stable,
        Some("beta") => UpdateChannel::Beta,
        Some(_) => return Err("stored updater channel is invalid".to_string()),
    };
    Ok(UpdaterSettings {
        channel,
        automatic_updates: read_bool_setting(connection, "updater.automatic_updates", false)?,
        check_on_startup: read_bool_setting(connection, "updater.check_on_startup", true)?,
    })
}

fn read_bool_setting(connection: &Connection, key: &str, default: bool) -> Result<bool, String> {
    match get_setting(connection, key)?.as_deref() {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err(format!("stored updater setting '{key}' is invalid")),
    }
}

async fn fetch_candidate(
    channel: UpdateChannel,
    current_version: Version,
) -> Result<Option<UpdateCandidate>, String> {
    let client = Client::builder()
        .user_agent(format!("Eefi-Browser/{CURRENT_VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| format!("could not configure update client: {error}"))?;
    let response = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("could not check GitHub releases: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub release check failed with HTTP {}.",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > 8 * 1024 * 1024)
    {
        return Err("GitHub returned an unexpectedly large release list.".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("could not read GitHub releases: {error}"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("GitHub returned an unexpectedly large release list.".to_string());
    }
    let releases: Vec<GitHubRelease> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("GitHub returned invalid release data: {error}"))?;
    select_candidate(releases, channel, current_version)
}

fn select_candidate(
    releases: Vec<GitHubRelease>,
    channel: UpdateChannel,
    current_version: Version,
) -> Result<Option<UpdateCandidate>, String> {
    let mut candidates = Vec::new();
    for release in releases {
        if release.draft {
            continue;
        }
        let version = release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name);
        let Ok(version) = Version::parse(version) else {
            continue;
        };
        if version <= current_version
            || (channel == UpdateChannel::Stable && (release.prerelease || !version.pre.is_empty()))
        {
            continue;
        }
        let expected_asset = format!("eefi-browser-{version}-windows-x64.exe");
        if !release
            .assets
            .iter()
            .any(|asset| asset.name == expected_asset)
        {
            continue;
        }
        candidates.push((version, release));
    }
    let Some((version, release)) = candidates
        .into_iter()
        .max_by(|(left, _), (right, _)| left.cmp(right))
    else {
        return Ok(None);
    };
    let expected_asset = format!("eefi-browser-{version}-windows-x64.exe");
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == expected_asset)
        .ok_or_else(|| "the selected release does not contain its update executable".to_string())?;
    if asset.size == 0 || asset.size > MAX_UPDATE_SIZE {
        return Err("the update executable has an invalid or excessive size.".to_string());
    }
    let digest = asset
        .digest
        .as_deref()
        .and_then(parse_sha256_digest)
        .ok_or_else(|| {
            "GitHub did not provide a valid SHA-256 digest for this update; refusing to install it."
                .to_string()
        })?;
    if !asset.browser_download_url.starts_with(DOWNLOADS_BASE) {
        return Err("the release contains an unexpected update download URL.".to_string());
    }
    Ok(Some(UpdateCandidate {
        version,
        asset_url: asset.browser_download_url,
        asset_size: asset.size,
        digest,
    }))
}

fn parse_sha256_digest(value: &str) -> Option<String> {
    let hex = value.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(hex.to_ascii_lowercase())
}

async fn download_update(candidate: &UpdateCandidate, destination: &PathBuf) -> Result<(), String> {
    let client = Client::builder()
        .user_agent(format!("Eefi-Browser/{CURRENT_VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| format!("could not configure update download: {error}"))?;
    let response = client
        .get(&candidate.asset_url)
        .send()
        .await
        .map_err(|error| format!("could not download the update: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Update download failed with HTTP {}.",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length != candidate.asset_size)
    {
        return Err("the update download size does not match the release metadata.".to_string());
    }

    let result = write_and_verify_update(response, candidate, destination).await;
    if result.is_err() {
        if let Err(error) = fs::remove_file(destination).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("could not remove incomplete updater file: {error}");
            }
        }
    }
    result
}

async fn write_and_verify_update(
    response: reqwest::Response,
    candidate: &UpdateCandidate,
    destination: &PathBuf,
) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .map_err(|error| format!("could not stage the update executable: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("update download was interrupted: {error}"))?;
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "update download size overflowed".to_string())?;
        if total > candidate.asset_size || total > MAX_UPDATE_SIZE {
            return Err("the update download exceeded its expected size.".to_string());
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("could not write the update executable: {error}"))?;
    }
    file.flush()
        .await
        .map_err(|error| format!("could not finish staging the update: {error}"))?;
    if total != candidate.asset_size {
        return Err("the update download is incomplete.".to_string());
    }
    let actual_digest = digest_to_hex(&hasher.finalize());
    if actual_digest != candidate.digest {
        return Err("the update executable failed its SHA-256 integrity check.".to_string());
    }
    Ok(())
}

fn digest_to_hex(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(*byte >> 4)] as char);
        encoded.push(HEX[usize::from(*byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn release(tag: &str, prerelease: bool) -> GitHubRelease {
        let version = tag.strip_prefix('v').unwrap_or(tag);
        GitHubRelease {
            tag_name: tag.to_string(),
            draft: false,
            prerelease,
            assets: vec![GitHubAsset {
                name: format!("eefi-browser-{version}-windows-x64.exe"),
                browser_download_url: format!(
                    "{DOWNLOADS_BASE}{tag}/eefi-browser-{version}-windows-x64.exe"
                ),
                size: 1024,
                digest: Some(format!("sha256:{}", "a".repeat(64))),
            }],
        }
    }

    #[test]
    fn stable_channel_skips_prereleases() {
        let releases = vec![release("v0.3.0-beta.1", true), release("v0.2.0", false)];
        let candidate = select_candidate(
            releases,
            UpdateChannel::Stable,
            Version::parse("0.1.0").expect("current version"),
        )
        .expect("select stable release")
        .expect("stable update");
        assert_eq!(
            candidate.version,
            Version::parse("0.2.0").expect("expected version")
        );
    }

    #[test]
    fn beta_channel_can_select_prereleases() {
        let releases = vec![release("v0.2.0", false), release("v0.3.0-beta.1", true)];
        let candidate = select_candidate(
            releases,
            UpdateChannel::Beta,
            Version::parse("0.2.0").expect("current version"),
        )
        .expect("select beta release")
        .expect("beta update");
        assert_eq!(
            candidate.version,
            Version::parse("0.3.0-beta.1").expect("expected version")
        );
    }

    #[test]
    fn sha256_digest_must_have_exactly_64_hex_digits() {
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{}", "A".repeat(64))),
            Some("a".repeat(64))
        );
        assert_eq!(parse_sha256_digest("sha256:abcd"), None);
        assert_eq!(
            parse_sha256_digest(&format!("sha1:{}", "a".repeat(64))),
            None
        );
    }

    #[test]
    fn sha256_digest_is_encoded_as_lowercase_hex() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");

        assert_eq!(
            digest_to_hex(&hasher.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn updater_preferences_have_safe_defaults_and_persisted_values() {
        let connection = Connection::open_in_memory().expect("open settings database");
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create settings table");
        assert_eq!(
            read_settings(&connection).expect("read default preferences"),
            UpdaterSettings::default()
        );

        set_setting(&connection, "updater.channel", "beta").expect("save channel");
        set_setting(&connection, "updater.automatic_updates", "true")
            .expect("save automatic updates");
        set_setting(&connection, "updater.check_on_startup", "false").expect("save startup check");
        assert_eq!(
            read_settings(&connection).expect("read saved preferences"),
            UpdaterSettings {
                channel: UpdateChannel::Beta,
                automatic_updates: true,
                check_on_startup: false,
            }
        );
    }
}

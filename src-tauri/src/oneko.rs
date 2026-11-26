use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

use crate::storage::{get_setting, set_setting, StorageService};

const ENABLED_KEY: &str = "oneko.enabled";
const SPEED_KEY: &str = "oneko.speed";
const SIZE_KEY: &str = "oneko.size";
const BROWSER_PAGES_KEY: &str = "oneko.show_browser_pages";
const SPRITES: [&[u8]; 32] = [
    include_bytes!("../resources/oneko/1.GIF"),
    include_bytes!("../resources/oneko/2.GIF"),
    include_bytes!("../resources/oneko/3.GIF"),
    include_bytes!("../resources/oneko/4.GIF"),
    include_bytes!("../resources/oneko/5.GIF"),
    include_bytes!("../resources/oneko/6.GIF"),
    include_bytes!("../resources/oneko/7.GIF"),
    include_bytes!("../resources/oneko/8.GIF"),
    include_bytes!("../resources/oneko/9.GIF"),
    include_bytes!("../resources/oneko/10.GIF"),
    include_bytes!("../resources/oneko/11.GIF"),
    include_bytes!("../resources/oneko/12.GIF"),
    include_bytes!("../resources/oneko/13.GIF"),
    include_bytes!("../resources/oneko/14.GIF"),
    include_bytes!("../resources/oneko/15.GIF"),
    include_bytes!("../resources/oneko/16.GIF"),
    include_bytes!("../resources/oneko/17.GIF"),
    include_bytes!("../resources/oneko/18.GIF"),
    include_bytes!("../resources/oneko/19.GIF"),
    include_bytes!("../resources/oneko/20.GIF"),
    include_bytes!("../resources/oneko/21.GIF"),
    include_bytes!("../resources/oneko/22.GIF"),
    include_bytes!("../resources/oneko/23.GIF"),
    include_bytes!("../resources/oneko/24.GIF"),
    include_bytes!("../resources/oneko/25.GIF"),
    include_bytes!("../resources/oneko/26.GIF"),
    include_bytes!("../resources/oneko/27.GIF"),
    include_bytes!("../resources/oneko/28.GIF"),
    include_bytes!("../resources/oneko/29.GIF"),
    include_bytes!("../resources/oneko/30.GIF"),
    include_bytes!("../resources/oneko/31.GIF"),
    include_bytes!("../resources/oneko/32.GIF"),
];

const SCRIPT_TEMPLATE: &str = include_str!("../resources/oneko.js");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct OnekoSettings {
    pub enabled: bool,
    pub speed: OnekoSpeed,
    pub size: u8,
    pub show_browser_pages: bool,
    pub hide_on_site: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnekoSpeed {
    Slow,
    #[default]
    Normal,
    Fast,
}

impl OnekoSpeed {
    fn as_str(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Normal => "normal",
            Self::Fast => "fast",
        }
    }
}

impl Default for OnekoSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            speed: OnekoSpeed::Normal,
            size: 1,
            show_browser_pages: false,
            hide_on_site: false,
        }
    }
}

pub fn initialization_script() -> &'static str {
    static SCRIPT: OnceLock<String> = OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            let data_urls = SPRITES
                .iter()
                .map(|sprite| format!("\"data:image/gif;base64,{}\"", encode_base64(sprite)))
                .collect::<Vec<_>>()
                .join(",");
            SCRIPT_TEMPLATE.replace("__EEFI_ONEKO_SPRITES__", &format!("[{data_urls}]"))
        })
        .as_str()
}

pub(crate) async fn settings_for_url(
    storage: StorageService,
    page_url: String,
) -> Result<(OnekoSettings, Option<String>), String> {
    let parsed = Url::parse(&page_url)
        .map_err(|error| format!("could not parse Oneko page URL: {error}"))?;
    let host = matches!(parsed.scheme(), "http" | "https")
        .then(|| parsed.host_str().map(str::to_ascii_lowercase))
        .flatten();
    let lookup_host = host.clone();
    let settings = storage
        .with_connection(move |connection| read_settings(connection, lookup_host.as_deref()))
        .await?;
    Ok((settings, host))
}

#[tauri::command]
pub async fn oneko_settings_get(
    host: Option<String>,
    storage: State<'_, StorageService>,
) -> Result<OnekoSettings, String> {
    let host = host.map(|value| normalize_host(&value)).transpose()?;
    storage
        .with_connection(move |connection| read_settings(connection, host.as_deref()))
        .await
}

#[tauri::command]
pub async fn oneko_settings_set(
    app: AppHandle,
    settings: OnekoSettings,
    storage: State<'_, StorageService>,
) -> Result<OnekoSettings, String> {
    validate_settings(&settings)?;
    let saved = storage
        .with_connection(move |connection| {
            save_settings(connection, &settings)?;
            Ok(OnekoSettings {
                hide_on_site: false,
                ..settings
            })
        })
        .await?;
    broadcast_settings(&app, &saved, None)?;
    Ok(saved)
}

#[tauri::command]
pub async fn oneko_toggle(
    app: AppHandle,
    storage: State<'_, StorageService>,
) -> Result<OnekoSettings, String> {
    let settings = storage
        .with_connection(|connection| {
            let mut settings = read_settings(connection, None)?;
            settings.enabled = !settings.enabled;
            save_settings(connection, &settings)?;
            Ok(settings)
        })
        .await?;
    broadcast_settings(&app, &settings, None)?;
    Ok(settings)
}

#[tauri::command]
pub async fn oneko_site_visibility_set(
    app: AppHandle,
    host: String,
    hide: bool,
    storage: State<'_, StorageService>,
) -> Result<OnekoSettings, String> {
    let host = normalize_host(&host)?;
    let broadcast_host = host.clone();
    let settings = storage
        .with_connection(move |connection| {
            if hide {
                connection
                    .execute(
                        "INSERT INTO oneko_hidden_sites(host) VALUES (?1)
                         ON CONFLICT(host) DO NOTHING",
                        [&host],
                    )
                    .map_err(|error| format!("could not hide Oneko on site: {error}"))?;
            } else {
                connection
                    .execute("DELETE FROM oneko_hidden_sites WHERE host = ?1", [&host])
                    .map_err(|error| format!("could not show Oneko on site: {error}"))?;
            }
            read_settings(connection, Some(&host))
        })
        .await?;
    broadcast_settings(&app, &settings, Some(&broadcast_host))?;
    Ok(settings)
}

fn validate_settings(settings: &OnekoSettings) -> Result<(), String> {
    if !matches!(settings.size, 1 | 2) {
        return Err("Oneko size must be 1x or 2x.".to_string());
    }
    Ok(())
}

fn save_settings(
    connection: &rusqlite::Connection,
    settings: &OnekoSettings,
) -> Result<(), String> {
    validate_settings(settings)?;
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|error| format!("could not begin Oneko settings update: {error}"))?;
    set_setting(
        connection,
        ENABLED_KEY,
        if settings.enabled { "true" } else { "false" },
    )?;
    set_setting(connection, SPEED_KEY, settings.speed.as_str())?;
    set_setting(connection, SIZE_KEY, &settings.size.to_string())?;
    set_setting(
        connection,
        BROWSER_PAGES_KEY,
        if settings.show_browser_pages {
            "true"
        } else {
            "false"
        },
    )?;
    connection
        .execute_batch("COMMIT")
        .map_err(|error| format!("could not commit Oneko settings: {error}"))?;
    Ok(())
}

fn read_settings(
    connection: &rusqlite::Connection,
    host: Option<&str>,
) -> Result<OnekoSettings, String> {
    let defaults = OnekoSettings::default();
    let enabled = read_bool(connection, ENABLED_KEY, defaults.enabled)?;
    let speed = match get_setting(connection, SPEED_KEY)
        .map_err(|error| format!("could not read Oneko speed: {error}"))?
    {
        None => defaults.speed,
        Some(value) => match value.as_str() {
            "slow" => OnekoSpeed::Slow,
            "normal" => OnekoSpeed::Normal,
            "fast" => OnekoSpeed::Fast,
            _ => return Err("stored Oneko speed is invalid".to_string()),
        },
    };
    let size = match get_setting(connection, SIZE_KEY)
        .map_err(|error| format!("could not read Oneko size: {error}"))?
    {
        None => defaults.size,
        Some(value) => value
            .parse::<u8>()
            .ok()
            .filter(|value| matches!(value, 1 | 2))
            .ok_or_else(|| "stored Oneko size is invalid".to_string())?,
    };
    let show_browser_pages = read_bool(connection, BROWSER_PAGES_KEY, defaults.show_browser_pages)?;
    let hide_on_site = match host {
        Some(host) => connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM oneko_hidden_sites WHERE host = ?1)",
                [host],
                |row| row.get(0),
            )
            .map_err(|error| format!("could not read Oneko site visibility: {error}"))?,
        None => false,
    };
    Ok(OnekoSettings {
        enabled,
        speed,
        size,
        show_browser_pages,
        hide_on_site,
    })
}

fn read_bool(connection: &rusqlite::Connection, key: &str, default: bool) -> Result<bool, String> {
    match get_setting(connection, key)
        .map_err(|error| format!("could not read Oneko preference {key}: {error}"))?
    {
        None => Ok(default),
        Some(value) if value == "true" => Ok(true),
        Some(value) if value == "false" => Ok(false),
        Some(_) => Err(format!("stored Oneko preference {key} is invalid")),
    }
}

fn normalize_host(host: &str) -> Result<String, String> {
    if host.is_empty()
        || host.len() > 253
        || host.trim() != host
        || host.chars().any(|character| {
            character.is_control() || matches!(character, '/' | '\\' | '?' | '#' | '@')
        })
    {
        return Err("site host is invalid".to_string());
    }
    let parsed =
        Url::parse(&format!("http://{host}/")).map_err(|_| "site host is invalid".to_string())?;
    if parsed.port().is_some() || parsed.path() != "/" || parsed.host_str().is_none() {
        return Err("site host is invalid".to_string());
    }
    let hostname = parsed
        .host_str()
        .ok_or_else(|| "site host is invalid".to_string())?;
    Ok(hostname.to_ascii_lowercase())
}

fn broadcast_settings(
    app: &AppHandle,
    settings: &OnekoSettings,
    site_host: Option<&str>,
) -> Result<(), String> {
    let mut payload = serde_json::to_value(settings)
        .map_err(|error| format!("could not serialize Oneko settings: {error}"))?;
    if let Some(host) = site_host {
        if let Value::Object(object) = &mut payload {
            object.insert("siteHost".to_string(), Value::String(host.to_string()));
        }
    }
    app.emit("oneko-toggled", &payload)
        .map_err(|error| format!("could not broadcast Oneko settings: {error}"))?;
    let script = format!("window.__eefiOnekoUpdate?.({});", payload);
    for (label, webview) in app.webviews() {
        if let Err(error) = webview.eval(&script) {
            eprintln!("could not update Oneko in webview {label}: {error}");
        }
    }
    Ok(())
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied();
        let third = chunk.get(2).copied();
        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[(((first & 0x03) << 4) | (second.unwrap_or(0) >> 4)) as usize] as char);
        match second {
            Some(second) => encoded
                .push(TABLE[(((second & 0x0f) << 2) | (third.unwrap_or(0) >> 6)) as usize] as char),
            None => encoded.push('='),
        }
        match third {
            Some(third) => encoded.push(TABLE[(third & 0x3f) as usize] as char),
            None => encoded.push('='),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open settings database");
        connection
            .execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE oneko_hidden_sites (
                    host TEXT PRIMARY KEY CHECK(length(host) BETWEEN 1 AND 253)
                 );",
            )
            .expect("create settings table");
        connection
    }

    #[test]
    fn settings_default_and_persisted_options_round_trip() {
        let connection = connection();
        assert_eq!(
            read_settings(&connection, Some("example.com")).expect("read defaults"),
            OnekoSettings::default()
        );
        let settings = OnekoSettings {
            enabled: true,
            speed: OnekoSpeed::Fast,
            size: 2,
            show_browser_pages: true,
            hide_on_site: false,
        };
        save_settings(&connection, &settings).expect("save settings");
        connection
            .execute(
                "INSERT INTO oneko_hidden_sites(host) VALUES (?1)",
                ["example.com"],
            )
            .expect("save per-site setting");
        assert_eq!(
            read_settings(&connection, Some("example.com")).expect("read settings"),
            OnekoSettings {
                hide_on_site: true,
                ..settings
            }
        );
    }

    #[test]
    fn settings_reject_invalid_speed_and_size() {
        let mut settings = OnekoSettings::default();
        settings.size = 3;
        assert!(validate_settings(&settings).is_err());
        assert!(serde_json::from_str::<OnekoSettings>(r#"{"speed":"turbo"}"#).is_err());
    }

    #[test]
    fn sprite_data_is_inline_and_base64_encodes_padding() {
        assert_eq!(encode_base64(b"cat"), "Y2F0");
        assert_eq!(encode_base64(b"ca"), "Y2E=");
        assert!(initialization_script().contains("data:image/gif;base64,"));
        assert!(!initialization_script().contains("__EEFI_ONEKO_SPRITES__"));
    }

    #[test]
    fn host_validation_rejects_paths_and_normalizes_case() {
        assert_eq!(
            normalize_host("Example.COM").expect("normalize host"),
            "example.com"
        );
        assert!(normalize_host("example.com/path").is_err());
    }
}

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;
use url::Url;

use crate::storage::StorageService;

const MAX_SCRIPTS: i64 = 64;
const MAX_NAME_CHARS: usize = 120;
const MAX_PATTERN_BYTES: usize = 1024;
const MAX_SOURCE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserScript {
    pub id: i64,
    pub name: String,
    pub match_pattern: String,
    pub source: String,
    pub enabled: bool,
    pub updated_at: i64,
}

struct MatchPattern {
    scheme: String,
    host: String,
    include_subdomains: bool,
    path: String,
}

#[tauri::command]
pub async fn user_scripts_list(
    storage: State<'_, StorageService>,
) -> Result<Vec<UserScript>, String> {
    storage.with_connection(list_user_scripts).await
}

#[tauri::command]
pub async fn user_scripts_save(
    id: Option<i64>,
    name: String,
    match_pattern: String,
    source: String,
    enabled: bool,
    storage: State<'_, StorageService>,
) -> Result<i64, String> {
    let name = name.trim().to_string();
    let match_pattern = match_pattern.trim().to_string();
    validate_script(&name, &match_pattern, &source)?;
    storage
        .with_connection(move |connection| {
            save_user_script(connection, id, &name, &match_pattern, &source, enabled)
        })
        .await
}

#[tauri::command]
pub async fn user_scripts_set_enabled(
    id: i64,
    enabled: bool,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    storage
        .with_connection(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE user_scripts SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                    params![enabled, unix_timestamp(), id],
                )
                .map_err(db_error)?;
            if changed == 0 {
                return Err("user script no longer exists".to_string());
            }
            Ok(())
        })
        .await
}

#[tauri::command]
pub async fn user_scripts_delete(
    id: i64,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    storage
        .with_connection(move |connection| {
            let changed = connection
                .execute("DELETE FROM user_scripts WHERE id = ?1", [id])
                .map_err(db_error)?;
            if changed == 0 {
                return Err("user script no longer exists".to_string());
            }
            Ok(())
        })
        .await
}

pub(crate) fn enabled_scripts_for_url(
    connection: &Connection,
    target: &str,
) -> Result<Vec<UserScript>, String> {
    let target = Url::parse(target)
        .map_err(|error| format!("could not parse page URL for user scripts: {error}"))?;
    if !matches!(target.scheme(), "http" | "https") {
        return Ok(Vec::new());
    }

    list_user_scripts(connection)?
        .into_iter()
        .filter(|script| script.enabled)
        .filter_map(
            |script| match pattern_matches(&script.match_pattern, &target) {
                Ok(true) => Some(Ok(script)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

fn list_user_scripts(connection: &Connection) -> Result<Vec<UserScript>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, match_pattern, source, enabled, updated_at
             FROM user_scripts ORDER BY id DESC",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(UserScript {
                id: row.get(0)?,
                name: row.get(1)?,
                match_pattern: row.get(2)?,
                source: row.get(3)?,
                enabled: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .map_err(db_error)?;
    let scripts = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
    for script in &scripts {
        validate_script(&script.name, &script.match_pattern, &script.source)?;
    }
    Ok(scripts)
}

fn save_user_script(
    connection: &Connection,
    id: Option<i64>,
    name: &str,
    match_pattern: &str,
    source: &str,
    enabled: bool,
) -> Result<i64, String> {
    let updated_at = unix_timestamp();
    if let Some(id) = id {
        let changed = connection
            .execute(
                "UPDATE user_scripts
                 SET name = ?1, match_pattern = ?2, source = ?3, enabled = ?4, updated_at = ?5
                 WHERE id = ?6",
                params![name, match_pattern, source, enabled, updated_at, id],
            )
            .map_err(db_error)?;
        if changed == 0 {
            return Err("user script no longer exists".to_string());
        }
        return Ok(id);
    }

    let count: i64 = connection
        .query_row("SELECT count(*) FROM user_scripts", [], |row| row.get(0))
        .map_err(db_error)?;
    if count >= MAX_SCRIPTS {
        return Err(format!("Only {MAX_SCRIPTS} user scripts can be saved."));
    }
    connection
        .execute(
            "INSERT INTO user_scripts(name, match_pattern, source, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![name, match_pattern, source, enabled, updated_at],
        )
        .map_err(db_error)?;
    Ok(connection.last_insert_rowid())
}

fn validate_script(name: &str, match_pattern: &str, source: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "Script names must contain between 1 and {MAX_NAME_CHARS} characters."
        ));
    }
    if match_pattern.len() > MAX_PATTERN_BYTES {
        return Err(format!(
            "Match patterns must be no longer than {MAX_PATTERN_BYTES} bytes."
        ));
    }
    parse_match_pattern(match_pattern)?;
    if source.trim().is_empty() {
        return Err("User script source cannot be empty.".to_string());
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "User script source must be no larger than {} KiB.",
            MAX_SOURCE_BYTES / 1024
        ));
    }
    Ok(())
}

fn parse_match_pattern(value: &str) -> Result<MatchPattern, String> {
    if value.len() > MAX_PATTERN_BYTES {
        return Err("Match pattern is too long.".to_string());
    }
    let (scheme, rest) = value
        .split_once("://")
        .ok_or_else(|| "Use a match pattern such as https://example.com/*.".to_string())?;
    if !matches!(scheme, "http" | "https" | "*") {
        return Err("Match patterns support only http, https, or * schemes.".to_string());
    }
    let (host_pattern, path_suffix) = rest
        .split_once('/')
        .ok_or_else(|| "Match patterns must include a path, for example /*.".to_string())?;
    if host_pattern.is_empty()
        || host_pattern.contains('@')
        || host_pattern.contains(':')
        || host_pattern.contains('\\')
        || host_pattern.chars().any(char::is_whitespace)
    {
        return Err("Match patterns require a hostname without credentials or a port.".to_string());
    }
    let (include_subdomains, host) = match host_pattern.strip_prefix("*.") {
        Some(host) => (true, host),
        None => (false, host_pattern),
    };
    if host.contains('*') || host.is_empty() {
        return Err(
            "Use a specific hostname; only a leading *. wildcard is supported.".to_string(),
        );
    }
    let parsed_host = Url::parse(&format!("https://{host}/"))
        .map_err(|_| "Match pattern contains an invalid hostname.".to_string())?;
    if parsed_host.host_str().is_none()
        || parsed_host.path() != "/"
        || parsed_host.query().is_some()
        || parsed_host.fragment().is_some()
    {
        return Err("Match pattern contains an invalid hostname.".to_string());
    }
    let path = format!("/{path_suffix}");
    if path.contains('#') || path.chars().any(char::is_control) {
        return Err(
            "Match pattern paths cannot contain fragments or control characters.".to_string(),
        );
    }
    Ok(MatchPattern {
        scheme: scheme.to_string(),
        host: parsed_host.host_str().unwrap_or_default().to_string(),
        include_subdomains,
        path,
    })
}

fn pattern_matches(pattern: &str, url: &Url) -> Result<bool, String> {
    let pattern = parse_match_pattern(pattern)?;
    let scheme_matches = pattern.scheme == "*" || pattern.scheme == url.scheme();
    let host = match url.host_str() {
        Some(host) => host,
        None => return Ok(false),
    };
    let host_matches = host == pattern.host
        || (pattern.include_subdomains
            && host
                .strip_suffix(&pattern.host)
                .is_some_and(|prefix| prefix.ends_with('.')));
    let path_and_query = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    Ok(scheme_matches && host_matches && wildcard_matches(&pattern.path, &path_and_query))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut retry_value_index) = (None, 0);

    while value_index < value.len() {
        if pattern.get(pattern_index) == Some(&value[value_index]) {
            pattern_index += 1;
            value_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            star_index = Some(pattern_index);
            pattern_index += 1;
            retry_value_index = value_index;
        } else if let Some(star) = star_index {
            retry_value_index += 1;
            value_index = retry_value_index;
            pattern_index = star + 1;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

fn db_error(error: rusqlite::Error) -> String {
    format!("user script database error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE user_scripts (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    match_pattern TEXT NOT NULL,
                    source TEXT NOT NULL,
                    enabled INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .expect("create user script table");
        connection
    }

    #[test]
    fn match_patterns_respect_scheme_host_and_path() {
        let url = Url::parse("https://example.com/docs/page?mode=1").expect("parse URL");
        assert!(pattern_matches("https://example.com/docs/*", &url).expect("match pattern"));
        assert!(pattern_matches("https://example.com/*?mode=*", &url).expect("match query"));
        assert!(!pattern_matches("http://example.com/docs/*", &url).expect("scheme mismatch"));
        assert!(!pattern_matches("https://other.example.com/docs/*", &url).expect("host mismatch"));
        assert!(!pattern_matches("https://example.com/private/*", &url).expect("path mismatch"));
    }

    #[test]
    fn wildcard_host_matches_root_and_subdomains_only() {
        let root = Url::parse("https://example.com/").expect("parse root URL");
        let subdomain = Url::parse("http://news.example.com/").expect("parse subdomain URL");
        let unrelated = Url::parse("https://notexample.com/").expect("parse unrelated URL");
        assert!(pattern_matches("*://*.example.com/*", &root).expect("match root"));
        assert!(pattern_matches("*://*.example.com/*", &subdomain).expect("match subdomain"));
        assert!(
            !pattern_matches("*://*.example.com/*", &unrelated).expect("reject suffix lookalike")
        );
    }

    #[test]
    fn match_patterns_reject_global_and_non_web_patterns() {
        assert!(parse_match_pattern("*://*/*").is_err());
        assert!(parse_match_pattern("file://example.com/*").is_err());
        assert!(parse_match_pattern("https://example.com:8443/*").is_err());
        assert!(parse_match_pattern("https://user@example.com/*").is_err());
    }

    #[test]
    fn scripts_round_trip_and_only_enabled_matches_are_returned() {
        let connection = test_connection();
        let id = save_user_script(
            &connection,
            None,
            "Page helper",
            "https://example.com/*",
            "document.documentElement.dataset.eefi = 'loaded';",
            true,
        )
        .expect("save matching script");
        save_user_script(
            &connection,
            None,
            "Disabled helper",
            "https://example.com/*",
            "throw new Error('must not run');",
            false,
        )
        .expect("save disabled script");

        let scripts = list_user_scripts(&connection).expect("list scripts");
        assert_eq!(scripts.len(), 2);
        assert_eq!(
            scripts.iter().find(|script| script.id == id).unwrap().name,
            "Page helper"
        );
        let matched = enabled_scripts_for_url(&connection, "https://example.com/page")
            .expect("find matching enabled scripts");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, id);
        assert!(
            enabled_scripts_for_url(&connection, "https://other.example/page")
                .expect("find other site scripts")
                .is_empty()
        );
    }
}

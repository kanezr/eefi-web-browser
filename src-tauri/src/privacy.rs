use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock,
};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tauri::{State, Webview, Wry};
use url::Url;

use crate::storage::{set_setting, StorageService};

const SITE_PERMISSION_KINDS: [&str; 5] = [
    "camera",
    "microphone",
    "location",
    "notifications",
    "clipboard",
];

#[derive(Clone, Default)]
pub struct PrivacyService(Arc<PrivacyInner>);

#[derive(Default)]
struct PrivacyInner {
    https_only: AtomicBool,
    block_third_party_cookies: AtomicBool,
    clear_cookies_on_exit: AtomicBool,
    preference_changed: AtomicBool,
    preferences_loaded: AtomicBool,
    preference_lock: Mutex<()>,
    site_permissions: RwLock<HashSet<(String, String)>>,
    site_permissions_changed: AtomicBool,
    site_permissions_loaded: AtomicBool,
    site_permissions_lock: Mutex<()>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivacyStatus {
    https_only: bool,
    block_third_party_cookies: bool,
    clear_cookies_on_exit: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SitePermissionStatus {
    origin: String,
    permissions: HashMap<String, bool>,
}

impl PrivacyService {
    fn https_only(&self) -> bool {
        self.0.https_only.load(Ordering::Relaxed)
    }

    pub(crate) fn block_third_party_cookies(&self) -> bool {
        self.0.block_third_party_cookies.load(Ordering::Relaxed)
    }

    pub(crate) fn clear_cookies_on_exit(&self) -> bool {
        self.0.clear_cookies_on_exit.load(Ordering::Relaxed)
    }

    fn set_https_only(&self, enabled: bool) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "privacy preference lock is poisoned".to_string())?;
        self.0.https_only.store(enabled, Ordering::Relaxed);
        self.0.preference_changed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn set_block_third_party_cookies(&self, enabled: bool) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "privacy preference lock is poisoned".to_string())?;
        self.0
            .block_third_party_cookies
            .store(enabled, Ordering::Relaxed);
        self.0.preference_changed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn set_clear_cookies_on_exit(&self, enabled: bool) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "privacy preference lock is poisoned".to_string())?;
        self.0
            .clear_cookies_on_exit
            .store(enabled, Ordering::Relaxed);
        self.0.preference_changed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn restore_preferences(
        &self,
        https_only: bool,
        block_third_party_cookies: bool,
        clear_cookies_on_exit: bool,
    ) -> Result<(), String> {
        let _guard = self
            .0
            .preference_lock
            .lock()
            .map_err(|_| "privacy preference lock is poisoned".to_string())?;
        if !self.0.preference_changed.load(Ordering::Relaxed) {
            self.0.https_only.store(https_only, Ordering::Relaxed);
            self.0
                .block_third_party_cookies
                .store(block_third_party_cookies, Ordering::Relaxed);
            self.0
                .clear_cookies_on_exit
                .store(clear_cookies_on_exit, Ordering::Relaxed);
        }
        self.0.preferences_loaded.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn upgrade_request_url(&self, request_url: &str) -> Option<String> {
        self.https_only()
            .then(|| upgrade_http_url(request_url))
            .flatten()
    }

    fn restore_site_permissions(
        &self,
        permissions: HashSet<(String, String)>,
    ) -> Result<(), String> {
        let _guard = self
            .0
            .site_permissions_lock
            .lock()
            .map_err(|_| "site permission lock is poisoned".to_string())?;
        if !self.0.site_permissions_changed.load(Ordering::Relaxed) {
            self.0
                .site_permissions
                .write()
                .map_err(|_| "site permission map is poisoned".to_string())?
                .extend(permissions);
        }
        self.0
            .site_permissions_loaded
            .store(true, Ordering::Relaxed);
        Ok(())
    }

    fn set_site_permission(
        &self,
        origin: &str,
        permission: &str,
        allowed: bool,
    ) -> Result<(), String> {
        let _guard = self
            .0
            .site_permissions_lock
            .lock()
            .map_err(|_| "site permission lock is poisoned".to_string())?;
        let mut permissions = self
            .0
            .site_permissions
            .write()
            .map_err(|_| "site permission map is poisoned".to_string())?;
        let grant = (origin.to_string(), permission.to_string());
        if allowed {
            permissions.insert(grant);
        } else {
            permissions.remove(&grant);
        }
        self.0
            .site_permissions_changed
            .store(true, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn site_permission_allowed(
        &self,
        origin: &str,
        permission: &str,
    ) -> Result<bool, String> {
        self.0
            .site_permissions
            .read()
            .map(|permissions| permissions.contains(&(origin.to_string(), permission.to_string())))
            .map_err(|_| "site permission map is poisoned".to_string())
    }
}

pub async fn ensure_preferences_loaded(
    storage: &StorageService,
    service: &PrivacyService,
) -> Result<(), String> {
    if service.0.preferences_loaded.load(Ordering::Relaxed) {
        return Ok(());
    }
    let (https_only, block_third_party_cookies, clear_cookies_on_exit) = storage
        .with_connection(|connection| {
            Ok((
                load_https_only(connection)?,
                load_block_third_party_cookies(connection)?,
                load_clear_cookies_on_exit(connection)?,
            ))
        })
        .await?;
    service.restore_preferences(https_only, block_third_party_cookies, clear_cookies_on_exit)
}

fn load_https_only(connection: &Connection) -> Result<bool, String> {
    load_privacy_boolean(connection, "privacy.https_only")
}

fn load_block_third_party_cookies(connection: &Connection) -> Result<bool, String> {
    load_privacy_boolean(connection, "privacy.block_third_party_cookies")
}

fn load_clear_cookies_on_exit(connection: &Connection) -> Result<bool, String> {
    load_privacy_boolean(connection, "privacy.clear_cookies_on_exit")
}

fn load_privacy_boolean(connection: &Connection, key: &str) -> Result<bool, String> {
    let value = connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|error| format!("could not read privacy preference {key}: {error}"))?;
    match value.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(format!("stored privacy preference {key} is invalid")),
    }
}

pub(crate) fn is_same_site_request(source_url: &str, request_url: &str) -> bool {
    let Ok(source) = Url::parse(source_url) else {
        return false;
    };
    let Ok(request) = Url::parse(request_url) else {
        return false;
    };
    if source.scheme() != request.scheme() {
        return false;
    }
    let (Some(source_host), Some(request_host)) = (source.host_str(), request.host_str()) else {
        return false;
    };
    if source_host.eq_ignore_ascii_case(request_host) {
        return true;
    }
    match (psl::domain_str(source_host), psl::domain_str(request_host)) {
        (Some(source_domain), Some(request_domain)) => {
            source_domain.eq_ignore_ascii_case(request_domain)
        }
        _ => false,
    }
}

pub async fn ensure_site_permissions_loaded(
    storage: &StorageService,
    service: &PrivacyService,
) -> Result<(), String> {
    if service.0.site_permissions_loaded.load(Ordering::Relaxed) {
        return Ok(());
    }
    let permissions = storage
        .with_connection(|connection| load_site_permissions(connection))
        .await?;
    service.restore_site_permissions(permissions)
}

fn load_site_permissions(connection: &Connection) -> Result<HashSet<(String, String)>, String> {
    let mut statement = connection
        .prepare("SELECT origin, permission FROM site_permission_grants")
        .map_err(|error| format!("could not read site permissions: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("could not read site permissions: {error}"))?;
    let permissions = rows
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| format!("could not decode site permissions: {error}"))?;
    Ok(permissions)
}

pub(crate) fn permission_origin_for_url(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn normalize_site_origin(origin: &str) -> Result<String, String> {
    let url = Url::parse(origin).map_err(|_| "invalid site origin".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("site permissions require an HTTP(S) origin without a path".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

fn validate_permission_kind(permission: &str) -> Result<&'static str, String> {
    SITE_PERMISSION_KINDS
        .iter()
        .copied()
        .find(|kind| *kind == permission)
        .ok_or_else(|| "unsupported site permission".to_string())
}

fn upgrade_http_url(request_url: &str) -> Option<String> {
    let mut url = Url::parse(request_url).ok()?;
    if url.scheme() != "http" {
        return None;
    }
    url.set_scheme("https").ok()?;
    if url.port() == Some(80) {
        url.set_port(None).ok()?;
    }
    Some(url.to_string())
}

#[tauri::command]
pub async fn privacy_status(
    storage: State<'_, StorageService>,
    service: State<'_, PrivacyService>,
) -> Result<PrivacyStatus, String> {
    ensure_preferences_loaded(storage.inner(), service.inner()).await?;
    Ok(PrivacyStatus {
        https_only: service.https_only(),
        block_third_party_cookies: service.block_third_party_cookies(),
        clear_cookies_on_exit: service.clear_cookies_on_exit(),
    })
}

#[tauri::command]
pub async fn privacy_set_block_third_party_cookies(
    enabled: bool,
    storage: State<'_, StorageService>,
    service: State<'_, PrivacyService>,
) -> Result<(), String> {
    ensure_preferences_loaded(storage.inner(), service.inner()).await?;
    storage
        .with_connection(move |connection| {
            set_setting(
                connection,
                "privacy.block_third_party_cookies",
                if enabled { "true" } else { "false" },
            )
        })
        .await?;
    service.set_block_third_party_cookies(enabled)
}

#[tauri::command]
pub async fn privacy_set_clear_cookies_on_exit(
    enabled: bool,
    storage: State<'_, StorageService>,
    service: State<'_, PrivacyService>,
) -> Result<(), String> {
    ensure_preferences_loaded(storage.inner(), service.inner()).await?;
    storage
        .with_connection(move |connection| {
            set_setting(
                connection,
                "privacy.clear_cookies_on_exit",
                if enabled { "true" } else { "false" },
            )
        })
        .await?;
    service.set_clear_cookies_on_exit(enabled)
}

#[tauri::command]
pub async fn privacy_site_permissions(
    origin: String,
    storage: State<'_, StorageService>,
    service: State<'_, PrivacyService>,
) -> Result<SitePermissionStatus, String> {
    let origin = normalize_site_origin(&origin)?;
    ensure_site_permissions_loaded(storage.inner(), service.inner()).await?;
    let mut permissions = HashMap::new();
    for kind in SITE_PERMISSION_KINDS {
        permissions.insert(
            kind.to_string(),
            service.site_permission_allowed(&origin, kind)?,
        );
    }
    Ok(SitePermissionStatus {
        origin,
        permissions,
    })
}

#[tauri::command]
pub async fn privacy_set_site_permission(
    origin: String,
    permission: String,
    allowed: bool,
    storage: State<'_, StorageService>,
    service: State<'_, PrivacyService>,
) -> Result<(), String> {
    let origin = normalize_site_origin(&origin)?;
    let permission = validate_permission_kind(&permission)?;
    ensure_site_permissions_loaded(storage.inner(), service.inner()).await?;
    let saved_origin = origin.clone();
    let saved_permission = permission.to_string();
    storage
        .with_connection(move |connection| {
            if allowed {
                connection
                    .execute(
                        "INSERT OR IGNORE INTO site_permission_grants(origin, permission)
                         VALUES (?1, ?2)",
                        [&saved_origin, &saved_permission],
                    )
                    .map_err(|error| format!("could not save site permission: {error}"))?;
            } else {
                connection
                    .execute(
                        "DELETE FROM site_permission_grants
                         WHERE origin = ?1 AND permission = ?2",
                        [&saved_origin, &saved_permission],
                    )
                    .map_err(|error| format!("could not remove site permission: {error}"))?;
            }
            Ok(())
        })
        .await?;
    service.set_site_permission(&origin, permission, allowed)
}

#[tauri::command]
pub async fn privacy_set_https_only(
    enabled: bool,
    storage: State<'_, StorageService>,
    service: State<'_, PrivacyService>,
) -> Result<(), String> {
    ensure_preferences_loaded(storage.inner(), service.inner()).await?;
    storage
        .with_connection(move |connection| {
            set_setting(
                connection,
                "privacy.https_only",
                if enabled { "true" } else { "false" },
            )
        })
        .await?;
    service.set_https_only(enabled)
}

#[tauri::command]
pub async fn privacy_clear_cookies_and_cache(
    webview: Webview<Wry>,
    clear_cookies: bool,
    clear_cache: bool,
) -> Result<(), String> {
    clear_profile_data(webview, clear_cookies, clear_cache).await
}

pub async fn clear_profile_data(
    webview: Webview<Wry>,
    clear_cookies: bool,
    clear_cache: bool,
) -> Result<(), String> {
    if !clear_cookies && !clear_cache {
        return Err("Select cookies, cached files, or both to clear.".to_string());
    }

    #[cfg(windows)]
    {
        use std::{sync::mpsc, time::Duration};

        use webview2_com::{
            ClearBrowsingDataCompletedHandler,
            Microsoft::Web::WebView2::Win32::{
                ICoreWebView2Profile2, ICoreWebView2_13, COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES,
                COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE,
            },
        };
        use windows::core::Interface;

        let mut kinds = Default::default();
        if clear_cookies {
            kinds |= COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES;
        }
        if clear_cache {
            kinds |= COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE;
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        let setup_sender = sender.clone();
        webview
            .with_webview(move |platform| {
                let setup_result = (|| -> Result<(), String> {
                    unsafe {
                        let core = platform
                            .controller()
                            .CoreWebView2()
                            .map_err(|error| format!("could not access WebView2: {error}"))?;
                        let profile = core
                            .cast::<ICoreWebView2_13>()
                            .map_err(|error| {
                                format!("WebView2 does not support profile data clearing: {error}")
                            })?
                            .Profile()
                            .map_err(|error| format!("could not access WebView2 profile: {error}"))?
                            .cast::<ICoreWebView2Profile2>()
                            .map_err(|error| {
                                format!("WebView2 does not support profile data clearing: {error}")
                            })?;
                        let completion_sender = setup_sender.clone();
                        let handler =
                            ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
                                let result = result.map_err(|error| {
                                    format!("WebView2 could not clear browsing data: {error}")
                                });
                                let _ = completion_sender.send(result);
                                Ok(())
                            }));
                        profile
                            .ClearBrowsingData(kinds, &handler)
                            .map_err(|error| {
                                format!("could not start clearing browsing data: {error}")
                            })?;
                    }
                    Ok(())
                })();
                if let Err(error) = setup_result {
                    let _ = setup_sender.send(Err(error));
                }
            })
            .map_err(|error| format!("could not access browser webview: {error}"))?;

        tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(Duration::from_secs(30))
                .map_err(|error| format!("timed out waiting for browsing data to clear: {error}"))?
        })
        .await
        .map_err(|error| format!("browsing-data worker failed: {error}"))??;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = webview;
        Err("browsing-data clearing is currently supported on Windows only.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_only_upgrades_http_and_preserves_url_components() {
        assert_eq!(
            upgrade_http_url("http://example.com:80/path?q=1#section").as_deref(),
            Some("https://example.com/path?q=1#section")
        );
        assert_eq!(
            upgrade_http_url("http://example.com:8080/path").as_deref(),
            Some("https://example.com:8080/path")
        );
    }

    #[test]
    fn https_only_does_not_change_other_or_invalid_schemes() {
        assert_eq!(upgrade_http_url("https://example.com/path"), None);
        assert_eq!(upgrade_http_url("data:text/plain,hello"), None);
        assert_eq!(upgrade_http_url("not a url"), None);
    }

    #[test]
    fn service_upgrades_requests_only_when_enabled() {
        let service = PrivacyService::default();
        assert_eq!(service.upgrade_request_url("http://example.com/"), None);
        service
            .set_https_only(true)
            .expect("enable HTTPS-only mode");
        assert_eq!(
            service.upgrade_request_url("http://example.com/"),
            Some("https://example.com/".to_string())
        );
        service
            .set_https_only(false)
            .expect("disable HTTPS-only mode");
        assert_eq!(service.upgrade_request_url("http://example.com/"), None);
    }

    #[test]
    fn late_preference_load_does_not_overwrite_a_user_change() {
        let service = PrivacyService::default();
        service
            .set_https_only(true)
            .expect("enable HTTPS-only mode");
        service
            .restore_preferences(false, true, true)
            .expect("restore old persisted values");
        assert!(service.https_only());
    }

    #[test]
    fn third_party_cookie_policy_uses_schemeful_registrable_domains() {
        assert!(is_same_site_request(
            "https://shop.example.co.uk/page",
            "https://cdn.example.co.uk/image.png"
        ));
        assert!(!is_same_site_request(
            "https://shop.example.co.uk/page",
            "https://example.net/image.png"
        ));
        assert!(!is_same_site_request(
            "http://example.com/page",
            "https://example.com/image.png"
        ));
    }

    #[test]
    fn persisted_preference_is_loaded_and_validated() {
        let connection = Connection::open_in_memory().expect("open in-memory database");
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create settings table");
        assert!(!load_https_only(&connection).expect("default HTTPS-only setting"));
        set_setting(&connection, "privacy.https_only", "true").expect("save setting");
        assert!(load_https_only(&connection).expect("load enabled preference"));
        set_setting(&connection, "privacy.https_only", "invalid").expect("save invalid setting");
        assert!(load_https_only(&connection).is_err());
        set_setting(&connection, "privacy.block_third_party_cookies", "true")
            .expect("save third-party cookie preference");
        assert!(load_block_third_party_cookies(&connection).expect("load cookie preference"));
        set_setting(&connection, "privacy.clear_cookies_on_exit", "true")
            .expect("save clear-on-exit preference");
        assert!(load_clear_cookies_on_exit(&connection).expect("load clear-on-exit preference"));
    }

    #[test]
    fn permission_origins_are_normalized_and_reject_non_web_origins() {
        assert_eq!(
            permission_origin_for_url("https://Example.com/path?q=1").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            normalize_site_origin("https://example.com:443").expect("normalize origin"),
            "https://example.com"
        );
        assert!(normalize_site_origin("https://example.com/path").is_err());
        assert_eq!(permission_origin_for_url("https://user@example.com/"), None);
    }

    #[test]
    fn site_permission_grants_load_from_storage() {
        let connection = Connection::open_in_memory().expect("open in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE site_permission_grants (
                    origin TEXT NOT NULL,
                    permission TEXT NOT NULL,
                    PRIMARY KEY(origin, permission)
                );
                INSERT INTO site_permission_grants(origin, permission)
                    VALUES ('https://example.com', 'camera');",
            )
            .expect("create site permission table");
        let permissions = load_site_permissions(&connection).expect("load grants");
        assert!(permissions.contains(&("https://example.com".into(), "camera".into())));
        assert!(!permissions.contains(&("https://example.com".into(), "microphone".into())));
    }

    #[test]
    fn site_permission_grants_are_origin_and_kind_scoped() {
        let service = PrivacyService::default();
        service
            .set_site_permission("https://example.com", "camera", true)
            .expect("allow camera");
        assert!(service
            .site_permission_allowed("https://example.com", "camera")
            .expect("read camera permission"));
        assert!(!service
            .site_permission_allowed("https://example.com", "microphone")
            .expect("read microphone permission"));
        assert!(!service
            .site_permission_allowed("https://sub.example.com", "camera")
            .expect("read subdomain permission"));
        service
            .set_site_permission("https://example.com", "camera", false)
            .expect("revoke camera");
        assert!(!service
            .site_permission_allowed("https://example.com", "camera")
            .expect("read revoked camera permission"));
    }
}

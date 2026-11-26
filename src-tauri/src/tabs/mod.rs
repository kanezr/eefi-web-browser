use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent, WebviewBuilder},
    AppHandle, Emitter, Manager, State, Webview, WebviewUrl, WebviewWindowBuilder, Wry,
};
use url::Url;

use crate::adblock::{self, AdblockService};
use crate::nav::{parse_input, ParsedInput};
use crate::privacy::{self, PrivacyService};
use crate::storage::{
    get_setting, load_session, record_download_finish, record_download_start, record_history,
    set_setting, update_latest_title, DownloadLocation, SessionGroup, SessionSnapshot, SessionTab,
    StorageService,
};

static PRIVATE_WINDOW_COUNTER: AtomicU64 = AtomicU64::new(1);
const TAB_SLEEP_SETTING_KEY: &str = "performance.tab_sleep_timeout_seconds";
const DEFAULT_TAB_SLEEP_TIMEOUT_SECONDS: u64 = 900;
const TAB_SLEEP_TIMEOUT_UNINITIALIZED: u64 = u64::MAX;
const TAB_SLEEP_SCAN_INTERVAL_SECONDS: u64 = 5;
const TAB_SLEEP_TIMEOUT_OPTIONS: &[u64] = &[0, 60, 300, 900, 1800, 3600];
const TAB_GROUP_COLORS: &[&str] = &[
    "#4f7ff0", "#8d63d8", "#159b9a", "#2d9a65", "#d49a2b", "#e17a32", "#d75059", "#788397",
];

const DEVTOOLS_F12_SCRIPT: &str = r#"
document.addEventListener('keydown', event => {
  if ((event.ctrlKey || event.metaKey) && event.shiftKey && !event.altKey && event.key.toLowerCase() === 'k' && !event.repeat) {
    if (window.top !== window) return;
    event.preventDefault();
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') {
      console.error('Eefi Browser: Oneko shortcut is unavailable in this frame.');
      return;
    }
    internals.invoke('oneko_toggle')
      .catch(error => console.error('Eefi Browser: could not toggle the Oneko cat.', error));
    return;
  }
  if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === 'f' && !event.repeat) {
    event.preventDefault();
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') {
      console.error('Eefi Browser: find-in-page shortcut is unavailable in this frame.');
      return;
    }
    internals.invoke('open_find_bar')
      .catch(error => console.error('Eefi Browser: could not open find-in-page.', error));
    return;
  }
  if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === 'p' && !event.repeat) {
    event.preventDefault();
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') {
      console.error('Eefi Browser: print shortcut is unavailable in this frame.');
      return;
    }
    internals.invoke('open_page_print_dialog')
      .catch(error => console.error('Eefi Browser: could not open print dialog.', error));
    return;
  }
  if (event.key !== 'F12' || event.repeat) return;
  event.preventDefault();
  const internals = window.__TAURI_INTERNALS__;
  if (!internals || typeof internals.invoke !== 'function') {
    console.error('Eefi Browser: DevTools shortcut is unavailable in this frame.');
    return;
  }
  internals.invoke('toggle_page_devtools')
    .catch(error => console.error('Eefi Browser: could not toggle DevTools.', error));
}, true);
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    pub id: String,
    pub title: String,
    pub url: String,
    pub group_id: Option<String>,
    pub sleeping: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabGroup {
    pub id: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FindResultEvent {
    tab_id: String,
    query: String,
    request_id: u64,
    match_count: i32,
    active_match_index: i32,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageEvent {
    tab_id: String,
    url: String,
    title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TabSleepEvent {
    tab_id: String,
    sleeping: bool,
}

#[derive(Default)]
struct BrowserData {
    tabs: Vec<TabInfo>,
    inactive_since: HashMap<String, Instant>,
    active_downloads: HashSet<String>,
    groups: Vec<TabGroup>,
    active_tab_id: Option<String>,
    next_id: u64,
    next_group_id: u64,
    revision: u64,
    page_visible: bool,
    restore_attempted: bool,
    navigation_generations: HashMap<String, u64>,
}

#[derive(Clone)]
pub struct BrowserState(Arc<Mutex<BrowserData>>, Arc<BrowserContext>);

struct BrowserContext {
    window_label: String,
    is_private: bool,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self::for_window("main".to_string(), false)
    }
}

impl BrowserState {
    fn for_window(window_label: String, is_private: bool) -> Self {
        Self(
            Arc::new(Mutex::new(BrowserData::default())),
            Arc::new(BrowserContext {
                window_label,
                is_private,
            }),
        )
    }

    fn page_label(&self, tab_id: &str) -> String {
        if self.1.window_label == "main" {
            format!("page-{tab_id}")
        } else {
            format!("page-{}-{tab_id}", self.1.window_label)
        }
    }

    fn window_label(&self) -> &str {
        &self.1.window_label
    }

    fn is_private(&self) -> bool {
        self.1.is_private
    }

    pub(super) fn initialize(
        &self,
        initial_tab: TabInfo,
        groups: Vec<TabGroup>,
    ) -> Result<(), String> {
        let mut data = self
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        data.tabs.push(initial_tab);
        data.groups = groups;
        data.active_tab_id = Some("1".to_string());
        data.next_id = 2;
        data.next_group_id = data
            .groups
            .iter()
            .filter_map(|group| group.id.parse::<u64>().ok())
            .max()
            .unwrap_or_default()
            .saturating_add(1)
            .max(1);
        data.revision = 1;
        data.page_visible = true;
        Ok(())
    }

    fn persists_session(&self) -> bool {
        self.window_label() == "main"
    }
}

#[derive(Clone, Default)]
pub struct BrowserWindowStates(Arc<Mutex<HashMap<String, BrowserState>>>);

impl BrowserWindowStates {
    fn get(&self, label: &str) -> Result<BrowserState, String> {
        self.0
            .lock()
            .map_err(|_| "browser window state lock is poisoned".to_string())?
            .get(label)
            .cloned()
            .ok_or_else(|| "browser window is not initialized".to_string())
    }

    fn insert(&self, label: String, state: BrowserState) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "private window state lock is poisoned".to_string())?
            .insert(label, state);
        Ok(())
    }

    pub fn remove(&self, label: &str) {
        match self.0.lock() {
            Ok(mut states) => {
                states.remove(label);
            }
            Err(_) => eprintln!("browser window state lock poisoned during cleanup"),
        }
    }

    fn all(&self) -> Result<Vec<BrowserState>, String> {
        self.0
            .lock()
            .map_err(|_| "browser window state lock is poisoned".to_string())
            .map(|states| states.values().cloned().collect())
    }
}

#[derive(Clone)]
pub struct TabSleepPolicy(Arc<AtomicU64>);

impl Default for TabSleepPolicy {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(TAB_SLEEP_TIMEOUT_UNINITIALIZED)))
    }
}

impl TabSleepPolicy {
    fn timeout_seconds(&self) -> u64 {
        match self.0.load(Ordering::Relaxed) {
            TAB_SLEEP_TIMEOUT_UNINITIALIZED => DEFAULT_TAB_SLEEP_TIMEOUT_SECONDS,
            seconds => seconds,
        }
    }

    fn set_timeout_seconds(&self, seconds: u64) {
        self.0.store(seconds, Ordering::Relaxed);
    }

    fn initialize_timeout_seconds(&self, seconds: u64) {
        let _ = self.0.compare_exchange(
            TAB_SLEEP_TIMEOUT_UNINITIALIZED,
            seconds,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
}

fn idle_tabs_past_timeout(data: &BrowserData, now: Instant, timeout: Duration) -> Vec<String> {
    data.tabs
        .iter()
        .filter(|tab| {
            data.active_tab_id.as_deref() != Some(&tab.id)
                && !tab.sleeping
                && !data.active_downloads.contains(&tab.id)
                && data
                    .inactive_since
                    .get(&tab.id)
                    .is_some_and(|inactive_since| now.duration_since(*inactive_since) >= timeout)
        })
        .map(|tab| tab.id.clone())
        .collect()
}

fn sleep_inactive_tabs(app: &AppHandle, state: &BrowserState, timeout: Duration) {
    let now = Instant::now();
    let candidates = match state.0.lock() {
        Ok(data) => idle_tabs_past_timeout(&data, now, timeout),
        Err(_) => {
            eprintln!("browser tab state lock poisoned while checking sleeping tabs");
            return;
        }
    };

    for tab_id in candidates {
        let label = state.page_label(&tab_id);
        match app.get_webview(&label) {
            Some(view) => {
                if let Err(error) = view.close() {
                    if let Ok(mut data) = state.0.lock() {
                        data.inactive_since.insert(tab_id.clone(), Instant::now());
                    }
                    eprintln!("could not discard idle tab {tab_id}: {error}");
                    continue;
                }
            }
            None => eprintln!("idle tab {tab_id} had no page webview to discard"),
        }

        let sleeping = match state.0.lock() {
            Ok(mut data) => {
                if data.active_tab_id.as_deref() == Some(&tab_id) {
                    false
                } else if let Some(tab) = data.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    tab.sleeping = true;
                    data.inactive_since.remove(&tab_id);
                    data.revision = data.revision.saturating_add(1);
                    true
                } else {
                    false
                }
            }
            Err(_) => {
                eprintln!("browser tab state lock poisoned after discarding idle tab");
                false
            }
        };
        if sleeping {
            let event = TabSleepEvent {
                tab_id,
                sleeping: true,
            };
            if let Err(error) = app.emit_to(state.window_label(), "browser:tab-sleep", event) {
                eprintln!("could not notify browser chrome about a sleeping tab: {error}");
            }
        }
    }
}

fn validate_tab_sleep_timeout(seconds: u64) -> Result<(), String> {
    if TAB_SLEEP_TIMEOUT_OPTIONS.contains(&seconds) {
        Ok(())
    } else {
        Err("Choose a supported tab sleep timeout.".to_string())
    }
}

fn parse_tab_sleep_timeout(value: Option<String>) -> Result<u64, String> {
    let seconds = match value {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "Saved tab sleep timeout is invalid.".to_string())?,
        None => DEFAULT_TAB_SLEEP_TIMEOUT_SECONDS,
    };
    validate_tab_sleep_timeout(seconds)?;
    Ok(seconds)
}

#[tauri::command]
pub async fn tab_sleep_timeout_get(
    storage: State<'_, StorageService>,
    policy: State<'_, TabSleepPolicy>,
) -> Result<u64, String> {
    let value = storage
        .with_connection(|connection| get_setting(connection, TAB_SLEEP_SETTING_KEY))
        .await?;
    let seconds = parse_tab_sleep_timeout(value)?;
    policy.set_timeout_seconds(seconds);
    Ok(seconds)
}

#[tauri::command]
pub async fn tab_sleep_timeout_set(
    seconds: u64,
    storage: State<'_, StorageService>,
    policy: State<'_, TabSleepPolicy>,
) -> Result<u64, String> {
    validate_tab_sleep_timeout(seconds)?;
    let value = seconds.to_string();
    storage
        .with_connection(move |connection| set_setting(connection, TAB_SLEEP_SETTING_KEY, &value))
        .await?;
    policy.set_timeout_seconds(seconds);
    Ok(seconds)
}

pub fn start_tab_sleep_monitor(
    app: AppHandle,
    normal_state: BrowserState,
    window_states: BrowserWindowStates,
    storage: StorageService,
    policy: TabSleepPolicy,
) {
    tauri::async_runtime::spawn(async move {
        match storage
            .with_connection(|connection| get_setting(connection, TAB_SLEEP_SETTING_KEY))
            .await
            .and_then(parse_tab_sleep_timeout)
        {
            Ok(seconds) => policy.initialize_timeout_seconds(seconds),
            Err(error) => eprintln!("could not load tab sleep preference: {error}"),
        }

        let mut interval =
            tokio::time::interval(Duration::from_secs(TAB_SLEEP_SCAN_INTERVAL_SECONDS));
        loop {
            interval.tick().await;
            let timeout_seconds = policy.timeout_seconds();
            if timeout_seconds == 0 {
                continue;
            }
            let mut states = vec![normal_state.clone()];
            match window_states.all() {
                Ok(additional_states) => states.extend(additional_states),
                Err(error) => {
                    eprintln!("could not inspect browser windows for sleeping tabs: {error}");
                    continue;
                }
            }
            let app_for_tick = app.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                let timeout = Duration::from_secs(timeout_seconds);
                for state in states {
                    sleep_inactive_tabs(&app_for_tick, &state, timeout);
                }
            }) {
                eprintln!("could not schedule tab sleep check: {error}");
            }
        }
    });
}

fn state_for_window(
    label: &str,
    normal_state: &BrowserState,
    window_states: &BrowserWindowStates,
) -> Result<BrowserState, String> {
    if label == "main" {
        return Ok(normal_state.clone());
    }
    window_states.get(label)
}

fn is_page_webview_label(label: &str) -> bool {
    let Some(remainder) = label.strip_prefix("page-") else {
        return false;
    };
    let is_id = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    if is_id(remainder) {
        return true;
    }
    let Some((window_label, tab_id)) = remainder.rsplit_once('-') else {
        return false;
    };
    window_label
        .strip_prefix("private-")
        .or_else(|| window_label.strip_prefix("window-"))
        .is_some_and(is_id)
        && is_id(tab_id)
}

#[tauri::command]
pub fn browser_window_mode(webview: Webview<Wry>) -> bool {
    webview.window().label().starts_with("private-")
}

#[tauri::command]
pub async fn open_private_window(
    app: AppHandle,
    _webview: Webview<Wry>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<String, String> {
    ensure_private_webview_runtime()?;
    create_secondary_window(
        app,
        window_states.inner(),
        storage.inner().clone(),
        download_location.inner().clone(),
        adblock.inner().clone(),
        privacy.inner().clone(),
        true,
        TabInfo {
            id: "1".to_string(),
            title: "New tab".to_string(),
            url: "about:blank".to_string(),
            group_id: None,
            sleeping: false,
        },
        Vec::new(),
    )
}

#[tauri::command]
pub async fn open_browser_window(
    app: AppHandle,
    _webview: Webview<Wry>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<String, String> {
    create_secondary_window(
        app,
        window_states.inner(),
        storage.inner().clone(),
        download_location.inner().clone(),
        adblock.inner().clone(),
        privacy.inner().clone(),
        false,
        TabInfo {
            id: "1".to_string(),
            title: "New tab".to_string(),
            url: "about:blank".to_string(),
            group_id: None,
            sleeping: false,
        },
        Vec::new(),
    )
}

#[tauri::command]
pub async fn detach_tab_to_new_window(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock_service: State<'_, AdblockService>,
    privacy_service: State<'_, PrivacyService>,
) -> Result<String, String> {
    let source_state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    validate_tab_id(&tab_id)?;
    let (source_tab, group, source_is_last_tab) = {
        let data = source_state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let tab = data
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
            .ok_or_else(|| "tab does not exist".to_string())?;
        let group = tab.group_id.as_ref().and_then(|group_id| {
            data.groups
                .iter()
                .find(|group| &group.id == group_id)
                .cloned()
        });
        (tab, group, data.tabs.len() == 1)
    };
    let target_url = safe_session_url(&source_tab.url)?;
    let target_group = group.map(|group| TabGroup {
        id: "1".to_string(),
        name: group.name,
        color: group.color,
    });
    let mut target_tab = source_tab.clone();
    target_tab.id = "1".to_string();
    target_tab.url = target_url.to_string();
    target_tab.group_id = target_group.as_ref().map(|group| group.id.clone());
    target_tab.sleeping = false;

    let target_label = create_secondary_window(
        app.clone(),
        window_states.inner(),
        storage.inner().clone(),
        download_location.inner().clone(),
        adblock_service.inner().clone(),
        privacy_service.inner().clone(),
        false,
        target_tab,
        target_group.into_iter().collect(),
    )?;

    if source_is_last_tab {
        let blank_url = Url::parse("about:blank")
            .map_err(|error| format!("could not construct blank page URL: {error}"))?;
        if let Err(error) = create_tab_internal(
            app.clone(),
            source_state.clone(),
            storage.inner().clone(),
            download_location.inner().clone(),
            adblock_service.inner().clone(),
            privacy_service.inner().clone(),
            blank_url,
            "New tab".to_string(),
            None,
        )
        .await
        {
            close_secondary_window(&app, window_states.inner(), &target_label);
            return Err(error);
        }
    }

    if let Err(error) = close_tab_for_state(
        &app,
        &tab_id,
        &source_state,
        storage.inner(),
        download_location.inner(),
        adblock_service.inner(),
        privacy_service.inner(),
    ) {
        close_secondary_window(&app, window_states.inner(), &target_label);
        return Err(error);
    }
    Ok(target_label)
}

fn close_secondary_window(app: &AppHandle, window_states: &BrowserWindowStates, label: &str) {
    window_states.remove(label);
    if let Some(window) = app.get_window(label) {
        if let Err(error) = window.close() {
            eprintln!("could not close browser window after tab detach failed: {error}");
        }
    }
}

fn create_secondary_window(
    app: AppHandle,
    window_states: &BrowserWindowStates,
    storage: StorageService,
    download_location: DownloadLocation,
    adblock_service: AdblockService,
    privacy_service: PrivacyService,
    is_private: bool,
    initial_tab: TabInfo,
    groups: Vec<TabGroup>,
) -> Result<String, String> {
    let id = PRIVATE_WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed);
    let label = if is_private {
        format!("private-{id}")
    } else {
        format!("window-{id}")
    };
    let state = BrowserState::for_window(label.clone(), is_private);
    state.initialize(initial_tab.clone(), groups)?;
    window_states.insert(label.clone(), state.clone())?;
    let builder =
        WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::App("index.html".into()))
            .on_page_load(|webview, payload| {
                let event = match payload.event() {
                    PageLoadEvent::Started => "started",
                    PageLoadEvent::Finished => "finished",
                };
                eprintln!(
                    "[browser-window:{}] chrome page {event}: {}",
                    webview.label(),
                    payload.url()
                );
            })
            .title(if is_private {
                "Eefi Browser - Private"
            } else {
                "Eefi Browser"
            })
            .inner_size(1180.0, 780.0)
            .min_inner_size(760.0, 540.0);
    let builder = if is_private {
        builder.incognito(true)
    } else {
        builder
    };
    eprintln!("[browser-window:{label}] creating chrome window");
    let window = match builder.build() {
        Ok(window) => {
            eprintln!("[browser-window:{label}] chrome window created");
            window
        }
        Err(error) => {
            window_states.remove(&label);
            return Err(format!("could not create browser window: {error}"));
        }
    };
    let initialize_result = (|| {
        let parent_window = window.as_ref().window();
        let size = parent_window
            .inner_size()
            .map_err(|error| format!("could not read browser window size: {error}"))?;
        let scale = parent_window
            .scale_factor()
            .map_err(|error| format!("could not read browser window scale: {error}"))?;
        let page_url = Url::parse("about:blank")
            .map_err(|error| format!("could not construct blank page URL: {error}"))?;
        let builder = page_builder(
            app.clone(),
            state.clone(),
            storage,
            download_location,
            initial_tab.id.clone(),
            WebviewUrl::External(page_url),
        );
        eprintln!("[browser-window:{label}] attaching initial page webview");
        let page = parent_window
            .add_child(
                builder,
                tauri::LogicalPosition::new(0.0, 128.0),
                tauri::LogicalSize::new(
                    f64::from(size.width) / scale,
                    (f64::from(size.height) / scale - 128.0).max(1.0),
                ),
            )
            .map_err(|error| format!("could not create browser tab: {error}"))?;
        eprintln!("[browser-window:{label}] initial page webview attached");
        adblock::attach_to_webview(&page, adblock_service, privacy_service)?;
        if initial_tab.url != "about:blank" {
            let target = safe_session_url(&initial_tab.url)?;
            page.navigate(target)
                .map_err(|error| format!("could not open initial browser page: {error}"))?;
        }
        Ok::<(), String>(())
    })();

    if let Err(error) = initialize_result {
        window_states.remove(&label);
        if let Err(close_error) = window.close() {
            eprintln!("could not close partially initialized browser window: {close_error}");
        }
        return Err(error);
    }
    Ok(label)
}

#[cfg(windows)]
fn ensure_private_webview_runtime() -> Result<(), String> {
    use webview2_com::{
        take_pwstr, Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString,
    };
    use windows::core::{PCWSTR, PWSTR};

    let mut version = PWSTR::null();
    unsafe {
        GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version).map_err(
            |error| format!("could not determine the WebView2 Runtime version: {error}"),
        )?;
    }
    let version = take_pwstr(version);
    let parsed = version
        .split('.')
        .take(4)
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "WebView2 Runtime returned an invalid version".to_string())?;
    if parsed.len() != 4 || (parsed[0], parsed[1], parsed[2], parsed[3]) < (101, 0, 1210, 39) {
        return Err(format!(
            "Private windows require WebView2 Runtime 101.0.1210.39 or newer; detected {version}"
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn ensure_private_webview_runtime() -> Result<(), String> {
    Ok(())
}

pub fn page_builder(
    app: AppHandle,
    state: BrowserState,
    storage: StorageService,
    download_location: DownloadLocation,
    tab_id: String,
    url: WebviewUrl,
) -> WebviewBuilder<Wry> {
    let label = state.page_label(&tab_id);
    let window_label = state.window_label().to_string();
    let title_window_label = window_label.clone();
    let is_private = state.is_private();
    let navigation_app = app.clone();
    let navigation_state = state.clone();
    let navigation_storage = storage.clone();
    let title_app = app.clone();
    let title_state = state.clone();
    let title_storage = storage.clone();
    let download_state = state.clone();
    let download_app = app;
    let download_storage = storage;
    let navigation_tab_id = tab_id.clone();
    let download_tab_id = tab_id.clone();
    let title_tab_id = tab_id;
    let download_location_for_hook = download_location;
    WebviewBuilder::new(label, url)
        .devtools(true)
        .initialization_script_for_all_frames(DEVTOOLS_F12_SCRIPT)
        .initialization_script(crate::oneko::initialization_script())
        .on_page_load(move |page, payload| {
            let url = payload.url().to_string();
            let finished = matches!(payload.event(), PageLoadEvent::Finished);
            if finished && !is_http_url(&url) {
                let page = page.clone();
                let storage = navigation_storage.clone();
                let page_url = url.clone();
                tauri::async_runtime::spawn(async move {
                    let (settings, host) =
                        match crate::oneko::settings_for_url(storage, page_url).await {
                            Ok(settings) => settings,
                            Err(error) => {
                                eprintln!("could not load Oneko settings for page: {error}");
                                return;
                            }
                        };
                    let mut payload = match serde_json::to_value(settings) {
                        Ok(payload) => payload,
                        Err(error) => {
                            eprintln!("could not serialize Oneko page settings: {error}");
                            return;
                        }
                    };
                    if let (Some(host), Some(object)) = (host, payload.as_object_mut()) {
                        object.insert("siteHost".to_string(), serde_json::Value::String(host));
                    }
                    let script = format!("window.__eefiOnekoUpdate?.({payload});");
                    if let Err(error) = page.eval(&script) {
                        eprintln!("could not initialize Oneko for page: {error}");
                    }
                });
            }
            let mut title = String::new();
            let load_generation = match navigation_state.0.lock() {
                Ok(mut data) => {
                    if let Some(tab) =
                        data.tabs.iter_mut().find(|tab| tab.id == navigation_tab_id)
                    {
                        tab.url = url.clone();
                        title = tab.title.clone();
                        data.revision = data.revision.saturating_add(1);
                    }
                    let generation = data
                        .navigation_generations
                        .entry(navigation_tab_id.clone())
                        .or_default();
                    if matches!(payload.event(), PageLoadEvent::Started) {
                        *generation = generation.saturating_add(1);
                    }
                    Some(*generation)
                }
                Err(_) => {
                    eprintln!("browser tab state lock poisoned while handling navigation");
                    None
                }
            };
            queue_session_save(navigation_app.clone(), navigation_state.clone());
            if !is_private && finished && is_http_url(&url) {
                let storage = navigation_storage.clone();
                let visited_url = url.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = storage
                        .with_connection(move |connection| {
                            record_history(connection, &visited_url, &title, unix_timestamp())
                        })
                        .await
                    {
                        eprintln!("could not save page history: {error}");
                    }
                });
                if let Some(generation) = load_generation {
                    let storage = navigation_storage.clone();
                    let app = navigation_app.clone();
                    let state = navigation_state.clone();
                    let tab_id = navigation_tab_id.clone();
                    let page_label = navigation_state.page_label(&navigation_tab_id);
                    let script_url = url.clone();
                    let lookup_url = script_url.clone();
                    tauri::async_runtime::spawn(async move {
                        let scripts = match storage
                            .with_connection(move |connection| {
                                crate::extensions::enabled_scripts_for_url(
                                    connection,
                                    &lookup_url,
                                )
                            })
                            .await
                        {
                            Ok(scripts) => scripts,
                            Err(error) => {
                                eprintln!("could not load matching user scripts: {error}");
                                return;
                            }
                        };
                        for script in scripts {
                            let is_current_navigation = match state.0.lock() {
                                Ok(data) => {
                                    data.navigation_generations.get(&tab_id) == Some(&generation)
                                        && data
                                            .tabs
                                            .iter()
                                            .any(|tab| tab.id == tab_id && tab.url == script_url)
                                }
                                Err(_) => {
                                    eprintln!(
                                        "browser tab state lock poisoned before user script injection"
                                    );
                                    return;
                                }
                            };
                            if !is_current_navigation {
                                return;
                            }
                            let Some(page) = app.get_webview(&page_label) else {
                                eprintln!("page webview disappeared before user script injection");
                                return;
                            };
                            if let Err(error) = page.eval(&script.source) {
                                eprintln!(
                                    "could not run user script {}: {error}",
                                    script.id
                                );
                            }
                        }
                    });
                }
            }
            let event = PageEvent {
                tab_id: navigation_tab_id.clone(),
                url,
                title: Some(if finished { "finished" } else { "loading" }.to_string()),
            };
            if let Err(error) = navigation_app.emit_to(&window_label, "browser:navigation", event) {
                eprintln!("could not notify browser chrome of navigation: {error}");
            }
        })
        .on_document_title_changed(move |_, title| {
            let mut current_url = None;
            if let Ok(mut data) = title_state.0.lock() {
                if let Some(tab) = data.tabs.iter_mut().find(|tab| tab.id == title_tab_id) {
                    tab.title = title.clone();
                    current_url = Some(tab.url.clone());
                    data.revision = data.revision.saturating_add(1);
                }
            } else {
                eprintln!("browser tab state lock poisoned while handling title update");
            }
            queue_session_save(title_app.clone(), title_state.clone());
            if !is_private {
                if let Some(url) = current_url.filter(|url| is_http_url(url)) {
                    let storage = title_storage.clone();
                    let history_title = title.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = storage
                            .with_connection(move |connection| {
                                update_latest_title(connection, &url, &history_title)
                            })
                            .await
                        {
                            eprintln!("could not update page history title: {error}");
                        }
                    });
                }
            }
            let event = PageEvent {
                tab_id: title_tab_id.clone(),
                url: String::new(),
                title: Some(title),
            };
            if let Err(error) = title_app.emit_to(&title_window_label, "browser:title", event) {
                eprintln!("could not notify browser chrome of title update: {error}");
            }
        })
        .on_download(move |_, event| match event {
            DownloadEvent::Requested { url, destination } => {
                match download_destination(&download_location_for_hook, destination) {
                    Ok(path) => {
                        *destination = path.clone();
                        match download_state.0.lock() {
                            Ok(mut data) => {
                                data.active_downloads.insert(download_tab_id.clone());
                            }
                            Err(_) => {
                                eprintln!("browser tab state lock poisoned while starting download");
                                return false;
                            }
                        }
                        if is_private {
                            return true;
                        }
                        let requested_url = url.to_string();
                        let target_path = path.to_string_lossy().into_owned();
                        let storage = download_storage.clone();
                        let app = download_app.clone();
                        tauri::async_runtime::spawn(async move {
                            match storage
                                .with_connection(move |connection| {
                                    record_download_start(
                                        connection,
                                        &requested_url,
                                        &target_path,
                                        unix_timestamp(),
                                    )
                                })
                                .await
                            {
                                Ok(_) => emit_download_changed(&app),
                                Err(error) => {
                                    eprintln!("could not record download start: {error}")
                                }
                            }
                        });
                        true
                    }
                    Err(error) => {
                        eprintln!("could not prepare download destination: {error}");
                        false
                    }
                }
            }
            DownloadEvent::Finished { url, path, success } => {
                match download_state.0.lock() {
                    Ok(mut data) => {
                        data.active_downloads.remove(&download_tab_id);
                    }
                    Err(_) => eprintln!("browser tab state lock poisoned while finishing download"),
                }
                if is_private {
                    return true;
                }
                let finished_url = url.to_string();
                let finished_path = path.map(|path| path.to_string_lossy().into_owned());
                let storage = download_storage.clone();
                let app = download_app.clone();
                tauri::async_runtime::spawn(async move {
                    for attempt in 0..20 {
                        let url = finished_url.clone();
                        let path = finished_path.clone();
                        match storage
                            .with_connection(move |connection| {
                                record_download_finish(
                                    connection,
                                    &url,
                                    path.as_deref(),
                                    success,
                                    unix_timestamp(),
                                )
                            })
                            .await
                        {
                            Ok(true) => {
                                emit_download_changed(&app);
                                return;
                            }
                            Ok(false) if attempt < 19 => {
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            }
                            Ok(false) => {
                                eprintln!(
                                    "download completion arrived without a matching start event"
                                );
                                return;
                            }
                            Err(error) => {
                                eprintln!("could not record download completion: {error}");
                                return;
                            }
                        }
                    }
                });
                true
            }
            _ => true,
        })
        .on_new_window(|_url, _features| NewWindowResponse::Deny)
        .incognito(is_private)
}

#[tauri::command]
pub fn list_tabs(
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<Vec<TabInfo>, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    list_tabs_for_state(&state)
}

fn list_tabs_for_state(state: &BrowserState) -> Result<Vec<TabInfo>, String> {
    let data = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?;
    Ok(data.tabs.clone())
}

#[tauri::command]
pub fn tab_groups_list(
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<Vec<TabGroup>, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let data = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?;
    Ok(data.groups.clone())
}

#[tauri::command]
pub fn tab_group_create(
    app: AppHandle,
    webview: Webview<Wry>,
    name: String,
    color: String,
    tab_ids: Vec<String>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<TabGroup, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let (name, color) = validate_group_details(&name, &color)?;
    if tab_ids.is_empty() || tab_ids.len() > 100 {
        return Err("Select between one and 100 tabs for a new group.".to_string());
    }
    let unique_tab_ids = tab_ids.iter().collect::<std::collections::HashSet<_>>();
    if unique_tab_ids.len() != tab_ids.len() {
        return Err("A tab can only be selected once for a group.".to_string());
    }
    for tab_id in &tab_ids {
        validate_tab_id(tab_id)?;
    }

    let group = {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        if data.groups.len() >= 64 {
            return Err("Only 64 tab groups can be created in one window.".to_string());
        }
        if tab_ids
            .iter()
            .any(|id| !data.tabs.iter().any(|tab| &tab.id == id))
        {
            return Err("A selected tab no longer exists.".to_string());
        }
        let group_id = data.next_group_id.to_string();
        data.next_group_id = data
            .next_group_id
            .checked_add(1)
            .ok_or_else(|| "tab group identifier space is exhausted".to_string())?;
        for tab in &mut data.tabs {
            if tab_ids.contains(&tab.id) {
                tab.group_id = Some(group_id.clone());
            }
        }
        let group = TabGroup {
            id: group_id,
            name,
            color,
        };
        data.groups.push(group.clone());
        data.revision = data.revision.saturating_add(1);
        group
    };
    queue_session_save(app, state);
    Ok(group)
}

#[tauri::command]
pub fn tab_group_update(
    app: AppHandle,
    webview: Webview<Wry>,
    group_id: String,
    name: String,
    color: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<TabGroup, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    validate_group_id(&group_id)?;
    let (name, color) = validate_group_details(&name, &color)?;
    let group = {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let group = data
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| "tab group no longer exists".to_string())?;
        group.name = name;
        group.color = color;
        let group = group.clone();
        data.revision = data.revision.saturating_add(1);
        group
    };
    queue_session_save(app, state);
    Ok(group)
}

#[tauri::command]
pub fn tab_group_assign(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    group_id: Option<String>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        assign_tab_to_group(&mut data, &tab_id, group_id.as_deref())?;
    }
    queue_session_save(app, state);
    Ok(())
}

#[tauri::command]
pub fn tab_group_delete(
    app: AppHandle,
    webview: Webview<Wry>,
    group_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    validate_group_id(&group_id)?;
    {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let previous_count = data.groups.len();
        data.groups.retain(|group| group.id != group_id);
        if data.groups.len() == previous_count {
            return Err("tab group no longer exists".to_string());
        }
        for tab in &mut data.tabs {
            if tab.group_id.as_deref() == Some(&group_id) {
                tab.group_id = None;
            }
        }
        data.revision = data.revision.saturating_add(1);
    }
    queue_session_save(app, state);
    Ok(())
}

#[tauri::command]
pub fn active_tab_id(
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<Option<String>, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let data = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?;
    Ok(data.active_tab_id.clone())
}

#[tauri::command]
pub fn toggle_page_devtools(webview: Webview<Wry>) -> Result<(), String> {
    if !is_page_webview_label(webview.label()) {
        return Err("DevTools can only be toggled for a page webview".to_string());
    }
    if webview.is_devtools_open() {
        webview.close_devtools();
    } else {
        webview.open_devtools();
    }
    Ok(())
}

#[tauri::command]
pub fn open_find_bar(app: AppHandle, webview: Webview<Wry>) -> Result<(), String> {
    if !is_page_webview_label(webview.label()) {
        return Err("Find-in-page can only be opened by a page webview".to_string());
    }
    app.emit_to(webview.window().label(), "browser:find-open", ())
        .map_err(|error| format!("could not open find-in-page: {error}"))
}

#[tauri::command]
pub async fn find_in_page(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    query: String,
    request_id: u64,
    direction: i32,
    start_new_search: bool,
    restore_page_focus: bool,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    if is_page_webview_label(webview.label()) {
        return Err("find-in-page can only be controlled by browser chrome".to_string());
    }
    validate_tab_id(&tab_id)?;
    validate_find_query(&query)?;
    if request_id == 0 {
        return Err("invalid find-in-page request identifier".to_string());
    }
    if !matches!(direction, -1..=1) {
        return Err("invalid find-in-page direction".to_string());
    }
    if restore_page_focus && !query.is_empty() {
        return Err("page focus can only be restored after clearing the search".to_string());
    }

    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let tab_is_sleeping = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?
        .tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .ok_or_else(|| "tab does not exist".to_string())?
        .sleeping;
    if tab_is_sleeping {
        return Err("cannot search a sleeping tab".to_string());
    }
    let view = get_tab_view(&app, &tab_id, &state)?;
    #[cfg(windows)]
    let window_label = webview.window().label().to_string();

    #[cfg(windows)]
    {
        use std::{sync::mpsc, time::Duration};

        use webview2_com::{
            FindStartCompletedHandler,
            Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment15, ICoreWebView2_28},
        };
        use windows::core::{Interface, HSTRING};

        let (sender, receiver) = mpsc::sync_channel(1);
        let setup_sender = sender.clone();
        let completion_app = app.clone();
        let completion_window = window_label.clone();
        let completion_tab = tab_id.clone();
        let completion_query = query.clone();
        view.with_webview(move |platform| {
            let setup_result = (|| -> Result<(), String> {
                unsafe {
                    let core = platform
                        .controller()
                        .CoreWebView2()
                        .map_err(|error| format!("could not access WebView2: {error}"))?;
                    let find = core
                        .cast::<ICoreWebView2_28>()
                        .map_err(|error| {
                            format!(
                                "find-in-page requires a newer WebView2 Runtime: {error}"
                            )
                        })?
                        .Find()
                        .map_err(|error| format!("could not initialize find-in-page: {error}"))?;

                    if query.is_empty() {
                        find.Stop()
                            .map_err(|error| format!("could not stop find-in-page: {error}"))?;
                        completion_app
                            .emit_to(
                                &completion_window,
                                "browser:find-result",
                                FindResultEvent {
                                    tab_id: completion_tab,
                                    query: String::new(),
                                    request_id,
                                    match_count: 0,
                                    active_match_index: -1,
                                    error: None,
                                },
                            )
                            .map_err(|error| format!("could not update find-in-page: {error}"))?;
                    } else if direction == 0 || start_new_search {
                        let options = platform
                            .environment()
                            .cast::<ICoreWebView2Environment15>()
                            .map_err(|error| {
                                format!(
                                    "find-in-page requires a newer WebView2 Runtime: {error}"
                                )
                            })?
                            .CreateFindOptions()
                            .map_err(|error| {
                                format!("could not create find-in-page options: {error}")
                            })?;
                        options
                            .SetFindTerm(&HSTRING::from(query.as_str()))
                            .map_err(|error| format!("could not set search text: {error}"))?;
                        options
                            .SetIsCaseSensitive(false)
                            .map_err(|error| format!("could not configure text search: {error}"))?;
                        options
                            .SetShouldHighlightAllMatches(true)
                            .map_err(|error| {
                                format!("could not configure match highlighting: {error}")
                            })?;
                        options
                            .SetSuppressDefaultFindDialog(true)
                            .map_err(|error| format!("could not configure find-in-page: {error}"))?;

                        let callback_app = completion_app.clone();
                        let callback_window = completion_window.clone();
                        let callback_tab = completion_tab.clone();
                        let callback_query = completion_query.clone();
                        let callback_request_id = request_id;
                        let callback_find = find.clone();
                        let handler = FindStartCompletedHandler::create(Box::new(move |result| {
                            let event = match result {
                                Ok(()) => {
                                    let navigation_result = match direction {
                                        1 => callback_find.FindNext(),
                                        -1 => callback_find.FindPrevious(),
                                        _ => Ok(()),
                                    };
                                    navigation_result
                                        .map_err(|error| {
                                            format!("could not navigate search matches: {error}")
                                        })
                                        .and_then(|()| {
                                            let mut match_count = 0;
                                            let mut active_match_index = -1;
                                            callback_find.MatchCount(&mut match_count).map_err(
                                                |error| {
                                                    format!(
                                                        "could not read search match count: {error}"
                                                    )
                                                },
                                            )?;
                                            callback_find
                                                .ActiveMatchIndex(&mut active_match_index)
                                                .map_err(|error| {
                                                    format!(
                                                        "could not read active search match: {error}"
                                                    )
                                                })?;
                                            Ok(FindResultEvent {
                                                tab_id: callback_tab.clone(),
                                                query: callback_query.clone(),
                                                request_id: callback_request_id,
                                                match_count,
                                                active_match_index,
                                                error: None,
                                            })
                                        })
                                }
                                Err(error) => Err(format!("find-in-page failed: {error}")),
                            };
                            let event = event.unwrap_or_else(|error| FindResultEvent {
                                tab_id: callback_tab,
                                query: callback_query,
                                request_id: callback_request_id,
                                match_count: 0,
                                active_match_index: -1,
                                error: Some(error),
                            });
                            if let Err(error) =
                                callback_app.emit_to(&callback_window, "browser:find-result", event)
                            {
                                eprintln!(
                                    "could not notify browser chrome about find results: {error}"
                                );
                            }
                            Ok(())
                        }));
                        find.Start(&options, &handler)
                            .map_err(|error| format!("could not start find-in-page: {error}"))?;
                    } else {
                        let navigation_result = if direction > 0 {
                            find.FindNext()
                        } else {
                            find.FindPrevious()
                        };
                        navigation_result
                            .map_err(|error| format!("could not navigate search matches: {error}"))?;

                        let mut match_count = 0;
                        let mut active_match_index = -1;
                        find.MatchCount(&mut match_count)
                            .map_err(|error| format!("could not read search match count: {error}"))?;
                        find.ActiveMatchIndex(&mut active_match_index).map_err(|error| {
                            format!("could not read active search match: {error}")
                        })?;
                        completion_app
                            .emit_to(
                                &completion_window,
                                "browser:find-result",
                                FindResultEvent {
                                    tab_id: completion_tab,
                                    query: completion_query,
                                    request_id,
                                    match_count,
                                    active_match_index,
                                    error: None,
                                },
                            )
                            .map_err(|error| {
                                format!("could not update find-in-page results: {error}")
                            })?;
                    }
                }
                Ok(())
            })();
            let _ = setup_sender.send(setup_result);
        })
        .map_err(|error| format!("could not access browser webview: {error}"))?;

        tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| format!("timed out setting up find-in-page: {error}"))?
        })
        .await
        .map_err(|error| format!("find-in-page worker failed: {error}"))??;
        if restore_page_focus {
            view.set_focus()
                .map_err(|error| format!("could not return focus to the page: {error}"))?;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = (view, query, request_id, direction, restore_page_focus);
        Err("find-in-page is supported on Windows only".to_string())
    }
}

fn validate_find_query(query: &str) -> Result<(), String> {
    if query.len() > 512 {
        return Err("search text must be 512 bytes or fewer".to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn toggle_devtools(
    app: AppHandle,
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let active_id = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?
        .active_tab_id
        .clone()
        .ok_or_else(|| "there is no active tab to inspect".to_string())?;
    let page = get_tab_view(&app, &active_id, &state)?;
    if page.is_devtools_open() {
        page.close_devtools();
    } else {
        page.open_devtools();
    }
    Ok(())
}

#[tauri::command]
pub async fn open_page_print_dialog(webview: Webview<Wry>) -> Result<(), String> {
    if !is_page_webview_label(webview.label()) {
        return Err("printing from page content is only allowed for a page webview".to_string());
    }
    show_print_ui(webview).await
}

#[tauri::command]
pub async fn print_page(
    app: AppHandle,
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    if is_page_webview_label(webview.label()) {
        return Err("print can only be started from browser chrome".to_string());
    }
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let tab_id = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?
        .active_tab_id
        .clone()
        .ok_or_else(|| "there is no active tab to print".to_string())?;
    let page = get_awake_tab_view(&app, &tab_id, &state)?;
    show_print_ui(page).await
}

#[tauri::command]
pub async fn save_page_as_pdf(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    file_path: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<String, String> {
    if is_page_webview_label(webview.label()) {
        return Err("saving a PDF can only be started from browser chrome".to_string());
    }
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let page = get_awake_tab_view(&app, &tab_id, &state)?;
    let pdf_path = validate_pdf_output_path(&file_path)?;
    let path_text = pdf_path
        .to_str()
        .ok_or_else(|| "the selected PDF path is not a valid Windows path".to_string())?
        .to_string();

    #[cfg(windows)]
    {
        use std::{sync::mpsc, time::Duration};

        use webview2_com::{
            Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment6, ICoreWebView2_7},
            PrintToPdfCompletedHandler,
        };
        use windows::core::{Interface, HSTRING};

        let output_path = HSTRING::from(path_text.clone());
        let (setup_sender, setup_receiver) = mpsc::sync_channel(1);
        let (completion_sender, completion_receiver) = mpsc::sync_channel(1);
        let callback_sender = completion_sender.clone();
        page.with_webview(move |platform| {
            let setup_result = (|| -> Result<(), String> {
                unsafe {
                    let core = platform
                        .controller()
                        .CoreWebView2()
                        .map_err(|error| format!("could not access WebView2: {error}"))?;
                    let environment = platform
                        .environment()
                        .cast::<ICoreWebView2Environment6>()
                        .map_err(|error| {
                            format!("Save as PDF requires a newer WebView2 Runtime: {error}")
                        })?;
                    let settings = environment
                        .CreatePrintSettings()
                        .map_err(|error| format!("could not create PDF print settings: {error}"))?;
                    settings
                        .SetShouldPrintBackgrounds(true)
                        .map_err(|error| format!("could not set PDF print settings: {error}"))?;
                    let handler =
                        PrintToPdfCompletedHandler::create(Box::new(move |result, succeeded| {
                            let result = result
                                .map_err(|error| {
                                    format!("WebView2 could not save the PDF: {error}")
                                })
                                .and_then(|()| {
                                    if succeeded {
                                        Ok(())
                                    } else {
                                        Err("WebView2 did not finish saving the PDF.".to_string())
                                    }
                                });
                            let _ = callback_sender.send(result);
                            Ok(())
                        }));
                    core.cast::<ICoreWebView2_7>()
                        .map_err(|error| {
                            format!("Save as PDF requires a newer WebView2 Runtime: {error}")
                        })?
                        .PrintToPdf(&output_path, &settings, &handler)
                        .map_err(|error| format!("could not start saving the PDF: {error}"))?;
                }
                Ok(())
            })();
            let _ = setup_sender.send(setup_result);
        })
        .map_err(|error| format!("could not access browser webview: {error}"))?;

        tauri::async_runtime::spawn_blocking(move || {
            setup_receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| format!("timed out starting PDF export: {error}"))?
        })
        .await
        .map_err(|error| format!("PDF setup worker failed: {error}"))??;

        tauri::async_runtime::spawn_blocking(move || {
            completion_receiver
                .recv_timeout(Duration::from_secs(120))
                .map_err(|error| format!("timed out waiting for PDF export: {error}"))?
        })
        .await
        .map_err(|error| format!("PDF export worker failed: {error}"))??;

        Ok(path_text)
    }

    #[cfg(not(windows))]
    {
        let _ = (page, path_text);
        Err("Save as PDF is only supported on Windows WebView2.".to_string())
    }
}

async fn show_print_ui(page: Webview<Wry>) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::{sync::mpsc, time::Duration};

        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_16, COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER,
        };
        use windows::core::Interface;

        let (sender, receiver) = mpsc::sync_channel(1);
        let callback_sender = sender.clone();
        page.with_webview(move |platform| {
            let result = unsafe {
                platform
                    .controller()
                    .CoreWebView2()
                    .map_err(|error| format!("could not access WebView2: {error}"))
                    .and_then(|core| {
                        core.cast::<ICoreWebView2_16>()
                            .map_err(|error| {
                                format!("printing requires a newer WebView2 Runtime: {error}")
                            })?
                            .ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER)
                            .map_err(|error| format!("could not open print dialog: {error}"))
                    })
            };
            let _ = callback_sender.send(result);
        })
        .map_err(|error| format!("could not access browser webview: {error}"))?;

        tauri::async_runtime::spawn_blocking(move || {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| format!("timed out opening print dialog: {error}"))?
        })
        .await
        .map_err(|error| format!("print dialog worker failed: {error}"))??;

        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = page;
        Err("Printing is only supported on Windows WebView2.".to_string())
    }
}

#[tauri::command]
pub async fn restore_session(
    app: AppHandle,
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<Vec<TabInfo>, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    adblock::ensure_preferences_loaded(storage.inner(), adblock.inner()).await?;
    privacy::ensure_preferences_loaded(storage.inner(), privacy.inner()).await?;
    privacy::ensure_site_permissions_loaded(storage.inner(), privacy.inner()).await?;
    if !state.persists_session() {
        return list_tabs_for_state(&state);
    }
    {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        if data.restore_attempted {
            return Ok(data.tabs.clone());
        }
        data.restore_attempted = true;
    }

    let mut saved_session = storage
        .with_connection(|connection| load_session(connection))
        .await?;
    saved_session.tabs.truncate(100);
    saved_session.groups.truncate(64);
    for group in &saved_session.groups {
        validate_group_id(&group.id)?;
        validate_group_details(&group.name, &group.color)?;
    }
    let group_ids = saved_session
        .groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for tab in &mut saved_session.tabs {
        if tab
            .group_id
            .as_ref()
            .is_some_and(|group_id| !group_ids.contains(group_id))
        {
            tab.group_id = None;
        }
    }
    {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        data.groups = saved_session
            .groups
            .iter()
            .map(|group| TabGroup {
                id: group.id.clone(),
                name: group.name.clone(),
                color: group.color.clone(),
            })
            .collect();
        data.revision = data.revision.max(saved_session.revision);
        data.next_group_id = saved_session
            .groups
            .iter()
            .filter_map(|group| group.id.parse::<u64>().ok())
            .max()
            .unwrap_or_default()
            .saturating_add(1)
            .max(1);
    }
    if saved_session.tabs.is_empty() {
        return list_tabs_for_state(&state);
    }

    let active_position = saved_session
        .tabs
        .iter()
        .position(|tab| tab.active)
        .unwrap_or_default();
    let first = &saved_session.tabs[0];
    let first_url = safe_session_url(&first.url)?;
    {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let tab = data
            .tabs
            .iter_mut()
            .find(|tab| tab.id == "1")
            .ok_or_else(|| "initial browser tab is missing".to_string())?;
        tab.title = first.title.chars().take(512).collect();
        tab.url = first_url.to_string();
        tab.group_id = first.group_id.clone();
        data.revision = data.revision.saturating_add(1);
    }
    if first_url.as_str() != "about:blank" {
        app.get_webview(&state.page_label("1"))
            .ok_or_else(|| "initial browser tab webview is missing".to_string())?
            .navigate(first_url)
            .map_err(|error| format!("could not restore initial tab: {error}"))?;
    }

    let mut tab_ids = vec!["1".to_string()];
    for saved in saved_session.tabs.iter().skip(1) {
        let url = safe_session_url(&saved.url)?;
        let tab = create_tab_internal(
            app.clone(),
            state.clone(),
            storage.inner().clone(),
            download_location.inner().clone(),
            adblock.inner().clone(),
            privacy.inner().clone(),
            url,
            saved.title.chars().take(512).collect(),
            saved.group_id.clone(),
        )
        .await?;
        tab_ids.push(tab.id);
    }
    let selected_id = tab_ids
        .get(active_position)
        .ok_or_else(|| "saved active tab is out of range".to_string())?;
    activate_tab_internal(
        &app,
        selected_id,
        &state,
        storage.inner(),
        download_location.inner(),
        adblock.inner(),
        privacy.inner(),
    )?;
    queue_session_save(app, state.clone());
    list_tabs_for_state(&state)
}

#[tauri::command]
pub fn set_page_content_visible(
    app: AppHandle,
    webview: Webview<Wry>,
    visible: bool,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let active_id = {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        data.page_visible = visible;
        data.active_tab_id.clone()
    };
    if let Some(active_id) = active_id {
        let view = app
            .get_webview(&state.page_label(&active_id))
            .ok_or_else(|| "active tab webview does not exist".to_string())?;
        if visible {
            view.show()
                .map_err(|error| format!("could not show active page: {error}"))?;
        } else {
            view.hide()
                .map_err(|error| format!("could not hide active page: {error}"))?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn create_tab(
    app: AppHandle,
    webview: Webview<Wry>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<TabInfo, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let url = Url::parse("about:blank")
        .map_err(|error| format!("could not construct blank page URL: {error}"))?;
    create_tab_internal(
        app,
        state,
        storage.inner().clone(),
        download_location.inner().clone(),
        adblock.inner().clone(),
        privacy.inner().clone(),
        url,
        "New tab".to_string(),
        None,
    )
    .await
}

async fn create_tab_internal(
    app: AppHandle,
    state: BrowserState,
    storage: StorageService,
    download_location: DownloadLocation,
    adblock: AdblockService,
    privacy: PrivacyService,
    url: Url,
    title: String,
    group_id: Option<String>,
) -> Result<TabInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_tab_blocking(
            app,
            state,
            storage,
            download_location,
            adblock,
            privacy,
            url,
            title,
            group_id,
        )
    })
    .await
    .map_err(|error| format!("tab creation worker failed: {error}"))?
}

fn create_tab_blocking(
    app: AppHandle,
    state: BrowserState,
    storage: StorageService,
    download_location: DownloadLocation,
    adblock: AdblockService,
    privacy: PrivacyService,
    url: Url,
    title: String,
    group_id: Option<String>,
) -> Result<TabInfo, String> {
    let (tab_id, previous_active, page_visible) = {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        if group_id
            .as_ref()
            .is_some_and(|group_id| !data.groups.iter().any(|group| &group.id == group_id))
        {
            return Err("tab group no longer exists".to_string());
        }
        let id = data.next_id.max(1);
        data.next_id = id
            .checked_add(1)
            .ok_or_else(|| "tab identifier space is exhausted".to_string())?;
        (
            id.to_string(),
            data.active_tab_id.clone(),
            data.page_visible,
        )
    };
    let tab = TabInfo {
        id: tab_id.clone(),
        title,
        url: url.to_string(),
        group_id,
        sleeping: false,
    };
    let window = app
        .get_window(state.window_label())
        .ok_or_else(|| "browser window is missing".to_string())?;
    let size = window
        .inner_size()
        .map_err(|error| format!("could not read browser window size: {error}"))?;
    let scale = window
        .scale_factor()
        .map_err(|error| format!("could not read browser window scale: {error}"))?;
    let width = f64::from(size.width) / scale;
    let height = f64::from(size.height) / scale;
    let blank_url = Url::parse("about:blank")
        .map_err(|error| format!("could not construct blank page URL: {error}"))?;
    let builder = page_builder(
        app.clone(),
        state.clone(),
        storage,
        download_location,
        tab_id.clone(),
        WebviewUrl::External(blank_url),
    );
    let view = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(0.0, 96.0),
            tauri::LogicalSize::new(width, (height - 96.0).max(1.0)),
        )
        .map_err(|error| format!("could not create browser tab: {error}"))?;

    if let Err(error) = view.hide() {
        if let Err(close_error) = view.close() {
            eprintln!("could not close uninitialized browser tab: {close_error}");
        }
        return Err(format!("could not hide new browser tab: {error}"));
    }
    if let Err(error) = adblock::attach_to_webview(&view, adblock, privacy) {
        if let Err(close_error) = view.close() {
            eprintln!("could not close browser tab after interception setup failed: {close_error}");
        }
        return Err(error);
    }
    match state.0.lock() {
        Ok(mut data) => {
            if let Some(previous_id) = previous_active.as_ref() {
                data.inactive_since
                    .entry(previous_id.clone())
                    .or_insert_with(Instant::now);
            }
            data.inactive_since.remove(&tab_id);
            data.tabs.push(tab.clone());
            data.active_tab_id = Some(tab_id.clone());
            data.revision = data.revision.saturating_add(1);
        }
        Err(_) => {
            if let Err(close_error) = view.close() {
                eprintln!("could not close browser tab after state update failed: {close_error}");
            }
            return Err("browser tab state lock is poisoned".to_string());
        }
    }
    if url.as_str() != "about:blank" {
        if let Err(error) = view.navigate(url) {
            return Err(rollback_tab_creation(
                &app,
                &state,
                &tab_id,
                previous_active.as_deref(),
                page_visible,
                &view,
                format!("could not navigate new browser tab: {error}"),
            ));
        }
    }
    match previous_active.as_deref() {
        Some(previous_id) => match app.get_webview(&state.page_label(previous_id)) {
            Some(previous) => {
                if let Err(error) = previous.hide() {
                    return Err(rollback_tab_creation(
                        &app,
                        &state,
                        &tab_id,
                        Some(previous_id),
                        page_visible,
                        &view,
                        format!("could not hide previous tab: {error}"),
                    ));
                }
            }
            None => {
                return Err(rollback_tab_creation(
                    &app,
                    &state,
                    &tab_id,
                    Some(previous_id),
                    page_visible,
                    &view,
                    "previous tab webview does not exist".to_string(),
                ));
            }
        },
        None => {}
    }
    if page_visible {
        if let Err(error) = view.show() {
            return Err(rollback_tab_creation(
                &app,
                &state,
                &tab_id,
                previous_active.as_deref(),
                page_visible,
                &view,
                format!("could not show new browser tab: {error}"),
            ));
        }
    }
    queue_session_save(app, state);
    Ok(tab)
}

fn rollback_tab_creation(
    app: &AppHandle,
    state: &BrowserState,
    tab_id: &str,
    previous_active: Option<&str>,
    page_visible: bool,
    view: &tauri::Webview<Wry>,
    error: String,
) -> String {
    let restore_previous = match state.0.lock() {
        Ok(mut data) => rollback_tab_metadata(&mut data, tab_id, previous_active),
        Err(_) => {
            eprintln!("browser tab state lock poisoned while rolling back tab creation");
            false
        }
    };
    if restore_previous {
        if let Some(previous_id) = previous_active {
            if let Some(previous) = app.get_webview(&state.page_label(previous_id)) {
                let restore = if page_visible {
                    previous.show()
                } else {
                    previous.hide()
                };
                if let Err(restore_error) = restore {
                    eprintln!("could not restore previous tab visibility: {restore_error}");
                }
            } else {
                eprintln!("could not restore previous tab because its webview is missing");
            }
        }
    }
    if let Err(close_error) = view.close() {
        eprintln!("could not close browser tab after creation failed: {close_error}");
    }
    error
}

fn rollback_tab_metadata(
    data: &mut BrowserData,
    tab_id: &str,
    previous_active: Option<&str>,
) -> bool {
    let was_active = data.active_tab_id.as_deref() == Some(tab_id);
    data.tabs.retain(|tab| tab.id != tab_id);
    data.inactive_since.remove(tab_id);
    if was_active {
        data.active_tab_id = previous_active.map(str::to_string);
        if let Some(previous_id) = previous_active {
            data.inactive_since.remove(previous_id);
        }
    }
    data.revision = data.revision.saturating_add(1);
    was_active
}

#[tauri::command]
pub async fn close_tab(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<Vec<TabInfo>, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let storage = storage.inner().clone();
    let download_location = download_location.inner().clone();
    let adblock = adblock.inner().clone();
    let privacy = privacy.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        close_tab_for_state(
            &app,
            &tab_id,
            &state,
            &storage,
            &download_location,
            &adblock,
            &privacy,
        )
    })
    .await
    .map_err(|error| format!("tab closing worker failed: {error}"))?
}

fn close_tab_for_state(
    app: &AppHandle,
    tab_id: &str,
    state: &BrowserState,
    storage: &StorageService,
    download_location: &DownloadLocation,
    adblock: &AdblockService,
    privacy: &PrivacyService,
) -> Result<Vec<TabInfo>, String> {
    validate_tab_id(&tab_id)?;
    let (was_active, was_sleeping, next_active, remaining) = {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let index = data
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or_else(|| "tab does not exist".to_string())?;
        let removed = data.tabs.remove(index);
        data.inactive_since.remove(tab_id);
        data.active_downloads.remove(tab_id);
        let was_active = data.active_tab_id.as_deref() == Some(&tab_id);
        if was_active {
            let next = data.tabs.get(index.min(data.tabs.len().saturating_sub(1)));
            data.active_tab_id = next.map(|tab| tab.id.clone());
        }
        data.revision = data.revision.saturating_add(1);
        (
            was_active,
            removed.sleeping,
            data.active_tab_id.clone(),
            data.tabs.clone(),
        )
    };
    if !was_sleeping {
        let view = app
            .get_webview(&state.page_label(&tab_id))
            .ok_or_else(|| "tab webview does not exist".to_string())?;
        view.close()
            .map_err(|error| format!("could not close tab: {error}"))?;
    }
    if was_active {
        if let Some(next_id) = next_active {
            activate_tab_internal(
                app,
                &next_id,
                state,
                storage,
                download_location,
                adblock,
                privacy,
            )?;
        }
    }
    queue_session_save(app.clone(), state.clone());
    Ok(remaining)
}

#[tauri::command]
pub async fn activate_tab(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    download_location: State<'_, DownloadLocation>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let storage = storage.inner().clone();
    let download_location = download_location.inner().clone();
    let adblock = adblock.inner().clone();
    let privacy = privacy.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        activate_tab_internal(
            &app,
            &tab_id,
            &state,
            &storage,
            &download_location,
            &adblock,
            &privacy,
        )
    })
    .await
    .map_err(|error| format!("tab activation worker failed: {error}"))?
}

#[tauri::command]
pub fn reorder_tabs(
    app: AppHandle,
    webview: Webview<Wry>,
    order: Vec<String>,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let mut data = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?;
    if order.len() != data.tabs.len() {
        return Err("tab order must contain every open tab exactly once".to_string());
    }
    let current: std::collections::HashSet<&str> =
        data.tabs.iter().map(|tab| tab.id.as_str()).collect();
    let requested: std::collections::HashSet<&str> = order.iter().map(String::as_str).collect();
    if requested.len() != order.len() || requested != current {
        return Err("tab order contains an unknown or duplicate tab".to_string());
    }
    let mut ordered = Vec::with_capacity(data.tabs.len());
    for id in order {
        let index = data
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or_else(|| "tab order contains an unknown tab".to_string())?;
        ordered.push(data.tabs[index].clone());
    }
    data.tabs = ordered;
    data.revision = data.revision.saturating_add(1);
    drop(data);
    queue_session_save(app, state);
    Ok(())
}

#[tauri::command]
pub async fn navigate(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    input: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<String, String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    adblock::ensure_preferences_loaded(storage.inner(), adblock.inner()).await?;
    privacy::ensure_preferences_loaded(storage.inner(), privacy.inner()).await?;
    privacy::ensure_site_permissions_loaded(storage.inner(), privacy.inner()).await?;
    let view = get_tab_view(&app, &tab_id, &state)?;
    let target = match parse_input(&input)? {
        ParsedInput::Url(url) | ParsedInput::Search(url) => url,
    };
    view.navigate(target.clone())
        .map_err(|error| format!("navigation failed: {error}"))?;
    Ok(target.to_string())
}

#[tauri::command]
pub fn go_back(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let view = get_tab_view(&app, &tab_id, &state)?;
    view.eval("history.back()")
        .map_err(|error| format!("could not navigate backward: {error}"))
}

#[tauri::command]
pub fn go_forward(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    let view = get_tab_view(&app, &tab_id, &state)?;
    view.eval("history.forward()")
        .map_err(|error| format!("could not navigate forward: {error}"))
}

#[tauri::command]
pub fn reload(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    get_tab_view(&app, &tab_id, &state)?
        .reload()
        .map_err(|error| format!("reload failed: {error}"))
}

#[tauri::command]
pub fn stop(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    get_tab_view(&app, &tab_id, &state)?
        .eval("window.stop()")
        .map_err(|error| format!("could not stop page loading: {error}"))
}

#[tauri::command]
pub async fn go_home(
    app: AppHandle,
    webview: Webview<Wry>,
    tab_id: String,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
    storage: State<'_, StorageService>,
    adblock: State<'_, AdblockService>,
    privacy: State<'_, PrivacyService>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    adblock::ensure_preferences_loaded(storage.inner(), adblock.inner()).await?;
    privacy::ensure_preferences_loaded(storage.inner(), privacy.inner()).await?;
    privacy::ensure_site_permissions_loaded(storage.inner(), privacy.inner()).await?;
    let view = get_tab_view(&app, &tab_id, &state)?;
    let target = Url::parse("https://www.google.com/")
        .map_err(|error| format!("could not construct home page URL: {error}"))?;
    view.navigate(target)
        .map_err(|error| format!("could not open home page: {error}"))
}

#[tauri::command]
pub fn resize_active_view(
    app: AppHandle,
    webview: Webview<Wry>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    normal_state: State<'_, BrowserState>,
    window_states: State<'_, BrowserWindowStates>,
) -> Result<(), String> {
    let state = state_for_window(
        webview.window().label(),
        normal_state.inner(),
        window_states.inner(),
    )?;
    if ![x, y, width, height].iter().all(|value| value.is_finite())
        || x < 0.0
        || y < 0.0
        || width < 1.0
        || height < 1.0
    {
        return Err("invalid webview bounds".to_string());
    }
    let active_id = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?
        .active_tab_id
        .clone();
    let Some(active_id) = active_id else {
        return Ok(());
    };
    let view = app
        .get_webview(&state.page_label(&active_id))
        .ok_or_else(|| "active tab webview does not exist".to_string())?;
    view.set_position(tauri::LogicalPosition::new(x, y))
        .map_err(|error| format!("could not reposition active tab: {error}"))?;
    view.set_size(tauri::LogicalSize::new(width, height))
        .map_err(|error| format!("could not resize active tab: {error}"))
}

fn get_tab_view(
    app: &AppHandle,
    tab_id: &str,
    state: &BrowserState,
) -> Result<tauri::Webview<Wry>, String> {
    validate_tab_id(tab_id)?;
    let exists = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?
        .tabs
        .iter()
        .any(|tab| tab.id == tab_id);
    if !exists {
        return Err("tab does not exist".to_string());
    }
    app.get_webview(&state.page_label(tab_id))
        .ok_or_else(|| "tab webview does not exist".to_string())
}

fn get_awake_tab_view(
    app: &AppHandle,
    tab_id: &str,
    state: &BrowserState,
) -> Result<tauri::Webview<Wry>, String> {
    validate_tab_id(tab_id)?;
    let sleeping = state
        .0
        .lock()
        .map_err(|_| "browser tab state lock is poisoned".to_string())?
        .tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .ok_or_else(|| "tab does not exist".to_string())?
        .sleeping;
    if sleeping {
        return Err("wake the sleeping tab before printing it".to_string());
    }
    get_tab_view(app, tab_id, state)
}

fn validate_pdf_output_path(file_path: &str) -> Result<PathBuf, String> {
    if file_path.is_empty() || file_path.len() > 32_767 {
        return Err("the selected PDF path is invalid".to_string());
    }
    let mut path = PathBuf::from(file_path);
    if !path.is_absolute() {
        return Err("the selected PDF path must be absolute".to_string());
    }
    match path.extension().and_then(|extension| extension.to_str()) {
        None => {
            path.set_extension("pdf");
        }
        Some(extension) if extension.eq_ignore_ascii_case("pdf") => {}
        Some(_) => return Err("the selected file must use the .pdf extension".to_string()),
    }
    if path.file_name().is_none() {
        return Err("the selected PDF path must include a file name".to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "the selected PDF path must include a directory".to_string())?;
    if !parent.is_dir() {
        return Err("the selected PDF directory does not exist".to_string());
    }
    if path.is_dir() {
        return Err("the selected PDF path is a directory".to_string());
    }
    Ok(path)
}

fn validate_tab_id(tab_id: &str) -> Result<(), String> {
    if tab_id.is_empty() || !tab_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid tab identifier".to_string());
    }
    Ok(())
}

fn validate_group_id(group_id: &str) -> Result<(), String> {
    if group_id.is_empty()
        || group_id.len() > 64
        || !group_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("invalid tab group identifier".to_string());
    }
    Ok(())
}

fn validate_group_details(name: &str, color: &str) -> Result<(String, String), String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err("Tab group names must contain between 1 and 64 characters.".to_string());
    }
    let color = color.to_ascii_lowercase();
    if !TAB_GROUP_COLORS.contains(&color.as_str()) {
        return Err("Choose a supported tab group color.".to_string());
    }
    Ok((name.to_string(), color))
}

fn assign_tab_to_group(
    data: &mut BrowserData,
    tab_id: &str,
    group_id: Option<&str>,
) -> Result<(), String> {
    validate_tab_id(tab_id)?;
    if let Some(group_id) = group_id {
        validate_group_id(group_id)?;
        if !data.groups.iter().any(|group| group.id == group_id) {
            return Err("tab group no longer exists".to_string());
        }
    }
    let tab = data
        .tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .ok_or_else(|| "tab does not exist".to_string())?;
    tab.group_id = group_id.map(str::to_string);
    data.revision = data.revision.saturating_add(1);
    Ok(())
}

fn activate_tab_internal(
    app: &AppHandle,
    tab_id: &str,
    state: &BrowserState,
    storage: &StorageService,
    download_location: &DownloadLocation,
    adblock_service: &AdblockService,
    privacy_service: &PrivacyService,
) -> Result<(), String> {
    validate_tab_id(tab_id)?;
    let (should_wake, saved_tab) = {
        let data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let tab = data
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| "tab does not exist".to_string())?;
        (tab.sleeping, tab.clone())
    };
    if should_wake {
        restore_sleeping_tab_webview(
            app,
            state,
            &saved_tab,
            storage.clone(),
            download_location.clone(),
            adblock_service.clone(),
            privacy_service.clone(),
        )?;
    }

    let (previous_id, visible) = {
        let mut data = state
            .0
            .lock()
            .map_err(|_| "browser tab state lock is poisoned".to_string())?;
        let now = Instant::now();
        if let Some(previous_id) = data
            .active_tab_id
            .as_ref()
            .filter(|previous_id| previous_id.as_str() != tab_id)
            .cloned()
        {
            data.inactive_since.entry(previous_id).or_insert(now);
        }
        data.inactive_since.remove(tab_id);
        data.active_downloads.remove(tab_id);
        let tab = data
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .ok_or_else(|| "tab does not exist".to_string())?;
        tab.sleeping = false;
        let previous = data.active_tab_id.replace(tab_id.to_string());
        data.revision = data.revision.saturating_add(1);
        (previous, data.page_visible)
    };
    if let Some(previous_id) = previous_id.filter(|previous| previous != tab_id) {
        if let Some(previous) = app.get_webview(&state.page_label(&previous_id)) {
            previous
                .hide()
                .map_err(|error| format!("could not hide previous tab: {error}"))?;
        }
    }
    let active = app
        .get_webview(&state.page_label(tab_id))
        .ok_or_else(|| "tab webview does not exist".to_string())?;
    if visible {
        active
            .show()
            .map_err(|error| format!("could not show tab: {error}"))?;
    } else {
        active
            .hide()
            .map_err(|error| format!("could not hide tab: {error}"))?;
    }
    if should_wake {
        let event = TabSleepEvent {
            tab_id: tab_id.to_string(),
            sleeping: false,
        };
        if let Err(error) = app.emit_to(state.window_label(), "browser:tab-sleep", event) {
            eprintln!("could not notify browser chrome about a resumed tab: {error}");
        }
    }
    queue_session_save(app.clone(), state.clone());
    Ok(())
}

fn restore_sleeping_tab_webview(
    app: &AppHandle,
    state: &BrowserState,
    tab: &TabInfo,
    storage: StorageService,
    download_location: DownloadLocation,
    adblock_service: AdblockService,
    privacy_service: PrivacyService,
) -> Result<(), String> {
    let url = safe_session_url(&tab.url)?;
    let window = app
        .get_window(state.window_label())
        .ok_or_else(|| "browser window is missing".to_string())?;
    let size = window
        .inner_size()
        .map_err(|error| format!("could not read browser window size: {error}"))?;
    let scale = window
        .scale_factor()
        .map_err(|error| format!("could not read browser window scale: {error}"))?;
    let blank_url =
        Url::parse("about:blank").map_err(|error| format!("invalid blank URL: {error}"))?;
    let builder = page_builder(
        app.clone(),
        state.clone(),
        storage,
        download_location,
        tab.id.clone(),
        WebviewUrl::External(blank_url),
    );
    let view = window
        .add_child(
            builder,
            tauri::LogicalPosition::new(0.0, 96.0),
            tauri::LogicalSize::new(
                f64::from(size.width) / scale,
                (f64::from(size.height) / scale - 96.0).max(1.0),
            ),
        )
        .map_err(|error| format!("could not restore sleeping tab: {error}"))?;
    if let Err(error) = view.hide() {
        if let Err(close_error) = view.close() {
            eprintln!("could not close partially restored tab: {close_error}");
        }
        return Err(format!("could not hide restored tab: {error}"));
    }
    if let Err(error) = adblock::attach_to_webview(&view, adblock_service, privacy_service) {
        if let Err(close_error) = view.close() {
            eprintln!(
                "could not close restored tab after interception setup failed: {close_error}"
            );
        }
        return Err(error);
    }
    if url.as_str() != "about:blank" {
        if let Err(error) = view.navigate(url) {
            if let Err(close_error) = view.close() {
                eprintln!("could not close restored tab after navigation failed: {close_error}");
            }
            return Err(format!("could not reload sleeping tab: {error}"));
        }
    }
    Ok(())
}

fn queue_session_save(app: AppHandle, state: BrowserState) {
    if !state.persists_session() {
        return;
    }
    let snapshot = match state.0.lock() {
        Ok(data) => SessionSnapshot {
            revision: data.revision,
            tabs: data
                .tabs
                .iter()
                .enumerate()
                .map(|(position, tab)| SessionTab {
                    id: tab.id.clone(),
                    title: tab.title.clone(),
                    url: tab.url.clone(),
                    position,
                    active: data.active_tab_id.as_deref() == Some(&tab.id),
                    group_id: tab.group_id.clone(),
                })
                .collect(),
            groups: data
                .groups
                .iter()
                .map(|group| SessionGroup {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    color: group.color.clone(),
                })
                .collect(),
        },
        Err(_) => {
            eprintln!("browser tab state lock poisoned while saving session");
            return;
        }
    };
    let storage = app.state::<StorageService>().inner().clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = storage
            .with_connection(move |connection| crate::storage::save_session(connection, snapshot))
            .await
        {
            eprintln!("could not persist browser session: {error}");
        }
    });
}

fn safe_session_url(value: &str) -> Result<Url, String> {
    if value == "about:blank" {
        return Url::parse(value).map_err(|error| format!("invalid saved blank URL: {error}"));
    }
    let url = Url::parse(value).map_err(|_| "saved tab URL is invalid".to_string())?;
    if !is_http_url(url.as_str()) {
        return Err("saved tab URL is not an HTTP(S) URL".to_string());
    }
    Ok(url)
}

fn is_http_url(value: &str) -> bool {
    Url::parse(value)
        .map(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
        .unwrap_or(false)
}

fn download_destination(
    location: &DownloadLocation,
    suggested_path: &Path,
) -> Result<PathBuf, String> {
    let directory = location.current()?;
    if !directory.is_absolute() {
        return Err("download directory must be an absolute path".to_string());
    }
    let name = suggested_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    let safe_name = name
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_string();
    let safe_name = if safe_name.is_empty() {
        "download".to_string()
    } else {
        safe_name
    };
    let candidate = directory.join(&safe_name);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let path = Path::new(&safe_name);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("download");
    let extension = path.extension().and_then(|extension| extension.to_str());
    for suffix in 1..=10_000 {
        let name = match extension {
            Some(extension) => format!("{stem} ({suffix}).{extension}"),
            None => format!("{stem} ({suffix})"),
        };
        let candidate = directory.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("could not choose a unique download filename".to_string())
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

fn emit_download_changed(app: &AppHandle) {
    if let Err(error) = app.emit("browser:downloads-updated", ()) {
        eprintln!("could not notify browser windows of download change: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assign_tab_to_group, idle_tabs_past_timeout, is_page_webview_label,
        parse_tab_sleep_timeout, rollback_tab_metadata, validate_find_query,
        validate_group_details, validate_pdf_output_path, validate_tab_sleep_timeout, BrowserData,
        BrowserState, TabGroup, TabInfo, TabSleepPolicy,
    };
    use std::{
        collections::{HashMap, HashSet},
        time::{Duration, Instant},
    };

    fn tab(id: &str) -> TabInfo {
        TabInfo {
            id: id.to_string(),
            title: format!("Tab {id}"),
            url: "about:blank".to_string(),
            group_id: None,
            sleeping: false,
        }
    }

    #[test]
    fn failed_tab_creation_restores_previous_active_tab() {
        let mut data = BrowserData {
            tabs: vec![tab("1"), tab("2")],
            inactive_since: HashMap::new(),
            active_downloads: HashSet::new(),
            groups: Vec::new(),
            active_tab_id: Some("2".to_string()),
            next_id: 3,
            next_group_id: 1,
            revision: 2,
            page_visible: true,
            restore_attempted: false,
            navigation_generations: std::collections::HashMap::new(),
        };

        assert!(rollback_tab_metadata(&mut data, "2", Some("1")));

        assert_eq!(
            data.tabs
                .iter()
                .map(|tab| tab.id.as_str())
                .collect::<Vec<_>>(),
            ["1"]
        );
        assert_eq!(data.active_tab_id.as_deref(), Some("1"));
    }

    #[test]
    fn find_query_accepts_empty_and_bounded_text() {
        assert!(validate_find_query("").is_ok());
        assert!(validate_find_query(&"a".repeat(512)).is_ok());
        assert!(validate_find_query(&"a".repeat(513)).is_err());
        assert!(validate_find_query(&"é".repeat(257)).is_err());
    }

    #[test]
    fn pdf_output_path_requires_a_pdf_file_in_an_existing_directory() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let path_without_extension = directory.path().join("web page");
        let path = validate_pdf_output_path(&path_without_extension.to_string_lossy())
            .expect("append PDF extension");
        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("pdf")
        );

        let wrong_extension = directory.path().join("web page.txt");
        assert!(validate_pdf_output_path(&wrong_extension.to_string_lossy()).is_err());
        assert!(validate_pdf_output_path("relative.pdf").is_err());
    }

    #[test]
    fn failed_tab_creation_does_not_override_a_newer_active_tab() {
        let mut data = BrowserData {
            tabs: vec![tab("1"), tab("2"), tab("3")],
            inactive_since: HashMap::new(),
            active_downloads: HashSet::new(),
            groups: Vec::new(),
            active_tab_id: Some("3".to_string()),
            next_id: 4,
            next_group_id: 1,
            revision: 3,
            page_visible: true,
            restore_attempted: false,
            navigation_generations: std::collections::HashMap::new(),
        };

        assert!(!rollback_tab_metadata(&mut data, "2", Some("1")));

        assert_eq!(
            data.tabs
                .iter()
                .map(|tab| tab.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "3"]
        );
        assert_eq!(data.active_tab_id.as_deref(), Some("3"));
    }

    #[test]
    fn tab_groups_assign_and_remove_tabs_with_validation() {
        let mut data = BrowserData {
            tabs: vec![tab("1"), tab("2")],
            inactive_since: HashMap::new(),
            active_downloads: HashSet::new(),
            groups: vec![TabGroup {
                id: "1".into(),
                name: "Research".into(),
                color: "#4f7ff0".into(),
            }],
            active_tab_id: Some("1".into()),
            next_id: 3,
            next_group_id: 2,
            revision: 1,
            page_visible: true,
            restore_attempted: false,
            navigation_generations: std::collections::HashMap::new(),
        };

        assign_tab_to_group(&mut data, "2", Some("1")).expect("assign tab");
        assert_eq!(data.tabs[1].group_id.as_deref(), Some("1"));
        assign_tab_to_group(&mut data, "2", None).expect("remove tab from group");
        assert_eq!(data.tabs[1].group_id, None);
        assert!(assign_tab_to_group(&mut data, "2", Some("9")).is_err());
        assert!(assign_tab_to_group(&mut data, "9", None).is_err());
    }

    #[test]
    fn tab_group_name_and_color_are_validated() {
        assert_eq!(
            validate_group_details("  Research  ", "#4F7FF0").expect("valid group details"),
            ("Research".to_string(), "#4f7ff0".to_string())
        );
        assert!(validate_group_details("   ", "#4f7ff0").is_err());
        assert!(validate_group_details("Research", "red; background:url(x)").is_err());
    }

    #[test]
    fn tab_sleep_timeout_accepts_only_supported_durations() {
        assert_eq!(parse_tab_sleep_timeout(None).expect("default timeout"), 900);
        assert_eq!(
            parse_tab_sleep_timeout(Some("60".to_string())).expect("one minute timeout"),
            60
        );
        assert!(parse_tab_sleep_timeout(Some("61".to_string())).is_err());
        assert!(parse_tab_sleep_timeout(Some("invalid".to_string())).is_err());
        assert!(validate_tab_sleep_timeout(0).is_ok());
        assert!(validate_tab_sleep_timeout(3600).is_ok());
    }

    #[test]
    fn late_startup_preference_does_not_override_a_user_change() {
        let policy = TabSleepPolicy::default();
        policy.set_timeout_seconds(300);
        policy.initialize_timeout_seconds(60);

        assert_eq!(policy.timeout_seconds(), 300);
    }

    #[test]
    fn idle_sleep_selection_excludes_active_and_recent_tabs() {
        let now = Instant::now();
        let mut data = BrowserData {
            tabs: vec![tab("1"), tab("2"), tab("3"), tab("4"), tab("5")],
            inactive_since: HashMap::from([
                ("1".to_string(), now - Duration::from_secs(120)),
                ("2".to_string(), now - Duration::from_secs(120)),
                ("3".to_string(), now - Duration::from_secs(30)),
                ("4".to_string(), now - Duration::from_secs(120)),
                ("5".to_string(), now - Duration::from_secs(120)),
            ]),
            active_downloads: HashSet::from(["5".to_string()]),
            groups: Vec::new(),
            active_tab_id: Some("1".to_string()),
            next_id: 6,
            next_group_id: 1,
            revision: 1,
            page_visible: true,
            restore_attempted: false,
            navigation_generations: HashMap::new(),
        };
        data.tabs[3].sleeping = true;

        assert_eq!(
            idle_tabs_past_timeout(&data, now, Duration::from_secs(60)),
            vec!["2".to_string()]
        );
    }

    #[test]
    fn private_window_tabs_are_isolated_from_normal_window_state() {
        let normal = BrowserState::default();
        let private = BrowserState::for_window("private-1".to_string(), true);
        normal
            .initialize(tab("1"), Vec::new())
            .expect("initialize normal tabs");
        private
            .initialize(tab("1"), Vec::new())
            .expect("initialize private tabs");
        private.0.lock().expect("lock private state").tabs[0].title = "Private page".to_string();

        assert_ne!(normal.page_label("1"), private.page_label("1"));
        assert!(!normal.is_private());
        assert!(private.is_private());
        assert_eq!(
            normal.0.lock().expect("lock normal state").tabs[0].title,
            "Tab 1"
        );
        assert_eq!(
            private.0.lock().expect("lock private state").tabs[0].title,
            "Private page"
        );
    }

    #[test]
    fn secondary_normal_windows_have_isolated_nonpersistent_state() {
        let main = BrowserState::default();
        let secondary = BrowserState::for_window("window-2".to_string(), false);
        main.initialize(tab("1"), Vec::new())
            .expect("initialize main window");
        secondary
            .initialize(tab("1"), Vec::new())
            .expect("initialize secondary window");
        let windows = super::BrowserWindowStates::default();
        windows
            .insert("window-2".to_string(), secondary.clone())
            .expect("register secondary window");

        assert!(!secondary.is_private());
        assert!(!secondary.persists_session());
        assert!(main.persists_session());
        assert_eq!(
            super::state_for_window("window-2", &main, &windows)
                .expect("resolve secondary window")
                .window_label(),
            "window-2"
        );
        assert!(super::state_for_window("window-3", &main, &windows).is_err());
    }

    #[test]
    fn devtools_shortcut_is_scoped_to_page_webviews() {
        assert!(is_page_webview_label("page-3"));
        assert!(is_page_webview_label("page-private-2-3"));
        assert!(is_page_webview_label("page-window-4-2"));
        assert!(!is_page_webview_label("main"));
        assert!(!is_page_webview_label("page-private-other-3"));
    }
}

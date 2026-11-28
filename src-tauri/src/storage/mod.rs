mod bookmarks;
mod commands;
mod downloads;
mod history;
mod models;
mod sessions;

pub use bookmarks::*;
pub use commands::*;
pub use downloads::*;
pub use history::*;
pub use models::*;
pub use sessions::*;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use rusqlite::Connection;

static MIGRATION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct StorageService {
    path: Arc<PathBuf>,
    connection: Arc<Mutex<Option<Connection>>>,
}

impl StorageService {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Arc::new(path),
            connection: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn with_connection<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    {
        let path = self.path.clone();
        let connection = self.connection.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut connection = connection
                .lock()
                .map_err(|_| "database connection lock is poisoned".to_string())?;
            if connection.is_none() {
                *connection = Some(open_connection(&path)?);
            }
            let connection = connection
                .as_ref()
                .ok_or_else(|| "database connection was not initialized".to_string())?;
            operation(connection)
        })
        .await
        .map_err(|error| format!("storage worker failed: {error}"))?
    }
}

fn open_connection(path: &std::path::Path) -> Result<Connection, String> {
    if path.as_os_str().is_empty() {
        return Err("application data directory is not configured".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create browser data directory: {error}"))?;
    }
    let connection = Connection::open(path)
        .map_err(|error| format!("could not open browser database: {error}"))?;
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(|error| format!("could not configure browser database: {error}"))?;
    connection
        .execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")
        .map_err(|error| format!("could not configure browser database: {error}"))?;
    let _migration_guard = MIGRATION_LOCK
        .lock()
        .map_err(|_| "database migration lock is poisoned".to_string())?;
    migrate(&connection)?;
    Ok(connection)
}

pub fn collect_omnibox_suggestions(
    connection: &Connection,
    query: &str,
) -> Result<Vec<OmniboxSuggestion>, String> {
    let query = query.trim();
    let query_length = query.chars().count();
    if query_length > 256 {
        return Err("Suggestion query is too long.".to_string());
    }
    if query_length < 2 {
        return Ok(Vec::new());
    }

    let saved = suggest_bookmarks(connection, query, 8)?;
    let visited = suggest_history(connection, query, 8)?;
    let mut seen = HashSet::new();
    let mut suggestions = Vec::with_capacity(8);
    for entry in saved.into_iter().chain(visited) {
        if seen.insert(entry.url.clone()) {
            suggestions.push(entry);
            if suggestions.len() == 8 {
                break;
            }
        }
    }
    Ok(suggestions)
}

fn migrate(connection: &Connection) -> Result<(), String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("could not read database schema version: {error}"))?;
    if version > 7 {
        return Err(format!(
            "database schema version {version} is newer than this browser"
        ));
    }
    if version == 0 {
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|error| format!("could not enable SQLite write-ahead logging: {error}"))?;
        connection
            .execute_batch(
                "BEGIN;
                CREATE TABLE bookmark_folders (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 120),
                    parent_id INTEGER REFERENCES bookmark_folders(id) ON DELETE CASCADE
                );
                CREATE UNIQUE INDEX bookmark_folders_sibling_name
                    ON bookmark_folders(ifnull(parent_id, 0), name COLLATE NOCASE);
                CREATE TABLE bookmarks (
                    id INTEGER PRIMARY KEY,
                    title TEXT NOT NULL CHECK(length(title) <= 512),
                    url TEXT NOT NULL UNIQUE CHECK(length(url) <= 4096),
                    folder_id INTEGER REFERENCES bookmark_folders(id) ON DELETE SET NULL,
                    on_bar INTEGER NOT NULL DEFAULT 0 CHECK(on_bar IN (0, 1)),
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX bookmarks_folder ON bookmarks(folder_id);
                CREATE TABLE history (
                    id INTEGER PRIMARY KEY,
                    url TEXT NOT NULL CHECK(length(url) <= 4096),
                    title TEXT NOT NULL DEFAULT '' CHECK(length(title) <= 512),
                    visited_at INTEGER NOT NULL
                );
                CREATE INDEX history_visited_at ON history(visited_at DESC);
                CREATE INDEX history_url ON history(url);
                CREATE TABLE downloads (
                    id INTEGER PRIMARY KEY,
                    url TEXT NOT NULL CHECK(length(url) <= 4096),
                    path TEXT NOT NULL CHECK(length(path) <= 4096),
                    status TEXT NOT NULL CHECK(status IN ('downloading', 'complete', 'failed', 'interrupted')),
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER
                );
                CREATE INDEX downloads_started_at ON downloads(started_at DESC);
                CREATE TABLE settings (
                    key TEXT PRIMARY KEY CHECK(length(key) <= 120),
                    value TEXT NOT NULL CHECK(length(value) <= 4096)
                );
                CREATE TABLE session_tabs (
                    id TEXT PRIMARY KEY CHECK(length(id) <= 64),
                    title TEXT NOT NULL CHECK(length(title) <= 512),
                    url TEXT NOT NULL CHECK(length(url) <= 4096),
                    position INTEGER NOT NULL,
                    is_active INTEGER NOT NULL CHECK(is_active IN (0, 1))
                );
                CREATE TABLE session_state (
                    id INTEGER PRIMARY KEY CHECK(id = 1),
                    revision INTEGER NOT NULL
                );
                INSERT INTO session_state(id, revision) VALUES (1, 0);
                PRAGMA user_version = 1;
                COMMIT;",
            )
            .map_err(|error| format!("could not initialize browser database: {error}"))?;
    }
    if version < 2 {
        connection
            .execute_batch(
                "BEGIN;
                CREATE TABLE site_permissions (
                    host TEXT PRIMARY KEY CHECK(length(host) BETWEEN 1 AND 253),
                    adblock_allowed INTEGER NOT NULL CHECK(adblock_allowed IN (0, 1))
                );
                PRAGMA user_version = 2;
                COMMIT;",
            )
            .map_err(|error| format!("could not update browser database schema: {error}"))?;
    }
    if version < 3 {
        connection
            .execute_batch(
                "BEGIN;
                CREATE TABLE password_credentials (
                    id INTEGER PRIMARY KEY,
                    origin TEXT NOT NULL CHECK(length(origin) BETWEEN 1 AND 2048),
                    encrypted_secret BLOB NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX password_credentials_origin ON password_credentials(origin);
                PRAGMA user_version = 3;
                COMMIT;",
            )
            .map_err(|error| format!("could not update browser database schema: {error}"))?;
    }
    if version < 4 {
        connection
            .execute_batch(
                "BEGIN;
                CREATE TABLE site_permission_grants (
                    origin TEXT NOT NULL CHECK(length(origin) BETWEEN 1 AND 2048),
                    permission TEXT NOT NULL CHECK(permission IN (
                        'camera', 'microphone', 'location', 'notifications', 'clipboard'
                    )),
                    PRIMARY KEY(origin, permission)
                );
                PRAGMA user_version = 4;
                COMMIT;",
            )
            .map_err(|error| format!("could not update browser database schema: {error}"))?;
    }
    if version < 5 {
        connection
            .execute_batch(
                "BEGIN;
                CREATE TABLE user_scripts (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 120),
                    match_pattern TEXT NOT NULL CHECK(length(match_pattern) BETWEEN 1 AND 1024),
                    source TEXT NOT NULL CHECK(length(source) BETWEEN 1 AND 65536),
                    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0, 1)),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                PRAGMA user_version = 5;
                COMMIT;",
            )
            .map_err(|error| format!("could not update browser database schema: {error}"))?;
    }
    if version < 6 {
        connection
            .execute_batch(
                "BEGIN;
                CREATE TABLE tab_groups (
                    id TEXT PRIMARY KEY CHECK(length(id) BETWEEN 1 AND 64),
                    name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 64),
                    color TEXT NOT NULL CHECK(color IN (
                        '#4f7ff0', '#8d63d8', '#159b9a', '#2d9a65',
                        '#d49a2b', '#e17a32', '#d75059', '#788397'
                    ))
                );
                ALTER TABLE session_tabs
                    ADD COLUMN group_id TEXT REFERENCES tab_groups(id) ON DELETE SET NULL;
                PRAGMA user_version = 6;
                COMMIT;",
            )
            .map_err(|error| format!("could not update browser database schema: {error}"))?;
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS oneko_hidden_sites (
                    host TEXT PRIMARY KEY CHECK(length(host) BETWEEN 1 AND 253)
                );",
        )
        .map_err(|error| format!("could not create hidden-site settings: {error}"))?;
    if version > 6 {
        // Version 7 only added this table; reset the marker so version-6 builds can reopen the profile.
        connection
            .pragma_update(None, "user_version", 6)
            .map_err(|error| format!("could not restore compatible database version: {error}"))?;
    }
    ensure_history_search(connection)?;
    Ok(())
}

fn ensure_history_search(connection: &Connection) -> Result<(), String> {
    let history_exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'history'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("could not inspect history table: {error}"))?;
    if !history_exists {
        return Ok(());
    }

    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS browser_features (
                name TEXT PRIMARY KEY
            );",
        )
        .map_err(|error| format!("could not create database feature registry: {error}"))?;
    let installed: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM browser_features WHERE name = 'history_trigram_search')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("could not inspect database search features: {error}"))?;
    if installed {
        return Ok(());
    }

    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
            CREATE VIRTUAL TABLE history_search USING fts5(
                title, url,
                content='history',
                content_rowid='id',
                tokenize='trigram'
            );
            CREATE TRIGGER history_search_insert AFTER INSERT ON history BEGIN
                INSERT INTO history_search(rowid, title, url)
                VALUES (new.id, new.title, new.url);
            END;
            CREATE TRIGGER history_search_delete AFTER DELETE ON history BEGIN
                INSERT INTO history_search(history_search, rowid, title, url)
                VALUES ('delete', old.id, old.title, old.url);
            END;
            CREATE TRIGGER history_search_update AFTER UPDATE ON history BEGIN
                INSERT INTO history_search(history_search, rowid, title, url)
                VALUES ('delete', old.id, old.title, old.url);
                INSERT INTO history_search(rowid, title, url)
                VALUES (new.id, new.title, new.url);
            END;
            INSERT INTO history_search(history_search) VALUES ('rebuild');
            INSERT INTO browser_features(name) VALUES ('history_trigram_search');
            COMMIT;",
        )
        .map_err(|error| format!("could not initialize history search index: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{
        add_bookmark, create_bookmark_folder, delete_history_for_site, finish_download,
        insert_download, list_bookmarks, list_downloads, list_history, record_history,
        save_session, SessionSnapshot, SessionTab,
    };

    fn test_connection() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("browser.sqlite3");
        let connection = open_connection(&path).expect("initialize test database");
        (directory, connection)
    }

    #[test]
    fn bookmarks_and_folders_round_trip() {
        let (_directory, connection) = test_connection();
        let folder_id =
            create_bookmark_folder(&connection, "Research", None).expect("create folder");
        let bookmark_id = add_bookmark(
            &connection,
            "Rust",
            "https://www.rust-lang.org/",
            Some(folder_id),
            true,
        )
        .expect("insert bookmark");

        let bookmarks = list_bookmarks(&connection, Some("rust".to_string()), true, None)
            .expect("list bookmarks");
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].id, bookmark_id);
        assert_eq!(bookmarks[0].folder_name.as_deref(), Some("Research"));
        assert!(bookmarks[0].on_bar);
    }

    #[test]
    fn history_suggestions_use_trigram_index_and_track_history_changes() {
        let (_directory, connection) = test_connection();
        record_history(
            &connection,
            "https://example.com/articles/performance",
            "Browser performance guide",
            1_700_000_000,
        )
        .expect("record history entry");

        let suggestions =
            suggest_history(&connection, "formance gu", 8).expect("search within title");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].title, "Browser performance guide");
        assert_eq!(
            suggest_history(&connection, "for", 8)
                .expect("search short query")
                .len(),
            1
        );

        update_latest_title(
            &connection,
            "https://example.com/articles/performance",
            "Updated performance guide",
        )
        .expect("update history title");
        assert_eq!(
            suggest_history(&connection, "Updated performance", 8).expect("search updated title")
                [0]
            .title,
            "Updated performance guide"
        );

        clear_history(&connection).expect("clear history");
        assert!(suggest_history(&connection, "performance", 8)
            .expect("search cleared history")
            .is_empty());
    }

    #[test]
    fn history_can_be_searched_and_deleted_by_site() {
        let (_directory, connection) = test_connection();
        record_history(&connection, "https://example.com/one", "One", 1_700_000_000)
            .expect("record first visit");
        record_history(
            &connection,
            "https://other.example/two",
            "Two",
            1_700_000_001,
        )
        .expect("record second visit");

        assert_eq!(
            list_history(&connection, Some("One".to_string()), 100)
                .expect("search history")
                .len(),
            1
        );
        assert_eq!(
            delete_history_for_site(&connection, "example.com").expect("delete site history"),
            1
        );
        assert_eq!(
            list_history(&connection, None, 100)
                .expect("remaining history")
                .len(),
            1
        );
    }

    #[test]
    fn download_state_and_session_are_persisted() {
        let (_directory, connection) = test_connection();
        let download_id = insert_download(
            &connection,
            "https://example.com/file.zip",
            "C:/Downloads/file.zip",
            1_700_000_000,
        )
        .expect("record download");
        finish_download(&connection, download_id, true, 1_700_000_010).expect("complete download");
        insert_download(
            &connection,
            "https://example.com/active.zip",
            "C:/Downloads/active.zip",
            1_700_000_020,
        )
        .expect("record active download");
        assert_eq!(
            list_downloads(&connection, 10).expect("list downloads")[0].status,
            "downloading"
        );
        assert_eq!(
            clear_downloads(&connection).expect("clear completed downloads"),
            1
        );
        assert_eq!(
            list_downloads(&connection, 10)
                .expect("active download remains")
                .len(),
            1
        );
        set_setting(&connection, "download.ask_where_to_save", "true")
            .expect("save download prompt preference");
        assert_eq!(
            get_setting(&connection, "download.ask_where_to_save")
                .expect("load download prompt preference")
                .as_deref(),
            Some("true")
        );

        save_session(
            &connection,
            SessionSnapshot {
                revision: 1,
                tabs: vec![SessionTab {
                    id: "1".into(),
                    title: "Example".into(),
                    url: "https://example.com/".into(),
                    position: 0,
                    active: true,
                    group_id: Some("7".into()),
                }],
                groups: vec![SessionGroup {
                    id: "7".into(),
                    name: "Research".into(),
                    color: "#4f7ff0".into(),
                }],
            },
        )
        .expect("save session");
        let restored = load_session(&connection).expect("load session");
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].url, "https://example.com/");
        assert_eq!(restored.tabs[0].group_id.as_deref(), Some("7"));
        assert_eq!(restored.groups[0].name, "Research");
    }

    #[test]
    fn adblock_preferences_and_site_allowlist_are_stored() {
        let (_directory, connection) = test_connection();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read database version");
        assert_eq!(version, 6);

        connection
            .execute(
                "INSERT INTO settings(key, value) VALUES ('adblock.enabled', 'false')",
                [],
            )
            .expect("store global ad blocker preference");
        connection
            .execute(
                "INSERT INTO site_permissions(host, adblock_allowed) VALUES (?1, 1)",
                ["news.example.test"],
            )
            .expect("store site allowlist");
        let enabled: String = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'adblock.enabled'",
                [],
                |row| row.get(0),
            )
            .expect("read global ad blocker preference");
        let allowed: i64 = connection
            .query_row(
                "SELECT adblock_allowed FROM site_permissions WHERE host = ?1",
                ["news.example.test"],
                |row| row.get(0),
            )
            .expect("read site allowlist");
        assert_eq!(enabled, "false");
        assert_eq!(allowed, 1);
    }

    #[test]
    fn existing_version_one_database_gets_later_schema_tables() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("version-one.sqlite3");
        let connection = Connection::open(path).expect("create version-one database");
        connection
            .execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE session_tabs (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    url TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    is_active INTEGER NOT NULL
                 );
                 PRAGMA user_version = 1;",
            )
            .expect("create version-one schema");

        migrate(&connection).expect("upgrade existing schema");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read database version");
        let table_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'site_permissions'",
                [],
                |row| row.get(0),
            )
            .expect("check site allowlist table");
        assert_eq!(version, 6);
        assert_eq!(table_exists, 1);
        let passwords_table_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'password_credentials'",
                [],
                |row| row.get(0),
            )
            .expect("check protected password table");
        assert_eq!(passwords_table_exists, 1);
        let site_permission_grants_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'site_permission_grants'",
                [],
                |row| row.get(0),
            )
            .expect("check per-site permissions table");
        assert_eq!(site_permission_grants_exists, 1);
        let user_scripts_table_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'user_scripts'",
                [],
                |row| row.get(0),
            )
            .expect("check user scripts table");
        assert_eq!(user_scripts_table_exists, 1);
        let tab_groups_table_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'tab_groups'",
                [],
                |row| row.get(0),
            )
            .expect("check tab groups table");
        assert_eq!(tab_groups_table_exists, 1);
        let oneko_hidden_sites_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'oneko_hidden_sites'",
                [],
                |row| row.get(0),
            )
            .expect("check hidden Oneko sites table");
        assert_eq!(oneko_hidden_sites_exists, 1);
    }

    #[test]
    fn existing_version_seven_database_is_downgraded_for_older_browser_compatibility() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("version-seven.sqlite3");
        let connection = Connection::open(&path).expect("create version-seven database");
        connection
            .execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE oneko_hidden_sites (
                    host TEXT PRIMARY KEY CHECK(length(host) BETWEEN 1 AND 253)
                 );
                 INSERT INTO oneko_hidden_sites(host) VALUES ('example.com');
                 PRAGMA user_version = 7;",
            )
            .expect("create version-seven schema");
        drop(connection);

        let connection = open_connection(&path).expect("open version-seven database");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read database version");
        assert_eq!(version, 6);
        let hidden_sites_exists: i64 = connection
            .query_row(
                "SELECT count(*) FROM oneko_hidden_sites WHERE host = 'example.com'",
                [],
                |row| row.get(0),
            )
            .expect("check stored hidden-site setting");
        assert_eq!(hidden_sites_exists, 1);
    }

    #[test]
    fn newer_session_snapshot_wins_over_a_late_stale_write() {
        let (_directory, connection) = test_connection();
        let snapshot = |revision, url: &str| SessionSnapshot {
            revision,
            tabs: vec![SessionTab {
                id: "1".into(),
                title: "Saved".into(),
                url: url.into(),
                position: 0,
                active: true,
                group_id: None,
            }],
            groups: Vec::new(),
        };
        save_session(&connection, snapshot(2, "https://new.example/")).expect("new snapshot");
        save_session(&connection, snapshot(1, "https://old.example/")).expect("stale snapshot");
        assert_eq!(
            load_session(&connection).expect("load session").tabs[0].url,
            "https://new.example/"
        );
    }

    #[test]
    fn concurrent_session_saves_do_not_fail_with_database_locked() {
        use std::sync::{Arc, Barrier};

        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("concurrent-sessions.sqlite3");
        let workers = 12;
        let barrier = Arc::new(Barrier::new(workers));
        let saves = (1..=workers)
            .map(|revision| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let connection = open_connection(&path).expect("open shared database");
                    barrier.wait();
                    save_session(
                        &connection,
                        SessionSnapshot {
                            revision: revision as u64,
                            tabs: vec![SessionTab {
                                id: "1".into(),
                                title: "Concurrent save".into(),
                                url: format!("https://revision-{revision}.example/"),
                                position: 0,
                                active: true,
                                group_id: None,
                            }],
                            groups: Vec::new(),
                        },
                    )
                })
            })
            .collect::<Vec<_>>();

        for save in saves {
            save.join()
                .expect("session save worker")
                .expect("concurrent session save");
        }

        let connection = open_connection(&path).expect("reopen shared database");
        let restored = load_session(&connection).expect("load latest session");
        assert_eq!(
            restored.tabs[0].url,
            format!("https://revision-{workers}.example/")
        );
    }

    #[test]
    fn concurrent_first_open_runs_schema_migration_once() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("concurrent.sqlite3");
        let workers = (0..6)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let connection = open_connection(&path).expect("open shared database");
                    connection
                        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                        .expect("read schema version")
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert_eq!(worker.join().expect("migration worker"), 6);
        }
    }
}

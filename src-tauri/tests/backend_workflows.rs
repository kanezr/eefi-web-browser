use eefi_browser_lib::{
    nav::{parse_input, ParsedInput},
    storage::{
        add_bookmark, collect_omnibox_suggestions, create_bookmark_folder,
        finish_download_for_request, get_setting, insert_download, list_bookmarks, list_downloads,
        list_history, load_session, record_history, recover_downloads, save_session, set_setting,
        update_latest_title, SessionGroup, SessionSnapshot, SessionTab, StorageService,
    },
};
use rusqlite::Connection;
use tempfile::tempdir;
use url::Url;

fn with_profile<T, F>(storage: &StorageService, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::block_on(storage.with_connection(operation))
}

#[test]
fn navigation_intents_flow_into_persisted_bookmarks_and_history() {
    let profile = tempdir().expect("create temporary profile");
    let database = profile.path().join("browser.sqlite3");
    let storage = StorageService::new(database.clone());

    let ParsedInput::Url(page_url) =
        parse_input("example.com/research").expect("parse address bar URL")
    else {
        panic!("host-like input should navigate directly");
    };
    let page_url = page_url.to_string();
    let ParsedInput::Search(search_url) =
        parse_input("rust browser integration").expect("parse address bar search")
    else {
        panic!("search input should produce a search URL");
    };
    let search_url = search_url.to_string();

    with_profile(&storage, move |connection| {
        let folder_id = create_bookmark_folder(connection, "Research", None)?;
        add_bookmark(connection, "Rust browser", &page_url, Some(folder_id), true)?;
        record_history(connection, &page_url, "Initial title", 1_700_000_000)?;
        update_latest_title(connection, &page_url, "Rust browser")?;
        record_history(connection, &search_url, "Search results", 1_700_000_001)?;
        Ok(())
    })
    .expect("persist navigation workflow");

    let reopened_profile = StorageService::new(database);
    let (bookmarks, history, search_history) = with_profile(&reopened_profile, |connection| {
        Ok((
            list_bookmarks(connection, Some("rust".into()), true, None)?,
            list_history(connection, Some("Rust browser".into()), 20)?,
            list_history(connection, Some("Search results".into()), 20)?,
        ))
    })
    .expect("read persisted navigation workflow");

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].url, "https://example.com/research");
    assert_eq!(bookmarks[0].folder_name.as_deref(), Some("Research"));
    assert!(bookmarks[0].on_bar);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].title, "Rust browser");
    assert_eq!(search_history.len(), 1);
    let persisted_search_url =
        Url::parse(&search_history[0].url).expect("stored search history URL is valid");
    assert_eq!(
        persisted_search_url
            .query_pairs()
            .find(|(key, _)| key == "q")
            .map(|(_, value)| value.into_owned())
            .as_deref(),
        Some("rust browser integration")
    );
}

#[test]
fn omnibox_suggestions_merge_bookmarks_and_indexed_history() {
    let profile = tempdir().expect("create temporary profile");
    let storage = StorageService::new(profile.path().join("browser.sqlite3"));

    with_profile(&storage, |connection| {
        let bookmarked_url = "https://example.com/performance";
        add_bookmark(
            connection,
            "Saved performance article",
            bookmarked_url,
            None,
            true,
        )?;
        record_history(
            connection,
            bookmarked_url,
            "Previously visited performance article",
            1_700_000_000,
        )?;
        record_history(
            connection,
            "https://example.org/other",
            "A different performance result",
            1_700_000_001,
        )?;

        let suggestions = collect_omnibox_suggestions(connection, "formance")?;
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].title, "Saved performance article");
        assert_eq!(
            suggestions
                .iter()
                .filter(|item| item.url == bookmarked_url)
                .count(),
            1
        );
        assert!(suggestions
            .iter()
            .any(|item| item.url == "https://example.org/other"));
        assert!(collect_omnibox_suggestions(connection, "x")?.is_empty());
        assert!(collect_omnibox_suggestions(connection, &"x".repeat(257)).is_err());

        for index in 0..10 {
            add_bookmark(
                connection,
                &format!("Performance bookmark {index:02}"),
                &format!("https://bookmarks.example/{index}"),
                None,
                false,
            )?;
        }
        let bounded = collect_omnibox_suggestions(connection, "performance")?;
        assert_eq!(bounded.len(), 8);
        Ok(())
    })
    .expect("collect combined omnibox suggestions");
}

#[test]
fn shared_storage_service_reuses_connection_and_serializes_writes() {
    let profile = tempdir().expect("create temporary profile");
    let storage = StorageService::new(profile.path().join("browser.sqlite3"));

    with_profile(&storage, |connection| {
        connection
            .execute_batch("CREATE TEMP TABLE connection_probe(value TEXT); INSERT INTO connection_probe VALUES ('reused');")
            .map_err(|error| error.to_string())
    })
    .expect("create connection-local probe");
    let probe: String = with_profile(&storage, |connection| {
        connection
            .query_row("SELECT value FROM connection_probe", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| error.to_string())
    })
    .expect("read connection-local probe through subsequent operation");
    assert_eq!(probe, "reused");

    let tasks = (0..8)
        .map(|index| {
            let storage = storage.clone();
            tauri::async_runtime::spawn(async move {
                let url = format!("https://concurrent.example/{index}");
                storage
                    .with_connection(move |connection| {
                        record_history(connection, &url, "Concurrent", 1_700_000_100 + index)
                    })
                    .await
            })
        })
        .collect::<Vec<_>>();
    tauri::async_runtime::block_on(async {
        for task in tasks {
            task.await
                .expect("database operation task")
                .expect("serialized database write");
        }
    });

    let history = with_profile(&storage, |connection| list_history(connection, None, 100))
        .expect("read concurrent history writes");
    assert_eq!(history.len(), 8);
}

#[test]
fn existing_history_is_reindexed_for_omnibox_search() {
    let profile = tempdir().expect("create temporary profile");
    let database = profile.path().join("browser.sqlite3");
    let storage = StorageService::new(database.clone());
    with_profile(&storage, |connection| {
        record_history(
            connection,
            "https://example.com/reindexed",
            "Existing history search phrase",
            1_700_000_200,
        )
    })
    .expect("record history before index rebuild");
    drop(storage);

    let legacy = Connection::open(&database).expect("open database without new search index");
    legacy
        .execute_batch(
            "DROP TRIGGER history_search_insert;
             DROP TRIGGER history_search_delete;
             DROP TRIGGER history_search_update;
             DROP TABLE history_search;
             DROP TABLE browser_features;",
        )
        .expect("remove feature marker and index to simulate an older profile");
    drop(legacy);

    let upgraded = StorageService::new(database);
    let suggestions = with_profile(&upgraded, |connection| {
        collect_omnibox_suggestions(connection, "history search")
    })
    .expect("query indexed legacy history");
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].title, "Existing history search phrase");
}

#[test]
fn download_callbacks_and_startup_recovery_share_persistent_state() {
    let profile = tempdir().expect("create temporary profile");
    let database = profile.path().join("browser.sqlite3");
    let storage = StorageService::new(database.clone());

    let (completed_id, interrupted_id) = with_profile(&storage, |connection| {
        let completed_id = insert_download(
            connection,
            "https://example.com/complete.zip",
            "downloads/complete.zip",
            1_700_000_000,
        )?;
        assert!(finish_download_for_request(
            connection,
            "https://example.com/complete.zip",
            Some("downloads/complete.zip"),
            true,
            1_700_000_010,
        )?);
        assert!(!finish_download_for_request(
            connection,
            "https://example.com/complete.zip",
            Some("downloads/wrong.zip"),
            false,
            1_700_000_011,
        )?);

        let interrupted_id = insert_download(
            connection,
            "https://example.com/interrupted.zip",
            "downloads/interrupted.zip",
            1_700_000_020,
        )?;
        Ok((completed_id, interrupted_id))
    })
    .expect("record download events");

    let reopened_profile = StorageService::new(database);
    let downloads = with_profile(&reopened_profile, |connection| {
        recover_downloads(connection)?;
        list_downloads(connection, 20)
    })
    .expect("recover downloads after restart");

    let completed = downloads
        .iter()
        .find(|download| download.id == completed_id)
        .expect("completed download remains in history");
    assert_eq!(completed.status, "complete");
    assert_eq!(completed.finished_at, Some(1_700_000_010));

    let interrupted = downloads
        .iter()
        .find(|download| download.id == interrupted_id)
        .expect("interrupted download remains in history");
    assert_eq!(interrupted.status, "interrupted");
    assert!(interrupted.finished_at.is_some());
}

#[test]
fn session_groups_and_revision_guard_survive_profile_reopen() {
    let profile = tempdir().expect("create temporary profile");
    let database = profile.path().join("browser.sqlite3");
    let storage = StorageService::new(database.clone());

    with_profile(&storage, |connection| {
        save_session(
            connection,
            SessionSnapshot {
                revision: 4,
                tabs: vec![
                    SessionTab {
                        id: "tab-a".into(),
                        title: "First".into(),
                        url: "https://example.com/".into(),
                        position: 0,
                        active: false,
                        group_id: Some("group-a".into()),
                    },
                    SessionTab {
                        id: "tab-b".into(),
                        title: "Second".into(),
                        url: "https://example.org/".into(),
                        position: 1,
                        active: true,
                        group_id: Some("group-a".into()),
                    },
                ],
                groups: vec![SessionGroup {
                    id: "group-a".into(),
                    name: "Research".into(),
                    color: "#4f7ff0".into(),
                }],
            },
        )?;
        save_session(
            connection,
            SessionSnapshot {
                revision: 3,
                tabs: Vec::new(),
                groups: Vec::new(),
            },
        )
    })
    .expect("save current and stale session snapshots");

    let reopened_profile = StorageService::new(database);
    let restored =
        with_profile(&reopened_profile, load_session).expect("restore persisted session");

    assert_eq!(restored.revision, 4);
    assert_eq!(restored.tabs.len(), 2);
    assert_eq!(restored.tabs[0].id, "tab-a");
    assert_eq!(restored.tabs[1].id, "tab-b");
    assert!(restored.tabs[1].active);
    assert_eq!(restored.tabs[0].group_id.as_deref(), Some("group-a"));
    assert_eq!(restored.groups[0].name, "Research");
}

#[test]
fn storage_service_upgrades_legacy_profile_before_repository_use() {
    let profile = tempdir().expect("create temporary profile");
    let database = profile.path().join("legacy.sqlite3");
    let legacy = Connection::open(&database).expect("create legacy database");
    legacy
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE session_tabs (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                position INTEGER NOT NULL,
                is_active INTEGER NOT NULL
             );
             CREATE TABLE session_state (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                revision INTEGER NOT NULL
             );
             INSERT INTO session_state(id, revision) VALUES (1, 0);
             PRAGMA user_version = 1;",
        )
        .expect("initialize legacy schema");
    drop(legacy);

    let storage = StorageService::new(database);
    let (schema_version, value) = with_profile(&storage, |connection| {
        set_setting(connection, "integration.profile", "migrated")?;
        let schema_version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        Ok((
            schema_version,
            get_setting(connection, "integration.profile")?,
        ))
    })
    .expect("use repositories through migrated profile");

    assert_eq!(schema_version, 6);
    assert_eq!(value.as_deref(), Some("migrated"));
}

use std::path::PathBuf;

use rusqlite::Connection;
use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use super::{
    add_bookmark, collect_omnibox_suggestions, create_bookmark_folder, delete_bookmark,
    delete_bookmark_folder, delete_history_before, delete_history_for_site,
    finish_download_for_request, get_download, get_setting, is_bookmarked, list_bookmark_folders,
    list_bookmarks, list_downloads, list_history, recover_downloads, set_setting, update_bookmark,
    Bookmark, BookmarkFolder, DownloadEntry, DownloadLocation, DownloadProgress, DownloadRuntime,
    HistoryEntry, OmniboxSuggestion, StorageService,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedBookmark {
    title: String,
    url: String,
}

#[tauri::command]
pub async fn bookmarks_list(
    query: Option<String>,
    bar_only: bool,
    folder_id: Option<i64>,
    storage: State<'_, StorageService>,
) -> Result<Vec<Bookmark>, String> {
    storage
        .with_connection(move |connection| list_bookmarks(connection, query, bar_only, folder_id))
        .await
}

#[tauri::command]
pub async fn bookmark_folders(
    storage: State<'_, StorageService>,
) -> Result<Vec<BookmarkFolder>, String> {
    storage
        .with_connection(|connection| list_bookmark_folders(connection))
        .await
}

#[tauri::command]
pub async fn bookmark_create(
    title: String,
    url: String,
    folder_id: Option<i64>,
    on_bar: bool,
    storage: State<'_, StorageService>,
) -> Result<i64, String> {
    storage
        .with_connection(move |connection| {
            add_bookmark(connection, &title, &url, folder_id, on_bar)
        })
        .await
}

#[tauri::command]
pub async fn bookmark_update(
    id: i64,
    title: String,
    url: String,
    folder_id: Option<i64>,
    on_bar: bool,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    storage
        .with_connection(move |connection| {
            update_bookmark(connection, id, &title, &url, folder_id, on_bar)
        })
        .await
}

#[tauri::command]
pub async fn bookmark_delete(id: i64, storage: State<'_, StorageService>) -> Result<(), String> {
    storage
        .with_connection(move |connection| delete_bookmark(connection, id))
        .await
}

#[tauri::command]
pub async fn bookmark_folder_create(
    name: String,
    parent_id: Option<i64>,
    storage: State<'_, StorageService>,
) -> Result<i64, String> {
    storage
        .with_connection(move |connection| create_bookmark_folder(connection, &name, parent_id))
        .await
}

#[tauri::command]
pub async fn bookmark_folder_delete(
    id: i64,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    storage
        .with_connection(move |connection| delete_bookmark_folder(connection, id))
        .await
}

#[tauri::command]
pub async fn bookmark_is_saved(
    url: String,
    storage: State<'_, StorageService>,
) -> Result<bool, String> {
    storage
        .with_connection(move |connection| is_bookmarked(connection, &url))
        .await
}

#[tauri::command]
pub async fn bookmark_import(
    bookmarks: Vec<ImportedBookmark>,
    storage: State<'_, StorageService>,
) -> Result<usize, String> {
    if bookmarks.len() > 5_000 {
        return Err("The import contains too many bookmarks (maximum 5,000).".to_string());
    }
    storage
        .with_connection(move |connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|error| format!("could not begin bookmark import: {error}"))?;
            for bookmark in &bookmarks {
                add_bookmark(&transaction, &bookmark.title, &bookmark.url, None, false)?;
            }
            transaction
                .commit()
                .map_err(|error| format!("could not commit bookmark import: {error}"))?;
            Ok(bookmarks.len())
        })
        .await
}

#[tauri::command]
pub async fn history_list(
    query: Option<String>,
    limit: usize,
    storage: State<'_, StorageService>,
) -> Result<Vec<HistoryEntry>, String> {
    storage
        .with_connection(move |connection| list_history(connection, query, limit))
        .await
}

#[tauri::command]
pub async fn omnibox_suggestions(
    query: String,
    storage: State<'_, StorageService>,
) -> Result<Vec<OmniboxSuggestion>, String> {
    storage
        .with_connection(move |connection| collect_omnibox_suggestions(connection, &query))
        .await
}

#[tauri::command]
pub async fn history_clear_before(
    timestamp: i64,
    storage: State<'_, StorageService>,
) -> Result<usize, String> {
    storage
        .with_connection(move |connection| delete_history_before(connection, timestamp))
        .await
}

#[tauri::command]
pub async fn history_delete_site(
    site: String,
    storage: State<'_, StorageService>,
) -> Result<usize, String> {
    storage
        .with_connection(move |connection| delete_history_for_site(connection, &site))
        .await
}

#[tauri::command]
pub async fn history_clear(storage: State<'_, StorageService>) -> Result<usize, String> {
    storage
        .with_connection(|connection| super::history::clear_history(connection))
        .await
}

#[tauri::command]
pub async fn downloads_list(
    limit: usize,
    storage: State<'_, StorageService>,
) -> Result<Vec<DownloadEntry>, String> {
    storage
        .with_connection(move |connection| list_downloads(connection, limit))
        .await
}

#[tauri::command]
pub fn downloads_progress(
    runtime: State<'_, DownloadRuntime>,
) -> Result<Vec<DownloadProgress>, String> {
    runtime.active()
}

#[tauri::command]
pub async fn download_control(
    app: AppHandle,
    id: i64,
    action: String,
    storage: State<'_, StorageService>,
    runtime: State<'_, DownloadRuntime>,
) -> Result<(), String> {
    if !matches!(action.as_str(), "pause" | "resume" | "cancel") {
        return Err("unsupported download action".to_string());
    }
    let download = storage
        .with_connection(move |connection| get_download(connection, id))
        .await?;
    if download.status != "downloading" {
        return Err("Only active downloads can be controlled.".to_string());
    }
    if !crate::tabs::control_download_operation(&app, &runtime, &download.path, &action)? {
        return Err("Download is no longer active.".to_string());
    }
    if action == "pause" {
        runtime.set_paused(&download.path, true)?;
    } else if action == "resume" {
        runtime.set_paused(&download.path, false)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn downloads_clear(storage: State<'_, StorageService>) -> Result<usize, String> {
    storage
        .with_connection(|connection| super::downloads::clear_downloads(connection))
        .await
}

#[tauri::command]
pub async fn downloads_recover(storage: State<'_, StorageService>) -> Result<usize, String> {
    storage
        .with_connection(|connection| recover_downloads(connection))
        .await
}

#[tauri::command]
pub async fn download_open(
    app: AppHandle,
    id: i64,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    let path = storage
        .with_connection(move |connection| {
            let download = get_download(connection, id)?;
            let path = PathBuf::from(download.path);
            if !path.is_file() {
                return Err("The downloaded file is not available on disk.".to_string());
            }
            Ok(path)
        })
        .await?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|error| format!("could not open downloaded file: {error}"))
}

#[tauri::command]
pub async fn download_reveal(
    app: AppHandle,
    id: i64,
    storage: State<'_, StorageService>,
) -> Result<(), String> {
    let path = storage
        .with_connection(move |connection| {
            let download = get_download(connection, id)?;
            let path = PathBuf::from(download.path);
            if !path.is_file() {
                return Err("The downloaded file is not available on disk.".to_string());
            }
            Ok(path)
        })
        .await?;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|error| format!("could not reveal downloaded file: {error}"))
}

#[tauri::command]
pub async fn download_directory(
    storage: State<'_, StorageService>,
    location: State<'_, DownloadLocation>,
) -> Result<String, String> {
    if let Some(path) = storage
        .with_connection(|connection| get_setting(connection, "download_directory"))
        .await?
    {
        let path = PathBuf::from(path);
        let directory = path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            std::fs::create_dir_all(&directory)
                .map_err(|error| format!("could not create download directory: {error}"))
        })
        .await
        .map_err(|error| format!("download-directory worker failed: {error}"))??;
        location.update(path.clone())?;
        return Ok(path.to_string_lossy().into_owned());
    }
    Ok(location.current()?.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn download_ask_where_to_save(
    storage: State<'_, StorageService>,
    location: State<'_, DownloadLocation>,
) -> Result<bool, String> {
    let setting = storage
        .with_connection(|connection| get_setting(connection, "download.ask_where_to_save"))
        .await?;
    let enabled = match setting.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err("Stored download prompt preference is invalid.".to_string()),
    };
    location.set_ask_where_to_save(enabled);
    Ok(enabled)
}

#[tauri::command]
pub async fn set_download_ask_where_to_save(
    enabled: bool,
    storage: State<'_, StorageService>,
    location: State<'_, DownloadLocation>,
) -> Result<(), String> {
    let value = if enabled { "true" } else { "false" }.to_string();
    storage
        .with_connection(move |connection| {
            set_setting(connection, "download.ask_where_to_save", &value)
        })
        .await?;
    location.set_ask_where_to_save(enabled);
    Ok(())
}

#[tauri::command]
pub async fn set_download_directory(
    path: String,
    storage: State<'_, StorageService>,
    location: State<'_, DownloadLocation>,
) -> Result<String, String> {
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("Download location must be an absolute path.".to_string());
    }
    let directory = path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("could not create download directory: {error}"))
    })
    .await
    .map_err(|error| format!("download-directory worker failed: {error}"))??;
    let stored_path = path.to_string_lossy().into_owned();
    storage
        .with_connection({
            let stored_path = stored_path.clone();
            move |connection| set_setting(connection, "download_directory", &stored_path)
        })
        .await?;
    location.update(path)?;
    Ok(stored_path)
}

pub fn record_download_start(
    connection: &Connection,
    url: &str,
    path: &str,
    started_at: i64,
) -> Result<i64, String> {
    super::downloads::insert_download(connection, url, path, started_at)
}

pub fn record_download_finish(
    connection: &Connection,
    url: &str,
    path: Option<&str>,
    success: bool,
    finished_at: i64,
) -> Result<bool, String> {
    finish_download_for_request(connection, url, path, success, finished_at)
}

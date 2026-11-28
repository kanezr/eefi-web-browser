use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::DownloadEntry;

#[derive(Clone)]
pub struct DownloadLocation {
    path: Arc<RwLock<PathBuf>>,
    ask_where_to_save: Arc<std::sync::atomic::AtomicBool>,
}

impl DownloadLocation {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Arc::new(RwLock::new(path)),
            ask_where_to_save: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn current(&self) -> Result<PathBuf, String> {
        self.path
            .read()
            .map(|path| path.clone())
            .map_err(|_| "download location lock is poisoned".to_string())
    }

    pub fn update(&self, path: PathBuf) -> Result<(), String> {
        let mut current = self
            .path
            .write()
            .map_err(|_| "download location lock is poisoned".to_string())?;
        *current = path;
        Ok(())
    }

    pub fn ask_where_to_save(&self) -> bool {
        self.ask_where_to_save
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_ask_where_to_save(&self, enabled: bool) {
        self.ask_where_to_save
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub path: String,
    pub bytes_received: u64,
    pub total_bytes: Option<u64>,
    pub paused: bool,
}

#[derive(Clone, Default)]
pub struct DownloadRuntime {
    active: Arc<Mutex<HashMap<String, DownloadRuntimeEntry>>>,
    #[cfg(windows)]
    prompts: Arc<Mutex<HashMap<String, PendingDownloadPrompt>>>,
    next_operation_id: Arc<AtomicU64>,
    next_prompt_id: Arc<AtomicU64>,
}

#[derive(Clone)]
struct DownloadRuntimeEntry {
    progress: DownloadProgress,
    last_reported_at: std::time::Instant,
    cancel_requested: bool,
    #[cfg(windows)]
    page_label: String,
    #[cfg(windows)]
    operation_id: String,
}

#[derive(Debug, Clone)]
pub struct DownloadPromptInfo {
    pub url: String,
    pub suggested_path: String,
    pub is_private: bool,
}

#[cfg(windows)]
pub struct PendingDownloadPrompt {
    pub info: DownloadPromptInfo,
    pub page_label: String,
    path_key: Option<Arc<Mutex<String>>>,
}

impl DownloadRuntime {
    #[cfg(windows)]
    pub fn register(&self, path: String, page_label: String) -> Result<String, String> {
        let operation_id = format!(
            "download-operation-{}",
            self.next_operation_id.fetch_add(1, Ordering::Relaxed)
        );
        let entry = DownloadRuntimeEntry {
            progress: DownloadProgress {
                path: path.clone(),
                bytes_received: 0,
                total_bytes: None,
                paused: false,
            },
            last_reported_at: std::time::Instant::now(),
            cancel_requested: false,
            page_label,
            operation_id: operation_id.clone(),
        };
        self.active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())?
            .insert(path, entry);
        Ok(operation_id)
    }

    pub fn update_progress(
        &self,
        path: &str,
        bytes_received: u64,
        total_bytes: Option<u64>,
    ) -> Result<Option<DownloadProgress>, String> {
        let mut downloads = self
            .active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())?;
        let Some(download) = downloads.get_mut(path) else {
            return Ok(None);
        };
        download.progress.bytes_received = bytes_received;
        download.progress.total_bytes = total_bytes.filter(|bytes| *bytes > 0);
        let now = std::time::Instant::now();
        let completed = download
            .progress
            .total_bytes
            .is_some_and(|total| bytes_received >= total);
        if completed
            || now.duration_since(download.last_reported_at)
                >= std::time::Duration::from_millis(100)
        {
            download.last_reported_at = now;
            Ok(Some(download.progress.clone()))
        } else {
            Ok(None)
        }
    }

    pub fn set_paused(&self, path: &str, paused: bool) -> Result<(), String> {
        let mut downloads = self
            .active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())?;
        let download = downloads
            .get_mut(path)
            .ok_or_else(|| "Download is no longer active.".to_string())?;
        download.progress.paused = paused;
        Ok(())
    }

    pub fn set_cancel_requested(&self, path: &str, requested: bool) -> Result<(), String> {
        let mut downloads = self
            .active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())?;
        if let Some(download) = downloads.get_mut(path) {
            download.cancel_requested = requested;
        } else if requested {
            return Err("Download is no longer active.".to_string());
        }
        Ok(())
    }

    pub fn take_completion(&self, path: &str, completed: bool) -> Result<Option<String>, String> {
        let mut downloads = self
            .active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())?;
        let Some(download) = downloads.remove(path) else {
            return Ok(None);
        };
        Ok((download.cancel_requested && !completed).then_some(download.progress.path))
    }

    pub fn active(&self) -> Result<Vec<DownloadProgress>, String> {
        self.active
            .lock()
            .map(|downloads| {
                downloads
                    .values()
                    .map(|download| download.progress.clone())
                    .collect()
            })
            .map_err(|_| "download manager state lock is poisoned".to_string())
    }

    #[cfg(windows)]
    pub fn control_target(&self, path: &str) -> Result<Option<(String, String)>, String> {
        self.active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())
            .map(|downloads| {
                downloads
                    .get(path)
                    .map(|download| (download.page_label.clone(), download.operation_id.clone()))
            })
    }

    pub fn remove(&self, path: &str) -> Result<(), String> {
        self.active
            .lock()
            .map_err(|_| "download manager state lock is poisoned".to_string())?
            .remove(path);
        Ok(())
    }

    #[cfg(windows)]
    pub fn register_prompt(
        &self,
        info: DownloadPromptInfo,
        page_label: String,
        path_key: Option<Arc<Mutex<String>>>,
    ) -> Result<String, String> {
        let id = self.next_prompt_id.fetch_add(1, Ordering::Relaxed);
        let id = format!("download-prompt-{id}");
        self.prompts
            .lock()
            .map_err(|_| "download prompt state lock is poisoned".to_string())?
            .insert(
                id.clone(),
                PendingDownloadPrompt {
                    info,
                    page_label,
                    path_key,
                },
            );
        Ok(id)
    }

    pub fn prompt_info(&self, id: &str) -> Result<DownloadPromptInfo, String> {
        #[cfg(windows)]
        {
            return self
                .prompts
                .lock()
                .map_err(|_| "download prompt state lock is poisoned".to_string())?
                .get(id)
                .map(|prompt| prompt.info.clone())
                .ok_or_else(|| "Download save prompt is no longer active.".to_string());
        }
        #[cfg(not(windows))]
        {
            let _ = id;
            Err("download save prompts are currently supported on Windows only".to_string())
        }
    }

    #[cfg(windows)]
    pub fn prompt_target(&self, id: &str) -> Result<String, String> {
        self.prompts
            .lock()
            .map_err(|_| "download prompt state lock is poisoned".to_string())?
            .get(id)
            .map(|prompt| prompt.page_label.clone())
            .ok_or_else(|| "Download save prompt is no longer active.".to_string())
    }

    #[cfg(windows)]
    pub fn take_prompt(&self, id: &str) -> Result<PendingDownloadPrompt, String> {
        self.prompts
            .lock()
            .map_err(|_| "download prompt state lock is poisoned".to_string())?
            .remove(id)
            .ok_or_else(|| "Download save prompt is no longer active.".to_string())
    }

    #[cfg(windows)]
    pub fn select_prompt_destination(
        &self,
        prompt: &PendingDownloadPrompt,
        path: String,
    ) -> Result<(), String> {
        if let Some(path_key) = &prompt.path_key {
            *path_key
                .lock()
                .map_err(|_| "download path state lock is poisoned".to_string())? = path.clone();
            let mut active = self
                .active
                .lock()
                .map_err(|_| "download manager state lock is poisoned".to_string())?;
            if let Some(mut download) = active.remove(&prompt.info.suggested_path) {
                download.progress.path = path.clone();
                active.insert(path, download);
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn cancel_prompt(&self, prompt: &PendingDownloadPrompt) -> Result<(), String> {
        if prompt.path_key.is_some() {
            self.remove(&prompt.info.suggested_path)?;
        }
        Ok(())
    }
}

pub fn insert_download(
    connection: &Connection,
    url: &str,
    path: &str,
    started_at: i64,
) -> Result<i64, String> {
    if url.len() > 4096 || path.len() > 4096 {
        return Err("Download URL or destination path is too long.".to_string());
    }
    connection
        .execute(
            "INSERT INTO downloads(url, path, status, started_at) VALUES (?1, ?2, 'downloading', ?3)",
            params![url, path, started_at],
        )
        .map_err(db_error)?;
    Ok(connection.last_insert_rowid())
}

pub fn finish_download(
    connection: &Connection,
    id: i64,
    success: bool,
    finished_at: i64,
) -> Result<(), String> {
    let affected = connection
        .execute(
            "UPDATE downloads SET status = ?1, finished_at = ?2 WHERE id = ?3",
            params![if success { "complete" } else { "failed" }, finished_at, id],
        )
        .map_err(db_error)?;
    if affected == 0 {
        return Err("Download record does not exist.".to_string());
    }
    Ok(())
}

pub fn finish_download_for_request(
    connection: &Connection,
    url: &str,
    path: Option<&str>,
    success: bool,
    finished_at: i64,
) -> Result<bool, String> {
    let download_id = connection
        .query_row(
            "SELECT id FROM downloads
             WHERE url = ?1 AND status = 'downloading'
               AND (?2 IS NULL OR path = ?2)
             ORDER BY id DESC LIMIT 1",
            params![url, path],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error)?;
    let Some(download_id) = download_id else {
        return Ok(false);
    };
    finish_download(connection, download_id, success, finished_at)?;
    Ok(true)
}

pub fn list_downloads(connection: &Connection, limit: usize) -> Result<Vec<DownloadEntry>, String> {
    let limit = limit.clamp(1, 500) as i64;
    let mut statement = connection
        .prepare(
            "SELECT id, url, path, status, started_at, finished_at
             FROM downloads ORDER BY started_at DESC, id DESC LIMIT ?1",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok(DownloadEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                path: row.get(2)?,
                status: row.get(3)?,
                started_at: row.get(4)?,
                finished_at: row.get(5)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

pub fn get_download(connection: &Connection, id: i64) -> Result<DownloadEntry, String> {
    connection
        .query_row(
            "SELECT id, url, path, status, started_at, finished_at FROM downloads WHERE id = ?1",
            [id],
            |row| {
                Ok(DownloadEntry {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    path: row.get(2)?,
                    status: row.get(3)?,
                    started_at: row.get(4)?,
                    finished_at: row.get(5)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => "Download record does not exist.".to_string(),
            other => db_error(other),
        })
}

pub fn clear_downloads(connection: &Connection) -> Result<usize, String> {
    connection
        .execute("DELETE FROM downloads WHERE status != 'downloading'", [])
        .map_err(db_error)
}

pub fn recover_downloads(connection: &Connection) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE downloads SET status = 'interrupted', finished_at = unixepoch()
             WHERE status = 'downloading'",
            [],
        )
        .map_err(db_error)
}

pub fn set_setting(connection: &Connection, key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > 120 || value.len() > 4096 {
        return Err("Setting key or value is too long.".to_string());
    }
    connection
        .execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(db_error)?;
    Ok(())
}

pub fn get_setting(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(db_error)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn only_explicitly_canceled_incomplete_downloads_return_a_file_to_delete() {
        let runtime = DownloadRuntime::default();

        runtime
            .register("failed-download.tmp".to_string(), "page-1".to_string())
            .unwrap();
        assert_eq!(
            runtime
                .take_completion("failed-download.tmp", false)
                .unwrap(),
            None
        );

        runtime
            .register("completed-download.tmp".to_string(), "page-1".to_string())
            .unwrap();
        runtime
            .set_cancel_requested("completed-download.tmp", true)
            .unwrap();
        assert_eq!(
            runtime
                .take_completion("completed-download.tmp", true)
                .unwrap(),
            None
        );

        runtime
            .register("canceled-download.tmp".to_string(), "page-1".to_string())
            .unwrap();
        runtime
            .set_cancel_requested("canceled-download.tmp", true)
            .unwrap();
        assert_eq!(
            runtime
                .take_completion("canceled-download.tmp", false)
                .unwrap(),
            Some("canceled-download.tmp".to_string())
        );
    }
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

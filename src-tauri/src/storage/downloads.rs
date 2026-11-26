use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use rusqlite::{params, Connection, OptionalExtension};

use super::DownloadEntry;

#[derive(Clone)]
pub struct DownloadLocation(Arc<RwLock<PathBuf>>);

impl DownloadLocation {
    pub fn new(path: PathBuf) -> Self {
        Self(Arc::new(RwLock::new(path)))
    }

    pub fn current(&self) -> Result<PathBuf, String> {
        self.0
            .read()
            .map(|path| path.clone())
            .map_err(|_| "download location lock is poisoned".to_string())
    }

    pub fn update(&self, path: PathBuf) -> Result<(), String> {
        let mut current = self
            .0
            .write()
            .map_err(|_| "download location lock is poisoned".to_string())?;
        *current = path;
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
        .execute("DELETE FROM downloads", [])
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

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

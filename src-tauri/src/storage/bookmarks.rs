use rusqlite::{params, Connection};

use super::{Bookmark, BookmarkFolder};

pub fn list_bookmarks(
    connection: &Connection,
    query: Option<String>,
    bar_only: bool,
    folder_id: Option<i64>,
) -> Result<Vec<Bookmark>, String> {
    let query = query
        .filter(|query| !query.trim().is_empty())
        .map(|query| format!("%{}%", escape_like(query.trim())));
    let mut statement = connection
        .prepare(
            "SELECT b.id, b.title, b.url, b.folder_id, f.name, b.on_bar, b.created_at
             FROM bookmarks b
             LEFT JOIN bookmark_folders f ON f.id = b.folder_id
             WHERE (?1 IS NULL OR b.folder_id = ?1)
               AND (?2 = 0 OR b.on_bar = 1)
               AND (?3 IS NULL OR b.title LIKE ?3 ESCAPE '\\' OR b.url LIKE ?3 ESCAPE '\\')
             ORDER BY b.on_bar DESC, b.title COLLATE NOCASE, b.id DESC",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![folder_id, i64::from(bar_only), query], |row| {
            Ok(Bookmark {
                id: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                folder_id: row.get(3)?,
                folder_name: row.get(4)?,
                on_bar: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

pub fn list_bookmark_folders(connection: &Connection) -> Result<Vec<BookmarkFolder>, String> {
    let mut statement = connection
        .prepare("SELECT id, name, parent_id FROM bookmark_folders ORDER BY name COLLATE NOCASE")
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(BookmarkFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

pub fn create_bookmark_folder(
    connection: &Connection,
    name: &str,
    parent_id: Option<i64>,
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 120 {
        return Err("Folder names must be between 1 and 120 bytes.".to_string());
    }
    connection
        .execute(
            "INSERT INTO bookmark_folders(name, parent_id) VALUES (?1, ?2)",
            params![name, parent_id],
        )
        .map_err(db_error)?;
    Ok(connection.last_insert_rowid())
}

pub fn delete_bookmark_folder(connection: &Connection, folder_id: i64) -> Result<(), String> {
    let affected = connection
        .execute("DELETE FROM bookmark_folders WHERE id = ?1", [folder_id])
        .map_err(db_error)?;
    if affected == 0 {
        return Err("Bookmark folder does not exist.".to_string());
    }
    Ok(())
}

pub fn add_bookmark(
    connection: &Connection,
    title: &str,
    url: &str,
    folder_id: Option<i64>,
    on_bar: bool,
) -> Result<i64, String> {
    let title = validate_title(title)?;
    let url = validate_bookmark_url(url)?;
    connection
        .execute(
            "INSERT INTO bookmarks(title, url, folder_id, on_bar, created_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())
             ON CONFLICT(url) DO UPDATE SET title = excluded.title,
               folder_id = excluded.folder_id, on_bar = excluded.on_bar",
            params![title, url, folder_id, i64::from(on_bar)],
        )
        .map_err(db_error)?;
    connection
        .query_row("SELECT id FROM bookmarks WHERE url = ?1", [url], |row| {
            row.get(0)
        })
        .map_err(db_error)
}

pub fn update_bookmark(
    connection: &Connection,
    id: i64,
    title: &str,
    url: &str,
    folder_id: Option<i64>,
    on_bar: bool,
) -> Result<(), String> {
    let title = validate_title(title)?;
    let url = validate_bookmark_url(url)?;
    let affected = connection
        .execute(
            "UPDATE bookmarks SET title = ?1, url = ?2, folder_id = ?3, on_bar = ?4 WHERE id = ?5",
            params![title, url, folder_id, i64::from(on_bar), id],
        )
        .map_err(db_error)?;
    if affected == 0 {
        return Err("Bookmark does not exist.".to_string());
    }
    Ok(())
}

pub fn delete_bookmark(connection: &Connection, id: i64) -> Result<(), String> {
    let affected = connection
        .execute("DELETE FROM bookmarks WHERE id = ?1", [id])
        .map_err(db_error)?;
    if affected == 0 {
        return Err("Bookmark does not exist.".to_string());
    }
    Ok(())
}

pub fn is_bookmarked(connection: &Connection, url: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM bookmarks WHERE url = ?1)",
            [url],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .map_err(db_error)
}

fn validate_title(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() || title.len() > 512 {
        return Err("Bookmark titles must be between 1 and 512 bytes.".to_string());
    }
    Ok(title.to_string())
}

fn validate_bookmark_url(value: &str) -> Result<String, String> {
    let url =
        url::Url::parse(value.trim()).map_err(|_| "Enter a valid HTTP(S) URL.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err("Only HTTP and HTTPS pages can be bookmarked.".to_string());
    }
    if url.as_str().len() > 4096 {
        return Err("Bookmark URL is too long.".to_string());
    }
    Ok(url.to_string())
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

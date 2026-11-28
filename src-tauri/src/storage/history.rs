use rusqlite::{params, Connection};
use url::Url;

use super::{HistoryEntry, OmniboxSuggestion};

pub fn record_history(
    connection: &Connection,
    url: &str,
    title: &str,
    visited_at: i64,
) -> Result<(), String> {
    let url = valid_web_url(url)?;
    let title = title.chars().take(512).collect::<String>();
    connection
        .execute(
            "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, ?3)",
            params![url, title, visited_at],
        )
        .map_err(db_error)?;
    Ok(())
}

pub fn update_latest_title(connection: &Connection, url: &str, title: &str) -> Result<(), String> {
    let title = title.chars().take(512).collect::<String>();
    connection
        .execute(
            "UPDATE history SET title = ?1 WHERE id = (
                SELECT id FROM history WHERE url = ?2 ORDER BY visited_at DESC, id DESC LIMIT 1
             )",
            params![title, url],
        )
        .map_err(db_error)?;
    Ok(())
}

pub fn list_history(
    connection: &Connection,
    query: Option<String>,
    limit: usize,
) -> Result<Vec<HistoryEntry>, String> {
    let query = query
        .filter(|query| !query.trim().is_empty())
        .map(|query| format!("%{}%", escape_like(query.trim())));
    let limit = limit.clamp(1, 1000) as i64;
    let mut statement = connection
        .prepare(
            "SELECT id, url, title, visited_at FROM history
             WHERE ?1 IS NULL OR url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
             ORDER BY visited_at DESC, id DESC LIMIT ?2",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![query, limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                visited_at: row.get(3)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

pub fn suggest_history(
    connection: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<OmniboxSuggestion>, String> {
    let query = query.trim();
    let limit = limit.clamp(1, 8) as i64;
    if query.chars().count() < 3 {
        let query = format!("%{}%", escape_like(query));
        let mut statement = connection
            .prepare(
                "SELECT url, title FROM history
                 WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
                 ORDER BY visited_at DESC, id DESC LIMIT ?2",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![query, limit], |row| {
                Ok(OmniboxSuggestion {
                    url: row.get(0)?,
                    title: row.get(1)?,
                })
            })
            .map_err(db_error)?;
        return rows.collect::<Result<Vec<_>, _>>().map_err(db_error);
    }

    let match_query = format!("\"{}\"", query.replace('"', "\"\""));
    let mut statement = connection
        .prepare(
            "SELECT h.url, h.title FROM history_search
             JOIN history h ON h.id = history_search.rowid
             WHERE history_search MATCH ?1
             ORDER BY h.visited_at DESC, h.id DESC LIMIT ?2",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![match_query, limit], |row| {
            Ok(OmniboxSuggestion {
                url: row.get(0)?,
                title: row.get(1)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}

pub fn delete_history_before(connection: &Connection, timestamp: i64) -> Result<usize, String> {
    if timestamp < 0 {
        return Err("History cutoff must be a non-negative timestamp.".to_string());
    }
    connection
        .execute("DELETE FROM history WHERE visited_at < ?1", [timestamp])
        .map_err(db_error)
}

pub fn delete_history_for_site(connection: &Connection, site: &str) -> Result<usize, String> {
    let site_url = Url::parse(&format!("https://{site}"))
        .map_err(|_| "Enter a valid site hostname.".to_string())?;
    let host = site_url
        .host_str()
        .filter(|_| site_url.path() == "/" && site_url.query().is_none())
        .ok_or_else(|| "Enter a valid site hostname.".to_string())?;
    let urls = {
        let mut statement = connection
            .prepare("SELECT id, url FROM history")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
    };
    let matching_ids = urls
        .into_iter()
        .filter_map(|(id, url)| {
            Url::parse(&url)
                .ok()
                .filter(|url| url.host_str() == Some(host))
                .map(|_| id)
        })
        .collect::<Vec<_>>();
    let transaction = connection.unchecked_transaction().map_err(db_error)?;
    for id in &matching_ids {
        transaction
            .execute("DELETE FROM history WHERE id = ?1", [id])
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)?;
    Ok(matching_ids.len())
}

pub fn clear_history(connection: &Connection) -> Result<usize, String> {
    connection
        .execute("DELETE FROM history", [])
        .map_err(db_error)
}

fn valid_web_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "Invalid history URL.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err("Only HTTP and HTTPS pages are saved to history.".to_string());
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

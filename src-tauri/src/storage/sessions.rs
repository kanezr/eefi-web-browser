use rusqlite::{params, Connection, Transaction, TransactionBehavior};

use super::{SessionGroup, SessionSnapshot, SessionTab};

pub fn save_session(connection: &Connection, snapshot: SessionSnapshot) -> Result<(), String> {
    let revision = i64::try_from(snapshot.revision)
        .map_err(|_| "Session revision is out of range.".to_string())?;
    let transaction =
        Transaction::new_unchecked(connection, TransactionBehavior::Immediate).map_err(db_error)?;
    let stored_revision: i64 = transaction
        .query_row(
            "SELECT revision FROM session_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if stored_revision > revision {
        transaction.rollback().map_err(db_error)?;
        return Ok(());
    }
    transaction
        .execute("DELETE FROM session_tabs", [])
        .map_err(db_error)?;
    transaction
        .execute("DELETE FROM tab_groups", [])
        .map_err(db_error)?;
    {
        let mut statement = transaction
            .prepare("INSERT INTO tab_groups(id, name, color) VALUES (?1, ?2, ?3)")
            .map_err(db_error)?;
        for group in snapshot.groups {
            statement
                .execute(params![group.id, group.name, group.color])
                .map_err(db_error)?;
        }
    }
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO session_tabs(id, title, url, position, is_active, group_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(db_error)?;
        for tab in snapshot.tabs {
            statement
                .execute(params![
                    tab.id,
                    tab.title,
                    tab.url,
                    tab.position as i64,
                    i64::from(tab.active),
                    tab.group_id
                ])
                .map_err(db_error)?;
        }
    }
    transaction
        .execute(
            "UPDATE session_state SET revision = ?1 WHERE id = 1",
            [revision],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}

pub fn load_session(connection: &Connection) -> Result<SessionSnapshot, String> {
    let revision: i64 = connection
        .query_row(
            "SELECT revision FROM session_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let mut group_statement = connection
        .prepare("SELECT id, name, color FROM tab_groups ORDER BY rowid ASC")
        .map_err(db_error)?;
    let groups = group_statement
        .query_map([], |row| {
            Ok(SessionGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let mut statement = connection
        .prepare(
            "SELECT id, title, url, position, is_active, group_id
             FROM session_tabs ORDER BY position ASC",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(SessionTab {
                id: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                position: row.get::<_, i64>(3)?.max(0) as usize,
                active: row.get::<_, i64>(4)? != 0,
                group_id: row.get(5)?,
            })
        })
        .map_err(db_error)?;
    let tabs = rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?;
    Ok(SessionSnapshot {
        revision: revision.max(0) as u64,
        tabs,
        groups,
    })
}

fn db_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

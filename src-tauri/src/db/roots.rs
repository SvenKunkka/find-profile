use rusqlite::{params, Connection, OptionalExtension};

use super::error::{DbError, Result};
use super::model::{Root, RootKind, RootStatus};
use super::util::now_iso;

const SELECT_COLS: &str =
    "id, path, kind, enabled, status, scan_interval, last_scanned_at, created_at, updated_at";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Root> {
    Ok(Root {
        id: row.get(0)?,
        path: row.get(1)?,
        kind: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        status: row.get(4)?,
        scan_interval: row.get(5)?,
        last_scanned_at: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub fn add_root(conn: &Connection, path: &str, kind: RootKind) -> Result<Root> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO roots (path, kind, enabled, status, created_at, updated_at)
         VALUES (?1, ?2, 1, ?3, ?4, ?4)",
        params![path, kind, RootStatus::Available, now],
    )?;
    let id = conn.last_insert_rowid();
    get_root(conn, id)
}

pub fn get_root(conn: &Connection, id: i64) -> Result<Root> {
    let sql = format!("SELECT {SELECT_COLS} FROM roots WHERE id = ?1");
    let root = conn
        .query_row(&sql, params![id], map_row)
        .optional()?
        .ok_or_else(|| DbError::NotFound(format!("root id {id}")))?;
    Ok(root)
}

pub fn update_root_status(conn: &Connection, root_id: i64, status: RootStatus) -> Result<()> {
    let now = now_iso();
    let n = conn.execute(
        "UPDATE roots SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, now, root_id],
    )?;
    if n == 0 {
        return Err(DbError::NotFound(format!("root id {root_id}")));
    }
    Ok(())
}

pub fn list_roots(conn: &Connection) -> Result<Vec<Root>> {
    let sql = format!("SELECT {SELECT_COLS} FROM roots ORDER BY id ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Find the most-specific root whose path contains the given filesystem path,
/// or `None` if no registered root owns it.
///
/// Used by `commands::open_file` / `reveal_in_finder` to detect that a target
/// path lives under a NAS root that is currently disconnected, so the UI can
/// surface a clear "NAS unavailable" error instead of the generic not-found.
pub fn find_root_for_path(conn: &Connection, path: &str) -> Result<Option<Root>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM roots
         WHERE ?1 = path OR ?1 LIKE path || '/%'
         ORDER BY length(path) DESC
         LIMIT 1"
    );
    let root = conn.query_row(&sql, params![path], map_row).optional()?;
    Ok(root)
}

/// Soft-remove a root by flipping its `enabled` flag to 0.
/// The `files` and `scan_jobs` rows for this root are preserved untouched —
/// the index remains queryable even though the root is no longer active.
pub fn disable_root(conn: &Connection, root_id: i64) -> Result<()> {
    let now = now_iso();
    let n = conn.execute(
        "UPDATE roots SET enabled = 0, updated_at = ?1 WHERE id = ?2",
        params![now, root_id],
    )?;
    if n == 0 {
        return Err(DbError::NotFound(format!("root id {root_id}")));
    }
    Ok(())
}

/// Hard-remove a root and cascade-delete its `files` and `scan_jobs` rows.
/// The FTS triggers on `files` keep the FTS5 table in sync via the cascade.
pub fn delete_root(conn: &Connection, root_id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM roots WHERE id = ?1", params![root_id])?;
    if n == 0 {
        return Err(DbError::NotFound(format!("root id {root_id}")));
    }
    Ok(())
}

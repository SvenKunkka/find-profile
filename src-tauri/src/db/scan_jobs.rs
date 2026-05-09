use rusqlite::{params, Connection, OptionalExtension};

use super::error::{DbError, Result};
use super::model::{ScanJob, ScanJobStatus};
use super::util::now_iso;

const SELECT_COLS: &str = "id, root_id, status, scanned_count, error_count,
       estimated_total_count, current_path,
       started_at, finished_at, error_message";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScanJob> {
    Ok(ScanJob {
        id: row.get(0)?,
        root_id: row.get(1)?,
        status: row.get(2)?,
        scanned_count: row.get(3)?,
        error_count: row.get(4)?,
        estimated_total_count: row.get(5)?,
        current_path: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        error_message: row.get(9)?,
    })
}

pub fn create_scan_job(conn: &Connection, root_id: i64) -> Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO scan_jobs (root_id, status, scanned_count, error_count, started_at)
         VALUES (?1, ?2, 0, 0, ?3)",
        params![root_id, ScanJobStatus::Running, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Update mid-scan counters and the most-recent path. Pass `current_path =
/// None` to leave the existing value untouched (e.g. progress emitted from a
/// phase that doesn't yet have a per-entry path).
pub fn update_scan_job_progress(
    conn: &Connection,
    job_id: i64,
    scanned_count: i64,
    error_count: i64,
    current_path: Option<&str>,
) -> Result<()> {
    let n = match current_path {
        Some(p) => conn.execute(
            "UPDATE scan_jobs
             SET scanned_count = ?1, error_count = ?2, current_path = ?3
             WHERE id = ?4",
            params![scanned_count, error_count, p, job_id],
        )?,
        None => conn.execute(
            "UPDATE scan_jobs
             SET scanned_count = ?1, error_count = ?2
             WHERE id = ?3",
            params![scanned_count, error_count, job_id],
        )?,
    };
    if n == 0 {
        return Err(DbError::NotFound(format!("scan job id {job_id}")));
    }
    Ok(())
}

/// Set the estimated total entry count produced by the scanner's pre-walk
/// counting phase. The frontend uses this to derive `progress_percent`.
pub fn update_scan_job_estimated_total(
    conn: &Connection,
    job_id: i64,
    estimated_total: i64,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE scan_jobs SET estimated_total_count = ?1 WHERE id = ?2",
        params![estimated_total, job_id],
    )?;
    if n == 0 {
        return Err(DbError::NotFound(format!("scan job id {job_id}")));
    }
    Ok(())
}

pub fn finish_scan_job(
    conn: &Connection,
    job_id: i64,
    status: ScanJobStatus,
    error_message: Option<&str>,
) -> Result<()> {
    let now = now_iso();
    let n = conn.execute(
        "UPDATE scan_jobs
         SET status = ?1, finished_at = ?2, error_message = ?3
         WHERE id = ?4",
        params![status, now, error_message, job_id],
    )?;
    if n == 0 {
        return Err(DbError::NotFound(format!("scan job id {job_id}")));
    }
    Ok(())
}

/// Fetch the most recent `scan_jobs` row for a given root, if any.
///
/// Used by the `get_scan_status` command to answer "what is this root doing
/// right now?" — returns `Ok(None)` when the root has never been scanned.
pub fn get_latest_scan_job_for_root(
    conn: &Connection,
    root_id: i64,
) -> Result<Option<ScanJob>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM scan_jobs
         WHERE root_id = ?1
         ORDER BY id DESC
         LIMIT 1"
    );
    let job = conn.query_row(&sql, params![root_id], map_row).optional()?;
    Ok(job)
}

pub fn get_scan_job(conn: &Connection, job_id: i64) -> Result<ScanJob> {
    let sql = format!("SELECT {SELECT_COLS} FROM scan_jobs WHERE id = ?1");
    let job = conn
        .query_row(&sql, params![job_id], map_row)
        .optional()?
        .ok_or_else(|| DbError::NotFound(format!("scan job id {job_id}")))?;
    Ok(job)
}

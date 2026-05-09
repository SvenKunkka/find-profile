use rusqlite::{params, Connection, OptionalExtension};

use super::error::{DbError, Result};
use super::model::{ScanJob, ScanJobStatus};
use super::util::now_iso;

pub fn create_scan_job(conn: &Connection, root_id: i64) -> Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO scan_jobs (root_id, status, scanned_count, error_count, started_at)
         VALUES (?1, ?2, 0, 0, ?3)",
        params![root_id, ScanJobStatus::Running, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_scan_job_progress(
    conn: &Connection,
    job_id: i64,
    scanned_count: i64,
    error_count: i64,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE scan_jobs SET scanned_count = ?1, error_count = ?2 WHERE id = ?3",
        params![scanned_count, error_count, job_id],
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

pub fn get_scan_job(conn: &Connection, job_id: i64) -> Result<ScanJob> {
    let job = conn
        .query_row(
            "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
             FROM scan_jobs WHERE id = ?1",
            params![job_id],
            |row| {
                Ok(ScanJob {
                    id: row.get(0)?,
                    root_id: row.get(1)?,
                    status: row.get(2)?,
                    scanned_count: row.get(3)?,
                    error_count: row.get(4)?,
                    started_at: row.get(5)?,
                    finished_at: row.get(6)?,
                    error_message: row.get(7)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound(format!("scan job id {job_id}")))?;
    Ok(job)
}

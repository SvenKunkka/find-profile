use chrono::Utc;
use rusqlite::{params, Row};
use crate::db::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl ScanJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanJobStatus::Pending => "pending",
            ScanJobStatus::Running => "running",
            ScanJobStatus::Completed => "completed",
            ScanJobStatus::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(ScanJobStatus::Pending),
            "running" => Some(ScanJobStatus::Running),
            "completed" => Some(ScanJobStatus::Completed),
            "failed" => Some(ScanJobStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanJob {
    pub id: i64,
    pub root_id: i64,
    pub status: ScanJobStatus,
    pub scanned_count: i64,
    pub error_count: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error_message: Option<String>,
}

fn scan_job_from_row(row: &Row) -> Result<ScanJob, rusqlite::Error> {
    let status_str: String = row.get(2)?;
    let status = ScanJobStatus::from_str(&status_str).ok_or(rusqlite::Error::InvalidQuery)?;

    Ok(ScanJob {
        id: row.get(0)?,
        root_id: row.get(1)?,
        status,
        scanned_count: row.get(3)?,
        error_count: row.get(4)?,
        started_at: row.get(5)?,
        finished_at: row.get(6)?,
        error_message: row.get(7)?,
    })
}

pub fn create_scan_job(db: &Database, root_id: i64) -> DbResult<ScanJob> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    conn.execute(
        "INSERT INTO scan_jobs (root_id, status, scanned_count, error_count, started_at) VALUES (?1, 'pending', 0, 0, ?2)",
        params![root_id, now],
    )?;

    let id = conn.last_insert_rowid();
    drop(conn);

    get_scan_job_by_id(db, id)
}

pub fn get_scan_job_by_id(db: &Database, id: i64) -> DbResult<ScanJob> {
    let conn = db.conn.lock().unwrap();
    get_scan_job_by_id_internal(&conn, id)
}

fn get_scan_job_by_id_internal(conn: &rusqlite::Connection, id: i64) -> DbResult<ScanJob> {
    let mut stmt = conn.prepare(
        "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
         FROM scan_jobs WHERE id = ?1"
    )?;

    stmt.query_row(params![id], scan_job_from_row)
        .map_err(DbError::Sqlite)
}

pub fn update_scan_job_progress(db: &Database, job_id: i64, scanned_count: i64, error_count: i64) -> DbResult<()> {
    let conn = db.conn.lock().unwrap();

    let rows_affected = conn.execute(
        "UPDATE scan_jobs SET scanned_count = ?1, error_count = ?2 WHERE id = ?3 AND status = 'pending'",
        params![scanned_count, error_count, job_id],
    )?;

    drop(conn);

    if rows_affected == 0 {
        return Err(DbError::NotFound(format!("ScanJob with id {} not found or not in pending state", job_id)));
    }

    Ok(())
}

pub fn start_scan_job(db: &Database, job_id: i64) -> DbResult<()> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    let rows_affected = conn.execute(
        "UPDATE scan_jobs SET status = 'running', started_at = ?1 WHERE id = ?2 AND status = 'pending'",
        params![now, job_id],
    )?;

    drop(conn);

    if rows_affected == 0 {
        return Err(DbError::NotFound(format!("ScanJob with id {} not found or not in pending state", job_id)));
    }

    Ok(())
}

pub fn finish_scan_job(
    db: &Database,
    job_id: i64,
    status: ScanJobStatus,
    error_message: Option<String>,
) -> DbResult<()> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    let rows_affected = conn.execute(
        "UPDATE scan_jobs SET status = ?1, finished_at = ?2, error_message = ?3 WHERE id = ?4 AND status = 'running'",
        params![status.as_str(), now, error_message, job_id],
    )?;

    drop(conn);

    if rows_affected == 0 {
        return Err(DbError::NotFound(format!("ScanJob with id {} not found or not in running state", job_id)));
    }

    Ok(())
}

pub fn get_latest_scan_job(db: &Database, root_id: i64) -> DbResult<Option<ScanJob>> {
    let conn = db.conn.lock().unwrap();

    let mut stmt = conn.prepare(
        "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
         FROM scan_jobs WHERE root_id = ?1 ORDER BY id DESC LIMIT 1"
    )?;

    let result = stmt.query_row(params![root_id], scan_job_from_row);

    match result {
        Ok(job) => Ok(Some(job)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(DbError::Sqlite(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::db::Database;
    use crate::db::roots::{add_root, RootKind};
    use tempfile::tempdir;

    fn create_test_db() -> Arc<Database> {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::new(db_path).unwrap();
        crate::db::schema::init_schema(&db.conn.lock().unwrap()).unwrap();
        Arc::new(db)
    }

    fn create_test_root(db: &Database) -> i64 {
        let root = add_root(db, "/test", RootKind::Local).unwrap();
        root.id
    }

    #[test]
    fn test_create_scan_job() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let job = create_scan_job(&db, root_id).unwrap();
        assert_eq!(job.root_id, root_id);
        assert_eq!(job.status, ScanJobStatus::Pending);
        assert_eq!(job.scanned_count, 0);
        assert_eq!(job.error_count, 0);
    }

    #[test]
    fn test_update_scan_job_progress() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let job = create_scan_job(&db, root_id).unwrap();
        update_scan_job_progress(&db, job.id, 100, 5).unwrap();

        let conn = db.conn.lock().unwrap();
        let updated: ScanJob = {
            let mut stmt = conn.prepare(
                "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
                 FROM scan_jobs WHERE id = ?1"
            ).unwrap();
            stmt.query_row(params![job.id], scan_job_from_row).unwrap()
        };
        drop(conn);

        assert_eq!(updated.scanned_count, 100);
        assert_eq!(updated.error_count, 5);
    }

    #[test]
    fn test_finish_scan_job_completed() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let job = create_scan_job(&db, root_id).unwrap();
        start_scan_job(&db, job.id).unwrap();
        finish_scan_job(&db, job.id, ScanJobStatus::Completed, None).unwrap();

        let conn = db.conn.lock().unwrap();
        let finished: ScanJob = {
            let mut stmt = conn.prepare(
                "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
                 FROM scan_jobs WHERE id = ?1"
            ).unwrap();
            stmt.query_row(params![job.id], scan_job_from_row).unwrap()
        };
        drop(conn);

        assert_eq!(finished.status, ScanJobStatus::Completed);
        assert!(finished.finished_at.is_some());
    }

    #[test]
    fn test_finish_scan_job_failed() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let job = create_scan_job(&db, root_id).unwrap();
        start_scan_job(&db, job.id).unwrap();
        finish_scan_job(
            &db,
            job.id,
            ScanJobStatus::Failed,
            Some("Permission denied".to_string()),
        ).unwrap();

        let conn = db.conn.lock().unwrap();
        let failed: ScanJob = {
            let mut stmt = conn.prepare(
                "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
                 FROM scan_jobs WHERE id = ?1"
            ).unwrap();
            stmt.query_row(params![job.id], scan_job_from_row).unwrap()
        };
        drop(conn);

        assert_eq!(failed.status, ScanJobStatus::Failed);
        assert_eq!(failed.error_message, Some("Permission denied".to_string()));
    }

    #[test]
    fn test_update_nonexistent_scan_job() {
        let db = create_test_db();

        let result = update_scan_job_progress(&db, 9999, 100, 5);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_latest_scan_job() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let job1 = create_scan_job(&db, root_id).unwrap();
        let job2 = create_scan_job(&db, root_id).unwrap();

        let latest = get_latest_scan_job(&db, root_id).unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.id, job2.id);
        assert!(latest.id > job1.id);
    }

    #[test]
    fn test_start_scan_job() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let job = create_scan_job(&db, root_id).unwrap();
        start_scan_job(&db, job.id).unwrap();

        let conn = db.conn.lock().unwrap();
        let started: ScanJob = {
            let mut stmt = conn.prepare(
                "SELECT id, root_id, status, scanned_count, error_count, started_at, finished_at, error_message
                 FROM scan_jobs WHERE id = ?1"
            ).unwrap();
            stmt.query_row(params![job.id], scan_job_from_row).unwrap()
        };
        drop(conn);

        assert_eq!(started.status, ScanJobStatus::Running);
        assert!(started.started_at.is_some());
    }
}
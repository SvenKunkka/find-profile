use chrono::Utc;
use rusqlite::{params, Row};
use crate::db::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootKind {
    Local,
    Nas,
}

impl RootKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RootKind::Local => "local",
            RootKind::Nas => "nas",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "local" => Some(RootKind::Local),
            "nas" => Some(RootKind::Nas),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootStatus {
    Available,
    Disconnected,
    Scanning,
    Error,
}

impl RootStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RootStatus::Available => "available",
            RootStatus::Disconnected => "disconnected",
            RootStatus::Scanning => "scanning",
            RootStatus::Error => "error",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "available" => Some(RootStatus::Available),
            "disconnected" => Some(RootStatus::Disconnected),
            "scanning" => Some(RootStatus::Scanning),
            "error" => Some(RootStatus::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Root {
    pub id: i64,
    pub path: String,
    pub kind: RootKind,
    pub enabled: bool,
    pub status: RootStatus,
    pub scan_interval: i64,
    pub last_scanned_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn root_from_row(row: &Row) -> Result<Root, rusqlite::Error> {
    let kind_str: String = row.get(2).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let status_str: String = row.get(4).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let kind = RootKind::from_str(&kind_str).ok_or(rusqlite::Error::InvalidQuery)?;
    let status = RootStatus::from_str(&status_str).ok_or(rusqlite::Error::InvalidQuery)?;

    Ok(Root {
        id: row.get(0)?,
        path: row.get(1)?,
        kind,
        enabled: row.get::<_, i64>(3)? != 0,
        status,
        scan_interval: row.get(5)?,
        last_scanned_at: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub fn add_root(db: &Database, path: &str, kind: RootKind) -> DbResult<Root> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    conn.execute(
        "INSERT INTO roots (path, kind, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
        params![path, kind.as_str(), now, now],
    )?;

    let id = conn.last_insert_rowid();

    let mut stmt = conn.prepare(
        "SELECT id, path, kind, enabled, status, scan_interval, last_scanned_at, created_at, updated_at FROM roots WHERE id = ?1"
    )?;
    stmt.query_row(params![id], root_from_row)
        .map_err(DbError::Sqlite)
}

pub fn get_root_by_id(db: &Database, id: i64) -> DbResult<Root> {
    let conn = db.conn.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, path, kind, enabled, status, scan_interval, last_scanned_at, created_at, updated_at FROM roots WHERE id = ?1"
    )?;
    stmt.query_row(params![id], root_from_row)
        .map_err(DbError::Sqlite)
}

pub fn update_root_status(db: &Database, root_id: i64, status: RootStatus) -> DbResult<()> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    let rows_affected = conn.execute(
        "UPDATE roots SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status.as_str(), now, root_id],
    )?;

    drop(conn);

    if rows_affected == 0 {
        return Err(DbError::NotFound(format!("Root with id {} not found", root_id)));
    }

    Ok(())
}

pub fn list_roots(db: &Database) -> DbResult<Vec<Root>> {
    let conn = db.conn.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, path, kind, enabled, status, scan_interval, last_scanned_at, created_at, updated_at FROM roots ORDER BY id"
    )?;

    let roots = stmt.query_map([], root_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DbError::Sqlite)?;

    drop(stmt);
    drop(conn);
    Ok(roots)
}

pub fn update_root_last_scanned(db: &Database, root_id: i64) -> DbResult<()> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    conn.execute(
        "UPDATE roots SET last_scanned_at = ?1, updated_at = ?1 WHERE id = ?2",
        params![now, root_id],
    )?;

    drop(conn);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::db::Database;
    use tempfile::tempdir;

    fn create_test_db() -> Arc<Database> {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::new(db_path).unwrap();
        crate::db::schema::init_schema(&db.conn.lock().unwrap()).unwrap();
        Arc::new(db)
    }

    #[test]
    fn test_add_and_get_root() {
        let db = create_test_db();
        let root = add_root(&db, "/test/path", RootKind::Local).unwrap();
        assert_eq!(root.path, "/test/path");
        assert_eq!(root.kind, RootKind::Local);
        assert_eq!(root.status, RootStatus::Available);
    }

    #[test]
    fn test_add_nas_root() {
        let db = create_test_db();
        let root = add_root(&db, "/Volumes/NAS/data", RootKind::Nas).unwrap();
        assert_eq!(root.kind, RootKind::Nas);
    }

    #[test]
    fn test_update_root_status() {
        let db = create_test_db();
        let root = add_root(&db, "/test/path", RootKind::Local).unwrap();

        update_root_status(&db, root.id, RootStatus::Scanning).unwrap();
        let updated = get_root_by_id(&db, root.id).unwrap();
        assert_eq!(updated.status, RootStatus::Scanning);

        update_root_status(&db, root.id, RootStatus::Available).unwrap();
        let updated = get_root_by_id(&db, root.id).unwrap();
        assert_eq!(updated.status, RootStatus::Available);
    }

    #[test]
    fn test_list_roots() {
        let db = create_test_db();
        add_root(&db, "/path1", RootKind::Local).unwrap();
        add_root(&db, "/path2", RootKind::Nas).unwrap();

        let roots = list_roots(&db).unwrap();
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_update_nonexistent_root_status() {
        let db = create_test_db();
        let result = update_root_status(&db, 9999, RootStatus::Error);
        assert!(result.is_err());
    }
}
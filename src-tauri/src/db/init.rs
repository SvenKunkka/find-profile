use crate::db::{Database, DbResult};
use std::path::PathBuf;

pub fn init_db(db_path: Option<PathBuf>) -> DbResult<Database> {
    let path = db_path.unwrap_or_else(|| {
        let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("find-profile");
        path.push("find-profile.db");
        path
    });

    let db = Database::new(path)?;
    let conn = db.conn.lock().unwrap();
    crate::db::schema::init_schema(&conn)?;
    drop(conn);

    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_init_db_with_custom_path() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let db = init_db(Some(db_path.clone())).unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='roots'", [], |row| row.get(0))
            .unwrap();
        drop(conn);

        assert_eq!(count, 1);
    }
}
use crate::db::{Database, DbResult};
use rusqlite::Connection;

pub struct IndexTransaction<'a> {
    conn: &'a Connection,
}

impl<'a> IndexTransaction<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn insert_file(&self, metadata: &crate::db::files::FileMetadata) -> DbResult<()> {
        crate::db::files::upsert_file_in_transaction(self.conn, metadata)
    }

    pub fn commit(self) -> DbResult<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn rollback(self) -> DbResult<()> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }
}

pub fn begin_index_transaction(conn: &Connection) -> DbResult<IndexTransaction<'_>> {
    conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;
    Ok(IndexTransaction::new(conn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::db::files::{FileMetadata, SourceKind};
    use crate::db::roots::{add_root, RootKind};
    use crate::db::Database;
    use std::sync::Arc;
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
    fn test_begin_index_transaction() {
        let db = create_test_db();
        let conn_guard = db.conn.lock().unwrap();
        let conn: &Connection = &conn_guard;
        let _tx = begin_index_transaction(conn).unwrap();
    }

    #[test]
    fn test_batch_insert_in_transaction() {
        let db = create_test_db();
        let root_id = create_test_root(&db);
        let conn_guard = db.conn.lock().unwrap();
        let conn: &Connection = &conn_guard;

        {
            let tx = begin_index_transaction(conn).unwrap();

            for i in 0..5 {
                let metadata = FileMetadata::new(
                    root_id,
                    format!("/test/file{}.txt", i),
                    format!("file{}.txt", i),
                    Some("txt".to_string()),
                    1024,
                    Utc::now().timestamp(),
                    false,
                    SourceKind::Local,
                );
                tx.insert_file(&metadata).unwrap();
            }

            tx.commit().unwrap();
        }

        drop(conn_guard);

        let results = crate::db::files::search_files(&db, "file", None, None).unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_rollback_transaction() {
        let db = create_test_db();
        let root_id = create_test_root(&db);
        let conn_guard = db.conn.lock().unwrap();
        let conn: &Connection = &conn_guard;

        {
            let tx = begin_index_transaction(conn).unwrap();

            let metadata = FileMetadata::new(
                root_id,
                "/test/rollback.txt".to_string(),
                "rollback.txt".to_string(),
                Some("txt".to_string()),
                1024,
                Utc::now().timestamp(),
                false,
                SourceKind::Local,
            );
            tx.insert_file(&metadata).unwrap();

            tx.rollback().unwrap();
        }

        drop(conn_guard);

        let results = crate::db::files::search_files(&db, "rollback", None, None).unwrap();
        assert_eq!(results.len(), 0);
    }
}
use chrono::Utc;
use rusqlite::{params, Row};
use crate::db::{Database, DbError, DbResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    Local,
    Nas,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Local => "local",
            SourceKind::Nas => "nas",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "local" => Some(SourceKind::Local),
            "nas" => Some(SourceKind::Nas),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub root_id: i64,
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub size: i64,
    pub modified_at: i64,
    pub is_dir: bool,
    pub source_kind: SourceKind,
    pub is_accessible: bool,
}

impl FileMetadata {
    pub fn new(
        root_id: i64,
        path: String,
        name: String,
        extension: Option<String>,
        size: i64,
        modified_at: i64,
        is_dir: bool,
        source_kind: SourceKind,
    ) -> Self {
        Self {
            root_id,
            path,
            name,
            extension,
            size,
            modified_at,
            is_dir,
            source_kind,
            is_accessible: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: i64,
    pub root_id: i64,
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub size: i64,
    pub modified_at: i64,
    pub is_dir: bool,
    pub source_kind: SourceKind,
    pub is_accessible: bool,
    pub indexed_at: i64,
}

fn file_record_from_row(row: &Row) -> Result<FileRecord, rusqlite::Error> {
    let source_kind_str: String = row.get(8).map_err(|e| match e {
        rusqlite::Error::InvalidColumnIndex(_) | rusqlite::Error::InvalidColumnName(_) => {
            rusqlite::Error::InvalidQuery
        }
        _ => e,
    })?;
    let source_kind = SourceKind::from_str(&source_kind_str).ok_or(rusqlite::Error::InvalidQuery)?;

    Ok(FileRecord {
        id: row.get(0)?,
        root_id: row.get(1)?,
        path: row.get(2)?,
        name: row.get(3)?,
        extension: row.get(4)?,
        size: row.get(5)?,
        modified_at: row.get(6)?,
        is_dir: row.get::<_, i64>(7)? != 0,
        source_kind,
        is_accessible: row.get::<_, i64>(9)? != 0,
        indexed_at: row.get(10)?,
    })
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub name: String,
    pub path: String,
    pub extension: Option<String>,
    pub size: i64,
    pub modified_at: i64,
    pub is_dir: bool,
    pub source_kind: SourceKind,
    pub is_accessible: bool,
}

fn search_result_from_row(row: &Row) -> Result<SearchResult, rusqlite::Error> {
    let is_dir_i64: i64 = row.get(5)?;
    let source_kind_str: String = row.get(6)?;
    let is_accessible_i64: i64 = row.get(7)?;
    let source_kind = SourceKind::from_str(&source_kind_str).ok_or(rusqlite::Error::InvalidQuery)?;

    Ok(SearchResult {
        name: row.get(0)?,
        path: row.get(1)?,
        extension: row.get(2)?,
        size: row.get(3)?,
        modified_at: row.get(4)?,
        is_dir: is_dir_i64 != 0,
        source_kind,
        is_accessible: is_accessible_i64 != 0,
    })
}

pub fn upsert_file(db: &Database, metadata: &FileMetadata) -> DbResult<()> {
    let now = Utc::now().timestamp();
    let conn = db.conn.lock().unwrap();

    conn.execute(
        "INSERT INTO files (root_id, path, name, extension, size, modified_at, is_dir, source_kind, is_accessible, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(path) DO UPDATE SET
             name = excluded.name,
             extension = excluded.extension,
             size = excluded.size,
             modified_at = excluded.modified_at,
             is_dir = excluded.is_dir,
             source_kind = excluded.source_kind,
             is_accessible = excluded.is_accessible,
             indexed_at = excluded.indexed_at",
        params![
            metadata.root_id,
            metadata.path,
            metadata.name,
            metadata.extension,
            metadata.size,
            metadata.modified_at,
            metadata.is_dir as i64,
            metadata.source_kind.as_str(),
            metadata.is_accessible as i64,
            now,
        ],
    )?;

    drop(conn);
    Ok(())
}

pub fn upsert_file_in_transaction(conn: &rusqlite::Connection, metadata: &FileMetadata) -> DbResult<()> {
    let now = Utc::now().timestamp();

    conn.execute(
        "INSERT INTO files (root_id, path, name, extension, size, modified_at, is_dir, source_kind, is_accessible, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(path) DO UPDATE SET
             name = excluded.name,
             extension = excluded.extension,
             size = excluded.size,
             modified_at = excluded.modified_at,
             is_dir = excluded.is_dir,
             source_kind = excluded.source_kind,
             is_accessible = excluded.is_accessible,
             indexed_at = excluded.indexed_at",
        params![
            metadata.root_id,
            metadata.path,
            metadata.name,
            metadata.extension,
            metadata.size,
            metadata.modified_at,
            metadata.is_dir as i64,
            metadata.source_kind.as_str(),
            metadata.is_accessible as i64,
            now,
        ],
    )?;

    Ok(())
}

pub fn remove_file(db: &Database, path: &str) -> DbResult<()> {
    let conn = db.conn.lock().unwrap();
    conn.execute("DELETE FROM files WHERE path = ?1", params![path])?;
    drop(conn);
    Ok(())
}

pub fn search_files(
    db: &Database,
    query: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> DbResult<Vec<SearchResult>> {
    let conn = db.conn.lock().unwrap();
    let limit = limit.unwrap_or(100);
    let offset = offset.unwrap_or(0);

    let fts_query = format!("{}*", query.replace('"', "\"\""));

    let mut stmt = conn.prepare(
        "SELECT f.name, f.path, f.extension, f.size, f.modified_at, f.is_dir, f.source_kind, f.is_accessible
         FROM files f
         JOIN files_fts fts ON f.id = fts.rowid
         WHERE files_fts MATCH ?1
         ORDER BY f.modified_at DESC
         LIMIT ?2 OFFSET ?3"
    )?;

    let results = stmt.query_map(params![fts_query, limit as i64, offset as i64], search_result_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DbError::Sqlite)?;

    drop(stmt);
    drop(conn);
    Ok(results)
}

pub fn get_file_by_path(db: &Database, path: &str) -> DbResult<Option<FileRecord>> {
    let conn = db.conn.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, root_id, path, name, extension, size, modified_at, is_dir, source_kind, is_accessible, indexed_at
         FROM files WHERE path = ?1"
    )?;

    let result = stmt.query_row(params![path], file_record_from_row);

    match result {
        Ok(file) => Ok(Some(file)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(DbError::Sqlite(e)),
    }
}

pub fn remove_files_by_root(db: &Database, root_id: i64) -> DbResult<u64> {
    let conn = db.conn.lock().unwrap();
    let rows_affected = conn.execute("DELETE FROM files WHERE root_id = ?1", params![root_id])?;
    drop(conn);
    Ok(rows_affected as u64)
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
    fn test_upsert_file() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let metadata = FileMetadata::new(
            root_id,
            "/test/file.txt".to_string(),
            "file.txt".to_string(),
            Some("txt".to_string()),
            1024,
            Utc::now().timestamp(),
            false,
            SourceKind::Local,
        );

        upsert_file(&db, &metadata).unwrap();

        let results = search_files(&db, "file", None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "file.txt");
        assert_eq!(results[0].path, "/test/file.txt");
    }

    #[test]
    fn test_upsert_file_update() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let metadata1 = FileMetadata::new(
            root_id,
            "/test/file.txt".to_string(),
            "file.txt".to_string(),
            Some("txt".to_string()),
            1024,
            Utc::now().timestamp(),
            false,
            SourceKind::Local,
        );

        upsert_file(&db, &metadata1).unwrap();

        let metadata2 = FileMetadata::new(
            root_id,
            "/test/file.txt".to_string(),
            "file.txt".to_string(),
            Some("txt".to_string()),
            2048,
            Utc::now().timestamp(),
            false,
            SourceKind::Local,
        );

        upsert_file(&db, &metadata2).unwrap();

        let results = search_files(&db, "file", None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size, 2048);
    }

    #[test]
    fn test_remove_file() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let metadata = FileMetadata::new(
            root_id,
            "/test/file.txt".to_string(),
            "file.txt".to_string(),
            Some("txt".to_string()),
            1024,
            Utc::now().timestamp(),
            false,
            SourceKind::Local,
        );

        upsert_file(&db, &metadata).unwrap();
        remove_file(&db, "/test/file.txt").unwrap();

        let results = search_files(&db, "file", None, None).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_search_files_with_prefix() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let metadata = FileMetadata::new(
            root_id,
            "/test/document.pdf".to_string(),
            "document.pdf".to_string(),
            Some("pdf".to_string()),
            2048,
            Utc::now().timestamp(),
            false,
            SourceKind::Local,
        );

        upsert_file(&db, &metadata).unwrap();

        let results = search_files(&db, "doc", None, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "document.pdf");
    }

    #[test]
    fn test_search_files_limit() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

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
            upsert_file(&db, &metadata).unwrap();
        }

        let results = search_files(&db, "file", Some(3), None).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_search_files_offset() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

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
            upsert_file(&db, &metadata).unwrap();
        }

        let results = search_files(&db, "file", Some(3), Some(2)).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_remove_files_by_root() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        for i in 0..3 {
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
            upsert_file(&db, &metadata).unwrap();
        }

        let count = remove_files_by_root(&db, root_id).unwrap();
        assert_eq!(count, 3);

        let results = search_files(&db, "file", None, None).unwrap();
        assert_eq!(results.len(), 0);
    }
}
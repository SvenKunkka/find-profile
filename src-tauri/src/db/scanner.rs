use crate::db::files::{FileMetadata, SourceKind};
use crate::db::scan_jobs::{self, ScanJobStatus};
use crate::db::roots::RootStatus;
use crate::db::{Database, DbResult};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub struct Scanner {
    db: Arc<Database>,
    root_id: i64,
    job_id: Mutex<Option<i64>>,
    root_path: String,
    source_kind: SourceKind,
    scanned_count: Mutex<i64>,
    error_count: Mutex<i64>,
    current_path: Mutex<String>,
}

impl Scanner {
    pub fn new(
        db: Arc<Database>,
        root_id: i64,
        root_path: String,
        source_kind: SourceKind,
    ) -> Self {
        Self {
            db,
            root_id,
            job_id: Mutex::new(None),
            root_path,
            source_kind,
            scanned_count: Mutex::new(0),
            error_count: Mutex::new(0),
            current_path: Mutex::new(String::new()),
        }
    }

    pub fn scan(&self) -> DbResult<()> {
        let root_path = Path::new(&self.root_path);

        if !root_path.exists() {
            return self.handle_root_not_found();
        }

        crate::db::roots::update_root_status(&self.db, self.root_id, RootStatus::Scanning)?;

        let job = self.create_scan_job()?;

        let result = self.scan_directory(root_path);

        self.finish_scan_job(job.id, &result);

        result
    }

    fn handle_root_not_found(&self) -> DbResult<()> {
        let status = match self.source_kind {
            SourceKind::Local => RootStatus::Error,
            SourceKind::Nas => RootStatus::Disconnected,
        };

        crate::db::roots::update_root_status(&self.db, self.root_id, status)?;

        Err(crate::db::DbError::NotFound(format!(
            "Root path does not exist: {}",
            self.root_path
        )))
    }

    fn create_scan_job(&self) -> DbResult<crate::db::scan_jobs::ScanJob> {
        let job = scan_jobs::create_scan_job(&self.db, self.root_id)?;
        scan_jobs::start_scan_job(&self.db, job.id)?;
        *self.job_id.lock().unwrap() = Some(job.id);
        Ok(job)
    }

    fn finish_scan_job(&self, job_id: i64, result: &DbResult<()>) {
        let status = match result {
            Ok(()) => ScanJobStatus::Completed,
            Err(_) => ScanJobStatus::Failed,
        };

        let error_message = match result {
            Ok(()) => None,
            Err(e) => Some(e.to_string()),
        };

        let _ = scan_jobs::finish_scan_job(&self.db, job_id, status, error_message);

        let final_status = match result {
            Ok(()) => RootStatus::Available,
            Err(_) => RootStatus::Error,
        };
        let _ = crate::db::roots::update_root_status(&self.db, self.root_id, final_status);
    }

    fn scan_directory(&self, dir: &Path) -> DbResult<()> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                self.record_error(dir);
                return Ok(());
            }
        };

        let mut batch: Vec<FileMetadata> = Vec::with_capacity(100);
        let batch_size = 100;

        for entry in entries.flatten() {
            let path = entry.path();
            let path_str = path.to_string_lossy().to_string();

            *self.current_path.lock().unwrap() = path_str.clone();

            if let Ok(metadata) = self.collect_file_metadata(&path) {
                batch.push(metadata);

                if batch.len() >= batch_size {
                    self.write_batch(&batch)?;
                    batch.clear();
                }
            }

            if path.is_dir() {
                if let Err(e) = self.scan_directory(&path) {
                    if self.is_unrecoverable_error(&e) {
                        return Err(e);
                    }
                }
            }
        }

        if !batch.is_empty() {
            self.write_batch(&batch)?;
        }

        Ok(())
    }

    fn collect_file_metadata(&self, path: &Path) -> Result<FileMetadata, std::io::Error> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let full_path = path.to_string_lossy().to_string();

        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_string());

        let metadata = fs::metadata(path)?;

        let size = metadata.len() as i64;

        let modified_at = metadata
            .modified()
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        let is_dir = metadata.is_dir();

        Ok(FileMetadata::new(
            self.root_id,
            full_path,
            name,
            extension,
            size,
            modified_at,
            is_dir,
            self.source_kind.clone(),
        ))
    }

    fn write_batch(&self, batch: &[FileMetadata]) -> DbResult<()> {
        {
            let conn = self.db.conn.lock().unwrap();

            conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;

            for metadata in batch {
                if let Err(e) = crate::db::files::upsert_file_in_transaction(&conn, metadata) {
                    conn.execute_batch("ROLLBACK")?;
                    return Err(e);
                }
            }

            conn.execute_batch("COMMIT")?;
        }

        *self.scanned_count.lock().unwrap() += batch.len() as i64;

        if let Some(job_id) = *self.job_id.lock().unwrap() {
            scan_jobs::update_scan_job_progress(
                &self.db,
                job_id,
                *self.scanned_count.lock().unwrap(),
                *self.error_count.lock().unwrap(),
            ).ok();
        }

        Ok(())
    }

    fn record_error(&self, path: &Path) {
        *self.error_count.lock().unwrap() += 1;

        let path_str = path.to_string_lossy().to_string();
        *self.current_path.lock().unwrap() = path_str;
    }

    fn is_unrecoverable_error(&self, e: &crate::db::DbError) -> bool {
        matches!(e, crate::db::DbError::NotFound(_))
    }

    pub fn get_progress(&self) -> ScanProgress {
        ScanProgress {
            scanned_count: *self.scanned_count.lock().unwrap(),
            error_count: *self.error_count.lock().unwrap(),
            current_path: self.current_path.lock().unwrap().clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub scanned_count: i64,
    pub error_count: i64,
    pub current_path: String,
}

pub fn scan_root(
    db: Arc<Database>,
    root_id: i64,
    root_path: String,
    source_kind: SourceKind,
) -> DbResult<()> {
    let scanner = Scanner::new(db, root_id, root_path, source_kind);
    scanner.scan()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::roots::{add_root, RootKind};
    use tempfile::{tempdir, TempDir};

    fn create_test_db() -> Arc<Database> {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::new(db_path).unwrap();
        crate::db::schema::init_schema(&db.conn.lock().unwrap()).unwrap();
        Arc::new(db)
    }

    fn create_temp_structure(temp_dir: &TempDir) -> std::path::PathBuf {
        let root = temp_dir.path().to_path_buf();

        std::fs::create_dir_all(root.join("subdir1")).unwrap();
        std::fs::create_dir_all(root.join("subdir2")).unwrap();

        std::fs::write(root.join("file1.txt"), "content1").unwrap();
        std::fs::write(root.join("file2.pdf"), "content2").unwrap();
        std::fs::write(root.join("subdir1/nested.txt"), "nested content").unwrap();

        root
    }

    #[test]
    fn test_scan_local_root() {
        let db = create_test_db();
        let temp_dir = tempdir().unwrap();
        let root_path = create_temp_structure(&temp_dir);

        let root_id = add_root(&db, root_path.to_string_lossy().as_ref(), RootKind::Local).unwrap();

        let result = scan_root(
            db.clone(),
            root_id.id,
            root_path.to_string_lossy().to_string(),
            SourceKind::Local,
        );

        assert!(result.is_ok());

        let roots = crate::db::roots::list_roots(&db).unwrap();
        assert_eq!(roots[0].status, RootStatus::Available);

        let scanned_count = crate::db::scan_jobs::get_latest_scan_job(&db, root_id.id)
            .unwrap().unwrap().scanned_count;
        assert_eq!(scanned_count, 5);
    }

    #[test]
    fn test_scan_nonexistent_local_root() {
        let db = create_test_db();
        let root_id = create_test_root(&db);

        let result = scan_root(
            db.clone(),
            root_id,
            "/nonexistent/path/12345".to_string(),
            SourceKind::Local,
        );

        assert!(result.is_err());

        let roots = crate::db::roots::list_roots(&db).unwrap();
        assert_eq!(roots[0].status, RootStatus::Error);
    }

    #[test]
    fn test_scan_nonexistent_nas_root() {
        let db = create_test_db();

        let root = add_root(&db, "/nas/nonexistent", RootKind::Nas).unwrap();

        let result = scan_root(
            db.clone(),
            root.id,
            "/nas/nonexistent".to_string(),
            SourceKind::Nas,
        );

        assert!(result.is_err());

        let roots = crate::db::roots::list_roots(&db).unwrap();
        assert_eq!(roots[0].status, RootStatus::Disconnected);
    }

    #[test]
    fn test_scan_with_files_indexed() {
        let db = create_test_db();
        let temp_dir = tempdir().unwrap();
        let root_path = create_temp_structure(&temp_dir);

        let root_id = add_root(&db, root_path.to_string_lossy().as_ref(), RootKind::Local).unwrap();

        scan_root(
            db.clone(),
            root_id.id,
            root_path.to_string_lossy().to_string(),
            SourceKind::Local,
        ).unwrap();

        let results = crate::db::files::search_files(&db, "file", None, None).unwrap();
        assert_eq!(results.len(), 2);

        let results = crate::db::files::search_files(&db, "nested", None, None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_scan_progress_tracking() {
        let db = create_test_db();
        let temp_dir = tempdir().unwrap();
        let root_path = create_temp_structure(&temp_dir);

        let root_id = add_root(&db, root_path.to_string_lossy().as_ref(), RootKind::Local).unwrap();

        let scanner = Scanner::new(
            db.clone(),
            root_id.id,
            root_path.to_string_lossy().to_string(),
            SourceKind::Local,
        );

        scanner.scan().unwrap();

        let progress = scanner.get_progress();
        assert_eq!(progress.scanned_count, 5);
        assert_eq!(progress.error_count, 0);
    }

    fn create_test_root(db: &Database) -> i64 {
        let root = add_root(db, "/test", RootKind::Local).unwrap();
        root.id
    }
}
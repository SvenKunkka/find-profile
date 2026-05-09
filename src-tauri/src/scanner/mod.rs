pub mod error;
mod extract;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db::{self, DbError, FileMetadata, RootKind, RootStatus, ScanJobStatus};
use crate::roots::{decide_status_from_accessibility, probe_path_accessibility};

pub use error::{Result, ScanError};
use extract::extract_metadata;

const BATCH_SIZE: usize = 500;

#[derive(Debug, Clone)]
pub struct ScanInput {
    pub root_id: i64,
    pub root_path: String,
    pub source_kind: RootKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub job_id: i64,
    pub scanned_count: i64,
    pub error_count: i64,
    pub current_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanOutcome {
    pub job_id: i64,
    pub status: ScanJobStatus,
    pub scanned_count: i64,
    pub error_count: i64,
    pub final_root_status: RootStatus,
    pub error_message: Option<String>,
}

/// Run a full scan for one root.
///
/// Lifecycle:
///  1. Create a `scan_jobs` row (status=running).
///  2. Probe the root path. If unreachable, finish the job as failed and
///     transition the root to `disconnected` (NAS) or `error` (local). The
///     `files` table is **not** touched — old index entries must remain
///     searchable while a NAS volume is offline.
///  3. Otherwise, mark the root as `scanning`, walk recursively (no symlink
///     following), and batch-upsert into `files` (and via triggers,
///     `files_fts`) inside transactions sized at `BATCH_SIZE`.
///  4. Per-batch and at end-of-scan, update `scan_jobs.scanned_count` /
///     `error_count` and invoke `on_progress` with the latest snapshot.
///  5. Finish the job as success and transition the root to `available`.
///
/// Errors encountered during walking (permission denied, dangling symlink,
/// metadata read failures) are counted into `error_count` and skipped — the
/// scan never panics on individual entries.
pub fn scan(
    conn: &mut Connection,
    input: ScanInput,
    mut on_progress: impl FnMut(&Progress),
) -> Result<ScanOutcome> {
    let job_id = db::create_scan_job(conn, input.root_id)?;

    if !probe_path_accessibility(&input.root_path) {
        let new_status = decide_status_from_accessibility(input.source_kind, false);
        let msg = format!("root path not accessible: {}", input.root_path);
        db::finish_scan_job(conn, job_id, ScanJobStatus::Failed, Some(&msg))?;
        db::update_root_status(conn, input.root_id, new_status)?;
        return Ok(ScanOutcome {
            job_id,
            status: ScanJobStatus::Failed,
            scanned_count: 0,
            error_count: 0,
            final_root_status: new_status,
            error_message: Some(msg),
        });
    }

    db::update_root_status(conn, input.root_id, RootStatus::Scanning)?;

    let mut buffer: Vec<FileMetadata> = Vec::with_capacity(BATCH_SIZE);
    let mut scanned: i64 = 0;
    let mut errors: i64 = 0;
    let mut current_path: Option<String> = None;

    let walker = WalkDir::new(&input.root_path)
        .follow_links(false)
        .min_depth(1);

    for entry_result in walker {
        match entry_result {
            Ok(entry) => {
                current_path = Some(entry.path().to_string_lossy().into_owned());
                match extract_metadata(&entry, input.root_id, input.source_kind) {
                    Some(m) => {
                        buffer.push(m);
                        scanned += 1;
                    }
                    None => {
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                current_path = e.path().map(|p| p.to_string_lossy().into_owned());
                errors += 1;
            }
        }

        if buffer.len() >= BATCH_SIZE {
            flush(conn, &mut buffer)?;
            db::update_scan_job_progress(conn, job_id, scanned, errors)?;
            on_progress(&Progress {
                job_id,
                scanned_count: scanned,
                error_count: errors,
                current_path: current_path.clone(),
            });
        }
    }

    flush(conn, &mut buffer)?;
    db::update_scan_job_progress(conn, job_id, scanned, errors)?;
    on_progress(&Progress {
        job_id,
        scanned_count: scanned,
        error_count: errors,
        current_path: current_path.clone(),
    });

    db::finish_scan_job(conn, job_id, ScanJobStatus::Success, None)?;
    db::update_root_status(conn, input.root_id, RootStatus::Available)?;

    Ok(ScanOutcome {
        job_id,
        status: ScanJobStatus::Success,
        scanned_count: scanned,
        error_count: errors,
        final_root_status: RootStatus::Available,
        error_message: None,
    })
}

fn flush(conn: &mut Connection, buffer: &mut Vec<FileMetadata>) -> Result<()> {
    if buffer.is_empty() {
        return Ok(());
    }
    let tx = db::begin_index_transaction(conn)?;
    for m in buffer.iter() {
        db::upsert_file_in_transaction(&tx, m)?;
    }
    tx.commit().map_err(|e| ScanError::Db(DbError::from(e)))?;
    buffer.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::db::{init_in_memory_db, search_files, upsert_file};
    use crate::roots::register_root;

    fn unique_test_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "find_profile_scanner_test_{}_{}_{}",
            label,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn unique_missing_path(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(
            "{prefix}__find_profile_scanner_missing_{}_{}__",
            std::process::id(),
            nanos
        )
    }

    fn cleanup(p: &PathBuf) {
        let _ = fs::remove_dir_all(p);
    }

    fn cached_row(root_id: i64, path: &str, name: &str, kind: RootKind) -> FileMetadata {
        FileMetadata {
            root_id,
            path: path.to_string(),
            name: name.to_string(),
            extension: name.rsplit_once('.').map(|(_, e)| e.to_lowercase()),
            size: Some(1),
            modified_at: None,
            is_dir: false,
            source_kind: kind,
            is_accessible: true,
        }
    }

    #[test]
    fn scan_indexes_files_and_dirs_search_works() {
        let dir = unique_test_dir("happy");
        // tree:
        //   dir/foo.txt
        //   dir/notes/
        //   dir/notes/bar.md
        fs::write(dir.join("foo.txt"), b"hello").unwrap();
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(dir.join("notes").join("bar.md"), b"world").unwrap();

        let mut conn = init_in_memory_db().unwrap();
        let root = register_root(&conn, dir.to_str().unwrap(), Some("local")).unwrap();

        let outcome = scan(
            &mut conn,
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.status, ScanJobStatus::Success);
        assert_eq!(outcome.error_count, 0);
        assert!(
            outcome.scanned_count >= 3,
            "expected ≥3 entries (foo.txt + notes/ + bar.md), got {}",
            outcome.scanned_count
        );
        assert_eq!(outcome.final_root_status, RootStatus::Available);

        let foo = search_files(&conn, "foo", 10, 0).unwrap();
        assert!(foo.iter().any(|h| h.name == "foo.txt" && !h.is_dir));
        assert!(foo.iter().any(|h| h.extension.as_deref() == Some("txt")));

        let bar = search_files(&conn, "bar", 10, 0).unwrap();
        assert!(bar.iter().any(|h| h.name == "bar.md" && !h.is_dir));
        assert!(bar.iter().any(|h| h.extension.as_deref() == Some("md")));

        // Folder is also indexed with is_dir=true.
        let notes = search_files(&conn, "notes", 10, 0).unwrap();
        assert!(
            notes.iter().any(|h| h.is_dir && h.name == "notes"),
            "notes/ must be indexed as a directory entry; got {notes:?}"
        );

        cleanup(&dir);
    }

    #[test]
    fn scan_creates_scan_job_record_with_final_state() {
        let dir = unique_test_dir("job");
        fs::write(dir.join("z.txt"), b"z").unwrap();

        let mut conn = init_in_memory_db().unwrap();
        let root = register_root(&conn, dir.to_str().unwrap(), Some("local")).unwrap();

        let outcome = scan(
            &mut conn,
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            |_| {},
        )
        .unwrap();

        let row: (String, i64, i64, Option<String>) = conn
            .query_row(
                "SELECT status, scanned_count, error_count, finished_at
                 FROM scan_jobs WHERE id = ?1",
                [outcome.job_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "success");
        assert_eq!(row.1, outcome.scanned_count);
        assert_eq!(row.2, outcome.error_count);
        assert!(row.3.is_some(), "finished_at must be populated");

        cleanup(&dir);
    }

    #[test]
    fn scan_invokes_progress_callback() {
        let dir = unique_test_dir("progress");
        fs::write(dir.join("a.txt"), b"a").unwrap();

        let mut conn = init_in_memory_db().unwrap();
        let root = register_root(&conn, dir.to_str().unwrap(), Some("local")).unwrap();

        let mut last: Option<Progress> = None;
        let mut calls = 0u32;
        scan(
            &mut conn,
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            |p| {
                calls += 1;
                last = Some(p.clone());
            },
        )
        .unwrap();

        assert!(calls >= 1, "progress callback must be invoked at least once");
        let p = last.unwrap();
        assert!(p.scanned_count >= 1);
        assert_eq!(p.error_count, 0);
        cleanup(&dir);
    }

    #[test]
    fn missing_local_root_marks_error_and_keeps_old_index() {
        let mut conn = init_in_memory_db().unwrap();
        let bogus = unique_missing_path("/tmp/");
        assert!(
            fs::metadata(&bogus).is_err(),
            "test fixture path must not exist on host"
        );

        let root = register_root(&conn, &bogus, Some("local")).unwrap();
        upsert_file(
            &conn,
            &cached_row(root.id, &format!("{bogus}/old.txt"), "old.txt", RootKind::Local),
        )
        .unwrap();

        let outcome = scan(
            &mut conn,
            ScanInput {
                root_id: root.id,
                root_path: bogus.clone(),
                source_kind: RootKind::Local,
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.status, ScanJobStatus::Failed);
        assert_eq!(outcome.final_root_status, RootStatus::Error);
        assert!(outcome.error_message.is_some());

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "old index must NOT be deleted on a failed scan");
        let hits = search_files(&conn, "old", 10, 0).unwrap();
        assert!(hits.iter().any(|h| h.name == "old.txt"));

        // The failed scan_job is recorded with a structured error message.
        let (status, msg): (String, Option<String>) = conn
            .query_row(
                "SELECT status, error_message FROM scan_jobs WHERE id = ?1",
                [outcome.job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert!(msg.unwrap().contains("not accessible"));
    }

    #[test]
    fn missing_nas_root_marks_disconnected_and_keeps_old_index() {
        let mut conn = init_in_memory_db().unwrap();
        let nas = unique_missing_path("/Volumes/");
        assert!(fs::metadata(&nas).is_err());

        let root = register_root(&conn, &nas, None).unwrap(); // auto-detect → nas
        assert_eq!(root.kind, RootKind::Nas);

        upsert_file(
            &conn,
            &cached_row(
                root.id,
                &format!("{nas}/cached.bin"),
                "cached.bin",
                RootKind::Nas,
            ),
        )
        .unwrap();

        let outcome = scan(
            &mut conn,
            ScanInput {
                root_id: root.id,
                root_path: nas.clone(),
                source_kind: RootKind::Nas,
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.status, ScanJobStatus::Failed);
        assert_eq!(
            outcome.final_root_status,
            RootStatus::Disconnected,
            "NAS unreachable must transition to Disconnected, not Error"
        );

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "cached NAS index must NEVER be cleared on disconnect");
        let hits = search_files(&conn, "cached", 10, 0).unwrap();
        assert!(hits.iter().any(|h| h.name == "cached.bin"));
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_panic_on_dangling_symlink() {
        let dir = unique_test_dir("symlink");
        fs::write(dir.join("real.txt"), b"r").unwrap();
        // Create a symlink pointing at something that doesn't exist.
        std::os::unix::fs::symlink(
            dir.join("__missing_target_for_dangling_symlink__"),
            dir.join("dangling"),
        )
        .unwrap();

        let mut conn = init_in_memory_db().unwrap();
        let root = register_root(&conn, dir.to_str().unwrap(), Some("local")).unwrap();
        let outcome = scan(
            &mut conn,
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            |_| {},
        )
        .unwrap();

        // The scan must complete; whether the symlink counts as scanned vs error
        // is platform-dependent — both are acceptable as long as we didn't panic.
        assert_eq!(outcome.status, ScanJobStatus::Success);
        assert!(outcome.scanned_count + outcome.error_count >= 1);

        // The real file must always be indexed regardless of how the symlink is handled.
        let real = search_files(&conn, "real", 10, 0).unwrap();
        assert!(real.iter().any(|h| h.name == "real.txt"));

        cleanup(&dir);
    }

    #[test]
    fn rescan_upserts_without_duplicating_rows() {
        let dir = unique_test_dir("rescan");
        fs::write(dir.join("a.txt"), b"a").unwrap();

        let mut conn = init_in_memory_db().unwrap();
        let root = register_root(&conn, dir.to_str().unwrap(), Some("local")).unwrap();
        let input = ScanInput {
            root_id: root.id,
            root_path: root.path.clone(),
            source_kind: root.kind,
        };
        let first = scan(&mut conn, input.clone(), |_| {}).unwrap();
        let second = scan(&mut conn, input, |_| {}).unwrap();
        assert_eq!(first.status, ScanJobStatus::Success);
        assert_eq!(second.status, ScanJobStatus::Success);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, first.scanned_count, "second scan must upsert in place");

        cleanup(&dir);
    }
}

pub mod cancel;
pub mod error;
mod extract;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::db::{self, DbError, FileMetadata, RootKind, RootStatus, ScanJobStatus};
use crate::roots::{decide_status_from_accessibility, probe_path_accessibility};

pub use cancel::ScanCancellations;
pub use error::{Result, ScanError};
use extract::extract_metadata;

/// Number of file metadata rows we accumulate before committing a transaction.
/// Tuned for first-pass throughput on macOS APFS — 500 keeps individual
/// commits short (a few ms) while amortising fixed transaction overhead.
const BATCH_SIZE: usize = 500;

/// Frequency (in scanned+errored entries) at which `scan_jobs.scanned_count`
/// and the `on_progress` callback fire. Lower = more responsive UI but more
/// write contention with concurrent searches.
const PROGRESS_DB_INTERVAL: i64 = 2500;

/// Wall-clock floor for progress emission, so a slow filesystem still produces
/// regular UI updates even before `PROGRESS_DB_INTERVAL` is hit.
const PROGRESS_TIME_MS: u128 = 500;

#[derive(Debug, Clone)]
pub struct ScanInput {
    pub root_id: i64,
    pub root_path: String,
    pub source_kind: RootKind,
}

/// Per-scan inclusion filters, derived from app settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanFilters {
    pub include_files: bool,
    pub include_folders: bool,
    pub show_hidden: bool,
}

impl Default for ScanFilters {
    fn default() -> Self {
        Self {
            include_files: true,
            include_folders: true,
            show_hidden: false,
        }
    }
}

fn is_hidden_basename(name: &str) -> bool {
    // Treat any dot-prefixed filename as hidden on every platform; this is
    // the convention macOS/Linux respect and what Finder hides by default.
    matches!(name.chars().next(), Some('.'))
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

/// Run a non-cancellable scan. Equivalent to `scan_cancellable` with a token
/// that is never set.
pub fn scan(
    conn: Arc<Mutex<Connection>>,
    input: ScanInput,
    on_progress: impl FnMut(&Progress),
) -> Result<ScanOutcome> {
    scan_cancellable(
        conn,
        input,
        ScanFilters::default(),
        Arc::new(AtomicBool::new(false)),
        on_progress,
    )
}

/// Run a full scan with cooperative cancellation.
///
/// Concurrency model: the connection mutex is acquired per batch (a few ms
/// at a time) rather than for the whole scan. Between batches the walker
/// runs without the lock, which lets `search_files`, `list_index_roots`,
/// `get_scan_status`, and the watcher's event handler interleave freely.
///
/// Cancellation: `cancel.load(Relaxed)` is checked at the top of every
/// iteration of the walker, so the worker stops within one entry of the
/// `cancel_scan` command. On cancel, partially-indexed rows are kept (no
/// rollback), `scan_jobs.status` becomes `cancelled` with a summary message,
/// and the root transitions back to `available`.
///
/// Error aggregation: per-entry walker errors and metadata-read failures
/// increment `error_count` instead of aborting the scan. On completion the
/// final `scan_jobs` row carries either `Some("scan completed with N errors")`
/// or `None`, never raw per-event noise.
pub fn scan_cancellable(
    conn: Arc<Mutex<Connection>>,
    input: ScanInput,
    filters: ScanFilters,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(&Progress),
) -> Result<ScanOutcome> {
    tracing::info!(
        root_id = input.root_id,
        root_path = %input.root_path,
        kind = ?input.source_kind,
        filters = ?filters,
        "scan starting",
    );
    let job_id = {
        let g = lock(&conn)?;
        db::create_scan_job(&g, input.root_id)?
    };

    if !probe_path_accessibility(&input.root_path) {
        let new_status = decide_status_from_accessibility(input.source_kind, false);
        let msg = format!("root path not accessible: {}", input.root_path);
        tracing::warn!(root_id = input.root_id, ?new_status, "{}", msg);
        let g = lock(&conn)?;
        db::finish_scan_job(&g, job_id, ScanJobStatus::Failed, Some(&msg))?;
        db::update_root_status(&g, input.root_id, new_status)?;
        return Ok(ScanOutcome {
            job_id,
            status: ScanJobStatus::Failed,
            scanned_count: 0,
            error_count: 0,
            final_root_status: new_status,
            error_message: Some(msg),
        });
    }

    {
        let g = lock(&conn)?;
        db::update_root_status(&g, input.root_id, RootStatus::Scanning)?;
    }

    let mut buffer: Vec<FileMetadata> = Vec::with_capacity(BATCH_SIZE);
    let mut scanned: i64 = 0;
    let mut errors: i64 = 0;
    let mut current_path: Option<String> = None;
    let mut last_progress_count: i64 = 0;
    let mut last_progress_at = Instant::now();

    let walker = WalkDir::new(&input.root_path)
        .follow_links(false)
        .min_depth(1);

    let mut cancelled = false;
    for entry_result in walker {
        // Cancellation check at the very top of each iteration so the worker
        // exits within one walkdir entry of the user clicking Cancel.
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }

        match entry_result {
            Ok(entry) => {
                current_path = Some(entry.path().to_string_lossy().into_owned());

                // Cheap basename filter before stat() — if the entry is hidden
                // and the user disabled hidden files, drop it without paying
                // the metadata read cost.
                if !filters.show_hidden {
                    let name = entry.file_name().to_string_lossy();
                    if is_hidden_basename(&name) {
                        continue;
                    }
                }

                match extract_metadata(&entry, input.root_id, input.source_kind) {
                    Some(m) => {
                        if m.is_dir && !filters.include_folders {
                            continue;
                        }
                        if !m.is_dir && !filters.include_files {
                            continue;
                        }
                        buffer.push(m);
                        scanned += 1;
                    }
                    None => errors += 1,
                }
            }
            Err(e) => {
                current_path = e.path().map(|p| p.to_string_lossy().into_owned());
                errors += 1;
            }
        }

        if buffer.len() >= BATCH_SIZE {
            let total_now = scanned + errors;
            let should_emit_progress = (total_now - last_progress_count)
                >= PROGRESS_DB_INTERVAL
                || last_progress_at.elapsed().as_millis() >= PROGRESS_TIME_MS;
            {
                let mut g = lock(&conn)?;
                flush_locked(&mut g, &mut buffer)?;
                if should_emit_progress {
                    db::update_scan_job_progress(&g, job_id, scanned, errors)?;
                }
            }
            if should_emit_progress {
                on_progress(&Progress {
                    job_id,
                    scanned_count: scanned,
                    error_count: errors,
                    current_path: current_path.clone(),
                });
                last_progress_count = total_now;
                last_progress_at = Instant::now();
            }
        }
    }

    // Final flush + a guaranteed terminal progress emission.
    {
        let mut g = lock(&conn)?;
        flush_locked(&mut g, &mut buffer)?;
        db::update_scan_job_progress(&g, job_id, scanned, errors)?;
    }
    on_progress(&Progress {
        job_id,
        scanned_count: scanned,
        error_count: errors,
        current_path: current_path.clone(),
    });

    let g = lock(&conn)?;
    if cancelled {
        let msg = format!(
            "cancelled by user after {scanned} indexed entr{} ({errors} error{})",
            if scanned == 1 { "y" } else { "ies" },
            if errors == 1 { "" } else { "s" },
        );
        tracing::info!(root_id = input.root_id, scanned, errors, "scan cancelled");
        db::finish_scan_job(&g, job_id, ScanJobStatus::Cancelled, Some(&msg))?;
        db::update_root_status(&g, input.root_id, RootStatus::Available)?;
        return Ok(ScanOutcome {
            job_id,
            status: ScanJobStatus::Cancelled,
            scanned_count: scanned,
            error_count: errors,
            final_root_status: RootStatus::Available,
            error_message: Some(msg),
        });
    }

    let final_msg = if errors > 0 {
        Some(format!(
            "scan completed: {scanned} indexed, {errors} skipped"
        ))
    } else {
        None
    };
    tracing::info!(
        root_id = input.root_id,
        scanned,
        errors,
        "scan finished",
    );
    db::finish_scan_job(&g, job_id, ScanJobStatus::Success, final_msg.as_deref())?;
    db::update_root_status(&g, input.root_id, RootStatus::Available)?;

    Ok(ScanOutcome {
        job_id,
        status: ScanJobStatus::Success,
        scanned_count: scanned,
        error_count: errors,
        final_root_status: RootStatus::Available,
        error_message: final_msg,
    })
}

fn lock(
    conn: &Arc<Mutex<Connection>>,
) -> Result<std::sync::MutexGuard<'_, Connection>> {
    conn.lock().map_err(|_| ScanError::PoisonedLock)
}

fn flush_locked(conn: &mut Connection, buffer: &mut Vec<FileMetadata>) -> Result<()> {
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::db::{init_in_memory_db, search_files, upsert_file, Root};
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

    fn make_conn() -> Arc<Mutex<Connection>> {
        Arc::new(Mutex::new(init_in_memory_db().unwrap()))
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
        fs::write(dir.join("foo.txt"), b"hello").unwrap();
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(dir.join("notes").join("bar.md"), b"world").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };

        let outcome = scan(
            conn.clone(),
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
        assert!(outcome.scanned_count >= 3);
        assert_eq!(outcome.final_root_status, RootStatus::Available);

        let g = conn.lock().unwrap();
        let foo = search_files(&g, "foo", 10, 0).unwrap();
        assert!(foo.iter().any(|h| h.name == "foo.txt" && !h.is_dir));
        let bar = search_files(&g, "bar", 10, 0).unwrap();
        assert!(bar.iter().any(|h| h.name == "bar.md"));
        let notes = search_files(&g, "notes", 10, 0).unwrap();
        assert!(notes.iter().any(|h| h.is_dir && h.name == "notes"));

        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn scan_creates_scan_job_record_with_final_state() {
        let dir = unique_test_dir("job");
        fs::write(dir.join("z.txt"), b"z").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let outcome = scan(
            conn.clone(),
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            |_| {},
        )
        .unwrap();

        let g = conn.lock().unwrap();
        let row: (String, i64, i64, Option<String>) = g
            .query_row(
                "SELECT status, scanned_count, error_count, finished_at
                 FROM scan_jobs WHERE id = ?1",
                [outcome.job_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "success");
        assert_eq!(row.1, outcome.scanned_count);
        assert!(row.3.is_some());

        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn scan_invokes_progress_callback() {
        let dir = unique_test_dir("progress");
        fs::write(dir.join("a.txt"), b"a").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };

        let mut last: Option<Progress> = None;
        let mut calls = 0u32;
        scan(
            conn.clone(),
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
        // Final progress is always emitted even for tiny scans.
        assert!(calls >= 1);
        let p = last.unwrap();
        assert!(p.scanned_count >= 1);
        assert_eq!(p.error_count, 0);

        cleanup(&dir);
    }

    #[test]
    fn missing_local_root_marks_error_and_keeps_old_index() {
        let conn = make_conn();
        let bogus = unique_missing_path("/tmp/");
        assert!(fs::metadata(&bogus).is_err());

        let root = {
            let g = conn.lock().unwrap();
            let r = register_root(&g, &bogus, Some("local")).unwrap();
            upsert_file(
                &g,
                &cached_row(r.id, &format!("{bogus}/old.txt"), "old.txt", RootKind::Local),
            )
            .unwrap();
            r
        };

        let outcome = scan(
            conn.clone(),
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

        let g = conn.lock().unwrap();
        let count: i64 = g
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "old index must be preserved");
    }

    #[test]
    fn missing_nas_root_marks_disconnected_and_keeps_old_index() {
        let conn = make_conn();
        let nas = unique_missing_path("/Volumes/");
        assert!(fs::metadata(&nas).is_err());

        let root = {
            let g = conn.lock().unwrap();
            let r = register_root(&g, &nas, None).unwrap();
            assert_eq!(r.kind, RootKind::Nas);
            upsert_file(
                &g,
                &cached_row(
                    r.id,
                    &format!("{nas}/cached.bin"),
                    "cached.bin",
                    RootKind::Nas,
                ),
            )
            .unwrap();
            r
        };

        let outcome = scan(
            conn.clone(),
            ScanInput {
                root_id: root.id,
                root_path: nas.clone(),
                source_kind: RootKind::Nas,
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.status, ScanJobStatus::Failed);
        assert_eq!(outcome.final_root_status, RootStatus::Disconnected);

        let g = conn.lock().unwrap();
        let count: i64 = g
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_panic_on_dangling_symlink() {
        let dir = unique_test_dir("symlink");
        fs::write(dir.join("real.txt"), b"r").unwrap();
        std::os::unix::fs::symlink(
            dir.join("__missing_target_for_dangling_symlink__"),
            dir.join("dangling"),
        )
        .unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let outcome = scan(
            conn.clone(),
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.status, ScanJobStatus::Success);
        assert!(outcome.scanned_count + outcome.error_count >= 1);

        let g = conn.lock().unwrap();
        let real = search_files(&g, "real", 10, 0).unwrap();
        assert!(real.iter().any(|h| h.name == "real.txt"));

        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn rescan_upserts_without_duplicating_rows() {
        let dir = unique_test_dir("rescan");
        fs::write(dir.join("a.txt"), b"a").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let input = ScanInput {
            root_id: root.id,
            root_path: root.path.clone(),
            source_kind: root.kind,
        };
        let first = scan(conn.clone(), input.clone(), |_| {}).unwrap();
        let second = scan(conn.clone(), input, |_| {}).unwrap();
        assert_eq!(first.status, ScanJobStatus::Success);
        assert_eq!(second.status, ScanJobStatus::Success);

        let g = conn.lock().unwrap();
        let count: i64 = g
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, first.scanned_count);

        drop(g);
        cleanup(&dir);
    }

    // --- cancellation ---------------------------------------------------

    #[test]
    fn pre_cancelled_scan_finishes_with_cancelled_status_and_no_writes() {
        // Pre-cancel before scan starts: the very first iteration of the
        // walker hits the cancel check and breaks out before any upsert.
        let dir = unique_test_dir("pre_cancel");
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::write(dir.join("b.txt"), b"b").unwrap();
        fs::write(dir.join("c.txt"), b"c").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };

        let cancel = Arc::new(AtomicBool::new(true));
        let outcome = scan_cancellable(
            conn.clone(),
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            ScanFilters::default(),
            cancel,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.status, ScanJobStatus::Cancelled);
        assert_eq!(outcome.final_root_status, RootStatus::Available);
        assert!(outcome
            .error_message
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("cancel"));

        let g = conn.lock().unwrap();
        let count: i64 = g
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "pre-cancelled scan must not have indexed anything");

        let job_status: String = g
            .query_row(
                "SELECT status FROM scan_jobs WHERE id = ?1",
                [outcome.job_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(job_status, "cancelled");

        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn cancelled_scan_can_be_followed_by_a_successful_rescan() {
        let dir = unique_test_dir("cancel_then_rescan");
        for i in 0..5 {
            fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let input = ScanInput {
            root_id: root.id,
            root_path: root.path.clone(),
            source_kind: root.kind,
        };

        // First run: pre-cancelled → no writes, status=Cancelled, root back
        // to Available so a follow-up scan can run.
        let cancel = Arc::new(AtomicBool::new(true));
        let first = scan_cancellable(
            conn.clone(),
            input.clone(),
            ScanFilters::default(),
            cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(first.status, ScanJobStatus::Cancelled);
        assert_eq!(first.final_root_status, RootStatus::Available);

        // Second run: no cancel → completes normally.
        let second = scan(conn.clone(), input, |_| {}).unwrap();
        assert_eq!(second.status, ScanJobStatus::Success);
        assert!(second.scanned_count >= 5);

        cleanup(&dir);
    }

    // --- filter tests ---------------------------------------------------

    fn run_with_filters(
        conn: Arc<Mutex<Connection>>,
        root: &Root,
        filters: ScanFilters,
    ) -> ScanOutcome {
        scan_cancellable(
            conn,
            ScanInput {
                root_id: root.id,
                root_path: root.path.clone(),
                source_kind: root.kind,
            },
            filters,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap()
    }

    #[test]
    fn filter_default_indexes_visible_files_and_dirs() {
        let dir = unique_test_dir("filter_default");
        fs::write(dir.join("visible.txt"), b"v").unwrap();
        fs::create_dir_all(dir.join("subdir")).unwrap();
        fs::write(dir.join(".hidden_file"), b"h").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let outcome = run_with_filters(conn.clone(), &root, ScanFilters::default());
        // visible.txt + subdir = 2 (hidden file dropped)
        assert_eq!(outcome.scanned_count, 2);

        let g = conn.lock().unwrap();
        assert!(search_files(&g, "visible", 10, 0)
            .unwrap()
            .iter()
            .any(|h| h.name == "visible.txt"));
        assert!(search_files(&g, "subdir", 10, 0)
            .unwrap()
            .iter()
            .any(|h| h.is_dir));
        assert!(search_files(&g, "hidden", 10, 0).unwrap().is_empty());

        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn filter_show_hidden_includes_dotfiles() {
        let dir = unique_test_dir("filter_show_hidden");
        fs::write(dir.join(".hidden_file"), b"h").unwrap();
        fs::write(dir.join("plain.txt"), b"p").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let outcome = run_with_filters(
            conn.clone(),
            &root,
            ScanFilters {
                show_hidden: true,
                ..ScanFilters::default()
            },
        );
        assert_eq!(outcome.scanned_count, 2);
        let g = conn.lock().unwrap();
        assert!(!search_files(&g, "hidden", 10, 0).unwrap().is_empty());
        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn filter_excludes_files() {
        let dir = unique_test_dir("filter_no_files");
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::create_dir_all(dir.join("only_dir")).unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let outcome = run_with_filters(
            conn.clone(),
            &root,
            ScanFilters {
                include_files: false,
                ..ScanFilters::default()
            },
        );
        assert_eq!(outcome.scanned_count, 1, "only the directory should be indexed");
        let g = conn.lock().unwrap();
        let hits = search_files(&g, "txt", 10, 0).unwrap();
        assert!(hits.iter().all(|h| h.is_dir));
        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn filter_excludes_folders() {
        let dir = unique_test_dir("filter_no_folders");
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::create_dir_all(dir.join("only_dir")).unwrap();
        fs::write(dir.join("only_dir").join("nested.txt"), b"n").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let outcome = run_with_filters(
            conn.clone(),
            &root,
            ScanFilters {
                include_folders: false,
                ..ScanFilters::default()
            },
        );
        // a.txt + nested.txt — directory itself is excluded, but walker still
        // recurses into it so its children are indexed.
        assert_eq!(outcome.scanned_count, 2);
        let g = conn.lock().unwrap();
        let dirs: Vec<_> = search_files(&g, "only", 10, 0).unwrap();
        assert!(
            dirs.iter().all(|h| !h.is_dir),
            "no directory entries should be indexed"
        );
        drop(g);
        cleanup(&dir);
    }

    #[test]
    fn cancellation_is_decoupled_from_path_unreachability() {
        // If a scan was cancelled but the root path is fine, the next scan
        // must reach Available (not Disconnected/Error).
        let dir = unique_test_dir("cancel_path_ok");
        fs::write(dir.join("a.txt"), b"a").unwrap();

        let conn = make_conn();
        let root = {
            let g = conn.lock().unwrap();
            register_root(&g, dir.to_str().unwrap(), Some("local")).unwrap()
        };
        let input = ScanInput {
            root_id: root.id,
            root_path: root.path.clone(),
            source_kind: root.kind,
        };

        let cancel = Arc::new(AtomicBool::new(true));
        scan_cancellable(
            conn.clone(),
            input.clone(),
            ScanFilters::default(),
            cancel,
            |_| {},
        )
        .unwrap();

        let g = conn.lock().unwrap();
        let status: String = g
            .query_row(
                "SELECT status FROM roots WHERE id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        drop(g);
        assert_eq!(status, "available");
        cleanup(&dir);
    }
}

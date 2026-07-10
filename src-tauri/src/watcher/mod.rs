pub mod error;
mod handler;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use notify::{recommended_watcher, Event, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::Connection;

use crate::db::{self, Root};
use crate::roots::should_watch_root;

pub use error::{Result, WatcherError};

use handler::handle_event;

/// Holds the live `notify` watchers, keyed by `root.id`.
///
/// Each `RecommendedWatcher` runs on a `notify`-owned background thread; the
/// registry just owns the handles so that they don't drop and tear down
/// prematurely. Dropping a value out of the map automatically stops the
/// underlying watcher.
pub struct WatcherRegistry {
    inner: Mutex<HashMap<i64, RecommendedWatcher>>,
}

impl Default for WatcherRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WatcherRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Start watching a single root if its kind warrants live updates.
    ///
    /// NAS roots are skipped silently and the function returns `Ok(false)` —
    /// see `roots::should_watch_root` for the rationale (FSEvents/inotify on
    /// SMB/AFP/NFS is unreliable). Watching an already-watched root replaces
    /// the previous handle.
    pub fn start(&self, root: &Root, conn: Arc<Mutex<Connection>>) -> Result<bool> {
        if !should_watch_root(root.kind) {
            return Ok(false);
        }
        let root_id = root.id;
        let source_kind = root.kind;
        let handler_conn = conn.clone();
        let mut watcher = recommended_watcher(move |res: notify::Result<Event>| {
            handle_event(res, root_id, source_kind, &handler_conn);
        })?;
        watcher.watch(Path::new(&root.path), RecursiveMode::Recursive)?;
        let mut guard = self.inner.lock().map_err(|_| WatcherError::PoisonedLock)?;
        guard.insert(root_id, watcher);
        Ok(true)
    }

    /// Stop watching a root. Idempotent — calling on an unknown id is a no-op.
    pub fn stop(&self, root_id: i64) -> Result<()> {
        let mut guard = self.inner.lock().map_err(|_| WatcherError::PoisonedLock)?;
        guard.remove(&root_id);
        Ok(())
    }

    /// Number of currently active watchers. Mainly useful for tests.
    pub fn active_count(&self) -> Result<usize> {
        let guard = self.inner.lock().map_err(|_| WatcherError::PoisonedLock)?;
        Ok(guard.len())
    }

    /// Restore watchers for every `enabled && local` root in the database.
    ///
    /// Per-root failures are logged via stderr and do not abort the rest of
    /// the restore — the app's invariant is that watcher problems never
    /// block startup. Returns the number of watchers successfully started.
    pub fn restore_all(&self, conn: Arc<Mutex<Connection>>) -> Result<usize> {
        let roots = {
            let conn_guard = conn.lock().map_err(|_| WatcherError::PoisonedLock)?;
            db::list_roots(&conn_guard)?
        };

        let mut count = 0;
        for root in roots {
            if !root.enabled {
                continue;
            }
            if !should_watch_root(root.kind) {
                continue;
            }
            match self.start(&root, conn.clone()) {
                Ok(true) => count += 1,
                Ok(false) => {} // already filtered, but defensive
                Err(e) => {
                    eprintln!(
                        "watcher: failed to restore root {} ({}): {e}",
                        root.id, root.path
                    );
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::db::{init_in_memory_db, search_files, upsert_file, FileMetadata, RootKind};
    use crate::roots::register_root;

    fn unique_test_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "find_profile_watcher_test_{label}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&p).unwrap();
        // macOS: `std::env::temp_dir()` is `/var/folders/...` which is a
        // symlink to `/private/var/folders/...`. FSEvents reports events
        // using the canonical (resolved) path, so we canonicalize up front
        // to keep the root path, pre-populated rows, and watcher event paths
        // all in the same form.
        p.canonicalize().expect("canonicalize test dir")
    }

    fn cleanup(p: &PathBuf) {
        let _ = fs::remove_dir_all(p);
    }

    fn make_conn() -> Arc<Mutex<Connection>> {
        Arc::new(Mutex::new(init_in_memory_db().unwrap()))
    }

    fn add_root(conn_arc: &Arc<Mutex<Connection>>, path: &str, kind: &str) -> Root {
        let conn = conn_arc.lock().unwrap();
        register_root(&conn, path, Some(kind)).unwrap()
    }

    /// Poll an in-memory DB for a search hit until `deadline`. Returns true
    /// when the predicate is satisfied. Used to absorb the FSEvents / inotify
    /// dispatch latency without sleeping a fixed (potentially-flaky) amount.
    fn wait_for<F>(
        conn_arc: &Arc<Mutex<Connection>>,
        query: &str,
        deadline: Instant,
        mut f: F,
    ) -> bool
    where
        F: FnMut(&[crate::db::FileHit]) -> bool,
    {
        while Instant::now() < deadline {
            let hits = {
                let conn = conn_arc.lock().unwrap();
                search_files(&conn, query, 50, 0).unwrap()
            };
            if f(&hits) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        false
    }

    // --- deterministic policy / registry tests ---------------------------

    #[test]
    fn registry_skips_nas_roots_silently() {
        let dir = unique_test_dir("nas_skip");
        let conn_arc = make_conn();
        let root = add_root(&conn_arc, dir.to_str().unwrap(), "nas");
        let registry = WatcherRegistry::new();
        let started = registry.start(&root, conn_arc.clone()).unwrap();
        assert!(!started, "NAS roots must never be watched");
        assert_eq!(registry.active_count().unwrap(), 0);
        cleanup(&dir);
    }

    #[test]
    fn registry_starts_then_stops_local_root() {
        let dir = unique_test_dir("local_start_stop");
        let conn_arc = make_conn();
        let root = add_root(&conn_arc, dir.to_str().unwrap(), "local");
        let registry = WatcherRegistry::new();

        let started = registry.start(&root, conn_arc.clone()).unwrap();
        assert!(started);
        assert_eq!(registry.active_count().unwrap(), 1);

        registry.stop(root.id).unwrap();
        assert_eq!(registry.active_count().unwrap(), 0);

        // stop is idempotent
        registry.stop(root.id).unwrap();
        cleanup(&dir);
    }

    #[test]
    fn restore_all_includes_local_excludes_nas() {
        let local_dir = unique_test_dir("restore_local");
        let nas_dir = unique_test_dir("restore_nas");
        let conn_arc = make_conn();
        add_root(&conn_arc, local_dir.to_str().unwrap(), "local");
        add_root(&conn_arc, nas_dir.to_str().unwrap(), "nas");

        let registry = WatcherRegistry::new();
        let n = registry.restore_all(conn_arc.clone()).unwrap();
        assert_eq!(n, 1, "only the local root should be watched");
        assert_eq!(registry.active_count().unwrap(), 1);

        cleanup(&local_dir);
        cleanup(&nas_dir);
    }

    #[test]
    fn restore_all_skips_disabled_roots() {
        let dir = unique_test_dir("restore_disabled");
        let conn_arc = make_conn();
        let root = add_root(&conn_arc, dir.to_str().unwrap(), "local");
        {
            let conn = conn_arc.lock().unwrap();
            db::disable_root(&conn, root.id).unwrap();
        }

        let registry = WatcherRegistry::new();
        let n = registry.restore_all(conn_arc.clone()).unwrap();
        assert_eq!(n, 0, "disabled roots must not be watched");
        cleanup(&dir);
    }

    // --- live FS event tests --------------------------------------------
    //
    // These exercise real notify watchers against the OS file system. Event
    // delivery on macOS FSEvents has a small but variable latency (a few
    // hundred ms typical, occasionally up to ~1 s), so we poll with a wide
    // deadline rather than sleep a fixed amount. Tests use a unique temp dir
    // each run so they don't conflict with each other when run in parallel.

    #[test]
    fn watcher_indexes_newly_created_file() {
        let dir = unique_test_dir("create");
        let conn_arc = make_conn();
        let root = add_root(&conn_arc, dir.to_str().unwrap(), "local");
        let registry = WatcherRegistry::new();
        registry.start(&root, conn_arc.clone()).unwrap();
        // Give the OS a moment to subscribe before issuing changes.
        std::thread::sleep(Duration::from_millis(200));

        fs::write(dir.join("hello.txt"), b"hi").unwrap();

        let deadline = Instant::now() + Duration::from_secs(8);
        let found = wait_for(&conn_arc, "hello", deadline, |hits| {
            hits.iter().any(|h| h.name == "hello.txt" && !h.is_dir)
        });
        assert!(found, "watcher must index newly created file within 8s");

        registry.stop(root.id).unwrap();
        cleanup(&dir);
    }

    #[test]
    fn watcher_removes_deleted_file_from_index() {
        let dir = unique_test_dir("delete");
        fs::write(dir.join("doomed.txt"), b"x").unwrap();
        let conn_arc = make_conn();
        let root = add_root(&conn_arc, dir.to_str().unwrap(), "local");

        // Pre-populate as if a previous scan had indexed the file.
        {
            let conn = conn_arc.lock().unwrap();
            upsert_file(
                &conn,
                &FileMetadata {
                    root_id: root.id,
                    path: dir.join("doomed.txt").to_string_lossy().into_owned(),
                    name: "doomed.txt".to_string(),
                    extension: Some("txt".to_string()),
                    size: Some(1),
                    modified_at: None,
                    added_at: None,
                    is_dir: false,
                    source_kind: RootKind::Local,
                    is_accessible: true,
                },
            )
            .unwrap();
        }
        // Sanity check the row exists before deletion.
        {
            let conn = conn_arc.lock().unwrap();
            let pre = search_files(&conn, "doomed", 10, 0).unwrap();
            assert_eq!(pre.len(), 1);
        }

        let registry = WatcherRegistry::new();
        registry.start(&root, conn_arc.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        fs::remove_file(dir.join("doomed.txt")).unwrap();

        let deadline = Instant::now() + Duration::from_secs(8);
        let gone = wait_for(&conn_arc, "doomed", deadline, |hits| hits.is_empty());
        assert!(
            gone,
            "watcher must remove deleted file from index within 8s"
        );

        registry.stop(root.id).unwrap();
        cleanup(&dir);
    }

    #[test]
    fn watcher_updates_index_on_rename() {
        let dir = unique_test_dir("rename");
        fs::write(dir.join("before.txt"), b"x").unwrap();
        let conn_arc = make_conn();
        let root = add_root(&conn_arc, dir.to_str().unwrap(), "local");
        {
            let conn = conn_arc.lock().unwrap();
            upsert_file(
                &conn,
                &FileMetadata {
                    root_id: root.id,
                    path: dir.join("before.txt").to_string_lossy().into_owned(),
                    name: "before.txt".to_string(),
                    extension: Some("txt".to_string()),
                    size: Some(1),
                    modified_at: None,
                    added_at: None,
                    is_dir: false,
                    source_kind: RootKind::Local,
                    is_accessible: true,
                },
            )
            .unwrap();
        }

        let registry = WatcherRegistry::new();
        registry.start(&root, conn_arc.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        fs::rename(dir.join("before.txt"), dir.join("after.txt")).unwrap();

        let deadline = Instant::now() + Duration::from_secs(8);
        let after_indexed = wait_for(&conn_arc, "after", deadline, |hits| {
            hits.iter().any(|h| h.name == "after.txt")
        });
        assert!(after_indexed, "watcher must index renamed file (after.txt)");

        // The old name should also have been cleaned up. We poll separately
        // because rename can deliver the From/To events out of order.
        let deadline2 = Instant::now() + Duration::from_secs(4);
        let before_gone = wait_for(&conn_arc, "before", deadline2, |hits| {
            !hits.iter().any(|h| h.name == "before.txt")
        });
        assert!(
            before_gone,
            "watcher must drop old name (before.txt) after rename"
        );

        registry.stop(root.id).unwrap();
        cleanup(&dir);
    }
}

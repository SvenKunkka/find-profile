use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::scanner::ScanCancellations;
use crate::watcher::WatcherRegistry;

/// Tauri-managed application state.
///
/// Wraps the SQLite `Connection` in an `Arc<Mutex<_>>` so it can be:
///  - cloned cheaply into background threads (e.g. by `start_scan` or by the
///    notify watcher's event handler thread),
///  - locked from sync `#[tauri::command]` handlers without contention with
///    the async runtime (Tauri runs sync commands on a worker thread).
///
/// Concurrency caveat: holding the mutex during a long scan blocks other
/// commands. Acceptable for MVP — a connection pool is a future optimisation.
///
/// `watchers` is shared by reference (`Arc`) so command handlers and the
/// startup hook can both mutate the registry without owning it exclusively.
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    pub watchers: Arc<WatcherRegistry>,
    pub scan_cancellations: Arc<ScanCancellations>,
}

impl AppState {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            watchers: Arc::new(WatcherRegistry::new()),
            scan_cancellations: Arc::new(ScanCancellations::new()),
        }
    }
}

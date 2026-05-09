use std::sync::{Arc, Mutex};

use rusqlite::Connection;

/// Tauri-managed application state.
///
/// Wraps the SQLite `Connection` in an `Arc<Mutex<_>>` so it can be:
///  - cloned cheaply into background threads (e.g. by `start_scan`),
///  - locked from sync `#[tauri::command]` handlers without contention with
///    the async runtime (Tauri runs sync commands on a worker thread).
///
/// Concurrency caveat: holding the mutex during a long scan blocks other
/// commands. Acceptable for MVP — a connection pool is a future optimisation.
pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
}

impl AppState {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }
}

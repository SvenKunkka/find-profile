use std::path::Path;
use std::process::Command;
use std::sync::MutexGuard;

use rusqlite::Connection;
use tauri::State;

use crate::db::{self, FileHit, Root, RootKind, RootStatus, ScanJob};
use crate::roots::register_root;
use crate::scanner::{self, ScanInput};

use super::error::{CommandError, CommandResult};
use super::state::AppState;

const SEARCH_LIMIT_MAX: i64 = 1000;

/// Lock the shared connection without ever panicking on poisoning.
fn lock_conn(state: &AppState) -> CommandResult<MutexGuard<'_, Connection>> {
    state
        .conn
        .lock()
        .map_err(|_| CommandError::new("internal", "database mutex poisoned"))
}

#[tauri::command]
pub fn add_index_root(
    state: State<'_, AppState>,
    path: String,
    kind: Option<String>,
) -> CommandResult<Root> {
    let root = {
        let conn = lock_conn(&state)?;
        register_root(&conn, &path, kind.as_deref())?
    };
    // Best-effort: a watcher failure must not undo the registration. If the
    // user added a NAS root, `start` is a no-op by design.
    if let Err(e) = state.watchers.start(&root, state.conn.clone()) {
        eprintln!(
            "watcher: failed to start watcher for new root {} ({}): {e}",
            root.id, root.path
        );
    }
    Ok(root)
}

#[tauri::command]
pub fn list_index_roots(state: State<'_, AppState>) -> CommandResult<Vec<Root>> {
    let conn = lock_conn(&state)?;
    Ok(db::list_roots(&conn)?)
}

/// Kick off a non-blocking scan for the given root.
///
/// The command returns as soon as the root is validated; the actual recursive
/// walk runs on a dedicated OS thread so the UI thread is never blocked.
/// Frontends poll `get_scan_status` for progress.
#[tauri::command]
pub fn start_scan(state: State<'_, AppState>, root_id: i64) -> CommandResult<()> {
    spawn_scan(&state, root_id)
}

/// Re-trigger a scan for a previously-added root. Functionally equivalent to
/// `start_scan` for MVP — separated as a distinct command so the frontend can
/// surface "Rescan" as its own intent in the UI.
#[tauri::command]
pub fn rescan_root(state: State<'_, AppState>, root_id: i64) -> CommandResult<()> {
    spawn_scan(&state, root_id)
}

fn spawn_scan(state: &AppState, root_id: i64) -> CommandResult<()> {
    // Validate the root exists and capture its path/kind synchronously so we
    // can return a useful error to the caller without entering the worker.
    let (root_path, source_kind) = {
        let conn = lock_conn(state)?;
        let root = db::get_root(&conn, root_id)?;
        (root.path, root.kind)
    };

    let conn_arc = state.conn.clone();
    std::thread::spawn(move || {
        // Acquire the lock inside the worker; failures are recorded into
        // scan_jobs by scanner::scan itself, so we don't need to surface
        // anything back from this thread.
        let Ok(mut guard) = conn_arc.lock() else {
            eprintln!("scan worker: db mutex poisoned for root {root_id}");
            return;
        };
        let result = scanner::scan(
            &mut guard,
            ScanInput {
                root_id,
                root_path,
                source_kind,
            },
            |_progress| {
                // TODO(next stage): emit `scan-progress` Tauri events. For
                // now the frontend polls get_scan_status.
            },
        );
        if let Err(e) = result {
            eprintln!("scan worker for root {root_id} failed: {e}");
        }
    });

    Ok(())
}

#[tauri::command]
pub fn get_scan_status(
    state: State<'_, AppState>,
    root_id: i64,
) -> CommandResult<Option<ScanJob>> {
    let conn = lock_conn(&state)?;
    Ok(db::get_latest_scan_job_for_root(&conn, root_id)?)
}

#[tauri::command]
pub fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: i64,
    offset: i64,
) -> CommandResult<Vec<FileHit>> {
    let limit = limit.clamp(1, SEARCH_LIMIT_MAX);
    let offset = offset.max(0);
    let conn = lock_conn(&state)?;
    Ok(db::search_files(&conn, &query, limit, offset)?)
}

#[tauri::command]
pub fn open_file(state: State<'_, AppState>, path: String) -> CommandResult<()> {
    {
        let conn = lock_conn(&state)?;
        check_root_reachable(&conn, &path)?;
    }
    open_with_macos(&path, /* reveal */ false)
}

#[tauri::command]
pub fn reveal_in_finder(state: State<'_, AppState>, path: String) -> CommandResult<()> {
    {
        let conn = lock_conn(&state)?;
        check_root_reachable(&conn, &path)?;
    }
    open_with_macos(&path, /* reveal */ true)
}

/// Reject open/reveal early when the path lives under a root that is currently
/// disconnected or in error state.
///
/// The asymmetry between NAS and local roots is deliberate: a disconnected NAS
/// is the common, transient case the MVP should handle gracefully (the UI can
/// instruct the user to reconnect and rescan), so it gets its own error code
/// `nas_unavailable`. A local root in error state is a less common situation
/// and falls under the broader `root_unavailable` code.
pub(crate) fn check_root_reachable(conn: &Connection, path: &str) -> CommandResult<()> {
    let Some(root) = db::find_root_for_path(conn, path)? else {
        // Path does not belong to any registered root; let the OS layer decide.
        return Ok(());
    };
    match root.status {
        RootStatus::Available | RootStatus::Scanning => Ok(()),
        RootStatus::Disconnected if root.kind == RootKind::Nas => Err(CommandError::new(
            "nas_unavailable",
            format!(
                "NAS root '{}' is currently disconnected. Reconnect the volume and click Scan to refresh.",
                root.path
            ),
        )),
        RootStatus::Disconnected => Err(CommandError::new(
            "root_unavailable",
            format!("Root '{}' is currently disconnected.", root.path),
        )),
        RootStatus::Error => Err(CommandError::new(
            "root_unavailable",
            format!("Root '{}' is in error state; rescan it before opening files.", root.path),
        )),
    }
}

fn open_with_macos(path: &str, reveal: bool) -> CommandResult<()> {
    if !Path::new(path).exists() {
        return Err(CommandError::new(
            "not_found",
            format!("path not found or not accessible: {path}"),
        ));
    }
    if !cfg!(target_os = "macos") {
        return Err(CommandError::new(
            "unsupported",
            "open/reveal is only implemented for macOS in MVP",
        ));
    }
    let mut cmd = Command::new("open");
    if reveal {
        cmd.arg("-R");
    }
    cmd.arg(path);
    let status = cmd.status().map_err(CommandError::from)?;
    if !status.success() {
        return Err(CommandError::new(
            "io",
            format!(
                "`open{}` exited with status {:?}",
                if reveal { " -R" } else { "" },
                status.code()
            ),
        ));
    }
    Ok(())
}

#[tauri::command]
pub fn remove_index_root(
    state: State<'_, AppState>,
    root_id: i64,
    keep_index: bool,
) -> CommandResult<()> {
    // Stop the watcher *before* mutating the DB so a stale event from notify
    // can't race with a removed/disabled root row.
    if let Err(e) = state.watchers.stop(root_id) {
        eprintln!("watcher: failed to stop watcher for root {root_id}: {e}");
    }
    let conn = lock_conn(&state)?;
    if keep_index {
        db::disable_root(&conn, root_id)?;
    } else {
        db::delete_root(&conn, root_id)?;
    }
    Ok(())
}

use std::path::Path;
use std::process::Command;
use std::sync::MutexGuard;

use rusqlite::Connection;
use tauri::State;

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::db::{
    self, FileHit, Root, RootKind, RootStatus, ScanJob, ScanJobStatus, SearchOptions, SortBy,
};
use crate::roots::register_root;
use crate::scanner::{self, ScanFilters, ScanInput};
use crate::settings::{self, Settings};
use std::sync::Arc;

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
    // Validate the root exists, capture its path/kind, and snapshot the
    // user's current scan filters synchronously so we can return a clean
    // error to the caller without entering the worker.
    let (root_path, source_kind, filters) = {
        let conn = lock_conn(state)?;
        let root = db::get_root(&conn, root_id)?;
        let s = settings::load(&conn).unwrap_or_default();
        let filters = ScanFilters {
            include_files: s.index_files,
            include_folders: s.index_folders,
            show_hidden: s.show_hidden_files,
        };
        (root.path, root.kind, filters)
    };

    tracing::info!(root_id, root_path = %root_path, ?filters, "spawn_scan");

    // Race-free claim of the cancellation slot. If a worker is already
    // running for this root, return Ok without spawning — the user's
    // double-click on Scan is benign-deduplicated rather than racing two
    // workers against the same DB rows. `cancel_scan` flips this token;
    // the worker polls it at every walker iteration.
    let cancel = match state.scan_cancellations.try_register(root_id) {
        Some(t) => t,
        None => {
            tracing::info!(root_id, "spawn_scan: scan already in flight, skipping");
            return Ok(());
        }
    };
    let conn_arc: Arc<_> = state.conn.clone();
    let cancellations = state.scan_cancellations.clone();

    std::thread::spawn(move || {
        let result = scanner::scan_cancellable(
            conn_arc,
            ScanInput {
                root_id,
                root_path,
                source_kind,
            },
            filters,
            cancel,
            |_progress| {
                // TODO(next stage): emit `scan-progress` Tauri events. For
                // now the frontend polls get_scan_status.
            },
        );
        // Always clear the token so a follow-up Scan can register a fresh one.
        cancellations.clear(root_id);
        if let Err(e) = result {
            tracing::error!(root_id, error = %e, "scan worker failed");
        }
    });

    Ok(())
}

/// Signal cooperative cancellation for an in-flight scan.
///
/// Returns `true` when a running scan token was found and flipped, `false`
/// when no scan is currently tracked for this root (either it never started
/// or it already finished). Either way the call is safe — it never races a
/// destructive DB operation.
#[tauri::command]
pub fn cancel_scan(state: State<'_, AppState>, root_id: i64) -> CommandResult<bool> {
    Ok(state.scan_cancellations.cancel(root_id))
}

/// Frontend-facing scan progress DTO. `progress_percent` is derived from
/// `scanned_count / estimated_total_count` and clamped to `[0, 100]`. Stored
/// `ScanJob` fields are kept verbatim so the frontend can display them too.
#[derive(Debug, Clone, Serialize)]
pub struct ScanStatus {
    pub root_id: i64,
    pub job_id: i64,
    pub status: ScanJobStatus,
    pub scanned_count: i64,
    pub error_count: i64,
    pub estimated_total_count: Option<i64>,
    pub progress_percent: i64,
    pub current_path: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error_message: Option<String>,
}

impl ScanStatus {
    pub(crate) fn from_job(job: ScanJob) -> Self {
        let progress_percent = match job.estimated_total_count {
            Some(t) if t > 0 => {
                let raw = (job.scanned_count.saturating_mul(100)) / t;
                raw.clamp(0, 100)
            }
            _ => 0,
        };
        Self {
            root_id: job.root_id,
            job_id: job.id,
            status: job.status,
            scanned_count: job.scanned_count,
            error_count: job.error_count,
            estimated_total_count: job.estimated_total_count,
            progress_percent,
            current_path: job.current_path,
            started_at: job.started_at,
            finished_at: job.finished_at,
            error_message: job.error_message,
        }
    }
}

#[tauri::command]
pub fn get_scan_status(
    state: State<'_, AppState>,
    root_id: i64,
) -> CommandResult<Option<ScanStatus>> {
    let conn = lock_conn(&state)?;
    Ok(db::get_latest_scan_job_for_root(&conn, root_id)?.map(ScanStatus::from_job))
}

/// Spawn a scan worker for every enabled root that isn't already scanning.
///
/// Skips roots where the cancellation registry already has a token (a worker
/// is in-flight). A single failed `spawn_scan` does not abort the whole
/// loop. Returns the count of newly-spawned scans.
#[tauri::command]
pub fn start_scan_all_roots(state: State<'_, AppState>) -> CommandResult<i64> {
    scan_all_roots_inner(&state)
}

pub(crate) fn scan_all_roots_inner(state: &AppState) -> CommandResult<i64> {
    let roots = {
        let conn = lock_conn(state)?;
        db::list_roots(&conn)?
    };

    // Build a queue of (root, snapshot inputs, cancel token) for every
    // enabled root that isn't already in flight. Claims happen via
    // `try_register`, so a parallel `start_scan` from the UI can't race
    // us onto the same root.
    //
    // Why sequential instead of parallel: scanning N roots concurrently
    // creates an I/O storm on the disk, fights for the mutex, and on a
    // mixed local+NAS setup the slow NAS scan can starve the fast local
    // one of writer time. A single worker that processes roots one at a
    // time gives each scan full bandwidth and produces predictable timing.
    type QueueItem = (i64, String, RootKind, ScanFilters, std::sync::Arc<std::sync::atomic::AtomicBool>);
    let mut queue: Vec<QueueItem> = Vec::new();
    for r in roots.iter().filter(|r| r.enabled) {
        let cancel = match state.scan_cancellations.try_register(r.id) {
            Some(t) => t,
            None => {
                tracing::info!(root_id = r.id, "auto-scan-all: skip (already running)");
                continue;
            }
        };
        let filters = {
            let conn = lock_conn(state)?;
            let s = settings::load(&conn).unwrap_or_default();
            ScanFilters {
                include_files: s.index_files,
                include_folders: s.index_folders,
                show_hidden: s.show_hidden_files,
            }
        };
        queue.push((r.id, r.path.clone(), r.kind, filters, cancel));
    }

    let spawned = queue.len() as i64;
    if spawned == 0 {
        tracing::info!(spawned, "start_scan_all_roots completed (nothing to do)");
        return Ok(spawned);
    }

    let conn_arc = state.conn.clone();
    let cancellations = state.scan_cancellations.clone();
    std::thread::spawn(move || {
        for (root_id, root_path, source_kind, filters, cancel) in queue {
            // If the user cancelled this root before we got to it (rare but
            // possible — they could click Cancel on a queued root), the flag
            // is already set; the scanner will see it on the very first
            // walker iteration and return Cancelled cheaply.
            let result = scanner::scan_cancellable(
                conn_arc.clone(),
                ScanInput {
                    root_id,
                    root_path,
                    source_kind,
                },
                filters,
                cancel,
                |_progress| {},
            );
            cancellations.clear(root_id);
            if let Err(e) = result {
                tracing::error!(
                    root_id,
                    error = %e,
                    "auto-scan worker: scan failed; continuing with next root",
                );
            }
        }
        tracing::info!("auto-scan worker: queue drained");
    });

    tracing::info!(spawned, "start_scan_all_roots queued");
    Ok(spawned)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn search_files(
    state: State<'_, AppState>,
    query: String,
    limit: i64,
    offset: i64,
    extensions: Option<Vec<String>>,
    source: Option<String>,
    type_filter: Option<String>,
    sort: Option<String>,
) -> CommandResult<Vec<FileHit>> {
    let limit = limit.clamp(1, SEARCH_LIMIT_MAX);
    let offset = offset.max(0);

    let source_kind = match source.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s) => Some(RootKind::parse(s).ok_or_else(|| {
            CommandError::new("invalid_source", format!("invalid source filter: {s}"))
        })?),
    };

    let is_dir_filter = match type_filter.as_deref() {
        None | Some("") | Some("all") => None,
        Some("files") => Some(false),
        Some("dirs") | Some("folders") => Some(true),
        Some(other) => {
            return Err(CommandError::new(
                "invalid_type_filter",
                format!("invalid type filter: {other}"),
            ))
        }
    };

    let sort_by = match sort.as_deref() {
        None | Some("") | Some("relevance") => SortBy::Relevance,
        Some("modified") => SortBy::Modified,
        Some("size") => SortBy::Size,
        Some("name") => SortBy::Name,
        Some(other) => {
            return Err(CommandError::new(
                "invalid_sort",
                format!("invalid sort: {other}"),
            ))
        }
    };

    let extensions = extensions
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty())
        .collect::<Vec<_>>();

    let options = SearchOptions {
        query,
        limit,
        offset,
        extensions,
        source: source_kind,
        is_dir_filter,
        sort: sort_by,
    };

    let conn = lock_conn(&state)?;
    let hits = db::search_files_with_options(&conn, &options)?;
    tracing::info!(
        query = %options.query,
        count = hits.len(),
        sort = ?options.sort,
        source = ?options.source,
        "search_files",
    );
    Ok(hits)
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
        tracing::warn!(root_id, error = %e, "failed to stop watcher");
    }
    let conn = lock_conn(&state)?;
    if keep_index {
        db::disable_root(&conn, root_id)?;
        tracing::info!(root_id, "root disabled (index kept)");
    } else {
        db::delete_root(&conn, root_id)?;
        tracing::info!(root_id, "root deleted (cascade)");
    }
    Ok(())
}

// ---- settings & maintenance commands ---------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CommandResult<Settings> {
    let conn = lock_conn(&state)?;
    Ok(settings::load(&conn)?)
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> CommandResult<Settings> {
    let sanitized = settings.sanitized();
    {
        let conn = lock_conn(&state)?;
        crate::settings::save(&conn, &sanitized)?;
    }
    tracing::info!(?sanitized, "settings updated");
    Ok(sanitized)
}

#[derive(Debug, Serialize)]
pub struct AppPaths {
    pub db_path: String,
    pub log_dir: String,
}

#[tauri::command]
pub fn get_app_paths(app: AppHandle) -> CommandResult<AppPaths> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| CommandError::new("path", e.to_string()))?;
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| CommandError::new("path", e.to_string()))?;
    Ok(AppPaths {
        db_path: app_data
            .join("find_profile.db")
            .to_string_lossy()
            .into_owned(),
        log_dir: log_dir.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub fn clear_index(state: State<'_, AppState>) -> CommandResult<()> {
    let conn = lock_conn(&state)?;
    let n = conn
        .execute("DELETE FROM files", [])
        .map_err(|e| CommandError::from(crate::db::DbError::Sqlite(e)))?;
    tracing::warn!(removed = n, "clear_index: all file rows deleted");
    Ok(())
}

#[tauri::command]
pub fn rebuild_index(state: State<'_, AppState>) -> CommandResult<()> {
    {
        let conn = lock_conn(&state)?;
        let n = conn
            .execute("DELETE FROM files", [])
            .map_err(|e| CommandError::from(crate::db::DbError::Sqlite(e)))?;
        tracing::warn!(removed = n, "rebuild_index: cleared file rows");
    }
    // Reuse the auto-scan queue worker so all roots run sequentially through
    // the same single thread instead of N parallel threads racing each
    // other on disk and DB.
    let spawned = scan_all_roots_inner(&state)?;
    tracing::info!(spawned, "rebuild_index: scans queued");
    Ok(())
}

#[tauri::command]
pub fn open_log_dir(app: AppHandle) -> CommandResult<()> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| CommandError::new("path", e.to_string()))?;
    if !log_dir.exists() {
        std::fs::create_dir_all(&log_dir).map_err(CommandError::from)?;
    }
    if !cfg!(target_os = "macos") {
        return Err(CommandError::new(
            "unsupported",
            "open_log_dir is implemented only for macOS in MVP",
        ));
    }
    let status = Command::new("open")
        .arg(&log_dir)
        .status()
        .map_err(CommandError::from)?;
    if !status.success() {
        return Err(CommandError::new(
            "io",
            format!("`open` exited with status {:?}", status.code()),
        ));
    }
    Ok(())
}

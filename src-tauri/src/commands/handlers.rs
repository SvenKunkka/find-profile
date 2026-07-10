use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::MutexGuard;

use rusqlite::Connection;
use tauri::State;

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::db::{
    self, FileHit, Root, RootKind, RootStatus, ScanJob, ScanJobStatus, SearchOptions, SortBy,
    SortDirection,
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
    type QueueItem = (
        i64,
        String,
        RootKind,
        ScanFilters,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    );
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
    sort_direction: Option<String>,
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
        Some("added") => SortBy::Added,
        Some("size") => SortBy::Size,
        Some("name") => SortBy::Name,
        Some("extension") => SortBy::Extension,
        Some("type") => SortBy::Type,
        Some("source") => SortBy::Source,
        Some("accessibility") => SortBy::Accessibility,
        Some(other) => {
            return Err(CommandError::new(
                "invalid_sort",
                format!("invalid sort: {other}"),
            ))
        }
    };

    let direction = match sort_direction.as_deref() {
        None | Some("") | Some("asc") => SortDirection::Asc,
        Some("desc") => SortDirection::Desc,
        Some(other) => {
            return Err(CommandError::new(
                "invalid_sort",
                format!("invalid sort direction: {other}"),
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
        sort_direction: direction,
    };

    let conn = lock_conn(&state)?;
    let hits = db::search_files_with_options(&conn, &options)?;
    tracing::info!(
        query = %options.query,
        count = hits.len(),
        sort = ?options.sort,
        direction = ?options.sort_direction,
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

#[tauri::command]
pub fn jump_to_path(path_input: String) -> CommandResult<String> {
    let path = normalize_jump_path(&path_input)?;
    let open_path = resolve_jump_open_path(&path)?;
    open_with_macos(&open_path, /* reveal */ false)?;
    Ok(open_path)
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

fn normalize_jump_path(input: &str) -> CommandResult<String> {
    let cleaned = clean_pasted_path(input);
    if cleaned.is_empty() {
        return Err(CommandError::new("path", "path input is empty"));
    }

    for candidate in extract_jump_path_candidates(&cleaned) {
        if let Some(path) = normalize_unc_or_smb_path(&candidate) {
            return Ok(path);
        }
        if candidate.starts_with('/') || candidate.starts_with('~') {
            return Ok(expand_home_path(&candidate));
        }
    }

    Err(CommandError::new(
        "unsupported_path",
        "only macOS absolute paths, smb URLs, and Windows UNC paths are supported",
    ))
}

fn clean_pasted_path(input: &str) -> String {
    let mut out = input.trim().to_string();
    out = strip_balanced_wrappers(out);
    out = replace_markdown_links(&out);
    strip_balanced_wrappers(out.trim().to_string())
}

fn extract_jump_path_candidates(input: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(start) = find_path_start(line) {
            let candidate = clean_candidate_path(&line[start..]);
            if !candidate.is_empty() {
                candidates.push(candidate);
            }
        }
    }

    if candidates.is_empty() {
        candidates.push(clean_candidate_path(input));
    }
    candidates
}

fn find_path_start(line: &str) -> Option<usize> {
    [
        "smb://",
        "file://",
        "\\\\",
        "//",
        "/Volumes/",
        "~/",
        "/Users/",
    ]
    .iter()
    .filter_map(|marker| line.find(marker))
    .min()
}

fn clean_candidate_path(input: &str) -> String {
    let mut out = strip_balanced_wrappers(input.trim().to_string());
    out = out
        .trim_matches(|c: char| {
            matches!(
                c,
                '<' | '>'
                    | '\''
                    | '"'
                    | '`'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '，'
                    | '。'
                    | '；'
                    | ';'
                    | ','
                    | '、'
                    | '：'
                    | ':'
            )
        })
        .trim()
        .to_string();
    strip_balanced_wrappers(out)
}

fn strip_balanced_wrappers(mut value: String) -> String {
    loop {
        let trimmed = value.trim();
        let Some(first) = trimmed.chars().next() else {
            return String::new();
        };
        let Some(last) = trimmed.chars().last() else {
            return String::new();
        };
        let wrapped = matches!(
            (first, last),
            ('\'', '\'') | ('"', '"') | ('`', '`') | ('“', '”') | ('‘', '’')
        );
        if !wrapped || trimmed.len() < 2 {
            return trimmed.to_string();
        }
        value = trimmed
            .trim_start_matches(first)
            .trim_end_matches(last)
            .trim()
            .to_string();
    }
}

fn replace_markdown_links(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('[') {
        let (before, after_start) = rest.split_at(start);
        out.push_str(before);
        let after_start = &after_start[1..];
        let Some(end_label) = after_start.find("](") else {
            out.push('[');
            out.push_str(after_start);
            return out;
        };
        let label = &after_start[..end_label];
        let after_label = &after_start[end_label + 2..];
        let Some(end_url) = after_label.find(')') else {
            out.push('[');
            out.push_str(after_start);
            return out;
        };
        out.push_str(label);
        rest = &after_label[end_url + 1..];
    }
    out.push_str(rest);
    out
}

fn normalize_unc_or_smb_path(input: &str) -> Option<String> {
    let normalized = input.replace('\\', "/");
    let without_scheme = normalized
        .strip_prefix("smb://")
        .or_else(|| normalized.strip_prefix("file://"))
        .unwrap_or(&normalized);
    let trimmed = without_scheme.trim_start_matches('/');
    let is_unc = normalized.starts_with("//") || input.starts_with("\\\\");
    let is_smb = input.starts_with("smb://");
    if !is_unc && !is_smb {
        return None;
    }

    let mut parts = trimmed.split('/').filter(|p| !p.is_empty());
    let _server = parts.next()?;
    let share = parts.next()?;
    let mut path = PathBuf::from("/Volumes");
    path.push(share);
    for part in parts {
        path.push(part);
    }
    Some(path.to_string_lossy().into_owned())
}

fn expand_home_path(input: &str) -> String {
    if input == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| input.to_string());
    }
    if let Some(rest) = input.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    }
    input.to_string()
}

fn resolve_jump_open_path(path: &str) -> CommandResult<String> {
    let target = Path::new(path);
    if target.is_dir() {
        return Ok(path.to_string());
    }

    if target.is_file() || looks_like_file_path(target) {
        if let Some(parent) = target.parent() {
            if parent.exists() && parent.is_dir() {
                return Ok(parent.to_string_lossy().into_owned());
            }
        }
    }

    Err(CommandError::new(
        "not_found",
        format!("path not found or not accessible: {path}"),
    ))
}

fn looks_like_file_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
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
pub fn update_settings(state: State<'_, AppState>, settings: Settings) -> CommandResult<Settings> {
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

#[cfg(test)]
mod path_jump_tests {
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{normalize_jump_path, resolve_jump_open_path};

    #[test]
    fn normalizes_windows_unc_path_to_macos_volume_path() {
        let path = normalize_jump_path(r"\\192.168.1.26\渲染图和3d模型\00-公司产品模型\鼠标\G6HE")
            .unwrap();
        assert_eq!(path, "/Volumes/渲染图和3d模型/00-公司产品模型/鼠标/G6HE");
    }

    #[test]
    fn normalizes_windows_unc_path_with_markdown_host() {
        let path = normalize_jump_path(
            r"\\[192.168.1.26](http://192.168.1.26)\渲染图和3d模型\00-公司产品模型\鼠标\G6HE",
        )
        .unwrap();
        assert_eq!(path, "/Volumes/渲染图和3d模型/00-公司产品模型/鼠标/G6HE");
    }

    #[test]
    fn accepts_quoted_macos_volume_path() {
        let path = normalize_jump_path("'/Volumes/Keychron 包材/02-Lemokey'").unwrap();
        assert_eq!(path, "/Volumes/Keychron 包材/02-Lemokey");
    }

    #[test]
    fn normalizes_smb_url_to_macos_volume_path() {
        let path = normalize_jump_path("smb://192.168.1.26/渲染图和3d模型/鼠标").unwrap();
        assert_eq!(path, "/Volumes/渲染图和3d模型/鼠标");
    }

    #[test]
    fn rejects_relative_paths() {
        let err = normalize_jump_path("渲染图和3d模型/鼠标").unwrap_err();
        assert_eq!(err.code, "unsupported_path");
    }

    #[test]
    fn extracts_path_from_mixed_chat_message() {
        let message = r"
@Nick Xu 已更新至NAS
\\KeychronNAS\亚马逊和官网图片\04-渲染&设计\轴体单独PNG\嘉达隆\星云轴V2\新旧对照
[图片]
";
        let path = normalize_jump_path(message).unwrap();
        assert_eq!(
            path,
            "/Volumes/亚马逊和官网图片/04-渲染&设计/轴体单独PNG/嘉达隆/星云轴V2/新旧对照"
        );
    }

    #[test]
    fn extracts_quoted_macos_path_from_mixed_message() {
        let message = "麻烦看下这个目录：'/Volumes/Keychron 包材/02-Lemokey'\n图片如下";
        let path = normalize_jump_path(message).unwrap();
        assert_eq!(path, "/Volumes/Keychron 包材/02-Lemokey");
    }

    #[test]
    fn file_path_opens_parent_folder_when_file_exists() {
        let dir = make_test_dir("existing-file-parent");
        let file = dir.join("磁轴438.png");
        fs::write(&file, b"png").unwrap();

        let open_path = resolve_jump_open_path(file.to_str().unwrap()).unwrap();
        assert_eq!(Path::new(&open_path), dir.as_path());

        fs::remove_file(file).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn file_like_missing_path_opens_parent_folder_when_parent_exists() {
        let dir = make_test_dir("missing-file-parent");
        let file = dir.join("磁轴438.png");

        let open_path = resolve_jump_open_path(file.to_str().unwrap()).unwrap();
        assert_eq!(Path::new(&open_path), dir.as_path());

        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn missing_folder_path_still_returns_not_found() {
        let dir = make_test_dir("missing-folder-parent");
        let missing_folder = dir.join("新旧对照");

        let err = resolve_jump_open_path(missing_folder.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, "not_found");

        fs::remove_dir(dir).unwrap();
    }

    fn make_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("find-profile-{name}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

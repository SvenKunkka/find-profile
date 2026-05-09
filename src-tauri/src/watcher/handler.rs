use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind};
use rusqlite::Connection;

use crate::db::{self, FileMetadata, RootKind};

/// Top-level watcher event sink. All errors are logged but never propagated —
/// we never want a watcher event to crash the host process.
pub fn handle_event(
    res: notify::Result<Event>,
    root_id: i64,
    source_kind: RootKind,
    conn: &Arc<Mutex<Connection>>,
) {
    let event = match res {
        Ok(ev) => ev,
        Err(e) => {
            eprintln!(
                "watcher: notify reported an error for root {root_id}: {e}"
            );
            return;
        }
    };

    if let Err(e) = process_event(&event, root_id, source_kind, conn) {
        eprintln!(
            "watcher: failed to apply event {:?} for root {root_id}: {e}",
            event.kind
        );
    }
}

fn process_event(
    event: &Event,
    root_id: i64,
    source_kind: RootKind,
    conn: &Arc<Mutex<Connection>>,
) -> Result<(), String> {
    match &event.kind {
        EventKind::Create(_)
        | EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Metadata(_))
        | EventKind::Modify(ModifyKind::Any)
        | EventKind::Modify(ModifyKind::Other) => {
            for path in &event.paths {
                upsert_path(path, root_id, source_kind, conn)?;
            }
        }
        EventKind::Modify(ModifyKind::Name(rename_mode)) => match rename_mode {
            RenameMode::From => {
                for path in &event.paths {
                    remove_path(path, conn)?;
                }
            }
            RenameMode::To => {
                for path in &event.paths {
                    upsert_path(path, root_id, source_kind, conn)?;
                }
            }
            RenameMode::Both => {
                if event.paths.len() == 2 {
                    remove_path(&event.paths[0], conn)?;
                    upsert_path(&event.paths[1], root_id, source_kind, conn)?;
                } else {
                    // Best-effort fallback when the platform delivered an
                    // unexpected number of paths.
                    for path in &event.paths {
                        upsert_path(path, root_id, source_kind, conn)?;
                    }
                }
            }
            _ => {
                for path in &event.paths {
                    upsert_path(path, root_id, source_kind, conn)?;
                }
            }
        },
        EventKind::Remove(_) => {
            for path in &event.paths {
                remove_path(path, conn)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn upsert_path(
    path: &Path,
    root_id: i64,
    source_kind: RootKind,
    conn: &Arc<Mutex<Connection>>,
) -> Result<(), String> {
    // If the file no longer exists by the time we try to stat it (rapid
    // create+delete sequences), drop the row instead of trying to upsert with
    // synthetic data.
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return remove_path(path, conn),
    };

    let path_str = path.to_string_lossy().into_owned();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_dir = metadata.is_dir();
    let size = if is_dir { None } else { Some(metadata.len() as i64) };
    let modified_at = metadata.modified().ok().and_then(system_time_to_iso);
    let extension = if is_dir {
        None
    } else {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
    };

    let m = FileMetadata {
        root_id,
        path: path_str,
        name,
        extension,
        size,
        modified_at,
        is_dir,
        source_kind,
        is_accessible: true,
    };

    let conn_guard = conn
        .lock()
        .map_err(|_| "db mutex poisoned in watcher".to_string())?;
    db::upsert_file(&conn_guard, &m).map_err(|e| e.to_string())?;
    Ok(())
}

fn remove_path(
    path: &Path,
    conn: &Arc<Mutex<Connection>>,
) -> Result<(), String> {
    let path_str = path.to_string_lossy().into_owned();
    let conn_guard = conn
        .lock()
        .map_err(|_| "db mutex poisoned in watcher".to_string())?;
    db::remove_file(&conn_guard, &path_str).map_err(|e| e.to_string())?;
    Ok(())
}

fn system_time_to_iso(t: SystemTime) -> Option<String> {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let dt = DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0)?;
    Some(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, Utc};
use walkdir::DirEntry;

use crate::db::{FileMetadata, RootKind};

/// Extract a `FileMetadata` from a walkdir entry.
///
/// Returns `None` when metadata cannot be read (permission denied, broken
/// symlink target, etc.). Callers treat that as an error to count, not a
/// reason to abort the scan.
pub fn extract_metadata(
    entry: &DirEntry,
    root_id: i64,
    source_kind: RootKind,
) -> Option<FileMetadata> {
    let metadata = entry.metadata().ok()?;
    let path = entry.path();
    let path_str = path.to_string_lossy().into_owned();
    let name = entry.file_name().to_string_lossy().into_owned();
    let is_dir = metadata.is_dir();

    let size = if is_dir {
        None
    } else {
        Some(metadata.len() as i64)
    };

    let modified_at = metadata.modified().ok().and_then(system_time_to_iso);
    let added_at = metadata.created().ok().and_then(system_time_to_iso);

    let extension = if is_dir {
        None
    } else {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
    };

    Some(FileMetadata {
        root_id,
        path: path_str,
        name,
        extension,
        size,
        modified_at,
        added_at,
        is_dir,
        source_kind,
        is_accessible: true,
    })
}

fn system_time_to_iso(t: SystemTime) -> Option<String> {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let dt = DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0)?;
    Some(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

use std::path::Path;

use crate::db::{RootKind, RootStatus};

/// Probe whether a root path is currently reachable as a directory.
///
/// Returns `true` when `std::fs::metadata` succeeds *and* the entry is a
/// directory. Symlinks are followed (we want the target to be a usable root),
/// and any I/O error (NotFound, PermissionDenied, NAS disconnect, etc.) yields
/// `false` — the caller decides what status that means.
pub fn probe_path_accessibility<P: AsRef<Path>>(path: P) -> bool {
    std::fs::metadata(path.as_ref())
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

/// Decide what `RootStatus` a root should carry, given its kind and whether
/// its path is currently accessible.
///
/// The asymmetry between `local` and `nas` is intentional and load-bearing:
/// - A `nas` root being unreachable is a normal, transient condition (volume
///   unmounted, network down). It must NOT cause us to delete the index.
/// - A `local` root being unreachable is unexpected (the directory the user
///   pointed us at no longer exists or is locked) — surface it as an error.
pub fn decide_status_from_accessibility(kind: RootKind, accessible: bool) -> RootStatus {
    if accessible {
        RootStatus::Available
    } else {
        match kind {
            RootKind::Nas => RootStatus::Disconnected,
            RootKind::Local => RootStatus::Error,
        }
    }
}

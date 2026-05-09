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

/// Whether a root of this kind should be subscribed to a filesystem watcher.
///
/// Local roots benefit from FSEvents/inotify-style live updates. NAS roots
/// must NOT be watched: macOS FSEvents on SMB/AFP mounts is unreliable, and
/// fsnotify-style mechanisms over network filesystems either silently miss
/// events or fire spurious ones. NAS roots rely on manual `Scan` (and, in
/// future, scheduled scans) instead.
pub fn should_watch_root(kind: RootKind) -> bool {
    match kind {
        RootKind::Local => true,
        RootKind::Nas => false,
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn local_roots_get_a_watcher() {
        assert!(should_watch_root(RootKind::Local));
    }

    #[test]
    fn nas_roots_must_never_be_watched() {
        assert!(
            !should_watch_root(RootKind::Nas),
            "NAS roots must rely on manual / scheduled scans, never on FS watchers"
        );
    }
}

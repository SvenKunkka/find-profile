pub mod availability;
pub mod error;
pub mod kind;

use rusqlite::Connection;

use crate::db::{self, Root, RootStatus};

pub use availability::{
    decide_status_from_accessibility, probe_path_accessibility, should_watch_root,
};
pub use error::{Result, RootError};
pub use kind::{parse_kind, resolve_kind};

/// Register a new root: resolve kind (user-provided wins, else auto-detect)
/// and persist it via the low-level db layer.
///
/// Initial status is left at the db-layer default (`available`); accessibility
/// is decided lazily by `refresh_root_status` so that adding a NAS root while
/// the volume is unmounted doesn't fail outright.
pub fn register_root(conn: &Connection, path: &str, user_kind: Option<&str>) -> Result<Root> {
    let kind = resolve_kind(path, user_kind)?;
    let root = db::add_root(conn, path, kind)?;
    Ok(root)
}

/// Probe a root's path on disk and update its status accordingly.
///
/// Critically, this never touches the `files` table: an unreachable NAS root
/// must keep its previously-indexed records visible to search.
pub fn refresh_root_status(conn: &Connection, root_id: i64) -> Result<RootStatus> {
    let root = db::get_root(conn, root_id)?;
    let accessible = probe_path_accessibility(&root.path);
    let status = decide_status_from_accessibility(root.kind, accessible);
    db::update_root_status(conn, root_id, status)?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::db::{init_in_memory_db, FileMetadata, RootKind, RootStatus};

    fn unique_nonexistent_nas_path() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!(
            "/Volumes/__find_profile_test_nonexistent_{}_{}__",
            std::process::id(),
            nanos
        )
    }

    fn meta(root_id: i64, path: &str, name: &str, kind: RootKind) -> FileMetadata {
        FileMetadata {
            root_id,
            path: path.to_string(),
            name: name.to_string(),
            extension: None,
            size: Some(1),
            modified_at: None,
            added_at: None,
            is_dir: false,
            source_kind: kind,
            is_accessible: true,
        }
    }

    // --- kind resolution -------------------------------------------------

    #[test]
    fn auto_detect_nas_for_volumes_path() {
        let kind = resolve_kind("/Volumes/NAS", None).unwrap();
        assert_eq!(kind, RootKind::Nas);
    }

    #[test]
    fn auto_detect_local_for_user_documents() {
        let kind = resolve_kind("/Users/keychron/Documents", None).unwrap();
        assert_eq!(kind, RootKind::Local);
    }

    #[test]
    fn user_provided_kind_takes_priority_over_auto_detection() {
        // Path looks local, but user override forces nas.
        let kind = resolve_kind("/Users/keychron/Code", Some("nas")).unwrap();
        assert_eq!(kind, RootKind::Nas);

        // Path looks nas, but user override forces local.
        let kind2 = resolve_kind("/Volumes/SomeDisk", Some("local")).unwrap();
        assert_eq!(kind2, RootKind::Local);
    }

    #[test]
    fn invalid_kind_string_returns_structured_error() {
        let err = resolve_kind("/x", Some("cloud")).unwrap_err();
        match err {
            RootError::InvalidKind(s) => assert_eq!(s, "cloud"),
            other => panic!("expected InvalidKind, got {other:?}"),
        }

        let err = parse_kind("").unwrap_err();
        assert!(matches!(err, RootError::InvalidKind(_)));

        let err = parse_kind("LOCAL").unwrap_err();
        match err {
            RootError::InvalidKind(s) => assert_eq!(s, "LOCAL"),
            other => panic!("expected InvalidKind, got {other:?}"),
        }
    }

    #[test]
    fn parse_kind_accepts_canonical_values() {
        assert_eq!(parse_kind("local").unwrap(), RootKind::Local);
        assert_eq!(parse_kind("nas").unwrap(), RootKind::Nas);
    }

    // --- register_root integration --------------------------------------

    #[test]
    fn register_root_uses_auto_detected_kind_when_user_kind_absent() {
        let conn = init_in_memory_db().unwrap();

        let nas = register_root(&conn, "/Volumes/Backup", None).unwrap();
        assert_eq!(nas.kind, RootKind::Nas);

        let local = register_root(&conn, "/Users/keychron/Code", None).unwrap();
        assert_eq!(local.kind, RootKind::Local);
    }

    #[test]
    fn register_root_uses_user_kind_when_provided() {
        let conn = init_in_memory_db().unwrap();
        let r = register_root(&conn, "/Users/keychron/X", Some("nas")).unwrap();
        assert_eq!(r.kind, RootKind::Nas);
    }

    #[test]
    fn register_root_rejects_invalid_kind() {
        let conn = init_in_memory_db().unwrap();
        let err = register_root(&conn, "/Users/keychron/X", Some("ssd")).unwrap_err();
        assert!(matches!(err, RootError::InvalidKind(_)));
    }

    // --- accessibility / status decision -------------------------------

    #[test]
    fn decide_status_rules_match_spec() {
        assert_eq!(
            decide_status_from_accessibility(RootKind::Local, true),
            RootStatus::Available
        );
        assert_eq!(
            decide_status_from_accessibility(RootKind::Nas, true),
            RootStatus::Available
        );
        assert_eq!(
            decide_status_from_accessibility(RootKind::Nas, false),
            RootStatus::Disconnected,
            "an unreachable NAS root must be flagged disconnected, not error"
        );
        assert_eq!(
            decide_status_from_accessibility(RootKind::Local, false),
            RootStatus::Error,
            "an unreachable local root surfaces as an error"
        );
    }

    #[test]
    fn probe_path_accessibility_handles_missing_path() {
        assert!(!probe_path_accessibility(unique_nonexistent_nas_path()));
    }

    #[test]
    fn probe_path_accessibility_handles_existing_dir() {
        // /tmp exists on macOS and Linux test runners; /Users does too on macOS.
        // Pick the OS temp dir which is always present.
        let tmp = std::env::temp_dir();
        assert!(probe_path_accessibility(&tmp));
    }

    // --- the load-bearing NAS-disconnect invariant ---------------------

    #[test]
    fn nas_disconnect_does_not_remove_old_index() {
        let conn = init_in_memory_db().unwrap();
        let nas_path = unique_nonexistent_nas_path();
        // Sanity: confirm the path really doesn't exist on this host before
        // we rely on probe_path_accessibility returning false.
        assert!(
            std::fs::metadata(&nas_path).is_err(),
            "test fixture path must not exist on host"
        );

        // Add the NAS root and pre-populate the index as if a previous
        // successful scan had run while the volume was mounted.
        let root = register_root(&conn, &nas_path, None).unwrap();
        assert_eq!(root.kind, RootKind::Nas);

        let file_path = format!("{nas_path}/old_indexed_file.txt");
        db::upsert_file(
            &conn,
            &meta(root.id, &file_path, "old_indexed_file.txt", RootKind::Nas),
        )
        .unwrap();
        let pre_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_count, 1);

        // The volume is unreachable now — refresh should detect that and
        // mark the root as disconnected without touching the file rows.
        let new_status = refresh_root_status(&conn, root.id).unwrap();
        assert_eq!(new_status, RootStatus::Disconnected);

        // Files must still be present in both the base table and FTS.
        let post_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            post_count, 1,
            "NAS disconnect must NEVER delete previously indexed rows"
        );

        let hits = db::search_files(&conn, "old_indexed_file", 10, 0).unwrap();
        assert_eq!(hits.len(), 1, "search must still hit the cached row");
        assert_eq!(hits[0].path, file_path);
    }

    #[test]
    fn refresh_root_status_marks_local_root_error_when_path_missing() {
        let conn = init_in_memory_db().unwrap();
        let bogus_local = format!(
            "/tmp/__find_profile_test_local_missing_{}_{}__",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        assert!(std::fs::metadata(&bogus_local).is_err());

        let root = register_root(&conn, &bogus_local, None).unwrap();
        assert_eq!(root.kind, RootKind::Local);

        let new_status = refresh_root_status(&conn, root.id).unwrap();
        assert_eq!(new_status, RootStatus::Error);
    }

    #[test]
    fn refresh_root_status_marks_accessible_local_root_available() {
        let conn = init_in_memory_db().unwrap();
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().to_string();
        let root = register_root(&conn, &tmp_str, None).unwrap();
        // Path is auto-detected as local (doesn't start with /Volumes/).
        assert_eq!(root.kind, RootKind::Local);

        // First mark it as something other than Available so we can prove
        // the refresh actually transitioned it.
        db::update_root_status(&conn, root.id, RootStatus::Error).unwrap();

        let new_status = refresh_root_status(&conn, root.id).unwrap();
        assert_eq!(new_status, RootStatus::Available);
    }
}

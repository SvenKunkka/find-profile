pub mod error;
pub mod files;
pub mod model;
pub mod roots;
pub mod scan_jobs;
mod schema;
mod util;

use std::path::Path;

use rusqlite::Connection;

pub use error::{DbError, Result};
pub use files::{
    begin_index_transaction, remove_file, search_files, upsert_file, upsert_file_in_transaction,
};
pub use model::{
    EnumParseError, FileHit, FileMetadata, Root, RootKind, RootStatus, ScanJob, ScanJobStatus,
};
pub use roots::{
    add_root, delete_root, disable_root, find_root_for_path, get_root, list_roots,
    update_root_status,
};
pub use scan_jobs::{
    create_scan_job, finish_scan_job, get_latest_scan_job_for_root, get_scan_job,
    update_scan_job_progress,
};

pub fn init_db<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path.as_ref())?;
    configure(&conn)?;
    schema::apply(&conn)?;
    Ok(conn)
}

pub fn init_in_memory_db() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    schema::apply(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(root_id: i64, path: &str, name: &str) -> FileMetadata {
        FileMetadata {
            root_id,
            path: path.to_string(),
            name: name.to_string(),
            extension: name
                .rsplit_once('.')
                .map(|(_, ext)| ext.to_lowercase()),
            size: Some(1024),
            modified_at: Some("2026-05-09T00:00:00Z".to_string()),
            is_dir: false,
            source_kind: RootKind::Local,
            is_accessible: true,
        }
    }

    #[test]
    fn init_creates_required_tables() {
        let conn = init_in_memory_db().expect("init_in_memory_db");

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name IN ('roots','files','scan_jobs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 3, "roots/files/scan_jobs should all exist");

        let fts_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='files_fts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(fts_count >= 1, "files_fts should exist");
    }

    #[test]
    fn root_kind_detection_uses_volumes_prefix() {
        assert_eq!(RootKind::detect_from_path("/Volumes/Backup"), RootKind::Nas);
        assert_eq!(RootKind::detect_from_path("/Users/me"), RootKind::Local);
        assert_eq!(RootKind::detect_from_path("/"), RootKind::Local);
    }

    #[test]
    fn add_root_persists_with_defaults() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/Users/me/Documents", RootKind::Local).unwrap();
        assert_eq!(r.path, "/Users/me/Documents");
        assert_eq!(r.kind, RootKind::Local);
        assert_eq!(r.status, RootStatus::Available);
        assert!(r.enabled);
        assert!(r.scan_interval.is_none());
        assert!(r.last_scanned_at.is_none());
        assert!(!r.created_at.is_empty());
    }

    #[test]
    fn add_root_supports_nas_kind() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/Volumes/Backup", RootKind::Nas).unwrap();
        assert_eq!(r.kind, RootKind::Nas);
    }

    #[test]
    fn duplicate_root_path_fails() {
        let conn = init_in_memory_db().unwrap();
        add_root(&conn, "/a", RootKind::Local).unwrap();
        let err = add_root(&conn, "/a", RootKind::Local);
        assert!(err.is_err(), "second insert with same path should fail");
    }

    #[test]
    fn list_roots_returns_all() {
        let conn = init_in_memory_db().unwrap();
        add_root(&conn, "/a", RootKind::Local).unwrap();
        add_root(&conn, "/Volumes/B", RootKind::Nas).unwrap();
        let list = list_roots(&conn).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].kind, RootKind::Local);
        assert_eq!(list[1].kind, RootKind::Nas);
    }

    #[test]
    fn update_root_status_changes_status_and_updated_at() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        let original_updated_at = r.updated_at.clone();
        // Sleep just enough so updated_at is observably different — even if our
        // ISO formatter has 1-second resolution, equality is acceptable here;
        // the meaningful assertion is the status change.
        update_root_status(&conn, r.id, RootStatus::Disconnected).unwrap();
        let after = get_root(&conn, r.id).unwrap();
        assert_eq!(after.status, RootStatus::Disconnected);
        assert!(!after.updated_at.is_empty());
        assert!(after.updated_at >= original_updated_at);
    }

    #[test]
    fn update_root_status_unknown_id_returns_not_found() {
        let conn = init_in_memory_db().unwrap();
        let err = update_root_status(&conn, 9999, RootStatus::Error);
        assert!(matches!(err, Err(DbError::NotFound(_))));
    }

    #[test]
    fn upsert_file_inserts_then_updates_same_row() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();

        let mut m = meta(r.id, "/a/report.pdf", "report.pdf");
        let id1 = upsert_file(&conn, &m).unwrap();

        m.name = "renamed.pdf".to_string();
        m.size = Some(2048);
        let id2 = upsert_file(&conn, &m).unwrap();
        assert_eq!(id1, id2, "upsert should keep the same row id");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn search_files_matches_by_name() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        upsert_file(&conn, &meta(r.id, "/a/annual_report.pdf", "annual_report.pdf")).unwrap();
        upsert_file(&conn, &meta(r.id, "/a/cat.jpg", "cat.jpg")).unwrap();

        let hits = search_files(&conn, "annual", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "annual_report.pdf");

        let hits2 = search_files(&conn, "report", 10, 0).unwrap();
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].name, "annual_report.pdf");
    }

    #[test]
    fn search_files_matches_by_path_segment() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/Volumes/Backup", RootKind::Nas).unwrap();
        upsert_file(
            &conn,
            &meta(r.id, "/Volumes/Backup/2023/photo.jpg", "photo.jpg"),
        )
        .unwrap();

        let hits = search_files(&conn, "Backup", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/Volumes/Backup/2023/photo.jpg");
    }

    #[test]
    fn search_files_after_upsert_drops_old_name() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();

        // Path tokens are deliberately unrelated to the name so we can
        // distinguish matches via the `name` FTS column from matches via the
        // `path` column. If the name token also lived in the path, FTS would
        // still match via path even after the name changed.
        let mut m = meta(r.id, "/a/123.tmp", "alphareport");
        upsert_file(&conn, &m).unwrap();
        assert_eq!(search_files(&conn, "alphareport", 10, 0).unwrap().len(), 1);

        m.name = "betareport".to_string();
        upsert_file(&conn, &m).unwrap();
        assert_eq!(
            search_files(&conn, "betareport", 10, 0).unwrap().len(),
            1,
            "new name should be searchable"
        );
        assert!(
            search_files(&conn, "alphareport", 10, 0).unwrap().is_empty(),
            "old name should no longer match after upsert"
        );
    }

    #[test]
    fn remove_file_drops_from_search() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        upsert_file(&conn, &meta(r.id, "/a/report.pdf", "report.pdf")).unwrap();
        assert_eq!(search_files(&conn, "report", 10, 0).unwrap().len(), 1);

        remove_file(&conn, "/a/report.pdf").unwrap();
        assert!(search_files(&conn, "report", 10, 0).unwrap().is_empty());

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn search_files_empty_query_returns_empty() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        upsert_file(&conn, &meta(r.id, "/a/report.pdf", "report.pdf")).unwrap();
        assert!(search_files(&conn, "", 10, 0).unwrap().is_empty());
        assert!(search_files(&conn, "   ", 10, 0).unwrap().is_empty());
    }

    #[test]
    fn search_files_respects_limit_and_offset() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        for i in 0..5 {
            let p = format!("/a/note{i}.txt");
            let n = format!("note{i}.txt");
            upsert_file(&conn, &meta(r.id, &p, &n)).unwrap();
        }
        let page1 = search_files(&conn, "note", 2, 0).unwrap();
        let page2 = search_files(&conn, "note", 2, 2).unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        let p1_ids: Vec<i64> = page1.iter().map(|h| h.id).collect();
        let p2_ids: Vec<i64> = page2.iter().map(|h| h.id).collect();
        assert!(
            p1_ids.iter().all(|id| !p2_ids.contains(id)),
            "pagination should not repeat rows"
        );
    }

    #[test]
    fn scan_job_lifecycle_create_progress_finish() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();

        let job_id = create_scan_job(&conn, r.id).unwrap();
        let initial = get_scan_job(&conn, job_id).unwrap();
        assert_eq!(initial.status, ScanJobStatus::Running);
        assert_eq!(initial.scanned_count, 0);
        assert_eq!(initial.error_count, 0);
        assert!(initial.finished_at.is_none());

        update_scan_job_progress(&conn, job_id, 100, 2).unwrap();
        let mid = get_scan_job(&conn, job_id).unwrap();
        assert_eq!(mid.scanned_count, 100);
        assert_eq!(mid.error_count, 2);
        assert_eq!(mid.status, ScanJobStatus::Running);

        finish_scan_job(&conn, job_id, ScanJobStatus::Success, None).unwrap();
        let done = get_scan_job(&conn, job_id).unwrap();
        assert_eq!(done.status, ScanJobStatus::Success);
        assert!(done.finished_at.is_some());
        assert!(done.error_message.is_none());
    }

    #[test]
    fn scan_job_finish_with_error_records_message() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        let job_id = create_scan_job(&conn, r.id).unwrap();
        finish_scan_job(&conn, job_id, ScanJobStatus::Failed, Some("io error")).unwrap();
        let done = get_scan_job(&conn, job_id).unwrap();
        assert_eq!(done.status, ScanJobStatus::Failed);
        assert_eq!(done.error_message.as_deref(), Some("io error"));
    }

    #[test]
    fn upsert_in_transaction_is_searchable_after_commit() {
        let mut conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();

        {
            let tx = begin_index_transaction(&mut conn).unwrap();
            for i in 0..5 {
                let p = format!("/a/file{i}.txt");
                let n = format!("file{i}.txt");
                upsert_file_in_transaction(&tx, &meta(r.id, &p, &n)).unwrap();
            }
            tx.commit().unwrap();
        }

        let hits = search_files(&conn, "file2", 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "file2.txt");
    }

    #[test]
    fn upsert_in_transaction_rollback_leaves_no_rows() {
        let mut conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();

        {
            let tx = begin_index_transaction(&mut conn).unwrap();
            upsert_file_in_transaction(&tx, &meta(r.id, "/a/x.txt", "x.txt")).unwrap();
            // dropped without commit -> rollback
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn disable_root_flips_enabled_flag_without_touching_files() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        upsert_file(&conn, &meta(r.id, "/a/keep.txt", "keep.txt")).unwrap();

        disable_root(&conn, r.id).unwrap();

        let after = get_root(&conn, r.id).unwrap();
        assert!(!after.enabled, "disable should set enabled=0");

        // Files for the disabled root must remain — index is preserved.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [r.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "disable_root must NOT delete files");
    }

    #[test]
    fn disable_root_unknown_id_returns_not_found() {
        let conn = init_in_memory_db().unwrap();
        let err = disable_root(&conn, 9999).unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[test]
    fn delete_root_cascades_to_files_and_scan_jobs() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        upsert_file(&conn, &meta(r.id, "/a/x.txt", "x.txt")).unwrap();
        let job_id = create_scan_job(&conn, r.id).unwrap();
        finish_scan_job(&conn, job_id, ScanJobStatus::Success, None).unwrap();

        delete_root(&conn, r.id).unwrap();

        let roots_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM roots WHERE id = ?1", [r.id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(roots_left, 0);

        let files_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [r.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(files_left, 0, "FK cascade must remove files");

        let jobs_left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM scan_jobs WHERE root_id = ?1",
                [r.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(jobs_left, 0, "FK cascade must remove scan_jobs");

        // FTS must also be cleaned up via the AFTER DELETE trigger on files.
        assert!(search_files(&conn, "x", 10, 0).unwrap().is_empty());
    }

    #[test]
    fn delete_root_unknown_id_returns_not_found() {
        let conn = init_in_memory_db().unwrap();
        let err = delete_root(&conn, 9999).unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[test]
    fn get_latest_scan_job_for_root_returns_most_recent_or_none() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/a", RootKind::Local).unwrap();
        assert!(get_latest_scan_job_for_root(&conn, r.id).unwrap().is_none());

        let j1 = create_scan_job(&conn, r.id).unwrap();
        finish_scan_job(&conn, j1, ScanJobStatus::Success, None).unwrap();
        let j2 = create_scan_job(&conn, r.id).unwrap();

        let latest = get_latest_scan_job_for_root(&conn, r.id).unwrap().unwrap();
        assert_eq!(latest.id, j2);
        assert_eq!(latest.status, ScanJobStatus::Running);
    }

    #[test]
    fn search_files_returns_current_root_status() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/Volumes/Backup", RootKind::Nas).unwrap();
        upsert_file(
            &conn,
            &meta(r.id, "/Volumes/Backup/file.txt", "file.txt"),
        )
        .unwrap();

        let hits_avail = search_files(&conn, "file", 10, 0).unwrap();
        assert_eq!(hits_avail.len(), 1);
        assert_eq!(hits_avail[0].root_status, RootStatus::Available);

        update_root_status(&conn, r.id, RootStatus::Disconnected).unwrap();
        let hits_disc = search_files(&conn, "file", 10, 0).unwrap();
        assert_eq!(hits_disc.len(), 1, "old index must remain searchable");
        assert_eq!(
            hits_disc[0].root_status,
            RootStatus::Disconnected,
            "FileHit must carry the live root status"
        );
    }

    #[test]
    fn find_root_for_path_returns_owning_root() {
        let conn = init_in_memory_db().unwrap();
        let r = add_root(&conn, "/Volumes/Backup", RootKind::Nas).unwrap();

        // Path under the root.
        let hit = find_root_for_path(&conn, "/Volumes/Backup/2026/notes.txt")
            .unwrap()
            .expect("expected a match");
        assert_eq!(hit.id, r.id);

        // Path equal to the root path itself.
        let exact = find_root_for_path(&conn, "/Volumes/Backup")
            .unwrap()
            .expect("root path should match itself");
        assert_eq!(exact.id, r.id);
    }

    #[test]
    fn find_root_for_path_returns_none_for_unowned_path() {
        let conn = init_in_memory_db().unwrap();
        add_root(&conn, "/Volumes/Backup", RootKind::Nas).unwrap();
        let none = find_root_for_path(&conn, "/Users/keychron/Other").unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn find_root_for_path_picks_longest_match_for_nested_roots() {
        let conn = init_in_memory_db().unwrap();
        let outer = add_root(&conn, "/Volumes", RootKind::Nas).unwrap();
        let inner = add_root(&conn, "/Volumes/Backup", RootKind::Nas).unwrap();

        let hit = find_root_for_path(&conn, "/Volumes/Backup/x.txt")
            .unwrap()
            .unwrap();
        assert_eq!(hit.id, inner.id, "longest matching root must win");

        let outer_hit = find_root_for_path(&conn, "/Volumes/Other.dmg")
            .unwrap()
            .unwrap();
        assert_eq!(outer_hit.id, outer.id);
    }

    #[test]
    fn find_root_for_path_does_not_match_sibling_prefix() {
        let conn = init_in_memory_db().unwrap();
        add_root(&conn, "/Users/me/Photos", RootKind::Local).unwrap();
        // "/Users/me/PhotosArchive/x" must NOT match "/Users/me/Photos".
        let none = find_root_for_path(&conn, "/Users/me/PhotosArchive/x").unwrap();
        assert!(
            none.is_none(),
            "prefix-without-slash must not be treated as a parent"
        );
    }

    #[test]
    fn init_db_on_disk_persists_across_connections() {
        let dir = std::env::temp_dir().join(format!(
            "find_profile_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");

        {
            let conn = init_db(&db_path).unwrap();
            let r = add_root(&conn, "/a", RootKind::Local).unwrap();
            upsert_file(&conn, &meta(r.id, "/a/keep.txt", "keep.txt")).unwrap();
        }

        {
            let conn = init_db(&db_path).unwrap();
            let roots = list_roots(&conn).unwrap();
            assert_eq!(roots.len(), 1);
            let hits = search_files(&conn, "keep", 10, 0).unwrap();
            assert_eq!(hits.len(), 1);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

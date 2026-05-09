use rusqlite::{params, Connection, Transaction};

use super::error::Result;
use super::model::{FileHit, FileMetadata};
use super::util::now_iso;

const UPSERT_SQL: &str = r#"
INSERT INTO files (
    root_id, path, name, extension, size, modified_at,
    is_dir, source_kind, is_accessible, indexed_at
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(path) DO UPDATE SET
    root_id = excluded.root_id,
    name = excluded.name,
    extension = excluded.extension,
    size = excluded.size,
    modified_at = excluded.modified_at,
    is_dir = excluded.is_dir,
    source_kind = excluded.source_kind,
    is_accessible = excluded.is_accessible,
    indexed_at = excluded.indexed_at
RETURNING id
"#;

fn do_upsert(conn: &Connection, m: &FileMetadata) -> Result<i64> {
    let now = now_iso();
    let id: i64 = conn.query_row(
        UPSERT_SQL,
        params![
            m.root_id,
            m.path,
            m.name,
            m.extension,
            m.size,
            m.modified_at,
            m.is_dir,
            m.source_kind,
            m.is_accessible,
            now,
        ],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn upsert_file(conn: &Connection, metadata: &FileMetadata) -> Result<i64> {
    do_upsert(conn, metadata)
}

pub fn upsert_file_in_transaction(tx: &Transaction<'_>, metadata: &FileMetadata) -> Result<i64> {
    do_upsert(tx, metadata)
}

pub fn begin_index_transaction(conn: &mut Connection) -> Result<Transaction<'_>> {
    Ok(conn.transaction()?)
}

pub fn remove_file(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM files WHERE path = ?1", params![path])?;
    Ok(())
}

const SEARCH_SQL: &str = r#"
SELECT f.id, f.root_id, f.path, f.name, f.extension, f.size, f.modified_at,
       f.is_dir, f.source_kind, f.is_accessible, f.indexed_at,
       r.status AS root_status
FROM files_fts fts
JOIN files f ON f.id = fts.rowid
JOIN roots r ON r.id = f.root_id
WHERE files_fts MATCH ?1
ORDER BY rank
LIMIT ?2 OFFSET ?3
"#;

pub fn search_files(
    conn: &Connection,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<FileHit>> {
    let fts_query = build_fts_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(SEARCH_SQL)?;
    let rows = stmt.query_map(params![fts_query, limit, offset], |row| {
        Ok(FileHit {
            id: row.get(0)?,
            root_id: row.get(1)?,
            path: row.get(2)?,
            name: row.get(3)?,
            extension: row.get(4)?,
            size: row.get(5)?,
            modified_at: row.get(6)?,
            is_dir: row.get::<_, i64>(7)? != 0,
            source_kind: row.get(8)?,
            is_accessible: row.get::<_, i64>(9)? != 0,
            indexed_at: row.get(10)?,
            root_status: row.get(11)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn build_fts_query(input: &str) -> String {
    input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}*", t.to_lowercase()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod query_tests {
    use super::build_fts_query;

    #[test]
    fn build_fts_query_splits_and_lowercases() {
        assert_eq!(build_fts_query("Report.PDF"), "report* pdf*");
        assert_eq!(build_fts_query("  annual   report  "), "annual* report*");
    }

    #[test]
    fn build_fts_query_handles_empty() {
        assert_eq!(build_fts_query(""), "");
        assert_eq!(build_fts_query("   "), "");
        assert_eq!(build_fts_query("---"), "");
    }
}

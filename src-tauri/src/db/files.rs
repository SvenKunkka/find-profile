use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, Transaction};
use serde::{Deserialize, Serialize};

use super::error::Result;
use super::model::{FileHit, FileMetadata, RootKind};
use super::util::now_iso;

const UPSERT_SQL: &str = r#"
INSERT INTO files (
    root_id, path, name, extension, size, modified_at, added_at,
    is_dir, source_kind, is_accessible, indexed_at
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
ON CONFLICT(path) DO UPDATE SET
    root_id = excluded.root_id,
    name = excluded.name,
    extension = excluded.extension,
    size = excluded.size,
    modified_at = excluded.modified_at,
    added_at = excluded.added_at,
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
            m.added_at,
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

/// Subset of `files` columns that the scanner uses to decide whether a
/// previously-indexed entry is still up to date. Loading just these columns
/// (instead of full `FileMetadata`) keeps the per-root memory footprint small
/// even for million-entry roots.
#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub size: Option<i64>,
    pub modified_at: Option<String>,
    pub added_at: Option<String>,
    pub is_dir: bool,
}

/// Load every indexed entry for `root_id` into a `(path → snapshot)` map.
///
/// Used by the scanner's incremental flow: while walking the filesystem we
/// `remove()` each entry we still see; whatever remains in the map at the
/// end is now missing and gets marked inaccessible (only when the walk
/// completed cleanly — see `scan_cancellable`).
pub fn load_root_file_index(
    conn: &Connection,
    root_id: i64,
) -> Result<std::collections::HashMap<String, FileSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT path, size, modified_at, added_at, is_dir FROM files WHERE root_id = ?1",
    )?;
    let rows = stmt.query_map(params![root_id], |row| {
        let path: String = row.get(0)?;
        let snap = FileSnapshot {
            size: row.get(1)?,
            modified_at: row.get(2)?,
            added_at: row.get(3)?,
            is_dir: row.get::<_, i64>(4)? != 0,
        };
        Ok((path, snap))
    })?;
    let mut map = std::collections::HashMap::new();
    for r in rows {
        let (p, s) = r?;
        map.insert(p, s);
    }
    Ok(map)
}

/// Mark a batch of paths under `root_id` as `is_accessible = 0` without
/// deleting their FTS rows. Used when the scanner finishes a successful walk
/// and the leftover paths in `load_root_file_index`'s map must be flagged as
/// no-longer-present on disk.
///
/// We never call this on cancellation or preflight failure — both leave the
/// index untouched so old data stays usable.
pub fn mark_files_inaccessible(tx: &Transaction<'_>, root_id: i64, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let now = now_iso();
    // Chunk so we never produce a query with > 999 placeholders (SQLite's
    // default `SQLITE_MAX_VARIABLE_NUMBER`). 500 leaves headroom for the
    // fixed parameters and matches the scanner's batch size.
    const CHUNK: usize = 500;
    for chunk in paths.chunks(CHUNK) {
        // Param order: now (1), root_id (2), then one path per slot.
        let mut placeholders = String::new();
        for i in 0..chunk.len() {
            if i > 0 {
                placeholders.push(',');
            }
            placeholders.push_str(&format!("?{}", i + 3));
        }
        let sql = format!(
            "UPDATE files SET is_accessible = 0, indexed_at = ?1
             WHERE root_id = ?2 AND path IN ({placeholders})"
        );
        let mut stmt = tx.prepare(&sql)?;
        let mut params: Vec<rusqlite::types::Value> = Vec::with_capacity(chunk.len() + 2);
        params.push(rusqlite::types::Value::Text(now.clone()));
        params.push(rusqlite::types::Value::Integer(root_id));
        for p in chunk {
            params.push(rusqlite::types::Value::Text(p.clone()));
        }
        stmt.execute(params_from_iter(params.iter()))?;
    }
    Ok(())
}

const SEARCH_SELECT: &str =
    "SELECT f.id, f.root_id, f.path, f.name, f.extension, f.size, f.modified_at, f.added_at,
       f.is_dir, f.source_kind, f.is_accessible, f.indexed_at,
       r.status AS root_status
FROM files_fts fts
JOIN files f ON f.id = fts.rowid
JOIN roots r ON r.id = f.root_id
WHERE files_fts MATCH ?";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// FTS5 BM25 with the `name` column weighted 10× the `path` column.
    #[default]
    Relevance,
    /// `modified_at DESC NULLS LAST` — newest first.
    Modified,
    /// File-system creation/birth time, used as the macOS "date added" value.
    Added,
    /// `size DESC NULLS LAST` — largest first.
    Size,
    /// `name COLLATE NOCASE ASC` — A→Z, case-insensitive.
    Name,
    Extension,
    Type,
    Source,
    Accessibility,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

impl SortDirection {
    fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub query: String,
    pub limit: i64,
    pub offset: i64,
    /// Lowercase, dot-stripped extensions. Empty = no filter.
    pub extensions: Vec<String>,
    /// `None` = both local and nas roots.
    pub source: Option<RootKind>,
    /// `None` = both files and directories. `Some(true)` = directories only.
    pub is_dir_filter: Option<bool>,
    pub sort: SortBy,
    pub sort_direction: SortDirection,
}

impl SearchOptions {
    /// Defaults: no filters, relevance sort.
    pub fn simple(query: impl Into<String>, limit: i64, offset: i64) -> Self {
        Self {
            query: query.into(),
            limit,
            offset,
            extensions: Vec::new(),
            source: None,
            is_dir_filter: None,
            sort: SortBy::Relevance,
            sort_direction: SortDirection::Asc,
        }
    }
}

/// Backwards-compatible search: relevance sort, no filters.
pub fn search_files(
    conn: &Connection,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<FileHit>> {
    search_files_with_options(conn, &SearchOptions::simple(query, limit, offset))
}

pub fn search_files_with_options(
    conn: &Connection,
    options: &SearchOptions,
) -> Result<Vec<FileHit>> {
    let fts_query = build_fts_query(&options.query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let (sql, params) = build_search_sql(options, fts_query);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |row| {
        Ok(FileHit {
            id: row.get(0)?,
            root_id: row.get(1)?,
            path: row.get(2)?,
            name: row.get(3)?,
            extension: row.get(4)?,
            size: row.get(5)?,
            modified_at: row.get(6)?,
            added_at: row.get(7)?,
            is_dir: row.get::<_, i64>(8)? != 0,
            source_kind: row.get(9)?,
            is_accessible: row.get::<_, i64>(10)? != 0,
            indexed_at: row.get(11)?,
            root_status: row.get(12)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn build_search_sql(options: &SearchOptions, fts_query: String) -> (String, Vec<Value>) {
    let mut sql = String::from(SEARCH_SELECT);
    let mut params: Vec<Value> = vec![Value::Text(fts_query)];

    if let Some(kind) = options.source {
        sql.push_str(" AND f.source_kind = ?");
        params.push(Value::Text(kind.as_str().to_string()));
    }

    if let Some(is_dir) = options.is_dir_filter {
        sql.push_str(" AND f.is_dir = ?");
        params.push(Value::Integer(if is_dir { 1 } else { 0 }));
    }

    if !options.extensions.is_empty() {
        let placeholders = vec!["?"; options.extensions.len()].join(",");
        sql.push_str(&format!(" AND f.extension IN ({placeholders})"));
        for ext in &options.extensions {
            params.push(Value::Text(ext.trim_start_matches('.').to_lowercase()));
        }
    }

    let dir = options.sort_direction.sql();
    match options.sort {
        // BM25 returns lower scores for stronger matches; weighting `name`
        // 10× `path` makes filename matches outrank path-only matches.
        SortBy::Relevance => {
            sql.push_str(" ORDER BY bm25(files_fts, 10.0, 1.0) ASC");
        }
        SortBy::Modified => {
            sql.push_str(&format!(" ORDER BY f.modified_at {dir} NULLS LAST"));
        }
        SortBy::Added => {
            sql.push_str(&format!(" ORDER BY f.added_at {dir} NULLS LAST"));
        }
        SortBy::Size => {
            sql.push_str(&format!(" ORDER BY f.size {dir} NULLS LAST"));
        }
        SortBy::Name => {
            sql.push_str(&format!(" ORDER BY f.name COLLATE NOCASE {dir}"));
        }
        SortBy::Extension => {
            sql.push_str(&format!(
                " ORDER BY f.extension IS NULL ASC, f.extension COLLATE NOCASE {dir}"
            ));
        }
        SortBy::Type => {
            sql.push_str(&format!(" ORDER BY f.is_dir {dir}"));
        }
        SortBy::Source => {
            sql.push_str(&format!(" ORDER BY f.source_kind COLLATE NOCASE {dir}"));
        }
        SortBy::Accessibility => {
            sql.push_str(&format!(
                " ORDER BY CASE WHEN r.status IN ('disconnected', 'error') THEN 0 ELSE f.is_accessible END {dir}"
            ));
        }
    }

    sql.push_str(", f.name COLLATE NOCASE ASC, f.path COLLATE NOCASE ASC");

    sql.push_str(" LIMIT ? OFFSET ?");
    params.push(Value::Integer(options.limit));
    params.push(Value::Integer(options.offset));

    (sql, params)
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

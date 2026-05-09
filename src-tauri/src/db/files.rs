use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, Transaction};
use serde::{Deserialize, Serialize};

use super::error::Result;
use super::model::{FileHit, FileMetadata, RootKind};
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

const SEARCH_SELECT: &str = "SELECT f.id, f.root_id, f.path, f.name, f.extension, f.size, f.modified_at,
       f.is_dir, f.source_kind, f.is_accessible, f.indexed_at,
       r.status AS root_status
FROM files_fts fts
JOIN files f ON f.id = fts.rowid
JOIN roots r ON r.id = f.root_id
WHERE files_fts MATCH ?";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// FTS5 BM25 with the `name` column weighted 10× the `path` column.
    Relevance,
    /// `modified_at DESC NULLS LAST` — newest first.
    Modified,
    /// `size DESC NULLS LAST` — largest first.
    Size,
    /// `name COLLATE NOCASE ASC` — A→Z, case-insensitive.
    Name,
}

impl Default for SortBy {
    fn default() -> Self {
        SortBy::Relevance
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
            params.push(Value::Text(
                ext.trim_start_matches('.').to_lowercase(),
            ));
        }
    }

    sql.push_str(match options.sort {
        // BM25 returns lower scores for stronger matches; weighting `name`
        // 10× `path` makes filename matches outrank path-only matches.
        SortBy::Relevance => " ORDER BY bm25(files_fts, 10.0, 1.0) ASC",
        SortBy::Modified => " ORDER BY f.modified_at DESC NULLS LAST",
        SortBy::Size => " ORDER BY f.size DESC NULLS LAST",
        SortBy::Name => " ORDER BY f.name COLLATE NOCASE ASC",
    });

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

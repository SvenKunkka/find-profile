use rusqlite::Connection;

use super::error::Result;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS roots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT UNIQUE NOT NULL,
    kind TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL,
    scan_interval TEXT,
    last_scanned_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root_id INTEGER NOT NULL,
    path TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    extension TEXT,
    size INTEGER,
    modified_at TEXT,
    is_dir INTEGER NOT NULL,
    source_kind TEXT NOT NULL,
    is_accessible INTEGER NOT NULL,
    indexed_at TEXT NOT NULL,
    FOREIGN KEY(root_id) REFERENCES roots(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_files_root_id ON files(root_id);

CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
    name,
    path,
    content='files',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files BEGIN
    INSERT INTO files_fts(rowid, name, path) VALUES (new.id, new.name, new.path);
END;

CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name, path) VALUES('delete', old.id, old.name, old.path);
END;

CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE ON files BEGIN
    INSERT INTO files_fts(files_fts, rowid, name, path) VALUES('delete', old.id, old.name, old.path);
    INSERT INTO files_fts(rowid, name, path) VALUES (new.id, new.name, new.path);
END;

CREATE TABLE IF NOT EXISTS scan_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root_id INTEGER NOT NULL,
    status TEXT NOT NULL,
    scanned_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    error_message TEXT,
    estimated_total_count INTEGER,
    current_path TEXT,
    FOREIGN KEY(root_id) REFERENCES roots(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_scan_jobs_root_id ON scan_jobs(root_id);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"#;

/// Idempotent ALTER TABLE statements applied after the canonical CREATE
/// TABLE. Fresh installs already get the new columns from `SCHEMA_SQL`;
/// existing on-disk databases need these to add the columns. Each statement
/// is wrapped in `.ok()` so the SQLite "duplicate column name" error from a
/// re-run is silently ignored — there is no point in matching on the error
/// string.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE scan_jobs ADD COLUMN estimated_total_count INTEGER",
    "ALTER TABLE scan_jobs ADD COLUMN current_path TEXT",
];

pub fn apply(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL)?;
    for stmt in MIGRATIONS {
        let _ = conn.execute(stmt, []);
    }
    Ok(())
}

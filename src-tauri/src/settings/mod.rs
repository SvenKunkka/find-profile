use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{now_iso, DbError};

/// Single row in the `settings` table holds the entire `Settings` struct as
/// JSON under this key. Schema is deliberately key/value-shaped so future
/// additions (per-root preferences, feature flags) can use the same table
/// without migrations.
const KEY: &str = "app";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Default page size for the search command when the frontend does not
    /// supply one.
    pub search_default_limit: i64,

    /// Whether to call `WatcherRegistry::restore_all` at app start. When
    /// disabled, local roots only update via manual `Scan`.
    pub restore_watchers_on_startup: bool,

    /// Reserved: minutes between scheduled NAS rescans. The MVP scheduler
    /// is not implemented yet — this value persists for forward compatibility.
    /// `None` means "do not auto-scan NAS roots".
    pub nas_scan_interval_minutes: Option<i64>,

    /// Index entries whose basename starts with `.`. Default off.
    pub show_hidden_files: bool,

    /// Index regular files (and symlinks).
    pub index_files: bool,

    /// Index directory entries themselves.
    pub index_folders: bool,

    /// Hard rule: when a NAS root becomes unreachable, keep the existing
    /// rows in `files`. The MVP enforces this in scanner regardless of the
    /// setting; the toggle exists for visibility and for future opt-out.
    pub keep_disconnected_nas_index: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            search_default_limit: 50,
            restore_watchers_on_startup: true,
            nas_scan_interval_minutes: None,
            show_hidden_files: false,
            index_files: true,
            index_folders: true,
            keep_disconnected_nas_index: true,
        }
    }
}

impl Settings {
    /// Bring user-supplied values into safe ranges. Called on every save so
    /// the frontend can pass raw input without policing it.
    pub fn sanitized(mut self) -> Self {
        if self.search_default_limit < 1 {
            self.search_default_limit = 1;
        }
        if self.search_default_limit > 1000 {
            self.search_default_limit = 1000;
        }
        if let Some(m) = self.nas_scan_interval_minutes {
            if m < 0 {
                self.nas_scan_interval_minutes = None;
            }
        }
        // Keep-on-disconnect cannot be turned off in MVP — see scanner docs.
        self.keep_disconnected_nas_index = true;
        self
    }
}

/// Load settings, returning defaults if the row is missing or its JSON is
/// corrupt. We never fail the app over corrupt settings — best-effort.
pub fn load(conn: &Connection) -> Result<Settings, DbError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match row {
        Some(json) => serde_json::from_str::<Settings>(&json).unwrap_or_default(),
        None => Settings::default(),
    })
}

pub fn save(conn: &Connection, s: &Settings) -> Result<(), DbError> {
    let json = serde_json::to_string(s).unwrap_or_else(|_| {
        serde_json::to_string(&Settings::default()).unwrap_or_else(|_| String::from("{}"))
    });
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
           value = excluded.value,
           updated_at = excluded.updated_at",
        params![KEY, json, now_iso()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_in_memory_db;

    #[test]
    fn default_values_match_spec() {
        let s = Settings::default();
        assert_eq!(s.search_default_limit, 50);
        assert!(s.restore_watchers_on_startup);
        assert!(s.nas_scan_interval_minutes.is_none());
        assert!(!s.show_hidden_files);
        assert!(s.index_files);
        assert!(s.index_folders);
        assert!(s.keep_disconnected_nas_index);
    }

    #[test]
    fn load_returns_defaults_when_row_is_missing() {
        let conn = init_in_memory_db().unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let conn = init_in_memory_db().unwrap();
        let mut s = Settings::default();
        s.search_default_limit = 25;
        s.show_hidden_files = true;
        s.index_files = false;
        s.nas_scan_interval_minutes = Some(360);
        save(&conn, &s).unwrap();
        let loaded = load(&conn).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_returns_defaults_on_corrupt_json() {
        let conn = init_in_memory_db().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('app', 'not-json', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn save_overwrites_existing_row() {
        let conn = init_in_memory_db().unwrap();
        let mut s = Settings::default();
        s.search_default_limit = 100;
        save(&conn, &s).unwrap();
        s.search_default_limit = 200;
        save(&conn, &s).unwrap();
        let loaded = load(&conn).unwrap();
        assert_eq!(loaded.search_default_limit, 200);
    }

    #[test]
    fn sanitized_clamps_search_limit() {
        let mut s = Settings::default();
        s.search_default_limit = 0;
        assert_eq!(s.sanitized().search_default_limit, 1);
        let mut s = Settings::default();
        s.search_default_limit = 9999;
        assert_eq!(s.sanitized().search_default_limit, 1000);
    }

    #[test]
    fn sanitized_drops_negative_nas_interval() {
        let mut s = Settings::default();
        s.nas_scan_interval_minutes = Some(-5);
        assert_eq!(s.sanitized().nas_scan_interval_minutes, None);
    }

    #[test]
    fn sanitized_forces_keep_disconnected_index_on() {
        let mut s = Settings::default();
        s.keep_disconnected_nas_index = false;
        assert!(s.sanitized().keep_disconnected_nas_index);
    }

    #[test]
    fn load_recovers_after_partial_field_update() {
        // Older app version saved a Settings JSON missing fields added later.
        // `#[serde(default)]` should fill them in instead of failing.
        let conn = init_in_memory_db().unwrap();
        let now = now_iso();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('app', ?1, ?2)",
            params![r#"{"search_default_limit": 7}"#, now],
        )
        .unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s.search_default_limit, 7);
        assert!(s.index_files);
        assert!(s.index_folders);
        assert!(s.keep_disconnected_nas_index);
    }
}

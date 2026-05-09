use serde::Serialize;
use thiserror::Error;

use crate::db::DbError;
use crate::roots::RootError;
use crate::scanner::ScanError;

/// Structured error surfaced to the frontend.
///
/// Always carries a stable `code` (machine-readable) and a `message`
/// (human-readable). Tauri serialises this via `serde_json` so the JS side
/// receives `{ code, message }` instead of an opaque string.
#[derive(Debug, Error, Serialize)]
#[error("{message}")]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    pub fn new<C: Into<String>, M: Into<String>>(code: C, message: M) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<DbError> for CommandError {
    fn from(e: DbError) -> Self {
        let code = match &e {
            DbError::NotFound(_) => "not_found",
            DbError::Sqlite(_) => "db",
        };
        Self::new(code, e.to_string())
    }
}

impl From<RootError> for CommandError {
    fn from(e: RootError) -> Self {
        let code = match &e {
            RootError::InvalidKind(_) => "invalid_kind",
            RootError::Db(DbError::NotFound(_)) => "not_found",
            RootError::Db(_) => "db",
        };
        Self::new(code, e.to_string())
    }
}

impl From<ScanError> for CommandError {
    fn from(e: ScanError) -> Self {
        Self::new("scan", e.to_string())
    }
}

impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io", e.to_string())
    }
}

pub type CommandResult<T> = Result<T, CommandError>;

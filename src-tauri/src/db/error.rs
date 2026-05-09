use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

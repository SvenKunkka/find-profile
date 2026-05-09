use thiserror::Error;

use crate::db::DbError;

#[derive(Debug, Error)]
pub enum RootError {
    #[error("invalid root kind: '{0}' (expected 'local' or 'nas')")]
    InvalidKind(String),

    #[error(transparent)]
    Db(#[from] DbError),
}

pub type Result<T> = std::result::Result<T, RootError>;

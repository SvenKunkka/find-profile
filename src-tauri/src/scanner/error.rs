use thiserror::Error;

use crate::db::DbError;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error(transparent)]
    Db(#[from] DbError),
}

pub type Result<T> = std::result::Result<T, ScanError>;

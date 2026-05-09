use thiserror::Error;

use crate::db::DbError;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error("scan database mutex poisoned")]
    PoisonedLock,
}

pub type Result<T> = std::result::Result<T, ScanError>;

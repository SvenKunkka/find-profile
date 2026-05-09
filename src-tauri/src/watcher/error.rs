use thiserror::Error;

use crate::db::DbError;

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error(transparent)]
    Notify(#[from] notify::Error),

    #[error(transparent)]
    Db(#[from] DbError),

    #[error("watcher mutex poisoned")]
    PoisonedLock,
}

pub type Result<T> = std::result::Result<T, WatcherError>;

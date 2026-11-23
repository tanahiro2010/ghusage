use std::path::PathBuf;

use crate::error::Result;
use crate::session::{SessionConfig, open_store};
use ghusage_storage::{Store, SyncLock};

pub(crate) struct OpenCache {
    pub store: Store,
    pub data_dir: Option<PathBuf>,
}

pub(crate) fn open(cfg: &SessionConfig) -> Result<OpenCache> {
    let (store, data_dir) = open_store(cfg)?;
    Ok(OpenCache { store, data_dir })
}

pub(crate) fn lock(data_dir: Option<&PathBuf>) -> Result<Option<SyncLock>> {
    data_dir
        .map(|dir| SyncLock::acquire(dir))
        .transpose()
        .map_err(Into::into)
}

/// An exclusive lock preventing overlapping cache syncs.
#[derive(Debug)]
pub struct SyncLock {
    file: File,
    path: PathBuf,
}

impl SyncLock {
    /// Acquires `<dir>/sync.lock` without blocking; fails with
    /// [`StorageError::Locked`] if another process holds it.
    pub fn acquire(dir: &Path) -> Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join("sync.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path)?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => return Err(StorageError::Locked(path)),
            Err(fs4::TryLockError::Error(e)) => return Err(StorageError::Io(e)),
        }
        // Best effort: record the holder's process id for diagnostics.
        if file.set_len(0).is_ok() {
            drop(writeln!(file, "{}", std::process::id()));
        }
        Ok(Self { file, path })
    }

    /// Path of the lock file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        drop(fs4::FileExt::unlock(&self.file));
    }
}

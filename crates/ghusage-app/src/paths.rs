//! Location of the local cache.

use std::path::PathBuf;

/// Environment variable overriding the data directory.
pub const DATA_DIR_ENV: &str = "GHUSAGE_DATA_DIR";

/// Resolves the data directory: explicit value, then `GHUSAGE_DATA_DIR`,
/// then the OS user data directory.
pub fn data_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Some(p) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    directories::ProjectDirs::from("dev", "ghusage", "ghusage").map(|d| d.data_dir().to_path_buf())
}

/// Path of the cache database inside a data directory.
pub fn db_path(dir: &std::path::Path) -> PathBuf {
    dir.join("ghusage.db")
}

//! Location of the local cache.

use std::path::PathBuf;

/// Environment variable overriding the data directory.
pub const DATA_DIR_ENV: &str = "GHUSAGE_DATA_DIR";

/// A resolved data directory and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    /// The directory.
    pub path: PathBuf,
    /// Whether the user chose it (`--data-dir` or `GHUSAGE_DATA_DIR`)
    /// rather than ghusage using the OS user data directory.
    pub user_chosen: bool,
}

/// Resolves the data directory: explicit value, then `GHUSAGE_DATA_DIR`,
/// then the OS user data directory.
#[must_use]
pub fn data_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    resolve_data_dir(explicit).map(|d| d.path)
}

/// Like [`data_dir`], and also reports whether the user chose the directory.
#[must_use]
pub fn resolve_data_dir(explicit: Option<PathBuf>) -> Option<DataDir> {
    if let Some(p) = explicit {
        return Some(DataDir {
            path: p,
            user_chosen: true,
        });
    }
    if let Some(p) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return Some(DataDir {
            path: PathBuf::from(p),
            user_chosen: true,
        });
    }
    directories::ProjectDirs::from("dev", "ghusage", "ghusage").map(|d| DataDir {
        path: d.data_dir().to_path_buf(),
        user_chosen: false,
    })
}

/// Path of the cache database inside a data directory.
#[must_use]
pub fn db_path(dir: &std::path::Path) -> PathBuf {
    dir.join("ghusage.db")
}

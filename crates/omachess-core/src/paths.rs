//! Filesystem locations, honouring `OMACHESS_PROFILE`.
//!
//! A development run sets `OMACHESS_PROFILE=dev` along with redirected XDG
//! variables (see `scripts/dev.sh`), so it can never read or write the real
//! review database.

use std::env;
use std::path::PathBuf;

/// Base directory for application data, following the XDG spec.
pub fn data_dir() -> PathBuf {
    let base = match env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home_dir().join(".local/share"),
    };
    base.join("omachess")
}

/// Path to the SQLite database holding puzzles, cards and attempts.
pub fn db_path() -> PathBuf {
    data_dir().join("omachess.sqlite")
}

/// Directory for downloaded corpora. Kept outside the repository so that
/// `cargo clean` and git never touch several hundred megabytes of puzzles.
pub fn corpus_dir() -> PathBuf {
    data_dir().join("corpus")
}

/// Base cache directory, for things that can be regenerated.
pub fn cache_dir() -> PathBuf {
    let base = match env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home_dir().join(".cache"),
    };
    base.join("omachess")
}

/// Directory holding installed piece sets, one subdirectory per set.
pub fn pieces_dir() -> PathBuf {
    data_dir().join("pieces")
}

/// True when running against isolated development state.
pub fn is_dev_profile() -> bool {
    env::var("OMACHESS_PROFILE").as_deref() == Ok("dev")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

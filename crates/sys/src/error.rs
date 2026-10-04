use std::io;
use std::path::{Path, PathBuf};

/// Readers skip entries that vanish or aren't readable (expected races and other users'
/// processes) and fail only on unexpected errors.
#[derive(Debug, thiserror::Error)]
pub enum SysError {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("parsing {path}: {detail}")]
    Parse { path: PathBuf, detail: String },
}

impl SysError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn parse(path: &Path, detail: impl Into<String>) -> Self {
        Self::Parse {
            path: path.to_path_buf(),
            detail: detail.into(),
        }
    }
}

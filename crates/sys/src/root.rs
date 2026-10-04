//! Where the kernel interfaces live. Production reads the host's `/`; tests point at a
//! fixture tree with the same layout.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::SysError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysRoot {
    base: PathBuf,
}

impl SysRoot {
    /// The running system.
    #[must_use]
    pub fn host() -> Self {
        Self {
            base: PathBuf::from("/"),
        }
    }

    /// A tree laid out like `/` (containing `sys/`, `proc/`).
    #[must_use]
    pub fn at(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Resolves a path given relative to `/`, e.g. `sys/fs/cgroup`.
    #[must_use]
    pub fn path(&self, relative: &str) -> PathBuf {
        self.base.join(relative.trim_start_matches('/'))
    }

    /// Reads a whole file as text. `Ok(None)` when it is gone (see [`is_gone`]).
    ///
    /// # Errors
    /// Any other I/O error.
    pub fn read_optional(&self, path: &Path) -> Result<Option<String>, SysError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(err) if is_gone(&err) => Ok(None),
            Err(err) => Err(SysError::io(path, err)),
        }
    }
}

/// Errors that mean "this thing no longer exists or isn't ours to read": a cgroup removed or
/// a process that exited between listing and reading, or another user's process.
#[must_use]
pub fn is_gone(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    ) || err.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_to_base() {
        let root = SysRoot::at("/tmp/fixture");
        assert_eq!(
            root.path("/sys/fs/cgroup"),
            PathBuf::from("/tmp/fixture/sys/fs/cgroup")
        );
        assert_eq!(
            SysRoot::host().path("proc/stat"),
            PathBuf::from("/proc/stat")
        );
    }

    #[test]
    fn missing_files_read_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        assert_eq!(root.read_optional(&root.path("nope")).unwrap(), None);
    }
}

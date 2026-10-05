//! One daemon per database: an exclusive `flock` on a file next to it.
//!
//! The bus name alone doesn't guarantee this. A second session bus (`dbus-run-session`, a
//! nested shell) activates its own daemon, and two daemons writing windows into one
//! database double-count energy.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};

/// Exit status when another daemon holds the lock; the systemd unit doesn't restart on it.
pub const EXIT_ALREADY_RUNNING: i32 = 3;

/// Held for the daemon's lifetime; the kernel releases the lock when the process exits.
#[derive(Debug)]
pub struct DatabaseLock {
    _file: File,
}

/// The lock file for `database`: `drainscope.db` → `drainscope.db.lock`.
#[must_use]
pub fn lock_path(database: &Path) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Takes the exclusive lock for `database` without waiting; `None` if another process
/// holds it.
///
/// # Errors
/// If the lock file can't be opened or locked.
pub fn lock_database(database: &Path) -> io::Result<Option<DatabaseLock>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(lock_path(database))?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(DatabaseLock { _file: file })),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_path_appends_suffix() {
        assert_eq!(
            lock_path(Path::new("/s/drainscope/drainscope.db")),
            Path::new("/s/drainscope/drainscope.db.lock")
        );
    }

    #[test]
    fn second_lock_is_refused_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("drainscope.db");

        let first = lock_database(&db).unwrap();
        assert!(first.is_some());
        // flock locks belong to the open file description, so a second open in the same
        // process conflicts just like a second daemon would.
        assert!(lock_database(&db).unwrap().is_none());

        drop(first);
        assert!(lock_database(&db).unwrap().is_some());
    }

    #[test]
    fn missing_directory_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("absent/drainscope.db");
        assert!(lock_database(&db).is_err());
    }
}

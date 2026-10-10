//! Opening the history at start-up. A database from a newer drainscope stops the daemon for
//! good (the data is valid for that version), and one SQLite can't read is set aside so a new
//! history can start; restarting would otherwise fail the same way forever.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use drainscope_store::{Store, StoreError};

/// Exit status for a database written by a newer drainscope; the systemd unit doesn't restart
/// on it.
pub const EXIT_DATABASE_TOO_NEW: i32 = 4;

#[derive(Debug)]
pub enum Opened {
    /// Ready. `set_aside` is where an unreadable database was moved, if one was.
    Ready {
        store: Store,
        set_aside: Option<PathBuf>,
    },
    /// Written by a newer drainscope.
    TooNew(StoreError),
}

/// Opens the history at `path`. An unreadable database is renamed, with its `-wal` and `-shm`
/// files, to `<path>.unreadable-<stamp>` and a new one started.
///
/// # Errors
/// Errors other than the two above, and failures to set the database aside.
pub fn open(path: &Path, stamp: &str) -> anyhow::Result<Opened> {
    match Store::open(path) {
        Ok(store) => Ok(Opened::Ready {
            store,
            set_aside: None,
        }),
        Err(err @ StoreError::TooNew { .. }) => Ok(Opened::TooNew(err)),
        Err(err) if err.is_unreadable() => {
            let aside = set_aside(path, stamp)?;
            tracing::warn!(%err, set_aside = %aside.display(), "the history was unreadable: kept it aside and started a new one");
            Ok(Opened::Ready {
                store: Store::open(path)?,
                set_aside: Some(aside),
            })
        }
        Err(err) => Err(err.into()),
    }
}

/// Renames the database and its WAL and shared-memory files, keeping SQLite's pairing of names
/// (`X`, `X-wal`, `X-shm`) so the set can still be opened together for inspection.
fn set_aside(path: &Path, stamp: &str) -> io::Result<PathBuf> {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".unreadable-{stamp}"));
    let aside = PathBuf::from(aside);
    for suffix in ["", "-wal", "-shm"] {
        let mut from = path.as_os_str().to_owned();
        from.push(suffix);
        let mut to = aside.as_os_str().to_owned();
        to.push(suffix);
        match fs::rename(&from, &to) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(aside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_databases_are_set_aside_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drainscope.db");
        fs::write(&path, [0x5a_u8; 8192]).unwrap();
        let Opened::Ready { set_aside, .. } = open(&path, "1791643439").unwrap() else {
            panic!("expected a new database");
        };
        let aside = dir.path().join("drainscope.db.unreadable-1791643439");
        assert_eq!(set_aside.as_deref(), Some(aside.as_path()));
        assert_eq!(fs::read(&aside).unwrap(), [0x5a_u8; 8192]);
        // The new one is a working database.
        let Opened::Ready { set_aside, .. } = open(&path, "later").unwrap() else {
            panic!("expected the new database");
        };
        assert_eq!(set_aside, None);
    }

    #[test]
    fn newer_databases_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drainscope.db");
        drop(Store::open(&path).unwrap());
        rusqlite_free_bump(&path);
        assert!(matches!(open(&path, "x").unwrap(), Opened::TooNew(_)));
        assert!(path.exists());
        assert!(!dir.path().join("drainscope.db.unreadable-x").exists());
    }

    /// Marks the database as written by a newer schema, without rusqlite: SQLite stores
    /// `user_version` as a big-endian u32 at byte 60 of the header.
    fn rusqlite_free_bump(path: &Path) {
        let mut bytes = fs::read(path).unwrap();
        bytes[60..64].copy_from_slice(&99_u32.to_be_bytes());
        fs::write(path, bytes).unwrap();
    }
}

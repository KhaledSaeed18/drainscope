//! Repository layer: SQLite schema, migrations and usage storage. The only crate that
//! imports `rusqlite`.

mod calibration;
mod events;
mod health;
mod schema;
mod usage;

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

pub use events::{PowerEvent, PowerEventKind, SleepSession};
pub use health::HealthRecord;
pub use usage::{
    FocusRow, Granularity, HOUR_RETENTION_MS, MINUTE_RETENTION_MS, PruneStats, RAW_RETENTION_MS,
    SourceFilter, UsageRow, WindowRecord,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("securing {path}: {source}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
    #[error("corrupt data: {0}")]
    Corrupt(String),
}

impl StoreError {
    /// Whether SQLite can't read the file at all: not a database, or damaged.
    #[must_use]
    pub fn is_unreadable(&self) -> bool {
        matches!(
            self,
            Self::Sqlite(rusqlite::Error::SqliteFailure(err, _))
                if matches!(err.code, rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
        )
    }
}

/// The usage database. One connection, owned by the daemon.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    /// Consumer key → row id, filled only from committed rows.
    consumer_ids: HashMap<String, i64>,
}

impl Store {
    /// Opens (creating if needed) the database at `path`, readable only by its owner.
    ///
    /// # Errors
    /// SQLite errors, failing to restrict permissions, or a schema from a newer build.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let secure = |source| StoreError::Permissions {
            path: path.to_path_buf(),
            source,
        };
        // Create the file 0600 before SQLite sees it, so it is never briefly readable by others.
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(secure)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(secure)?;
        Self::init(Connection::open(path)?)
    }

    /// A private in-memory database, for tests.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        // WAL lets readers proceed during a write; NORMAL is durable enough for statistics.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn,
            consumer_ids: HashMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drainscope.db");
        drop(Store::open(&path).unwrap());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // Reopening an existing database works and keeps the schema.
        Store::open(&path).unwrap();
    }

    #[test]
    fn tells_unreadable_files_from_newer_ones() {
        let dir = tempfile::tempdir().unwrap();
        let garbage = dir.path().join("garbage.db");
        fs::write(&garbage, [0x5a_u8; 8192]).unwrap();
        assert!(Store::open(&garbage).unwrap_err().is_unreadable());

        let newer = dir.path().join("newer.db");
        drop(Store::open(&newer).unwrap());
        Connection::open(&newer)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let err = Store::open(&newer).unwrap_err();
        assert!(matches!(err, StoreError::TooNew { found: 99, .. }));
        assert!(!err.is_unreadable());
    }
}

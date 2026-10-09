//! Schema migrations, tracked with SQLite's `user_version`.
//!
//! Each entry migrates from version `i` to `i + 1`. Never edit a released migration; append a
//! new one.

use rusqlite::Connection;

use crate::StoreError;

const MIGRATIONS: &[&str] = &[
    // 1: windows, per-consumer usage at three resolutions, events, calibration.
    r"
    CREATE TABLE consumers (
        id            INTEGER PRIMARY KEY,
        key           TEXT    NOT NULL UNIQUE,
        first_seen_ms INTEGER NOT NULL
    );

    -- One row per closed attribution window (≥ 10 s).
    CREATE TABLE windows (
        id            INTEGER PRIMARY KEY,
        start_ms      INTEGER NOT NULL,
        end_ms        INTEGER NOT NULL CHECK (end_ms > start_ms),
        power_source  TEXT    NOT NULL CHECK (power_source IN ('battery', 'ac')),
        measurement   TEXT    NOT NULL CHECK (measurement IN ('battery', 'rapl', 'battery-only')),
        measured_j    REAL    NOT NULL CHECK (measured_j >= 0),
        shortfall_j   REAL    NOT NULL CHECK (shortfall_j >= 0),
        model_version INTEGER NOT NULL
    );
    CREATE INDEX windows_by_start ON windows (start_ms);

    CREATE TABLE usage_raw (
        window_id   INTEGER NOT NULL REFERENCES windows (id) ON DELETE CASCADE,
        consumer_id INTEGER NOT NULL REFERENCES consumers (id),
        cpu_j       REAL    NOT NULL,
        gpu_j       REAL    NOT NULL,
        other_j     REAL    NOT NULL,
        PRIMARY KEY (window_id, consumer_id)
    ) WITHOUT ROWID;

    -- Rollups, updated in the same transaction as usage_raw; buckets by window start.
    CREATE TABLE usage_minute (
        bucket_ms    INTEGER NOT NULL,
        power_source TEXT    NOT NULL,
        consumer_id  INTEGER NOT NULL REFERENCES consumers (id),
        cpu_j        REAL    NOT NULL,
        gpu_j        REAL    NOT NULL,
        other_j      REAL    NOT NULL,
        PRIMARY KEY (bucket_ms, power_source, consumer_id)
    ) WITHOUT ROWID;

    CREATE TABLE usage_hour (
        bucket_ms    INTEGER NOT NULL,
        power_source TEXT    NOT NULL,
        consumer_id  INTEGER NOT NULL REFERENCES consumers (id),
        cpu_j        REAL    NOT NULL,
        gpu_j        REAL    NOT NULL,
        other_j      REAL    NOT NULL,
        PRIMARY KEY (bucket_ms, power_source, consumer_id)
    ) WITHOUT ROWID;

    CREATE TABLE power_events (
        id              INTEGER PRIMARY KEY,
        ts_ms           INTEGER NOT NULL,
        kind            TEXT    NOT NULL CHECK (kind IN ('boot', 'plug', 'unplug', 'suspend', 'resume')),
        battery_percent REAL,
        energy_wh       REAL
    );
    CREATE INDEX power_events_by_kind ON power_events (kind, ts_ms);

    CREATE TABLE sleep_sessions (
        id           INTEGER PRIMARY KEY,
        start_ms     INTEGER NOT NULL,
        end_ms       INTEGER NOT NULL CHECK (end_ms >= start_ms),
        wh_lost      REAL,
        percent_lost REAL,
        mem_sleep    TEXT,
        wake_reason  TEXT
    );
    CREATE INDEX sleep_sessions_by_start ON sleep_sessions (start_ms);

    -- Idle-floor histograms per power source; `part` is a RAPL part or 'battery'.
    CREATE TABLE calibration (
        power_source TEXT NOT NULL,
        part         TEXT NOT NULL,
        histogram    BLOB NOT NULL,
        PRIMARY KEY (power_source, part)
    ) WITHOUT ROWID;

    CREATE TABLE psys_check (
        id               INTEGER PRIMARY KEY CHECK (id = 1),
        at_least_package INTEGER NOT NULL,
        below_package    INTEGER NOT NULL
    );
    ",
    // 2: battery wear, one row per battery per day (the day's last reading).
    r"
    CREATE TABLE battery_health (
        battery               TEXT    NOT NULL,
        day                   INTEGER NOT NULL, -- days since the Unix epoch, UTC
        ts_ms                 INTEGER NOT NULL,
        energy_full_wh        REAL    NOT NULL,
        energy_full_design_wh REAL,
        cycle_count           INTEGER,
        PRIMARY KEY (battery, day)
    ) WITHOUT ROWID;
    ",
    // 3: each app's energy split by focus (ADR 0011), kept and rolled up like usage. Only app
    // consumers get rows; the rest of an app's energy in usage_* is background. Every app usage
    // row has a focus row: history from before this migration is all unknown.
    r"
    CREATE TABLE focus_raw (
        window_id    INTEGER NOT NULL REFERENCES windows (id) ON DELETE CASCADE,
        consumer_id  INTEGER NOT NULL REFERENCES consumers (id),
        foreground_j REAL    NOT NULL,
        unknown_j    REAL    NOT NULL,
        focused_ms   INTEGER NOT NULL,
        PRIMARY KEY (window_id, consumer_id)
    ) WITHOUT ROWID;

    CREATE TABLE focus_minute (
        bucket_ms    INTEGER NOT NULL,
        power_source TEXT    NOT NULL,
        consumer_id  INTEGER NOT NULL REFERENCES consumers (id),
        foreground_j REAL    NOT NULL,
        unknown_j    REAL    NOT NULL,
        focused_ms   INTEGER NOT NULL,
        PRIMARY KEY (bucket_ms, power_source, consumer_id)
    ) WITHOUT ROWID;

    CREATE TABLE focus_hour (
        bucket_ms    INTEGER NOT NULL,
        power_source TEXT    NOT NULL,
        consumer_id  INTEGER NOT NULL REFERENCES consumers (id),
        foreground_j REAL    NOT NULL,
        unknown_j    REAL    NOT NULL,
        focused_ms   INTEGER NOT NULL,
        PRIMARY KEY (bucket_ms, power_source, consumer_id)
    ) WITHOUT ROWID;

    INSERT INTO focus_raw (window_id, consumer_id, foreground_j, unknown_j, focused_ms)
        SELECT u.window_id, u.consumer_id, 0, u.cpu_j + u.gpu_j + u.other_j, 0
        FROM usage_raw u JOIN consumers c ON c.id = u.consumer_id WHERE c.key LIKE 'app:%';
    INSERT INTO focus_minute (bucket_ms, power_source, consumer_id, foreground_j, unknown_j, focused_ms)
        SELECT u.bucket_ms, u.power_source, u.consumer_id, 0, u.cpu_j + u.gpu_j + u.other_j, 0
        FROM usage_minute u JOIN consumers c ON c.id = u.consumer_id WHERE c.key LIKE 'app:%';
    INSERT INTO focus_hour (bucket_ms, power_source, consumer_id, foreground_j, unknown_j, focused_ms)
        SELECT u.bucket_ms, u.power_source, u.consumer_id, 0, u.cpu_j + u.gpu_j + u.other_j, 0
        FROM usage_hour u JOIN consumers c ON c.id = u.consumer_id WHERE c.key LIKE 'app:%';
    ",
];

pub(crate) fn supported_version() -> i64 {
    i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX)
}

/// Brings the database to the latest version, one transaction per step.
pub(crate) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let supported = supported_version();
    if current > supported {
        return Err(StoreError::TooNew {
            found: current,
            supported,
        });
    }
    let start = usize::try_from(current)
        .map_err(|_| StoreError::Corrupt(format!("negative schema version {current}")))?;
    for (version, sql) in (1_i64..).zip(MIGRATIONS).skip(start) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migrates_from_empty_once() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(version(&conn), supported_version());
        // Running again is a no-op rather than a "table exists" error.
        migrate(&conn).unwrap();
    }

    #[test]
    fn earlier_app_usage_becomes_unknown_focus() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..2] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute_batch(
            "INSERT INTO consumers (id, key, first_seen_ms) VALUES (1, 'app:org.mozilla.firefox', 0), (2, 'kernel', 0);
             INSERT INTO windows VALUES (1, 0, 10000, 'battery', 'battery', 10, 0, 3);
             INSERT INTO usage_raw VALUES (1, 1, 3, 1, 0.5), (1, 2, 2, 0, 0);
             INSERT INTO usage_minute VALUES (0, 'battery', 1, 3, 1, 0.5), (0, 'battery', 2, 2, 0, 0);
             INSERT INTO usage_hour VALUES (0, 'battery', 1, 3, 1, 0.5), (0, 'battery', 2, 2, 0, 0);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        for table in ["focus_raw", "focus_minute", "focus_hour"] {
            let rows: (i64, i64, f64, f64, i64) = conn
                .query_row(
                    &format!("SELECT COUNT(*), MIN(consumer_id), SUM(foreground_j), SUM(unknown_j), SUM(focused_ms) FROM {table}"),
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .unwrap();
            assert_eq!(rows, (1, 1, 0.0, 4.5, 0), "{table}");
        }
    }

    #[test]
    fn refuses_databases_from_newer_builds() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", supported_version() + 1)
            .unwrap();
        assert!(matches!(migrate(&conn), Err(StoreError::TooNew { .. })));
    }
}

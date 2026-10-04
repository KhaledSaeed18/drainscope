//! Attribution windows and per-consumer usage at three resolutions.
//!
//! Every window is written to `usage_raw` and added to its minute and hour buckets in the same
//! transaction, so rollups always equal the raw data they summarize. Retention then only
//! deletes: raw rows after [`RAW_RETENTION_MS`], minutes after [`MINUTE_RETENTION_MS`], hours
//! after [`HOUR_RETENTION_MS`]. Queries read the finest resolution still covering their range.

use std::collections::HashMap;

use drainscope_model::{ClosedWindow, ConsumerKey, EnergySplit, Joules, PowerSource};
use rusqlite::{Transaction, params};

use crate::{Store, StoreError};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

pub const RAW_RETENTION_MS: i64 = 2 * DAY_MS;
pub const MINUTE_RETENTION_MS: i64 = 30 * DAY_MS;
pub const HOUR_RETENTION_MS: i64 = 365 * DAY_MS;

/// A closed window and when it ran (wall clock, Unix milliseconds).
#[derive(Debug, Clone, Copy)]
pub struct WindowRecord<'a> {
    pub start_ms: i64,
    pub end_ms: i64,
    pub window: &'a ClosedWindow,
    pub model_version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub key: ConsumerKey,
    pub split: EnergySplit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFilter {
    Any,
    Only(PowerSource),
}

impl SourceFilter {
    fn wire_name(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Only(source) => Some(source.wire_name()),
        }
    }
}

/// Which table answers a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Raw,
    Minute,
    Hour,
}

impl Granularity {
    /// The finest resolution still retained for data starting at `since_ms`.
    #[must_use]
    pub fn for_range(since_ms: i64, now_ms: i64) -> Self {
        let age = now_ms.saturating_sub(since_ms);
        if age <= RAW_RETENTION_MS {
            Self::Raw
        } else if age <= MINUTE_RETENTION_MS {
            Self::Minute
        } else {
            Self::Hour
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub windows: usize,
    pub minute_rows: usize,
    pub hour_rows: usize,
    pub consumers: usize,
}

fn bucket(ts_ms: i64, width_ms: i64) -> i64 {
    ts_ms - ts_ms.rem_euclid(width_ms)
}

/// The id for `key`, inserting it if new. New ids are reported through `created` and cached
/// by the caller only after the transaction commits.
fn consumer_id(
    tx: &Transaction<'_>,
    cache: &HashMap<String, i64>,
    created: &mut Vec<(String, i64)>,
    key: &str,
    seen_ms: i64,
) -> Result<i64, StoreError> {
    if let Some(&id) = cache.get(key) {
        return Ok(id);
    }
    tx.prepare_cached(
        "INSERT INTO consumers (key, first_seen_ms) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING",
    )?
    .execute(params![key, seen_ms])?;
    let id: i64 = tx
        .prepare_cached("SELECT id FROM consumers WHERE key = ?1")?
        .query_row(params![key], |row| row.get(0))?;
    created.push((key.to_owned(), id));
    Ok(id)
}

const ADD_TO_MINUTE: &str = "INSERT INTO usage_minute (bucket_ms, power_source, consumer_id, cpu_j, gpu_j, other_j)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
     ON CONFLICT (bucket_ms, power_source, consumer_id) DO UPDATE SET
       cpu_j = cpu_j + excluded.cpu_j, gpu_j = gpu_j + excluded.gpu_j, other_j = other_j + excluded.other_j";
const ADD_TO_HOUR: &str = "INSERT INTO usage_hour (bucket_ms, power_source, consumer_id, cpu_j, gpu_j, other_j)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
     ON CONFLICT (bucket_ms, power_source, consumer_id) DO UPDATE SET
       cpu_j = cpu_j + excluded.cpu_j, gpu_j = gpu_j + excluded.gpu_j, other_j = other_j + excluded.other_j";

impl Store {
    /// Stores a closed window and adds it to its rollups, atomically.
    ///
    /// # Errors
    /// SQLite errors, including constraint violations for malformed windows.
    pub fn record_window(&mut self, record: &WindowRecord<'_>) -> Result<i64, StoreError> {
        let window = record.window;
        let source = window.measurement.power_source().wire_name();
        let Self {
            conn, consumer_ids, ..
        } = self;
        let tx = conn.transaction()?;
        tx.prepare_cached(
            "INSERT INTO windows (start_ms, end_ms, power_source, measurement, measured_j, shortfall_j, model_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?
        .execute(params![
            record.start_ms,
            record.end_ms,
            source,
            window.measurement.wire_name(),
            window.measured.0,
            window.shortfall.0,
            record.model_version,
        ])?;
        let window_id = tx.last_insert_rowid();
        let minute = bucket(record.start_ms, MINUTE_MS);
        let hour = bucket(record.start_ms, HOUR_MS);

        let mut created = Vec::new();
        for (key, split) in &window.ledger {
            let key = key.to_string();
            let id = consumer_id(&tx, consumer_ids, &mut created, &key, record.start_ms)?;
            let (cpu, gpu, other) = (split.cpu.0, split.gpu.0, split.other.0);
            tx.prepare_cached(
                "INSERT INTO usage_raw (window_id, consumer_id, cpu_j, gpu_j, other_j) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![window_id, id, cpu, gpu, other])?;
            tx.prepare_cached(ADD_TO_MINUTE)?
                .execute(params![minute, source, id, cpu, gpu, other])?;
            tx.prepare_cached(ADD_TO_HOUR)?
                .execute(params![hour, source, id, cpu, gpu, other])?;
        }
        tx.commit()?;
        consumer_ids.extend(created);
        Ok(window_id)
    }

    /// Energy per consumer for windows starting in `[since_ms, until_ms)`, largest first,
    /// from the finest resolution still retained (see [`Granularity::for_range`]). Minute and
    /// hour data include whole buckets overlapping the range's start.
    ///
    /// # Errors
    /// SQLite errors, or a stored key that no longer parses.
    pub fn usage(
        &self,
        since_ms: i64,
        until_ms: i64,
        filter: SourceFilter,
        now_ms: i64,
    ) -> Result<Vec<UsageRow>, StoreError> {
        self.usage_at(
            Granularity::for_range(since_ms, now_ms),
            since_ms,
            until_ms,
            filter,
        )
    }

    /// Like [`Self::usage`], at a chosen resolution.
    ///
    /// # Errors
    /// SQLite errors, or a stored key that no longer parses.
    pub fn usage_at(
        &self,
        granularity: Granularity,
        since_ms: i64,
        until_ms: i64,
        filter: SourceFilter,
    ) -> Result<Vec<UsageRow>, StoreError> {
        let (sql, since) = match granularity {
            Granularity::Raw => (
                "SELECT c.key, SUM(u.cpu_j), SUM(u.gpu_j), SUM(u.other_j)
                 FROM usage_raw u
                 JOIN windows w ON w.id = u.window_id
                 JOIN consumers c ON c.id = u.consumer_id
                 WHERE w.start_ms >= ?1 AND w.start_ms < ?2 AND (?3 IS NULL OR w.power_source = ?3)
                 GROUP BY c.key",
                since_ms,
            ),
            Granularity::Minute => (
                "SELECT c.key, SUM(u.cpu_j), SUM(u.gpu_j), SUM(u.other_j)
                 FROM usage_minute u JOIN consumers c ON c.id = u.consumer_id
                 WHERE u.bucket_ms >= ?1 AND u.bucket_ms < ?2 AND (?3 IS NULL OR u.power_source = ?3)
                 GROUP BY c.key",
                bucket(since_ms, MINUTE_MS),
            ),
            Granularity::Hour => (
                "SELECT c.key, SUM(u.cpu_j), SUM(u.gpu_j), SUM(u.other_j)
                 FROM usage_hour u JOIN consumers c ON c.id = u.consumer_id
                 WHERE u.bucket_ms >= ?1 AND u.bucket_ms < ?2 AND (?3 IS NULL OR u.power_source = ?3)
                 GROUP BY c.key",
                bucket(since_ms, HOUR_MS),
            ),
        };
        let mut statement = self.conn.prepare_cached(sql)?;
        let rows = statement.query_map(params![since, until_ms, filter.wire_name()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                EnergySplit {
                    cpu: Joules(row.get(1)?),
                    gpu: Joules(row.get(2)?),
                    other: Joules(row.get(3)?),
                },
            ))
        })?;
        let mut usage = Vec::new();
        for row in rows {
            let (key, split) = row?;
            let key = key
                .parse()
                .map_err(|err| StoreError::Corrupt(format!("consumer key: {err}")))?;
            usage.push(UsageRow { key, split });
        }
        usage.sort_by(|a, b| {
            b.split
                .total()
                .0
                .total_cmp(&a.split.total().0)
                .then_with(|| a.key.cmp(&b.key))
        });
        Ok(usage)
    }

    /// Deletes data past its retention, and consumers no longer referenced.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn prune(&mut self, now_ms: i64) -> Result<PruneStats, StoreError> {
        let tx = self.conn.transaction()?;
        let stats = PruneStats {
            windows: tx.execute(
                "DELETE FROM windows WHERE start_ms < ?1",
                params![now_ms - RAW_RETENTION_MS],
            )?,
            minute_rows: tx.execute(
                "DELETE FROM usage_minute WHERE bucket_ms < ?1",
                params![now_ms - MINUTE_RETENTION_MS],
            )?,
            hour_rows: tx.execute(
                "DELETE FROM usage_hour WHERE bucket_ms < ?1",
                params![now_ms - HOUR_RETENTION_MS],
            )?,
            consumers: tx.execute(
                "DELETE FROM consumers WHERE id NOT IN (
                     SELECT consumer_id FROM usage_raw
                     UNION SELECT consumer_id FROM usage_minute
                     UNION SELECT consumer_id FROM usage_hour)",
                [],
            )?,
        };
        tx.execute(
            "DELETE FROM power_events WHERE ts_ms < ?1",
            params![now_ms - HOUR_RETENTION_MS],
        )?;
        tx.execute(
            "DELETE FROM sleep_sessions WHERE start_ms < ?1",
            params![now_ms - HOUR_RETENTION_MS],
        )?;
        tx.commit()?;
        if stats.consumers > 0 {
            self.consumer_ids.clear();
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drainscope_model::{Ledger, Measurement};
    use proptest::prelude::*;
    use std::time::Duration;

    const T0: i64 = 1_791_000_000_000; // a fixed wall-clock instant

    fn window(measurement: Measurement, entries: &[(ConsumerKey, f64, f64)]) -> ClosedWindow {
        let ledger: Ledger = entries
            .iter()
            .map(|(key, cpu, gpu)| {
                (
                    key.clone(),
                    EnergySplit {
                        cpu: Joules(*cpu),
                        gpu: Joules(*gpu),
                        other: Joules(0.0),
                    },
                )
            })
            .collect();
        let measured = ledger.values().map(EnergySplit::total).sum();
        ClosedWindow {
            duration: Duration::from_secs(10),
            ledger,
            measured,
            measurement,
            shortfall: Joules(0.0),
        }
    }

    fn record(store: &mut Store, start_ms: i64, closed: &ClosedWindow) {
        store
            .record_window(&WindowRecord {
                start_ms,
                end_ms: start_ms + 10_000,
                window: closed,
                model_version: 1,
            })
            .unwrap();
    }

    fn firefox() -> ConsumerKey {
        ConsumerKey::App("org.mozilla.firefox".into())
    }

    #[test]
    fn records_and_queries_usage_by_source() {
        let mut store = Store::open_in_memory().unwrap();
        record(
            &mut store,
            T0,
            &window(
                Measurement::Battery,
                &[(firefox(), 3.0, 1.0), (ConsumerKey::Idle, 12.0, 0.0)],
            ),
        );
        record(
            &mut store,
            T0 + 10_000,
            &window(Measurement::Rapl, &[(firefox(), 5.0, 0.0)]),
        );

        let all = store
            .usage(T0, T0 + 60_000, SourceFilter::Any, T0 + 60_000)
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].key, ConsumerKey::Idle);
        assert_eq!(all[1].split.cpu, Joules(8.0));

        let battery = store
            .usage(
                T0,
                T0 + 60_000,
                SourceFilter::Only(PowerSource::Battery),
                T0 + 60_000,
            )
            .unwrap();
        let firefox_on_battery = battery.iter().find(|r| r.key == firefox()).unwrap();
        assert_eq!(firefox_on_battery.split.total(), Joules(4.0));
    }

    #[test]
    fn malformed_windows_are_rejected_atomically() {
        let mut store = Store::open_in_memory().unwrap();
        let closed = window(Measurement::Battery, &[(firefox(), 1.0, 0.0)]);
        let bad = WindowRecord {
            start_ms: T0,
            end_ms: T0, // not after start
            window: &closed,
            model_version: 1,
        };
        assert!(store.record_window(&bad).is_err());
        assert_eq!(
            store.usage(T0 - 1, T0 + 1, SourceFilter::Any, T0).unwrap(),
            Vec::new()
        );
        assert!(store.consumer_ids.is_empty());
    }

    #[test]
    fn queries_fall_back_to_coarser_rollups_after_pruning() {
        let mut store = Store::open_in_memory().unwrap();
        record(
            &mut store,
            T0,
            &window(Measurement::Battery, &[(firefox(), 2.0, 0.0)]),
        );
        let three_days_later = T0 + 3 * DAY_MS;
        let stats = store.prune(three_days_later).unwrap();
        assert_eq!(stats.windows, 1);
        assert_eq!(stats.consumers, 0);
        assert_eq!(
            Granularity::for_range(T0, three_days_later),
            Granularity::Minute
        );
        let usage = store
            .usage(T0, T0 + 10_000, SourceFilter::Any, three_days_later)
            .unwrap();
        assert_eq!(usage[0].split.cpu, Joules(2.0));

        // Past every retention, everything goes, consumers included.
        let stats = store.prune(T0 + 2 * HOUR_RETENTION_MS).unwrap();
        assert_eq!(
            (stats.minute_rows, stats.hour_rows, stats.consumers),
            (1, 1, 1)
        );
    }

    #[test]
    fn granularity_follows_retention() {
        assert_eq!(Granularity::for_range(T0 - HOUR_MS, T0), Granularity::Raw);
        assert_eq!(
            Granularity::for_range(T0 - 7 * DAY_MS, T0),
            Granularity::Minute
        );
        assert_eq!(
            Granularity::for_range(T0 - 90 * DAY_MS, T0),
            Granularity::Hour
        );
    }

    proptest! {
        /// Minute and hour rollups always sum to the raw data, per consumer and source.
        #[test]
        fn rollups_equal_raw_sums(
            windows in proptest::collection::vec(
                (0i64..600, 0usize..4, 0.0f64..10.0, 0.0f64..3.0, any::<bool>()),
                1..60,
            ),
        ) {
            let keys = [firefox(), ConsumerKey::Kernel, ConsumerKey::Shell, ConsumerKey::Devices];
            let mut store = Store::open_in_memory().unwrap();
            for (minute, consumer, cpu, gpu, on_battery) in &windows {
                let measurement = if *on_battery { Measurement::Battery } else { Measurement::Rapl };
                let closed = window(measurement, &[(keys[*consumer].clone(), *cpu, *gpu)]);
                record(&mut store, T0 + minute * MINUTE_MS + 7_000, &closed);
            }
            let end = T0 + 700 * MINUTE_MS;
            for filter in [SourceFilter::Any, SourceFilter::Only(PowerSource::Battery), SourceFilter::Only(PowerSource::Ac)] {
                let raw = store.usage_at(Granularity::Raw, T0, end, filter).unwrap();
                for granularity in [Granularity::Minute, Granularity::Hour] {
                    let rolled = store.usage_at(granularity, T0, end, filter).unwrap();
                    prop_assert_eq!(raw.len(), rolled.len());
                    for row in &raw {
                        let other = rolled.iter().find(|r| r.key == row.key).unwrap();
                        prop_assert!((row.split.total().0 - other.split.total().0).abs() < 1e-9);
                        prop_assert!((row.split.gpu.0 - other.split.gpu.0).abs() < 1e-9);
                    }
                }
            }
        }
    }
}

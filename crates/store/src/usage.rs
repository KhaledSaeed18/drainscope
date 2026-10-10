//! Attribution windows and per-consumer usage at three resolutions.
//!
//! Every window is written to `usage_raw` and added to its minute and hour buckets in the same
//! transaction, so rollups always equal the raw data they summarize. Retention then only
//! deletes: raw rows after [`RAW_RETENTION_MS`], minutes after [`MINUTE_RETENTION_MS`], hours
//! after [`HOUR_RETENTION_MS`]. Queries read the finest resolution still covering their range.

use std::collections::{BTreeMap, HashMap};

use drainscope_model::{ClosedWindow, ConsumerKey, EnergySplit, FocusSplit, Joules, PowerSource};
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
    /// Apps' energy in the window split by focus (ADR 0011); empty when nothing reported it.
    pub focus: &'a BTreeMap<ConsumerKey, FocusSplit>,
}

/// One app's foreground and unknown energy, and focused time, over a period. The rest of its
/// energy (from [`Store::usage`]) was used in the background.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusRow {
    pub key: ConsumerKey,
    pub foreground_j: f64,
    pub unknown_j: f64,
    pub focused_ms: i64,
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

fn bucket_up(ts_ms: i64, width_ms: i64) -> i64 {
    let down = bucket(ts_ms, width_ms);
    if down == ts_ms { down } else { down + width_ms }
}

/// The pieces answering a query at `granularity` from the coarsest tables, with the same
/// result: within raw retention, exactly `[since, until)`; at minute resolution, the whole
/// minute buckets overlapping it (both ends rounded out to minutes, so no raw piece); at hour
/// resolution, the hour table alone.
fn pieces(granularity: Granularity, since_ms: i64, until_ms: i64) -> Vec<(Granularity, i64, i64)> {
    match granularity {
        Granularity::Raw => exact_pieces(since_ms, until_ms),
        Granularity::Minute => {
            exact_pieces(bucket(since_ms, MINUTE_MS), bucket_up(until_ms, MINUTE_MS))
        }
        Granularity::Hour => vec![(Granularity::Hour, since_ms, until_ms)],
    }
}

/// `[since, until)` split into pieces each answered exactly by one table: raw windows for the
/// partial minutes at the ends, minute buckets up to the hour boundaries, hour buckets in
/// between. Rollups hold exactly the windows starting in their bucket, so the pieces sum to
/// the raw query over far fewer rows (about 80 times fewer per day on the dev machine).
fn exact_pieces(since_ms: i64, until_ms: i64) -> Vec<(Granularity, i64, i64)> {
    let (m1, m2) = (bucket_up(since_ms, MINUTE_MS), bucket(until_ms, MINUTE_MS));
    if m1 >= m2 {
        return vec![(Granularity::Raw, since_ms, until_ms)];
    }
    let (h1, h2) = (bucket_up(m1, HOUR_MS), bucket(m2, HOUR_MS));
    let middle = if h1 < h2 {
        vec![
            (Granularity::Minute, m1, h1),
            (Granularity::Hour, h1, h2),
            (Granularity::Minute, h2, m2),
        ]
    } else {
        vec![(Granularity::Minute, m1, m2)]
    };
    std::iter::once((Granularity::Raw, since_ms, m1))
        .chain(middle)
        .chain(std::iter::once((Granularity::Raw, m2, until_ms)))
        .filter(|(_, from, to)| from < to)
        .collect()
}

/// Largest first, then by key.
fn sort_usage(usage: &mut [UsageRow]) {
    usage.sort_by(|a, b| {
        b.split
            .total()
            .0
            .total_cmp(&a.split.total().0)
            .then_with(|| a.key.cmp(&b.key))
    });
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

const ADD_FOCUS_TO_MINUTE: &str = "INSERT INTO focus_minute (bucket_ms, power_source, consumer_id, foreground_j, unknown_j, focused_ms)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
     ON CONFLICT (bucket_ms, power_source, consumer_id) DO UPDATE SET
       foreground_j = foreground_j + excluded.foreground_j, unknown_j = unknown_j + excluded.unknown_j,
       focused_ms = focused_ms + excluded.focused_ms";
const ADD_FOCUS_TO_HOUR: &str = "INSERT INTO focus_hour (bucket_ms, power_source, consumer_id, foreground_j, unknown_j, focused_ms)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
     ON CONFLICT (bucket_ms, power_source, consumer_id) DO UPDATE SET
       foreground_j = foreground_j + excluded.foreground_j, unknown_j = unknown_j + excluded.unknown_j,
       focused_ms = focused_ms + excluded.focused_ms";

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
        for (key, focus) in record.focus {
            let key = key.to_string();
            let id = consumer_id(&tx, consumer_ids, &mut created, &key, record.start_ms)?;
            let focused_ms = i64::try_from(focus.focused.as_millis()).unwrap_or(i64::MAX);
            let (foreground, unknown) = (focus.foreground_j, focus.unknown_j);
            tx.prepare_cached(
                "INSERT INTO focus_raw (window_id, consumer_id, foreground_j, unknown_j, focused_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![window_id, id, foreground, unknown, focused_ms])?;
            tx.prepare_cached(ADD_FOCUS_TO_MINUTE)?
                .execute(params![minute, source, id, foreground, unknown, focused_ms])?;
            tx.prepare_cached(ADD_FOCUS_TO_HOUR)?
                .execute(params![hour, source, id, foreground, unknown, focused_ms])?;
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
        let mut sums: HashMap<ConsumerKey, EnergySplit> = HashMap::new();
        let granularity = Granularity::for_range(since_ms, now_ms);
        for (granularity, from, to) in pieces(granularity, since_ms, until_ms) {
            for row in self.usage_at(granularity, from, to, filter)? {
                let sum = sums.entry(row.key).or_default();
                sum.cpu = Joules(sum.cpu.0 + row.split.cpu.0);
                sum.gpu = Joules(sum.gpu.0 + row.split.gpu.0);
                sum.other = Joules(sum.other.0 + row.split.other.0);
            }
        }
        let mut usage: Vec<UsageRow> = sums
            .into_iter()
            .map(|(key, split)| UsageRow { key, split })
            .collect();
        sort_usage(&mut usage);
        Ok(usage)
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
        sort_usage(&mut usage);
        Ok(usage)
    }

    /// Each app's foreground and unknown energy and focused time for windows starting in
    /// `[since_ms, until_ms)`, at the same resolution [`Self::usage`] uses. Apps without
    /// focus rows there have no known split.
    ///
    /// # Errors
    /// SQLite errors, or a stored key that no longer parses.
    pub fn focus(
        &self,
        since_ms: i64,
        until_ms: i64,
        filter: SourceFilter,
        now_ms: i64,
    ) -> Result<Vec<FocusRow>, StoreError> {
        let mut sums: HashMap<String, FocusRow> = HashMap::new();
        let granularity = Granularity::for_range(since_ms, now_ms);
        for (granularity, from, to) in pieces(granularity, since_ms, until_ms) {
            for row in self.focus_at(granularity, from, to, filter)? {
                match sums.entry(row.key.to_string()) {
                    std::collections::hash_map::Entry::Occupied(mut sum) => {
                        let sum = sum.get_mut();
                        sum.foreground_j += row.foreground_j;
                        sum.unknown_j += row.unknown_j;
                        sum.focused_ms += row.focused_ms;
                    }
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(row);
                    }
                }
            }
        }
        let mut focus: Vec<(String, FocusRow)> = sums.into_iter().collect();
        focus.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(focus.into_iter().map(|(_, row)| row).collect())
    }

    /// Like [`Self::focus`], at a chosen resolution.
    ///
    /// # Errors
    /// SQLite errors, or a stored key that no longer parses.
    pub fn focus_at(
        &self,
        granularity: Granularity,
        since_ms: i64,
        until_ms: i64,
        filter: SourceFilter,
    ) -> Result<Vec<FocusRow>, StoreError> {
        let (sql, since) = match granularity {
            Granularity::Raw => (
                "SELECT c.key, SUM(f.foreground_j), SUM(f.unknown_j), SUM(f.focused_ms)
                 FROM focus_raw f
                 JOIN windows w ON w.id = f.window_id
                 JOIN consumers c ON c.id = f.consumer_id
                 WHERE w.start_ms >= ?1 AND w.start_ms < ?2 AND (?3 IS NULL OR w.power_source = ?3)
                 GROUP BY c.key ORDER BY c.key",
                since_ms,
            ),
            Granularity::Minute => (
                "SELECT c.key, SUM(f.foreground_j), SUM(f.unknown_j), SUM(f.focused_ms)
                 FROM focus_minute f JOIN consumers c ON c.id = f.consumer_id
                 WHERE f.bucket_ms >= ?1 AND f.bucket_ms < ?2 AND (?3 IS NULL OR f.power_source = ?3)
                 GROUP BY c.key ORDER BY c.key",
                bucket(since_ms, MINUTE_MS),
            ),
            Granularity::Hour => (
                "SELECT c.key, SUM(f.foreground_j), SUM(f.unknown_j), SUM(f.focused_ms)
                 FROM focus_hour f JOIN consumers c ON c.id = f.consumer_id
                 WHERE f.bucket_ms >= ?1 AND f.bucket_ms < ?2 AND (?3 IS NULL OR f.power_source = ?3)
                 GROUP BY c.key ORDER BY c.key",
                bucket(since_ms, HOUR_MS),
            ),
        };
        let mut statement = self.conn.prepare_cached(sql)?;
        let rows = statement.query_map(params![since, until_ms, filter.wire_name()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut focus = Vec::new();
        for row in rows {
            let (key, foreground_j, unknown_j, focused_ms) = row?;
            let key = key
                .parse()
                .map_err(|err| StoreError::Corrupt(format!("consumer key: {err}")))?;
            focus.push(FocusRow {
                key,
                foreground_j,
                unknown_j,
                focused_ms,
            });
        }
        Ok(focus)
    }

    /// How much of `[since_ms, until_ms)` was measured, in milliseconds: the length of the
    /// windows recorded there, or, for older data, the number of minute or hour buckets with
    /// any data times their width (an upper bound at that resolution).
    ///
    /// # Errors
    /// SQLite errors.
    pub fn covered_ms(
        &self,
        since_ms: i64,
        until_ms: i64,
        filter: SourceFilter,
        now_ms: i64,
    ) -> Result<i64, StoreError> {
        let (sql, since) = match Granularity::for_range(since_ms, now_ms) {
            Granularity::Raw => (
                "SELECT COALESCE(SUM(end_ms - start_ms), 0) FROM windows
                 WHERE start_ms >= ?1 AND start_ms < ?2 AND (?3 IS NULL OR power_source = ?3)",
                since_ms,
            ),
            Granularity::Minute => (
                "SELECT COUNT(DISTINCT bucket_ms) * 60000 FROM usage_minute
                 WHERE bucket_ms >= ?1 AND bucket_ms < ?2 AND (?3 IS NULL OR power_source = ?3)",
                bucket(since_ms, MINUTE_MS),
            ),
            Granularity::Hour => (
                "SELECT COUNT(DISTINCT bucket_ms) * 3600000 FROM usage_hour
                 WHERE bucket_ms >= ?1 AND bucket_ms < ?2 AND (?3 IS NULL OR power_source = ?3)",
                bucket(since_ms, HOUR_MS),
            ),
        };
        Ok(self
            .conn
            .prepare_cached(sql)?
            .query_row(params![since, until_ms, filter.wire_name()], |row| {
                row.get(0)
            })?)
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
            consumers: {
                // Focus rows follow the same retention; raw ones go with their windows.
                tx.execute(
                    "DELETE FROM focus_minute WHERE bucket_ms < ?1",
                    params![now_ms - MINUTE_RETENTION_MS],
                )?;
                tx.execute(
                    "DELETE FROM focus_hour WHERE bucket_ms < ?1",
                    params![now_ms - HOUR_RETENTION_MS],
                )?;
                tx.execute(
                    "DELETE FROM consumers WHERE id NOT IN (
                         SELECT consumer_id FROM usage_raw
                         UNION SELECT consumer_id FROM usage_minute
                         UNION SELECT consumer_id FROM usage_hour
                         UNION SELECT consumer_id FROM focus_raw
                         UNION SELECT consumer_id FROM focus_minute
                         UNION SELECT consumer_id FROM focus_hour)",
                    [],
                )?
            },
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
                focus: &BTreeMap::new(),
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
            focus: &BTreeMap::new(),
        };
        assert!(store.record_window(&bad).is_err());
        assert_eq!(
            store.usage(T0 - 1, T0 + 1, SourceFilter::Any, T0).unwrap(),
            Vec::new()
        );
        assert!(store.consumer_ids.is_empty());
    }

    fn split(foreground_j: f64, unknown_j: f64, focused_s: u64) -> FocusSplit {
        FocusSplit {
            foreground_j,
            unknown_j,
            focused: Duration::from_secs(focused_s),
        }
    }

    fn record_focused(
        store: &mut Store,
        start_ms: i64,
        closed: &ClosedWindow,
        focus: &BTreeMap<ConsumerKey, FocusSplit>,
    ) {
        store
            .record_window(&WindowRecord {
                start_ms,
                end_ms: start_ms + 10_000,
                window: closed,
                model_version: 3,
                focus,
            })
            .unwrap();
    }

    #[test]
    fn records_focus_and_rolls_it_up_like_usage() {
        let mut store = Store::open_in_memory().unwrap();
        let editor = ConsumerKey::App("org.gnome.TextEditor".into());
        let hour = T0 - T0.rem_euclid(HOUR_MS);
        for (i, source) in [
            Measurement::Battery,
            Measurement::Rapl,
            Measurement::Battery,
        ]
        .into_iter()
        .enumerate()
        {
            let start = hour + i64::try_from(i).unwrap() * 20_000;
            // Firefox used energy; the editor had focus without using any.
            let focus = BTreeMap::from([
                (firefox(), split(2.0, 0.5, 4)),
                (editor.clone(), split(0.0, 0.0, 6)),
            ]);
            record_focused(
                &mut store,
                start,
                &window(source, &[(firefox(), 3.0, 0.0)]),
                &focus,
            );
        }
        let raw = store
            .focus(hour, hour + HOUR_MS, SourceFilter::Any, hour + HOUR_MS)
            .unwrap();
        let firefox_raw = raw.iter().find(|r| r.key == firefox()).unwrap();
        assert!((firefox_raw.foreground_j - 6.0).abs() < 1e-9);
        assert!((firefox_raw.unknown_j - 1.5).abs() < 1e-9);
        assert_eq!(firefox_raw.focused_ms, 12_000);
        assert_eq!(
            raw.iter().find(|r| r.key == editor).unwrap().focused_ms,
            18_000
        );
        // The same at minute and hour resolution, and filtered by power source.
        for now in [hour + 10 * DAY_MS, hour + 100 * DAY_MS] {
            assert_eq!(
                store
                    .focus(hour, hour + HOUR_MS, SourceFilter::Any, now)
                    .unwrap(),
                raw
            );
        }
        let on_battery = store
            .focus(
                hour,
                hour + HOUR_MS,
                SourceFilter::Only(PowerSource::Battery),
                hour + HOUR_MS,
            )
            .unwrap();
        assert_eq!(
            on_battery
                .iter()
                .find(|r| r.key == firefox())
                .unwrap()
                .focused_ms,
            8_000
        );

        // The editor has focus rows but no usage: pruning must keep its consumer.
        store.prune(hour + HOUR_MS).unwrap();
        assert_eq!(
            store
                .focus(hour, hour + HOUR_MS, SourceFilter::Any, hour + HOUR_MS)
                .unwrap(),
            raw
        );
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
    fn coverage_counts_measured_time_only() {
        let mut store = Store::open_in_memory().unwrap();
        // Two 10 s windows in a 10 minute range: 20 s measured.
        record(
            &mut store,
            T0,
            &window(Measurement::Battery, &[(firefox(), 1.0, 0.0)]),
        );
        record(
            &mut store,
            T0 + 60_000,
            &window(Measurement::Rapl, &[(firefox(), 1.0, 0.0)]),
        );
        let end = T0 + 600_000;
        assert_eq!(
            store.covered_ms(T0, end, SourceFilter::Any, end).unwrap(),
            20_000
        );
        let on_battery = SourceFilter::Only(PowerSource::Battery);
        assert_eq!(store.covered_ms(T0, end, on_battery, end).unwrap(), 10_000);
        // Days later only minute buckets remain: two buckets with data.
        let later = T0 + 3 * DAY_MS;
        store.prune(later).unwrap();
        assert_eq!(
            store.covered_ms(T0, end, SourceFilter::Any, later).unwrap(),
            120_000
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

    #[test]
    fn exact_pieces_cover_the_range_with_the_coarsest_tables() {
        let h = T0 - T0.rem_euclid(HOUR_MS);
        // 10:00:07 to 13:30:20 → raw, minute to 11:00, hours to 13:00, minutes, raw.
        assert_eq!(
            exact_pieces(h + 7_000, h + 3 * HOUR_MS + 30 * MINUTE_MS + 20_000),
            vec![
                (Granularity::Raw, h + 7_000, h + MINUTE_MS),
                (Granularity::Minute, h + MINUTE_MS, h + HOUR_MS),
                (Granularity::Hour, h + HOUR_MS, h + 3 * HOUR_MS),
                (
                    Granularity::Minute,
                    h + 3 * HOUR_MS,
                    h + 3 * HOUR_MS + 30 * MINUTE_MS
                ),
                (
                    Granularity::Raw,
                    h + 3 * HOUR_MS + 30 * MINUTE_MS,
                    h + 3 * HOUR_MS + 30 * MINUTE_MS + 20_000
                ),
            ]
        );
        // Within one minute: raw only. Aligned: rollups only.
        assert_eq!(
            exact_pieces(h + 1_000, h + 50_000),
            vec![(Granularity::Raw, h + 1_000, h + 50_000)]
        );
        assert_eq!(
            exact_pieces(h, h + 2 * HOUR_MS),
            vec![(Granularity::Hour, h, h + 2 * HOUR_MS)]
        );
    }

    proptest! {
        /// Queries within raw retention, answered from pieces, equal the raw data exactly.
        #[test]
        fn exact_queries_equal_raw_sums(
            windows in proptest::collection::vec((0i64..3 * 3600, 0usize..4, 0.0f64..10.0, any::<bool>()), 1..80),
            since_s in 0i64..3 * 3600,
            length_s in 1i64..3 * 3600,
        ) {
            let keys = [firefox(), ConsumerKey::App("org.gnome.TextEditor".into()), ConsumerKey::Kernel, ConsumerKey::Devices];
            let base = T0 - T0.rem_euclid(HOUR_MS);
            let mut store = Store::open_in_memory().unwrap();
            for (second, consumer, joules, on_battery) in &windows {
                let measurement = if *on_battery { Measurement::Battery } else { Measurement::Rapl };
                let key = keys[*consumer].clone();
                let focus = BTreeMap::from([(key.clone(), split(joules / 2.0, joules / 4.0, 3))]);
                record_focused(&mut store, base + second * 1000, &window(measurement, &[(key, *joules, 0.0)]), &focus);
            }
            let (since, until) = (base + since_s * 1000, base + (since_s + length_s) * 1000);
            let now = base + 4 * HOUR_MS;
            for filter in [SourceFilter::Any, SourceFilter::Only(PowerSource::Battery)] {
                let raw = store.usage_at(Granularity::Raw, since, until, filter).unwrap();
                let exact = store.usage(since, until, filter, now).unwrap();
                prop_assert_eq!(raw.len(), exact.len());
                for row in &raw {
                    let other = exact.iter().find(|r| r.key == row.key).unwrap();
                    prop_assert!((row.split.total().0 - other.split.total().0).abs() < 1e-9);
                }
                let raw_focus = store.focus_at(Granularity::Raw, since, until, filter).unwrap();
                let exact_focus = store.focus(since, until, filter, now).unwrap();
                prop_assert_eq!(raw_focus.len(), exact_focus.len());
                for (a, b) in raw_focus.iter().zip(&exact_focus) {
                    prop_assert_eq!(&a.key, &b.key);
                    prop_assert!((a.foreground_j - b.foreground_j).abs() < 1e-9);
                    prop_assert!((a.unknown_j - b.unknown_j).abs() < 1e-9);
                    prop_assert_eq!(a.focused_ms, b.focused_ms);
                }
            }
        }

        /// Older ranges, answered from minute and hour pieces, equal the minute table alone.
        #[test]
        fn minute_resolution_queries_are_unchanged(
            windows in proptest::collection::vec((0i64..3 * 3600, 0usize..3, 0.0f64..10.0), 1..60),
            since_s in 0i64..3 * 3600,
            length_s in 1i64..3 * 3600,
        ) {
            let keys = [firefox(), ConsumerKey::Kernel, ConsumerKey::Devices];
            let base = T0 - T0.rem_euclid(HOUR_MS);
            let mut store = Store::open_in_memory().unwrap();
            for (second, consumer, joules) in &windows {
                record(&mut store, base + second * 1000, &window(Measurement::Battery, &[(keys[*consumer].clone(), *joules, 0.0)]));
            }
            let (since, until) = (base + since_s * 1000, base + (since_s + length_s) * 1000);
            let now = base + 10 * DAY_MS;
            let minute = store.usage_at(Granularity::Minute, since, until, SourceFilter::Any).unwrap();
            let split = store.usage(since, until, SourceFilter::Any, now).unwrap();
            prop_assert_eq!(minute.len(), split.len());
            for row in &minute {
                let other = split.iter().find(|r| r.key == row.key).unwrap();
                prop_assert!((row.split.total().0 - other.split.total().0).abs() < 1e-9);
            }
        }

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

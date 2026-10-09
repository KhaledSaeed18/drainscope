//! The `Monitor1` D-Bus interface: what the CLI, the Shell extension and the app read.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use drainscope_model::{ConsumerKey, EnergySplit, Joules, MODEL_VERSION, PowerSource};
use drainscope_store::{PowerEventKind, SourceFilter, Store, StoreError};
use tokio::sync::Notify;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

/// Largest consumers returned by `GetSummary`.
const TOP: usize = 10;

/// How complete the measurements are (the `Status` property).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// RAPL from the sampler, plus the battery when discharging.
    Full,
    /// No sampler, on battery: battery energy split by CPU time.
    BatteryOnly,
    /// No sampler, on AC: only CPU and GPU time are known.
    TimeOnly,
}

impl Status {
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::BatteryOnly => "battery-only",
            Self::TimeOnly => "time-only",
        }
    }
}

/// What the tick loop knows right now.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveState {
    pub on_battery: bool,
    /// Combined capacity of all batteries.
    pub capacity: Option<Joules>,
    pub status: Status,
    /// RAPL domains in use (psys only when trusted).
    pub domains: Vec<String>,
    /// Idle exits per second by consumer key over the last minute, largest first; `None`
    /// without the eBPF probe.
    pub wakeups: Option<Vec<(String, f64)>>,
    /// (key, received B/s, sent B/s) over the last minute, busiest first; `None` without the
    /// probe's network counting.
    pub network: Option<Vec<(String, f64, f64)>>,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            on_battery: false,
            capacity: None,
            status: Status::TimeOnly,
            domains: Vec::new(),
            wakeups: None,
            network: None,
        }
    }
}

/// State shared between the tick loop and the D-Bus interface.
/// Two Monitor1 calls this close together mean a live view is open (see [`Shared::watched`]).
pub const WATCH_WINDOW: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub struct Shared {
    pub store: Mutex<Store>,
    pub live: Mutex<LiveState>,
    /// When clients last called, and the call before that.
    calls: Mutex<(Option<Instant>, Option<Instant>)>,
    /// Wakes the tick loop when a client calls.
    pub called: Notify,
}

impl Shared {
    #[must_use]
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Self {
            store: Mutex::new(store),
            live: Mutex::new(LiveState::default()),
            calls: Mutex::new((None, None)),
            called: Notify::new(),
        })
    }

    /// Records a client's call at `now`.
    pub fn note_call(&self, now: Instant) {
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        calls.1 = calls.0.replace(now);
        drop(calls);
        self.called.notify_one();
    }

    /// Whether someone is watching live: at least two calls within [`WATCH_WINDOW`]. An open
    /// app or Quick Settings menu calls on every tick; the extension's background refresh, one
    /// call a minute, doesn't count.
    #[must_use]
    pub fn watched(&self, now: Instant) -> bool {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        calls
            .1
            .is_some_and(|previous| now.saturating_duration_since(previous) <= WATCH_WINDOW)
    }
}

/// The consumer's kind: the part of its key before `:` (`app`, `unit`, …), or the whole key.
#[must_use]
pub fn kind_of(key: &ConsumerKey) -> String {
    let key = key.to_string();
    key.split_once(':')
        .map_or(key.clone(), |(kind, _)| kind.to_owned())
}

/// Energy caused by something, as opposed to the idle floor and what RAPL can't see.
fn attributable(key: &ConsumerKey) -> bool {
    !matches!(
        key,
        ConsumerKey::Idle | ConsumerKey::Devices | ConsumerKey::Platform
    )
}

fn failed(err: &StoreError) -> fdo::Error {
    tracing::error!(%err, "store query failed");
    fdo::Error::Failed("reading usage history failed".into())
}

/// `any`, `battery` or `ac`.
fn source_filter(power_source: &str) -> fdo::Result<SourceFilter> {
    match power_source {
        "any" => Ok(SourceFilter::Any),
        other => PowerSource::from_wire_name(other)
            .map(SourceFilter::Only)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("power_source {other:?}"))),
    }
}

fn to_ms(unix_seconds: i64) -> i64 {
    unix_seconds.saturating_mul(1000)
}

pub struct Monitor {
    shared: Arc<Shared>,
    /// Wall clock in Unix milliseconds (injectable for tests).
    now_ms: fn() -> i64,
}

impl Monitor {
    #[must_use]
    pub fn new(shared: Arc<Shared>, now_ms: fn() -> i64) -> Self {
        Self { shared, now_ms }
    }

    fn live(&self) -> LiveState {
        self.shared
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn summary(&self) -> Result<SummaryReply, StoreError> {
        let now = (self.now_ms)();
        let live = self.live();
        let store = self
            .shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(unplug) = store.last_power_event(PowerEventKind::Unplug)? else {
            // Never unplugged while drainscope was running: nothing to summarize.
            return Ok((live.on_battery, 0, 0.0, Vec::new()));
        };
        let since = unplug.ts_ms;
        // On AC, report the last discharge: up to the plug-in that ended it.
        let until = if live.on_battery {
            now
        } else {
            store
                .last_power_event(PowerEventKind::Plug)?
                .map(|event| event.ts_ms)
                .filter(|&plugged| plugged >= since)
                .unwrap_or(now)
        };
        let rows = store.usage(since, until, SourceFilter::Only(PowerSource::Battery), now)?;
        drop(store);

        let total: f64 = rows.iter().map(|r| r.split.total().0).sum();
        let attributable_total: f64 = rows
            .iter()
            .filter(|r| attributable(&r.key))
            .map(|r| r.split.total().0)
            .sum();
        let capacity = live.capacity.map(|c| c.0).filter(|c| *c > 0.0);
        let percent_of_battery = |joules: f64| capacity.map_or(0.0, |c| joules / c * 100.0);
        let top = rows
            .iter()
            .take(TOP)
            .map(|row| {
                let joules = row.split.total().0;
                let of_attributable = if attributable(&row.key) && attributable_total > 0.0 {
                    joules / attributable_total * 100.0
                } else {
                    0.0
                };
                (
                    row.key.to_string(),
                    joules,
                    percent_of_battery(joules),
                    of_attributable,
                )
            })
            .collect();
        Ok((
            live.on_battery,
            since / 1000,
            percent_of_battery(total),
            top,
        ))
    }
}

type UsageRows = Vec<(String, String, f64, f64, f64, f64)>;
/// (key, joules, % of battery, % of attributable energy).
type TopRow = (String, f64, f64, f64);
/// (on battery, since unplug, battery %, top consumers), as `GetSummary` returns it.
type SummaryReply = (bool, i64, f64, Vec<TopRow>);
/// (start, end, Wh lost, % lost, sleep mode).
type SleepRow = (i64, i64, f64, f64, String);
/// (start, end, Wh lost, % lost, sleep mode, wake reason).
type SleepHistoryRow = (i64, i64, f64, f64, String, String);
/// (battery, time, full-charge Wh, design Wh, cycles).
type HealthRow = (String, i64, f64, f64, u32);

#[zbus::interface(name = "io.github.khaledsaeed18.Drainscope.Monitor1")]
impl Monitor {
    // Tuples are spelled out so each element becomes its own out argument, as the contract
    // in data/dbus/interfaces/ requires.
    #[zbus(out_args("on_battery", "since_unplug", "battery_percent", "top"))]
    fn get_summary(&self) -> fdo::Result<(bool, i64, f64, Vec<TopRow>)> {
        self.shared.note_call(Instant::now());
        self.summary().map_err(|err| failed(&err))
    }

    #[zbus(out_args("usage"))]
    fn get_usage(
        &self,
        since: i64,
        until: i64,
        group_by: &str,
        power_source: &str,
    ) -> fdo::Result<UsageRows> {
        self.shared.note_call(Instant::now());
        let by_kind = match group_by {
            "consumer" => false,
            "kind" => true,
            other => return Err(fdo::Error::InvalidArgs(format!("group_by {other:?}"))),
        };
        let filter = source_filter(power_source)?;
        let rows = self
            .shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .usage(to_ms(since), to_ms(until), filter, (self.now_ms)())
            .map_err(|err| failed(&err))?;

        let mut grouped: BTreeMap<(String, String), EnergySplit> = BTreeMap::new();
        for row in rows {
            let kind = kind_of(&row.key);
            let key = if by_kind {
                kind.clone()
            } else {
                row.key.to_string()
            };
            grouped.entry((key, kind)).or_default().merge(&row.split);
        }
        let mut usage: UsageRows = grouped
            .into_iter()
            .map(|((key, kind), s)| (key, kind, s.total().0, s.cpu.0, s.gpu.0, s.other.0))
            .collect();
        usage.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        Ok(usage)
    }

    #[zbus(out_args("seconds"))]
    fn get_coverage(&self, since: i64, until: i64, power_source: &str) -> fdo::Result<u64> {
        self.shared.note_call(Instant::now());
        let filter = source_filter(power_source)?;
        let covered_ms = self
            .shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .covered_ms(to_ms(since), to_ms(until), filter, (self.now_ms)())
            .map_err(|err| failed(&err))?;
        Ok(u64::try_from(covered_ms / 1000).unwrap_or(0))
    }

    #[zbus(out_args("sessions"))]
    fn get_sleep_sessions(&self, since: i64) -> fdo::Result<Vec<SleepRow>> {
        self.shared.note_call(Instant::now());
        let sessions = self
            .shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .sleep_sessions(to_ms(since))
            .map_err(|err| failed(&err))?;
        Ok(sessions
            .into_iter()
            .map(|s| {
                (
                    s.start_ms / 1000,
                    s.end_ms / 1000,
                    s.wh_lost.unwrap_or(f64::NAN),
                    s.percent_lost.unwrap_or(f64::NAN),
                    s.mem_sleep.unwrap_or_default(),
                )
            })
            .collect())
    }

    #[zbus(out_args("sessions"))]
    fn get_sleep_history(&self, since: i64) -> fdo::Result<Vec<SleepHistoryRow>> {
        self.shared.note_call(Instant::now());
        let sessions = self
            .shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .sleep_sessions(to_ms(since))
            .map_err(|err| failed(&err))?;
        Ok(sessions
            .into_iter()
            .map(|s| {
                (
                    s.start_ms / 1000,
                    s.end_ms / 1000,
                    s.wh_lost.unwrap_or(f64::NAN),
                    s.percent_lost.unwrap_or(f64::NAN),
                    s.mem_sleep.unwrap_or_default(),
                    s.wake_reason.unwrap_or_default(),
                )
            })
            .collect())
    }

    #[zbus(out_args("readings"))]
    fn get_battery_health(&self, since: i64) -> fdo::Result<Vec<HealthRow>> {
        self.shared.note_call(Instant::now());
        let records = self
            .shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .battery_health(to_ms(since))
            .map_err(|err| failed(&err))?;
        Ok(records
            .into_iter()
            .map(|r| {
                (
                    r.health.battery,
                    r.ts_ms / 1000,
                    r.health.energy_full.0 / 3600.0,
                    r.health
                        .energy_full_design
                        .map_or(f64::NAN, |j| j.0 / 3600.0),
                    r.health.cycle_count.unwrap_or(0),
                )
            })
            .collect())
    }

    #[zbus(out_args("available", "wakeups"))]
    fn get_wakeups(&self) -> (bool, Vec<(String, f64)>) {
        self.shared.note_call(Instant::now());
        let live = self.live();
        match &live.wakeups {
            Some(wakeups) => (true, wakeups.clone()),
            None => (false, Vec::new()),
        }
    }

    #[zbus(out_args("available", "traffic"))]
    fn get_network(&self) -> (bool, Vec<(String, f64, f64)>) {
        self.shared.note_call(Instant::now());
        let live = self.live();
        match &live.network {
            Some(traffic) => (true, traffic.clone()),
            None => (false, Vec::new()),
        }
    }

    #[zbus(signal)]
    pub async fn tick(
        emitter: &SignalEmitter<'_>,
        duration_ms: u32,
        usage: Vec<(String, f64)>,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn status(&self) -> String {
        self.live().status.wire_name().to_owned()
    }

    #[zbus(property)]
    #[allow(clippy::unused_self)] // property getters always take &self
    fn model_version(&self) -> u32 {
        MODEL_VERSION
    }

    #[zbus(property)]
    fn domains(&self) -> Vec<String> {
        self.live().domains
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_keys() {
        assert_eq!(kind_of(&ConsumerKey::App("org.x".into())), "app");
        assert_eq!(
            kind_of(&ConsumerKey::UserUnit("a:b.service".into())),
            "user-unit"
        );
        assert_eq!(kind_of(&ConsumerKey::Devices), "devices");
    }

    #[test]
    fn two_calls_close_together_mean_someone_is_watching() {
        let shared = Shared::new(Store::open_in_memory().unwrap());
        let start = Instant::now();
        assert!(!shared.watched(start));
        // The extension's background refresh: one call a minute.
        shared.note_call(start);
        assert!(!shared.watched(start + Duration::from_secs(1)));
        shared.note_call(start + Duration::from_secs(60));
        assert!(!shared.watched(start + Duration::from_secs(61)));
        // A menu opened: its next call comes with the following tick.
        shared.note_call(start + Duration::from_secs(70));
        assert!(shared.watched(start + Duration::from_secs(71)));
        // Closed: calls stop, and it no longer counts once the earlier call is old enough.
        assert!(
            !shared
                .watched(start + Duration::from_secs(70) + WATCH_WINDOW + Duration::from_secs(1))
        );
    }
}

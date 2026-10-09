//! The tick pipeline, without I/O: snapshot in, attribution and closed windows out.
//!
//! Each tick diffs the new snapshot against the previous one, learns the idle floor and the
//! `psys` verdict from it, attributes the interval's energy, and adds it to the current
//! window. Windows close after `MIN_WINDOW` and are returned for storage.

use std::collections::BTreeMap;
use std::time::Duration;

use drainscope_model::{
    Activity, CgroupPath, ClosedWindow, ConsumerKey, FloorEstimator, PowerSource, PsysCheck,
    PsysVerdict, Resolver, Snapshot, Watts, Window, attribute, deltas_by_consumer, diff,
};

/// Cumulative network counters from the eBPF probe (ADR 0007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkCounters {
    /// Changes whenever the probe's totals restart.
    pub generation: u64,
    /// Bytes received plus sent, by cgroup.
    pub bytes: BTreeMap<CgroupPath, u64>,
    /// Kernel time in the network softirqs, machine-wide.
    pub softirq_ns: u64,
    /// Time of the network devices' threaded interrupt handlers (`irq/<n>-iwlwifi`), by
    /// thread id, from procfs (model v3, ADR 0008).
    pub irq_thread_ns: BTreeMap<u32, u64>,
}

/// Time the threads in both readings ran in between. A thread that appeared or exited in
/// between counts nothing, so a driver reload can't produce a bogus delta.
fn thread_time_ns(before: &BTreeMap<u32, u64>, after: &BTreeMap<u32, u64>) -> u64 {
    after
        .iter()
        .filter_map(|(tid, &ns)| Some(ns.saturating_sub(*before.get(tid)?)))
        .sum()
}

/// A snapshot and everything known about the moment it was taken.
#[derive(Debug, Clone)]
pub struct Reading {
    pub snapshot: Snapshot,
    /// Wall clock, Unix milliseconds.
    pub wall_ms: i64,
    /// Changes when the sampler's counters restart; `None` without RAPL.
    pub rapl_generation: Option<u64>,
    /// Labels for terminal-tab cgroups (see `drainscope_sys::terminal_labels`).
    pub terminal_labels: BTreeMap<CgroupPath, String>,
    /// `None` without the probe's network counting: attribution is then as in model v1.
    pub network: Option<NetworkCounters>,
}

/// A window ready for storage.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedWindow {
    pub window: ClosedWindow,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TickOutcome {
    pub duration: Duration,
    pub power_source: PowerSource,
    /// Average power per consumer over the tick, from RAPL attribution.
    pub live: Vec<(ConsumerKey, Watts)>,
    pub finished: Option<FinishedWindow>,
}

#[derive(Debug)]
pub struct Engine {
    own_uid: u32,
    previous: Option<Reading>,
    window: Window,
    window_start_ms: Option<i64>,
    floors: BTreeMap<PowerSource, FloorEstimator>,
    psys: PsysCheck,
}

impl Engine {
    #[must_use]
    pub fn new(
        own_uid: u32,
        floors: BTreeMap<PowerSource, FloorEstimator>,
        psys: PsysCheck,
    ) -> Self {
        Self {
            own_uid,
            previous: None,
            window: Window::default(),
            window_start_ms: None,
            floors,
            psys,
        }
    }

    /// Processes a new reading. The first reading (and the first after [`Self::reset`]) only
    /// establishes a baseline.
    pub fn tick(&mut self, mut reading: Reading) -> Option<TickOutcome> {
        let Some(mut previous) = self.previous.take() else {
            self.previous = Some(reading);
            return None;
        };
        // Counters from different sampler generations can't be compared.
        if previous.rapl_generation != reading.rapl_generation {
            previous.snapshot.rapl.clear();
        }
        let Ok(delta) = diff(&previous.snapshot, &reading.snapshot) else {
            // Out-of-order snapshots: start over from this one.
            reading.terminal_labels.clear();
            self.previous = Some(reading);
            return None;
        };

        let power_source = if delta.battery.is_some() {
            PowerSource::Battery
        } else {
            PowerSource::Ac
        };
        let resolver = Resolver {
            own_uid: self.own_uid,
            terminal_labels: &reading.terminal_labels,
        };
        let mut activity = Activity::from_delta(&delta, &resolver);
        if let (Some(before), Some(after)) = (&previous.network, &reading.network)
            && before.generation == after.generation
        {
            let bytes = deltas_by_consumer(&before.bytes, &after.bytes, &resolver);
            let softirq_ns = after.softirq_ns.saturating_sub(before.softirq_ns);
            let irq_ns = thread_time_ns(&before.irq_thread_ns, &after.irq_thread_ns);
            activity.charge_network((softirq_ns + irq_ns) / 1000, &bytes);
        }
        let estimator = self.floors.entry(power_source).or_default();
        estimator.observe(&delta, &activity);
        self.psys.observe(&delta);
        let floor = estimator.floor();

        let tick = attribute(&delta, activity, &floor, self.psys_trusted());
        let live = tick
            .ledger
            .iter()
            .map(|(key, split)| (key.clone(), split.total().over(delta.duration)))
            .collect();
        self.window_start_ms.get_or_insert(previous.wall_ms);
        self.window.push(tick, delta.battery);

        let finished = if self.window.is_ready() {
            let start_ms = self.window_start_ms.take().unwrap_or(previous.wall_ms);
            Some(FinishedWindow {
                window: std::mem::take(&mut self.window).close(&floor),
                start_ms,
                end_ms: reading.wall_ms.max(start_ms + 1),
            })
        } else {
            None
        };
        self.previous = Some(reading);
        Some(TickOutcome {
            duration: delta.duration,
            power_source,
            live,
            finished,
        })
    }

    /// Forgets the baseline and the open window, e.g. after resuming from suspend, when an
    /// interval spanning the sleep would be meaningless.
    pub fn reset(&mut self) {
        self.previous = None;
        self.window = Window::default();
        self.window_start_ms = None;
    }

    #[must_use]
    pub fn psys_trusted(&self) -> bool {
        self.psys.verdict() == PsysVerdict::Trusted
    }

    #[must_use]
    pub fn floors(&self) -> &BTreeMap<PowerSource, FloorEstimator> {
        &self.floors
    }

    #[must_use]
    pub fn psys(&self) -> &PsysCheck {
        &self.psys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drainscope_model::snapshot::{EnergyCounter, RaplDomain};
    use drainscope_model::{Measurement, Microjoules};

    const SCOPE: &str = "user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-org.mozilla.firefox-1.scope";

    /// Firefox busy 0.5 CPU and the package drawing 3 W, `seconds` after the start.
    fn reading(seconds: u64, generation: u64, package_uj: u64) -> Reading {
        let mut snapshot = Snapshot {
            taken_at: Duration::from_secs(seconds),
            ..Snapshot::default()
        };
        let usec = seconds * 500_000;
        snapshot.cgroup_cpu_usec.insert(CgroupPath::root(), usec);
        snapshot
            .cgroup_cpu_usec
            .insert(CgroupPath::new(SCOPE), usec);
        snapshot.rapl.insert(
            RaplDomain::Package,
            EnergyCounter {
                value: Microjoules(package_uj),
                range: Microjoules(u64::MAX),
            },
        );
        Reading {
            snapshot,
            wall_ms: i64::try_from(seconds).unwrap() * 1000,
            rapl_generation: Some(generation),
            terminal_labels: BTreeMap::new(),
            network: None,
        }
    }

    fn engine() -> Engine {
        Engine::new(1000, BTreeMap::new(), PsysCheck::default())
    }

    #[test]
    fn first_reading_is_a_baseline_and_windows_close_after_ten_seconds() {
        let mut engine = engine();
        assert!(engine.tick(reading(0, 1, 0)).is_none());
        let mut finished = Vec::new();
        for t in (2..=12).step_by(2) {
            let outcome = engine.tick(reading(t, 1, t * 3_000_000)).unwrap();
            assert_eq!(outcome.power_source, PowerSource::Ac);
            finished.extend(outcome.finished);
        }
        assert_eq!(finished.len(), 1);
        let window = &finished[0];
        assert_eq!((window.start_ms, window.end_ms), (0, 10_000));
        assert_eq!(window.window.measurement, Measurement::Rapl);
        assert!((window.window.measured.0 - 30.0).abs() < 1e-9);
        let firefox = ConsumerKey::App("org.mozilla.firefox".into());
        assert!(window.window.ledger.contains_key(&firefox));
    }

    #[test]
    fn a_new_sampler_generation_never_produces_a_bogus_delta() {
        let mut engine = engine();
        engine.tick(reading(0, 1, 50_000_000));
        // The sampler restarted: its totals start over from (nearly) zero.
        let outcome = engine.tick(reading(2, 2, 10_000)).unwrap();
        let total: f64 = outcome.live.iter().map(|(_, w)| w.0).sum();
        assert!(
            total.abs() < 1e-9,
            "no RAPL energy across generations: {total}"
        );
        let outcome = engine.tick(reading(4, 2, 6_010_000)).unwrap();
        let total: f64 = outcome.live.iter().map(|(_, w)| w.0).sum();
        assert!((total - 3.0).abs() < 1e-9);
    }

    #[test]
    fn network_time_moves_from_kernel_to_the_app_with_traffic() {
        const DNF: &str = "system.slice/dnf.service";
        // Kernel time in the root cgroup itself, Firefox (SCOPE) busy, dnf only moving bytes;
        // the network softirqs take 0.2 s of each 2 s tick.
        let with_network = |seconds: u64, generation: u64, softirq_ns: Option<u64>| {
            let mut r = reading(seconds, 1, seconds * 3_000_000);
            r.snapshot
                .cgroup_cpu_usec
                .insert(CgroupPath::root(), seconds * 1_000_000);
            r.network = softirq_ns.map(|softirq_ns| NetworkCounters {
                generation,
                bytes: BTreeMap::from([(CgroupPath::new(DNF), seconds * 1_000_000)]),
                softirq_ns,
                irq_thread_ns: BTreeMap::new(),
            });
            r
        };
        let watts = |outcome: &TickOutcome, key: &ConsumerKey| {
            outcome
                .live
                .iter()
                .find(|(k, _)| k == key)
                .map_or(0.0, |(_, w)| w.0)
        };
        let dnf = ConsumerKey::SystemUnit("dnf.service".into());

        let mut v1 = engine();
        v1.tick(with_network(0, 7, None));
        let without = v1.tick(with_network(2, 7, None)).unwrap();
        let mut v2 = engine();
        v2.tick(with_network(0, 7, Some(0)));
        let with = v2.tick(with_network(2, 7, Some(200_000_000))).unwrap();
        // The fixture has no intermediate cgroups, so the root's 2 s all count as Kernel's own
        // time: 3 s of CPU per tick with Firefox's 1 s. 0.2 s of it is 0.2 W of the 3 W, and
        // it moves from Kernel to dnf.
        assert!((watts(&with, &dnf) - 0.2).abs() < 1e-9);
        let kernel_moved =
            watts(&without, &ConsumerKey::Kernel) - watts(&with, &ConsumerKey::Kernel);
        assert!((kernel_moved - 0.2).abs() < 1e-9);
        let firefox = ConsumerKey::App("org.mozilla.firefox".into());
        assert!((watts(&with, &firefox) - watts(&without, &firefox)).abs() < 1e-12);

        // A restarted probe (new generation) moves nothing: its counters aren't comparable.
        let outcome = v2.tick(with_network(4, 8, Some(5))).unwrap();
        assert!(watts(&outcome, &dnf).abs() < 1e-12);
    }

    #[test]
    fn network_irq_threads_are_charged_with_the_softirqs() {
        const DNF: &str = "system.slice/dnf.service";
        // As above, plus the Wi-Fi driver's interrupt thread (tid 664) running 0.1 s per tick;
        // tid 700 only exists in the second reading.
        let with_irq = |seconds: u64, threads: &[(u32, u64)]| {
            let mut r = reading(seconds, 1, seconds * 3_000_000);
            r.snapshot
                .cgroup_cpu_usec
                .insert(CgroupPath::root(), seconds * 1_000_000);
            r.network = Some(NetworkCounters {
                generation: 7,
                bytes: BTreeMap::from([(CgroupPath::new(DNF), seconds * 1_000_000)]),
                softirq_ns: seconds * 100_000_000,
                irq_thread_ns: threads.iter().copied().collect(),
            });
            r
        };
        let mut engine = engine();
        engine.tick(with_irq(0, &[(664, 5_000_000_000)]));
        let outcome = engine
            .tick(with_irq(2, &[(664, 5_100_000_000), (700, 9_000_000_000)]))
            .unwrap();
        let dnf = ConsumerKey::SystemUnit("dnf.service".into());
        let charged = outcome
            .live
            .iter()
            .find(|(k, _)| *k == dnf)
            .map_or(0.0, |(_, w)| w.0);
        // 0.2 s of softirqs and 0.1 s of the interrupt thread, of 3 CPU-seconds at 3 W.
        assert!((charged - 0.3).abs() < 1e-9, "{charged}");
    }

    #[test]
    fn reset_starts_a_fresh_baseline() {
        let mut engine = engine();
        engine.tick(reading(0, 1, 0));
        engine.tick(reading(2, 1, 6_000_000));
        engine.reset();
        assert!(engine.tick(reading(100, 1, 9_000_000)).is_none());
        assert!(engine.tick(reading(102, 1, 15_000_000)).is_some());
    }
}

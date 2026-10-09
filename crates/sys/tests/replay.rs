//! Replays the recorded M0 traces through the real readers and the model.
//!
//! Each trace snapshot is materialized as a fixture tree laid out like `/`, so these tests
//! exercise the same parsing as a live system, on data from real hardware.

// Test helpers fail the test by panicking.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use drainscope_model::snapshot::BatteryStatus;
use drainscope_model::{
    Activity, ClosedWindow, ConsumerKey, EnergySplit, FloorEstimator, IdleFloor, Joules,
    Measurement, Part, PsysCheck, PsysVerdict, Resolver, Snapshot, Window, attribute, diff,
};
use drainscope_sys::{
    DrmScanner, SysRoot, read_batteries, read_cpu_usage, read_zones, terminal_labels,
};
use drainscope_testkit::{materialize, trace};

const OWN_UID: u32 = 1000;

struct Replayed {
    snapshot: Snapshot,
    labels: BTreeMap<drainscope_model::CgroupPath, String>,
}

fn replay(name: &str) -> Vec<Replayed> {
    let trace = trace(name);
    let has_rapl = trace.recorded_as_root;
    let mut scanner = DrmScanner::default();
    trace
        .snapshots
        .iter()
        .map(|recorded| {
            let dir = tempfile::tempdir().unwrap();
            materialize(recorded, dir.path());
            let root = SysRoot::at(dir.path());
            let taken_at = Duration::from_nanos(recorded.mono_ns);
            let cgroup_cpu_usec = read_cpu_usage(&root).unwrap();
            let labels = terminal_labels(&root, cgroup_cpu_usec.keys()).unwrap();
            let snapshot = Snapshot {
                taken_at,
                rapl: if has_rapl {
                    read_zones(&root)
                        .unwrap()
                        .into_iter()
                        .map(|zone| (zone.domain, zone.counter))
                        .collect()
                } else {
                    BTreeMap::new()
                },
                drm_clients: scanner.scan(&root, taken_at).unwrap(),
                batteries: read_batteries(&root).unwrap(),
                cgroup_cpu_usec,
            };
            Replayed { snapshot, labels }
        })
        .collect()
}

fn learn_idle_floor() -> IdleFloor {
    let replayed = replay("m0-idle-2min");
    let mut estimator = FloorEstimator::default();
    for pair in replayed.windows(2) {
        let [prev, next] = pair else { continue };
        let delta = diff(&prev.snapshot, &next.snapshot).unwrap();
        let resolver = Resolver {
            own_uid: OWN_UID,
            terminal_labels: &next.labels,
        };
        estimator.observe(&delta, &Activity::from_delta(&delta, &resolver));
    }
    estimator.floor()
}

struct Outcome {
    windows: Vec<ClosedWindow>,
    psys: PsysVerdict,
    totals: BTreeMap<ConsumerKey, Joules>,
}

fn attribute_trace(name: &str, floor: &IdleFloor) -> Outcome {
    let replayed = replay(name);
    let mut psys = PsysCheck::default();
    let mut window = Window::default();
    let mut windows = Vec::new();
    for pair in replayed.windows(2) {
        let [prev, next] = pair else { continue };
        let delta = diff(&prev.snapshot, &next.snapshot).unwrap();
        psys.observe(&delta);
        let resolver = Resolver {
            own_uid: OWN_UID,
            terminal_labels: &next.labels,
        };
        let activity = Activity::from_delta(&delta, &resolver);
        let trusted = psys.verdict() == PsysVerdict::Trusted;
        let tick = attribute(&delta, activity, floor, trusted);
        window.push(tick, delta.battery);
        if window.is_ready() {
            windows.push(std::mem::take(&mut window).close(floor));
        }
    }
    let mut totals: BTreeMap<ConsumerKey, Joules> = BTreeMap::new();
    for closed in &windows {
        for (key, split) in &closed.ledger {
            *totals.entry(key.clone()).or_default() += split.total();
        }
    }
    Outcome {
        windows,
        psys: psys.verdict(),
        totals,
    }
}

#[test]
fn readers_parse_recorded_hardware() {
    let first = &replay("m0-battery-2min")[0].snapshot;
    assert_eq!(first.rapl.len(), 5, "package, core, uncore, dram, psys");
    assert!(first.cgroup_cpu_usec.len() > 100);
    assert!(
        first
            .cgroup_cpu_usec
            .contains_key(&drainscope_model::CgroupPath::root())
    );
    let statuses: Vec<(&str, BatteryStatus)> = first
        .batteries
        .iter()
        .map(|b| (b.name.as_str(), b.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("BAT0", BatteryStatus::NotCharging),
            ("BAT1", BatteryStatus::Discharging)
        ]
    );
    // GNOME Shell's client is shared with PID 1 and logind (ADR 0001).
    let shared = first
        .drm_clients
        .values()
        .find(|c| c.holders.len() > 1)
        .unwrap();
    let pids: Vec<u32> = shared.holders.iter().map(|h| h.process.pid).collect();
    assert!(pids.contains(&1));
}

#[test]
fn idle_floor_matches_the_measured_true_idle() {
    let floor = learn_idle_floor();
    // ADR 0001: true idle had core ≈ 0.19 W and dram ≈ 0.37 W.
    assert!((floor.get(Part::Core).0 - 0.19).abs() < 0.05, "{floor:?}");
    assert!((floor.get(Part::Dram).0 - 0.37).abs() < 0.05, "{floor:?}");
}

#[test]
fn normal_use_trace_attributes_consistently() {
    let outcome = attribute_trace("m0-battery-2min", &learn_idle_floor());
    assert_eq!(outcome.psys, PsysVerdict::Rejected);
    assert!(outcome.windows.len() >= 10);
    for window in &outcome.windows {
        assert_eq!(window.measurement, Measurement::Battery);
        let total: Joules = window.ledger.values().map(EnergySplit::total).sum();
        assert!((total.0 - window.measured.0).abs() < 1e-6 * window.measured.0);
    }
    let gpu_of = |key: &ConsumerKey| {
        outcome
            .windows
            .iter()
            .filter_map(|w| w.ledger.get(key))
            .map(|s| s.gpu.0)
            .sum::<f64>()
    };
    assert_eq!(gpu_of(&ConsumerKey::SystemUnit("init.scope".into())), 0.0);
    assert!(gpu_of(&ConsumerKey::Shell) > 0.0);
    // Display and devices dominate a lightly used laptop.
    let devices = outcome.totals[&ConsumerKey::Devices].0;
    let all: f64 = outcome.totals.values().map(|j| j.0).sum();
    assert!(devices / all > 0.5, "devices share {}", devices / all);
    assert!(
        outcome
            .totals
            .contains_key(&ConsumerKey::App("org.mozilla.firefox".into()))
    );
}

#[test]
fn load_scopes_dominate_their_phases_without_rapl() {
    // Recorded unprivileged: no RAPL, so battery energy is split by CPU time. The recording
    // itself kept ~0.4 CPUs busy, so no interval was quiet enough to learn an idle floor.
    let outcome = attribute_trace("m0-load", &IdleFloor::default());
    assert!(
        outcome
            .windows
            .iter()
            .all(|w| w.measurement == Measurement::BatteryOnly)
    );
    for scope in ["load-1cpu", "load-2cpu", "load-4cpu"] {
        let key = ConsumerKey::UserUnit(format!("{scope}.scope"));
        let dominated = outcome
            .windows
            .iter()
            .filter(|w| {
                w.ledger
                    .get(&key)
                    .is_some_and(|s| s.total().0 > 0.8 * w.measured.0)
            })
            .count();
        // Each phase ran 40 s, i.e. ~4 windows of ≥ 10 s.
        assert!(dominated >= 3, "{key} dominated only {dominated} windows");
    }
}

/// Live checks against this machine: `cargo test -p drainscope-sys -- --ignored`.
mod live {
    use super::*;

    #[test]
    #[ignore = "reads the live system"]
    fn reads_the_host() {
        let root = SysRoot::host();
        let usage = read_cpu_usage(&root).unwrap();
        assert!(usage[&drainscope_model::CgroupPath::root()] > 0);
        assert_ne!(read_batteries(&root).unwrap(), Vec::new());
        let clients = DrmScanner::default().scan(&root, Duration::ZERO).unwrap();
        assert!(
            !clients.is_empty(),
            "the user's own GPU clients are visible"
        );
    }

    #[test]
    #[ignore = "reads the live system"]
    fn finds_the_wifi_interrupt_thread() {
        let root = SysRoot::host();
        let threads =
            drainscope_sys::irq_threads(&root, &drainscope_sys::network_irqs(&root).unwrap())
                .unwrap();
        assert!(!threads.is_empty(), "irq/<n>-iwlwifi on the dev machine");
        let runtimes = drainscope_sys::read_runtimes(&root, &threads).unwrap();
        assert!(runtimes.values().all(|&ns| ns > 0), "{runtimes:?}");
    }
}

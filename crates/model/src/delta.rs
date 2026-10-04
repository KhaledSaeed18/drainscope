//! Turning two snapshots into what happened in between.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::cgroup::CgroupPath;
use crate::snapshot::{
    BatteryReading, BatteryStatus, DrmClient, DrmHolder, EnergyCounter, RaplDomain, Snapshot,
};
use crate::units::{Joules, Microjoules, Watts};

/// What happened between two snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct IntervalDelta {
    pub duration: Duration,
    /// Energy per domain. Domains missing from either snapshot are absent.
    pub rapl: BTreeMap<RaplDomain, Joules>,
    pub cpu: CpuDelta,
    /// GPU engine time per cgroup of the process using it, in nanoseconds.
    pub gpu_ns: BTreeMap<CgroupPath, u64>,
    /// Present only if at least one battery discharged throughout the interval.
    pub battery: Option<BatteryDelta>,
}

/// CPU time split so that every microsecond lands in exactly one cgroup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuDelta {
    /// Each cgroup's own time: its delta minus its children's. For leaves that is their
    /// processes; for the root, kernel threads; for inner cgroups, descendants that exited
    /// during the interval (their usage stays accounted in the parent).
    pub own_usec: BTreeMap<CgroupPath, OwnTime>,
    /// Time removed when a child appeared to use more than its parent, which happens because
    /// files are read one at a time. Reported, not attributed.
    pub clamped_usec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnTime {
    pub usec: u64,
    pub has_children: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryDelta {
    /// Mean of the start and end discharge power, summed over discharging batteries.
    pub mean_power: Watts,
    /// Drop in remaining energy, summed over discharging batteries, when reported.
    pub energy_drop: Option<Joules>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiffError {
    #[error("snapshots are not in order: {next:?} is not after {prev:?}")]
    OutOfOrder { prev: Duration, next: Duration },
}

/// Computes the interval between two snapshots taken in order.
///
/// # Errors
/// [`DiffError::OutOfOrder`] if `next` wasn't taken after `prev`.
pub fn diff(prev: &Snapshot, next: &Snapshot) -> Result<IntervalDelta, DiffError> {
    let duration = next
        .taken_at
        .checked_sub(prev.taken_at)
        .filter(|d| !d.is_zero())
        .ok_or(DiffError::OutOfOrder {
            prev: prev.taken_at,
            next: next.taken_at,
        })?;
    Ok(IntervalDelta {
        duration,
        rapl: rapl_delta(&prev.rapl, &next.rapl),
        cpu: cpu_delta(&prev.cgroup_cpu_usec, &next.cgroup_cpu_usec),
        gpu_ns: gpu_delta(&prev.drm_clients, &next.drm_clients),
        battery: battery_delta(&prev.batteries, &next.batteries),
    })
}

/// Counter difference, allowing one wrap at `range`. `None` when the counter went backwards
/// without a usable range, which means it was reset rather than wrapped.
#[must_use]
pub fn counter_delta(prev: EnergyCounter, next: EnergyCounter) -> Option<Microjoules> {
    let (before, after, range) = (prev.value.0, next.value.0, next.range.0);
    if after >= before {
        Some(Microjoules(after - before))
    } else if range > before {
        Some(Microjoules(range - before + after))
    } else {
        None
    }
}

fn rapl_delta(
    prev: &BTreeMap<RaplDomain, EnergyCounter>,
    next: &BTreeMap<RaplDomain, EnergyCounter>,
) -> BTreeMap<RaplDomain, Joules> {
    next.iter()
        .filter_map(|(domain, after)| {
            let before = prev.get(domain)?;
            Some((*domain, counter_delta(*before, *after)?.to_joules()))
        })
        .collect()
}

fn cpu_delta(prev: &BTreeMap<CgroupPath, u64>, next: &BTreeMap<CgroupPath, u64>) -> CpuDelta {
    // A cgroup missing from `prev`, or whose counter went backwards (removed and recreated
    // under the same name), did all its work since it appeared.
    let total = |path: &CgroupPath, after: u64| match prev.get(path) {
        Some(&before) if after >= before => after - before,
        _ => after,
    };
    let deltas: BTreeMap<&CgroupPath, u64> = next
        .iter()
        .map(|(path, &after)| (path, total(path, after)))
        .collect();

    let mut children: BTreeMap<CgroupPath, u64> = BTreeMap::new();
    for (path, delta) in &deltas {
        if let Some(parent) = path.parent() {
            *children.entry(parent).or_default() += delta;
        }
    }

    let mut out = CpuDelta::default();
    for (path, delta) in deltas {
        let children_total = children.get(path).copied();
        let children_usec = children_total.unwrap_or(0);
        if children_usec > delta {
            out.clamped_usec += children_usec - delta;
        }
        out.own_usec.insert(
            path.clone(),
            OwnTime {
                usec: delta.saturating_sub(children_usec),
                has_children: children_total.is_some(),
            },
        );
    }
    out
}

/// PID 1 keeps duplicates of device fds in its fd store and logind brokers them to the
/// compositor; GPU time is never theirs.
fn is_fd_broker(holder: &DrmHolder) -> bool {
    let path = holder.cgroup.as_str();
    path == "init.scope" || path.ends_with("/systemd-logind.service")
}

/// The process to charge for a client: the first holder that isn't an fd broker, falling
/// back to any holder.
fn charged_holder(client: &DrmClient) -> Option<&DrmHolder> {
    client
        .holders
        .iter()
        .find(|h| !is_fd_broker(h))
        .or_else(|| client.holders.first())
}

fn gpu_delta(
    prev: &BTreeMap<u64, DrmClient>,
    next: &BTreeMap<u64, DrmClient>,
) -> BTreeMap<CgroupPath, u64> {
    let mut out: BTreeMap<CgroupPath, u64> = BTreeMap::new();
    for (id, client) in next {
        // A client first seen now is skipped rather than charged its whole lifetime at once;
        // client ids are never reused within a boot, so a decrease means bad data.
        let Some(before) = prev.get(id) else {
            continue;
        };
        let Some(busy) = client.engine_ns.checked_sub(before.engine_ns) else {
            continue;
        };
        if let Some(holder) = charged_holder(client) {
            *out.entry(holder.cgroup.clone()).or_default() += busy;
        }
    }
    out
}

fn battery_delta(prev: &[BatteryReading], next: &[BatteryReading]) -> Option<BatteryDelta> {
    let mut mean_power = 0.0;
    let mut energy_drop: Option<f64> = None;
    let mut any = false;
    for after in next {
        let Some(before) = prev.iter().find(|b| b.name == after.name) else {
            continue;
        };
        let discharging = |b: &BatteryReading| b.status == BatteryStatus::Discharging;
        if !(discharging(before) && discharging(after)) {
            continue;
        }
        any = true;
        if let (Some(p0), Some(p1)) = (before.power, after.power) {
            mean_power += f64::midpoint(p0.0, p1.0);
        }
        if let (Some(e0), Some(e1)) = (before.energy, after.energy) {
            *energy_drop.get_or_insert(0.0) += e0.0 - e1.0;
        }
    }
    any.then_some(BatteryDelta {
        mean_power: Watts(mean_power),
        energy_drop: energy_drop.map(Joules),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::ProcessId;
    use proptest::prelude::*;

    fn path(p: &str) -> CgroupPath {
        CgroupPath::new(p)
    }

    fn at(secs: u64) -> Snapshot {
        Snapshot {
            taken_at: Duration::from_secs(secs),
            ..Snapshot::default()
        }
    }

    fn counter(value: u64, range: u64) -> EnergyCounter {
        EnergyCounter {
            value: Microjoules(value),
            range: Microjoules(range),
        }
    }

    #[test]
    fn rejects_out_of_order_snapshots() {
        assert!(matches!(
            diff(&at(5), &at(5)),
            Err(DiffError::OutOfOrder { .. })
        ));
        assert!(matches!(
            diff(&at(5), &at(4)),
            Err(DiffError::OutOfOrder { .. })
        ));
    }

    #[test]
    fn counter_wraps_once() {
        assert_eq!(
            counter_delta(counter(10, 100), counter(30, 100)),
            Some(Microjoules(20))
        );
        assert_eq!(
            counter_delta(counter(90, 100), counter(5, 100)),
            Some(Microjoules(15))
        );
        // Went backwards with no usable range: a reset, not a wrap.
        assert_eq!(counter_delta(counter(90, 0), counter(5, 0)), None);
    }

    #[test]
    fn rapl_skips_domains_missing_on_either_side() {
        let mut prev = at(0);
        let mut next = at(2);
        prev.rapl
            .insert(RaplDomain::Package, counter(1_000_000, u64::MAX));
        prev.rapl.insert(RaplDomain::Core, counter(0, u64::MAX));
        next.rapl
            .insert(RaplDomain::Package, counter(4_000_000, u64::MAX));
        next.rapl.insert(RaplDomain::Dram, counter(0, u64::MAX));
        let delta = diff(&prev, &next).unwrap();
        assert_eq!(
            delta.rapl,
            BTreeMap::from([(RaplDomain::Package, Joules(3.0))])
        );
        assert_eq!(delta.duration, Duration::from_secs(2));
    }

    fn cpu(entries: &[(&str, u64)]) -> BTreeMap<CgroupPath, u64> {
        entries.iter().map(|(p, v)| (path(p), *v)).collect()
    }

    #[test]
    fn cpu_time_lands_in_exactly_one_cgroup() {
        let prev = cpu(&[
            ("", 1000),
            ("a.slice", 600),
            ("a.slice/x.scope", 400),
            ("a.slice/y.scope", 100),
        ]);
        // a.slice grew by 300: x by 100, y by 50, and 150 from a child that exited.
        let next = cpu(&[
            ("", 1500),
            ("a.slice", 900),
            ("a.slice/x.scope", 500),
            ("a.slice/y.scope", 150),
        ]);
        let delta = cpu_delta(&prev, &next);
        let own = |p: &str| delta.own_usec[&path(p)];
        assert_eq!(own("").usec, 200); // kernel
        assert_eq!(own("a.slice").usec, 150); // exited
        assert!(own("a.slice").has_children);
        assert_eq!(own("a.slice/x.scope").usec, 100);
        assert!(!own("a.slice/x.scope").has_children);
        assert_eq!(own("a.slice/y.scope").usec, 50);
        let total: u64 = delta.own_usec.values().map(|t| t.usec).sum();
        assert_eq!(total, 500);
        assert_eq!(delta.clamped_usec, 0);
    }

    #[test]
    fn new_and_recreated_cgroups_count_from_zero() {
        let prev = cpu(&[("", 0), ("s.scope", 900)]);
        let next = cpu(&[("", 1000), ("s.scope", 40), ("t.scope", 60)]);
        let delta = cpu_delta(&prev, &next);
        assert_eq!(delta.own_usec[&path("s.scope")].usec, 40);
        assert_eq!(delta.own_usec[&path("t.scope")].usec, 60);
        assert_eq!(delta.own_usec[&path("")].usec, 900);
    }

    #[test]
    fn children_read_later_than_parent_are_clamped() {
        let prev = cpu(&[("", 0), ("p.slice", 0), ("p.slice/c.scope", 0)]);
        let next = cpu(&[("", 100), ("p.slice", 100), ("p.slice/c.scope", 130)]);
        let delta = cpu_delta(&prev, &next);
        assert_eq!(delta.own_usec[&path("p.slice")].usec, 0);
        assert_eq!(delta.clamped_usec, 30);
    }

    fn holder(pid: u32, cgroup: &str) -> DrmHolder {
        DrmHolder {
            process: ProcessId {
                pid,
                start_ticks: 1,
            },
            cgroup: path(cgroup),
        }
    }

    const SHELL: &str =
        "user.slice/user-1000.slice/user@1000.service/session.slice/org.gnome.Shell@user.service";

    #[test]
    fn gpu_time_is_never_charged_to_fd_brokers() {
        let holders = vec![
            holder(1, "init.scope"),
            holder(803, "system.slice/systemd-logind.service"),
            holder(3416, SHELL),
        ];
        let client = |engine_ns| DrmClient {
            engine_ns,
            holders: holders.clone(),
        };
        let prev = BTreeMap::from([(16, client(1_000))]);
        let next = BTreeMap::from([(16, client(4_000))]);
        assert_eq!(
            gpu_delta(&prev, &next),
            BTreeMap::from([(path(SHELL), 3_000)])
        );
    }

    #[test]
    fn gpu_skips_new_and_decreasing_clients() {
        let client = |engine_ns| DrmClient {
            engine_ns,
            holders: vec![holder(10, "a.scope")],
        };
        let prev = BTreeMap::from([(1, client(500))]);
        let next = BTreeMap::from([(1, client(400)), (2, client(9_000))]);
        assert!(gpu_delta(&prev, &next).is_empty());
    }

    fn battery(name: &str, status: BatteryStatus, watts: f64, joules: f64) -> BatteryReading {
        BatteryReading {
            name: name.into(),
            status,
            power: Some(Watts(watts)),
            energy: Some(Joules(joules)),
            energy_full: None,
        }
    }

    #[test]
    fn battery_sums_only_batteries_discharging_throughout() {
        use BatteryStatus::{Discharging, NotCharging};
        let prev = [
            battery("BAT0", NotCharging, 0.0, 90_000.0),
            battery("BAT1", Discharging, 5.0, 11_000.0),
        ];
        let next = [
            battery("BAT0", NotCharging, 0.0, 90_000.0),
            battery("BAT1", Discharging, 6.0, 10_988.0),
        ];
        let delta = battery_delta(&prev, &next).unwrap();
        assert_eq!(delta.mean_power, Watts(5.5));
        assert_eq!(delta.energy_drop, Some(Joules(12.0)));
    }

    #[test]
    fn no_battery_delta_on_ac() {
        let prev = [battery("BAT0", BatteryStatus::Charging, 10.0, 1.0)];
        assert_eq!(battery_delta(&prev, &prev), None);
    }

    proptest! {
        /// With consistent reads, own times partition the root's growth exactly: leaves get
        /// their work, the slice gets its exited children, and the root gets kernel time.
        #[test]
        fn own_times_partition_root_delta(
            base in 0u64..1_000_000,
            a in 0u64..10_000, b in 0u64..10_000, exited in 0u64..10_000, kernel in 0u64..10_000,
        ) {
            let prev = cpu(&[("", base), ("s.slice", base / 2), ("s.slice/a.scope", 0), ("s.slice/b.scope", 0)]);
            let slice = a + b + exited;
            let next = cpu(&[
                ("", base + slice + kernel),
                ("s.slice", base / 2 + slice),
                ("s.slice/a.scope", a),
                ("s.slice/b.scope", b),
            ]);
            let delta = cpu_delta(&prev, &next);
            let total: u64 = delta.own_usec.values().map(|t| t.usec).sum();
            prop_assert_eq!(delta.clamped_usec, 0);
            prop_assert_eq!(total, slice + kernel);
            prop_assert_eq!(delta.own_usec[&path("s.slice")].usec, exited);
            prop_assert_eq!(delta.own_usec[&path("")].usec, kernel);
        }

        #[test]
        fn counter_delta_matches_modular_arithmetic(start in 0u64..1_000_000, step in 0u64..999_999) {
            let range = 1_000_000;
            let end = (start + step) % range;
            prop_assert_eq!(
                counter_delta(counter(start, range), counter(end, range)),
                Some(Microjoules(step))
            );
        }
    }
}

//! Attribution model v1 (ADR 0001): RAPL energy below the machine's idle floor is idle; the
//! rest was caused by activity and is split by each consumer's share of it.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::activity::Activity;
use crate::calibration::{IdleFloor, Part, part_energy};
use crate::consumer::ConsumerKey;
use crate::delta::IntervalDelta;
use crate::snapshot::RaplDomain;
use crate::units::Joules;

/// Stored with every row so a future model never silently reinterprets history.
/// 2: network-softirq time moves from Kernel to the consumers causing the traffic (ADR 0007).
pub const MODEL_VERSION: u32 = 2;

/// One consumer's energy by cause.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EnergySplit {
    /// Cores plus the package and DRAM energy CPU activity keeps awake.
    pub cpu: Joules,
    /// Integrated GPU.
    pub gpu: Joules,
    /// Outside RAPL's view: platform, display and devices.
    pub other: Joules,
}

impl EnergySplit {
    #[must_use]
    pub fn total(&self) -> Joules {
        self.cpu + self.gpu + self.other
    }

    fn add(&mut self, cause: Cause, energy: Joules) {
        match cause {
            Cause::Cpu => self.cpu += energy,
            Cause::Gpu => self.gpu += energy,
            Cause::Other => self.other += energy,
        }
    }

    /// Adds every field of `other`.
    pub fn merge(&mut self, other: &Self) {
        self.cpu += other.cpu;
        self.gpu += other.gpu;
        self.other += other.other;
    }
}

pub type Ledger = BTreeMap<ConsumerKey, EnergySplit>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cause {
    Cpu,
    Gpu,
    Other,
}

impl From<Part> for Cause {
    fn from(part: Part) -> Self {
        match part {
            Part::Uncore => Self::Gpu,
            Part::Core | Part::SocRest | Part::Dram => Self::Cpu,
        }
    }
}

pub(crate) fn credit(ledger: &mut Ledger, key: ConsumerKey, cause: Cause, energy: Joules) {
    if energy.0 > 0.0 {
        ledger.entry(key).or_default().add(cause, energy);
    }
}

/// Splits `energy` across `weights`; to `Idle` if nothing was active.
// Weights are microsecond/nanosecond counts per interval, far below 2^52.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn share(
    ledger: &mut Ledger,
    energy: Joules,
    weights: &BTreeMap<ConsumerKey, u64>,
    cause: Cause,
) {
    if energy.0 <= 0.0 {
        return;
    }
    let total: u64 = weights.values().sum();
    if total == 0 {
        credit(ledger, ConsumerKey::Idle, cause, energy);
        return;
    }
    for (key, &weight) in weights {
        credit(
            ledger,
            key.clone(),
            cause,
            energy * (weight as f64 / total as f64),
        );
    }
}

/// One tick's attribution of RAPL energy.
#[derive(Debug, Clone, PartialEq)]
pub struct TickAttribution {
    pub duration: Duration,
    pub ledger: Ledger,
    /// Total RAPL energy attributed: the parts, plus the platform when `psys` is trusted.
    pub measured: Joules,
    pub activity: Activity,
}

/// Attributes one interval's RAPL energy. Battery energy is reconciled later, over windows
/// (see [`crate::window`]), because battery readings lag activity by seconds.
#[must_use]
pub fn attribute(
    delta: &IntervalDelta,
    activity: Activity,
    floor: &IdleFloor,
    psys_trusted: bool,
) -> TickAttribution {
    let mut ledger = Ledger::new();
    let mut measured = Joules::ZERO;

    for (part, energy) in part_energy(&delta.rapl) {
        measured += energy;
        let cause = Cause::from(part);
        let base = Joules(
            floor
                .get(part)
                .for_duration(delta.duration)
                .0
                .clamp(0.0, energy.0),
        );
        credit(&mut ledger, ConsumerKey::Idle, cause, base);
        let weights = match part {
            Part::Uncore => &activity.gpu_ns,
            Part::Core | Part::SocRest | Part::Dram => &activity.cpu_usec,
        };
        share(&mut ledger, energy - base, weights, cause);
    }

    if psys_trusted
        && let (Some(psys), Some(package)) = (
            delta.rapl.get(&RaplDomain::Psys),
            delta.rapl.get(&RaplDomain::Package),
        )
    {
        let dram = delta
            .rapl
            .get(&RaplDomain::Dram)
            .copied()
            .unwrap_or_default();
        let platform = (*psys - *package - dram).non_negative();
        credit(&mut ledger, ConsumerKey::Platform, Cause::Other, platform);
        measured += platform;
    }

    TickAttribution {
        duration: delta.duration,
        ledger,
        measured,
        activity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::CpuDelta;
    use crate::units::Watts;
    use proptest::prelude::*;

    fn delta(rapl: &[(RaplDomain, f64)]) -> IntervalDelta {
        IntervalDelta {
            duration: Duration::from_secs(2),
            rapl: rapl.iter().map(|(d, j)| (*d, Joules(*j))).collect(),
            cpu: CpuDelta::default(),
            gpu_ns: BTreeMap::new(),
            battery: None,
        }
    }

    fn firefox() -> ConsumerKey {
        ConsumerKey::App("org.mozilla.firefox".into())
    }

    fn close(a: Joules, b: f64) -> bool {
        (a.0 - b).abs() < 1e-9
    }

    #[test]
    fn energy_above_the_floor_follows_activity() {
        let activity = Activity {
            cpu_usec: BTreeMap::from([(firefox(), 300_000), (ConsumerKey::Kernel, 100_000)]),
            gpu_ns: BTreeMap::from([(ConsumerKey::Shell, 1_000)]),
        };
        let floor = IdleFloor::new(BTreeMap::from([
            (Part::Core, Watts(0.2)),
            (Part::Uncore, Watts(0.1)),
        ]));
        let tick = attribute(
            &delta(&[(RaplDomain::Core, 2.4), (RaplDomain::Uncore, 0.6)]),
            activity,
            &floor,
            false,
        );
        // Core: 0.4 J idle, 2.0 J active split 3:1. Uncore: 0.2 J idle, 0.4 J to the shell.
        assert!(close(tick.ledger[&ConsumerKey::Idle].cpu, 0.4));
        assert!(close(tick.ledger[&ConsumerKey::Idle].gpu, 0.2));
        assert!(close(tick.ledger[&firefox()].cpu, 1.5));
        assert!(close(tick.ledger[&ConsumerKey::Kernel].cpu, 0.5));
        assert!(close(tick.ledger[&ConsumerKey::Shell].gpu, 0.4));
        assert!(close(tick.measured, 3.0));
    }

    #[test]
    fn without_uncore_the_package_rest_follows_cpu_time() {
        // AMD: no uncore domain, so integrated-GPU energy stays in the package and is split
        // by CPU time; GPU activity alone earns nothing (ADR 0005).
        let activity = Activity {
            cpu_usec: BTreeMap::from([(firefox(), 100_000)]),
            gpu_ns: BTreeMap::from([(ConsumerKey::Shell, 1_000)]),
        };
        let tick = attribute(
            &delta(&[(RaplDomain::Package, 5.0), (RaplDomain::Core, 2.0)]),
            activity,
            &IdleFloor::default(),
            false,
        );
        assert!(close(tick.ledger[&firefox()].cpu, 5.0));
        assert!(!tick.ledger.contains_key(&ConsumerKey::Shell));
        assert!(close(tick.measured, 5.0));
    }

    #[test]
    fn energy_below_the_floor_is_all_idle() {
        let floor = IdleFloor::new(BTreeMap::from([(Part::Core, Watts(5.0))]));
        let activity = Activity {
            cpu_usec: BTreeMap::from([(firefox(), 1)]),
            gpu_ns: BTreeMap::new(),
        };
        let tick = attribute(&delta(&[(RaplDomain::Core, 1.0)]), activity, &floor, false);
        assert_eq!(tick.ledger.keys().collect::<Vec<_>>(), [&ConsumerKey::Idle]);
    }

    #[test]
    fn untrusted_psys_is_ignored_and_trusted_psys_adds_platform() {
        let rapl = [
            (RaplDomain::Package, 3.0),
            (RaplDomain::Dram, 0.8),
            (RaplDomain::Psys, 6.0),
        ];
        let untrusted = attribute(
            &delta(&rapl),
            Activity::default(),
            &IdleFloor::default(),
            false,
        );
        assert!(close(untrusted.measured, 3.8));
        assert!(!untrusted.ledger.contains_key(&ConsumerKey::Platform));
        let trusted = attribute(
            &delta(&rapl),
            Activity::default(),
            &IdleFloor::default(),
            true,
        );
        assert!(close(trusted.measured, 6.0));
        assert!(close(trusted.ledger[&ConsumerKey::Platform].other, 2.2));
    }

    fn any_activity() -> impl Strategy<Value = BTreeMap<ConsumerKey, u64>> {
        proptest::collection::btree_map(
            "[a-z]{1,8}".prop_map(ConsumerKey::App),
            0u64..5_000_000,
            0..6,
        )
    }

    proptest! {
        #[test]
        fn attribution_conserves_energy_and_is_non_negative(
            package in 0.0f64..40.0, core in 0.0f64..30.0, uncore in 0.0f64..10.0,
            dram in 0.0f64..5.0, psys in 0.0f64..60.0,
            floor_core in 0.0f64..3.0, floor_rest in 0.0f64..3.0,
            cpu in any_activity(), gpu in any_activity(), psys_trusted: bool,
        ) {
            let rapl = [
                (RaplDomain::Package, package), (RaplDomain::Core, core),
                (RaplDomain::Uncore, uncore), (RaplDomain::Dram, dram), (RaplDomain::Psys, psys),
            ];
            let floor = IdleFloor::new(BTreeMap::from([
                (Part::Core, Watts(floor_core)), (Part::SocRest, Watts(floor_rest)),
            ]));
            let activity = Activity { cpu_usec: cpu, gpu_ns: gpu };
            let tick = attribute(&delta(&rapl), activity.clone(), &floor, psys_trusted);
            let total: Joules = tick.ledger.values().map(EnergySplit::total).sum();
            prop_assert!((total.0 - tick.measured.0).abs() <= 1e-9 * tick.measured.0.max(1.0));
            for split in tick.ledger.values() {
                prop_assert!(split.cpu.0 >= 0.0 && split.gpu.0 >= 0.0 && split.other.0 >= 0.0);
            }
            // Deterministic: same input, same output.
            let again = attribute(&delta(&rapl), activity, &floor, psys_trusted);
            prop_assert_eq!(tick, again);
        }
    }
}

//! What "idle" costs on this machine, and whether its counters can be trusted.
//!
//! Both are learned from the machine's own history, never from one session (ADR 0001): the
//! idle floor from quiet intervals only, and the `psys` verdict from how it compares with the
//! package counter.

use std::collections::BTreeMap;

use crate::activity::Activity;
use crate::delta::IntervalDelta;
use crate::snapshot::RaplDomain;
use crate::units::{Joules, Watts};

/// The parts RAPL energy is split into for attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// CPU cores.
    Core,
    /// Integrated GPU.
    Uncore,
    /// Package minus cores minus GPU: caches, memory controller, the rest of the `SoC`.
    SocRest,
    Dram,
}

impl Part {
    pub const ALL: [Self; 4] = [Self::Core, Self::Uncore, Self::SocRest, Self::Dram];

    /// Name used in storage.
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Uncore => "uncore",
            Self::SocRest => "soc-rest",
            Self::Dram => "dram",
        }
    }

    /// Inverse of [`Self::wire_name`].
    #[must_use]
    pub fn from_wire_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.wire_name() == name)
    }
}

/// Splits measured domains into parts. A part is present only if its domains were measured.
#[must_use]
pub fn part_energy(rapl: &BTreeMap<RaplDomain, Joules>) -> BTreeMap<Part, Joules> {
    let core = rapl.get(&RaplDomain::Core).copied();
    let uncore = rapl.get(&RaplDomain::Uncore).copied();
    let mut parts = BTreeMap::new();
    if let Some(core) = core {
        parts.insert(Part::Core, core.non_negative());
    }
    if let Some(uncore) = uncore {
        parts.insert(Part::Uncore, uncore.non_negative());
    }
    if let Some(package) = rapl.get(&RaplDomain::Package) {
        let rest = *package - core.unwrap_or(Joules::ZERO) - uncore.unwrap_or(Joules::ZERO);
        parts.insert(Part::SocRest, rest.non_negative());
    }
    if let Some(dram) = rapl.get(&RaplDomain::Dram) {
        parts.insert(Part::Dram, dram.non_negative());
    }
    parts
}

/// Power the machine draws when idle: per RAPL part, and at the battery (used when RAPL is
/// unavailable). Anything without a learned floor has none, so all of its energy is
/// attributed by activity.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdleFloor {
    parts: BTreeMap<Part, Watts>,
    battery: Watts,
}

impl IdleFloor {
    #[must_use]
    pub fn new(parts: BTreeMap<Part, Watts>) -> Self {
        Self {
            parts,
            battery: Watts(0.0),
        }
    }

    #[must_use]
    pub fn with_battery(mut self, battery: Watts) -> Self {
        self.battery = battery;
        self
    }

    #[must_use]
    pub fn get(&self, part: Part) -> Watts {
        self.parts.get(&part).copied().unwrap_or_default()
    }

    /// Whole-system idle power at the battery, screen included.
    #[must_use]
    pub fn battery(&self) -> Watts {
        self.battery
    }
}

/// An interval counts as idle below these activity levels. The compositor keeps drawing
/// frames on an idle desktop, so a little GPU time is allowed.
const QUIET_BUSY_CPUS: f64 = 0.25;
const QUIET_GPU_BUSY: f64 = 0.05;
/// Below this many quiet intervals (≈ 1 min at 2 s ticks) no floor is reported.
const MIN_SAMPLES: u64 = 30;
/// Counts are halved past this many samples so the floor follows the machine over time.
const AGE_AT: u64 = 20_000;
const BIN_WIDTH_W: f64 = 0.005;
const BIN_COUNT: usize = 4_000; // 0–20 W per part

/// A fixed-resolution histogram of power readings: bounded memory, exact quantiles to
/// 5 mW, and trivially persistable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerHistogram {
    counts: Vec<u32>,
    total: u64,
}

impl Default for PowerHistogram {
    fn default() -> Self {
        Self {
            counts: vec![0; BIN_COUNT],
            total: 0,
        }
    }
}

impl PowerHistogram {
    pub fn add(&mut self, power: Watts) {
        if power.0.is_nan() || power.0 < 0.0 {
            return;
        }
        // Non-negative and clamped to the last bin before converting.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bin = ((power.0 / BIN_WIDTH_W) as usize).min(BIN_COUNT - 1);
        if let Some(count) = self.counts.get_mut(bin) {
            *count = count.saturating_add(1);
            self.total += 1;
        }
        if self.total >= AGE_AT {
            for count in &mut self.counts {
                *count /= 2;
            }
            self.total = self.counts.iter().map(|&c| u64::from(c)).sum();
        }
    }

    #[must_use]
    pub fn samples(&self) -> u64 {
        self.total
    }

    /// Non-empty bins as (index, count), for persistence.
    pub fn nonzero_bins(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(bin, count)| (bin, *count))
    }

    /// Rebuilds a histogram from [`Self::nonzero_bins`]; `None` if a bin is out of range.
    #[must_use]
    pub fn from_bins(bins: impl IntoIterator<Item = (usize, u32)>) -> Option<Self> {
        let mut histogram = Self::default();
        for (bin, count) in bins {
            *histogram.counts.get_mut(bin)? = count;
            histogram.total += u64::from(count);
        }
        Some(histogram)
    }

    /// The `q`-quantile (bin centre), or `None` with too few samples.
    #[must_use]
    // Bin indexes and sample counts are small; the products are exact in f64.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn quantile(&self, q: f64) -> Option<Watts> {
        if self.total < MIN_SAMPLES {
            return None;
        }
        let target = ((q.clamp(0.0, 1.0) * self.total as f64).ceil() as u64).max(1);
        let mut seen = 0;
        for (bin, &count) in self.counts.iter().enumerate() {
            seen += u64::from(count);
            if seen >= target {
                return Some(Watts((bin as f64 + 0.5) * BIN_WIDTH_W));
            }
        }
        None
    }
}

/// Learns the idle floor per part from quiet intervals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FloorEstimator {
    histograms: BTreeMap<Part, PowerHistogram>,
    battery: PowerHistogram,
}

impl FloorEstimator {
    /// Restores an estimator from persisted histograms.
    #[must_use]
    pub fn from_histograms(parts: BTreeMap<Part, PowerHistogram>, battery: PowerHistogram) -> Self {
        Self {
            histograms: parts,
            battery,
        }
    }

    pub fn part_histograms(&self) -> impl Iterator<Item = (Part, &PowerHistogram)> {
        self.histograms.iter().map(|(part, h)| (*part, h))
    }

    #[must_use]
    pub fn battery_histogram(&self) -> &PowerHistogram {
        &self.battery
    }

    /// Records the interval's power per part if the machine was quiet.
    pub fn observe(&mut self, delta: &IntervalDelta, activity: &Activity) {
        let quiet = activity.busy_cpus(delta.duration) < QUIET_BUSY_CPUS
            && activity.gpu_busy(delta.duration) < QUIET_GPU_BUSY;
        if !quiet {
            return;
        }
        for (part, energy) in part_energy(&delta.rapl) {
            self.histograms
                .entry(part)
                .or_default()
                .add(energy.over(delta.duration));
        }
        if let Some(battery) = delta.battery {
            self.battery.add(battery.mean_power);
        }
    }

    /// The median power of quiet intervals per part: what this machine typically draws when
    /// nothing is running.
    #[must_use]
    pub fn floor(&self) -> IdleFloor {
        IdleFloor {
            parts: self
                .histograms
                .iter()
                .filter_map(|(part, h)| Some((*part, h.quantile(0.5)?)))
                .collect(),
            battery: self.battery.quantile(0.5).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsysVerdict {
    /// Not enough intervals compared yet; treated as untrusted.
    Undecided,
    Trusted,
    /// `psys` reads below the package more often than not, so it doesn't measure the platform.
    Rejected,
}

/// Comparisons needed before deciding (≈ 1 min at 2 s ticks).
const PSYS_WARMUP: u32 = 30;

/// Decides whether `psys` measures the whole platform. It must be at least the package;
/// the median `psys / package ≥ 1` exactly when at least half the comparisons pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PsysCheck {
    at_least_package: u32,
    below_package: u32,
}

impl PsysCheck {
    /// Restores persisted counts.
    #[must_use]
    pub fn from_counts(at_least_package: u32, below_package: u32) -> Self {
        Self {
            at_least_package,
            below_package,
        }
    }

    /// (intervals with `psys ≥ package`, intervals with `psys < package`).
    #[must_use]
    pub fn counts(&self) -> (u32, u32) {
        (self.at_least_package, self.below_package)
    }

    pub fn observe(&mut self, delta: &IntervalDelta) {
        let (Some(psys), Some(package)) = (
            delta.rapl.get(&RaplDomain::Psys),
            delta.rapl.get(&RaplDomain::Package),
        ) else {
            return;
        };
        if package.0 <= 0.0 {
            return;
        }
        if psys.0 >= package.0 {
            self.at_least_package = self.at_least_package.saturating_add(1);
        } else {
            self.below_package = self.below_package.saturating_add(1);
        }
    }

    #[must_use]
    pub fn verdict(&self) -> PsysVerdict {
        let total = self.at_least_package.saturating_add(self.below_package);
        if total < PSYS_WARMUP {
            PsysVerdict::Undecided
        } else if u64::from(self.at_least_package) * 2 >= u64::from(total) {
            PsysVerdict::Trusted
        } else {
            PsysVerdict::Rejected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer::ConsumerKey;
    use crate::delta::{BatteryDelta, CpuDelta};
    use std::time::Duration;

    fn interval(rapl: &[(RaplDomain, f64)]) -> IntervalDelta {
        IntervalDelta {
            duration: Duration::from_secs(2),
            rapl: rapl.iter().map(|(d, j)| (*d, Joules(*j))).collect(),
            cpu: CpuDelta::default(),
            gpu_ns: BTreeMap::new(),
            battery: None,
        }
    }

    fn busy(cpus_usec: u64) -> Activity {
        Activity {
            cpu_usec: BTreeMap::from([(ConsumerKey::Kernel, cpus_usec)]),
            gpu_ns: BTreeMap::new(),
        }
    }

    #[test]
    fn soc_rest_is_package_minus_core_and_gpu() {
        let parts = part_energy(
            &interval(&[
                (RaplDomain::Package, 3.0),
                (RaplDomain::Core, 0.8),
                (RaplDomain::Uncore, 0.2),
            ])
            .rapl,
        );
        assert_eq!(parts[&Part::SocRest], Joules(2.0));
        assert!(!parts.contains_key(&Part::Dram));
    }

    #[test]
    fn floor_learns_only_from_quiet_intervals() {
        let mut estimator = FloorEstimator::default();
        // 40 quiet intervals at 1.5 W package (core 0.2), 40 busy ones at 4 W.
        for _ in 0..40 {
            let quiet = interval(&[(RaplDomain::Package, 3.0), (RaplDomain::Core, 0.4)]);
            estimator.observe(&quiet, &busy(100_000)); // 0.05 CPUs
            let loaded = interval(&[(RaplDomain::Package, 8.0), (RaplDomain::Core, 5.0)]);
            estimator.observe(&loaded, &busy(4_000_000)); // 2 CPUs
        }
        let floor = estimator.floor();
        assert!((floor.get(Part::Core).0 - 0.2).abs() < 0.005);
        assert!((floor.get(Part::SocRest).0 - 1.3).abs() < 0.005);
    }

    #[test]
    fn battery_floor_learns_from_quiet_discharging_intervals() {
        let mut estimator = FloorEstimator::default();
        for _ in 0..40 {
            let mut quiet = interval(&[]);
            quiet.battery = Some(BatteryDelta {
                mean_power: Watts(5.1),
                energy_drop: None,
            });
            estimator.observe(&quiet, &busy(0));
            let mut loaded = quiet.clone();
            loaded.battery = Some(BatteryDelta {
                mean_power: Watts(14.0),
                energy_drop: None,
            });
            estimator.observe(&loaded, &busy(8_000_000));
        }
        assert!((estimator.floor().battery().0 - 5.1).abs() < 0.005);
    }

    #[test]
    fn no_floor_before_enough_samples() {
        let mut estimator = FloorEstimator::default();
        for _ in 0..(MIN_SAMPLES - 1) {
            estimator.observe(&interval(&[(RaplDomain::Core, 0.4)]), &busy(0));
        }
        assert_eq!(estimator.floor().get(Part::Core), Watts(0.0));
    }

    #[test]
    fn histogram_ages_without_losing_its_shape() {
        let mut histogram = PowerHistogram::default();
        for i in 0..(AGE_AT + 100) {
            histogram.add(Watts(if i % 2 == 0 { 1.0 } else { 3.0 }));
        }
        assert!(histogram.samples() < AGE_AT);
        let median = histogram.quantile(0.5).unwrap();
        assert!(median.0 > 0.99 && median.0 < 3.01);
        histogram.add(Watts(f64::NAN));
        histogram.add(Watts(-1.0));
    }

    #[test]
    fn estimators_survive_persistence() {
        let mut estimator = FloorEstimator::default();
        for i in 0..50 {
            let mut quiet = interval(&[(RaplDomain::Core, 0.3 + f64::from(i) * 0.01)]);
            quiet.battery = Some(BatteryDelta {
                mean_power: Watts(5.0),
                energy_drop: None,
            });
            estimator.observe(&quiet, &busy(0));
        }
        let parts = estimator
            .part_histograms()
            .map(|(part, h)| (part, PowerHistogram::from_bins(h.nonzero_bins()).unwrap()))
            .collect();
        let battery =
            PowerHistogram::from_bins(estimator.battery_histogram().nonzero_bins()).unwrap();
        let restored = FloorEstimator::from_histograms(parts, battery);
        assert_eq!(restored, estimator);
        assert_eq!(PowerHistogram::from_bins([(BIN_COUNT, 1)]), None);
        for part in Part::ALL {
            assert_eq!(Part::from_wire_name(part.wire_name()), Some(part));
        }
        let check = PsysCheck::from_counts(3, 40);
        assert_eq!(check.counts(), (3, 40));
    }

    #[test]
    fn psys_below_package_is_rejected() {
        // The M0 machine: psys ≈ 0.74 × package.
        let mut check = PsysCheck::default();
        for _ in 0..(PSYS_WARMUP - 1) {
            check.observe(&interval(&[
                (RaplDomain::Package, 3.4),
                (RaplDomain::Psys, 2.5),
            ]));
        }
        assert_eq!(check.verdict(), PsysVerdict::Undecided);
        check.observe(&interval(&[
            (RaplDomain::Package, 3.4),
            (RaplDomain::Psys, 2.5),
        ]));
        assert_eq!(check.verdict(), PsysVerdict::Rejected);
    }

    #[test]
    fn plausible_psys_is_trusted() {
        let mut check = PsysCheck::default();
        for _ in 0..PSYS_WARMUP {
            check.observe(&interval(&[
                (RaplDomain::Package, 3.0),
                (RaplDomain::Psys, 7.5),
            ]));
        }
        assert_eq!(check.verdict(), PsysVerdict::Trusted);
    }
}

//! Recent rates per consumer from cumulative per-cgroup counters (ADR 0006): idle exits
//! (which apps keep waking the processor) and network bytes. Metrics of their own, not (yet)
//! inputs to attribution.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use crate::activity::Resolver;
use crate::cgroup::CgroupPath;
use crate::consumer::ConsumerKey;

/// Rates are averaged over at least this much recent time.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Increases of cumulative per-cgroup counters from the probe, grouped by consumer. A cgroup
/// absent `before` counts from zero: the probe lists every cgroup with a count.
#[must_use]
pub fn deltas_by_consumer(
    before: &BTreeMap<CgroupPath, u64>,
    after: &BTreeMap<CgroupPath, u64>,
    resolver: &Resolver<'_>,
) -> BTreeMap<ConsumerKey, u64> {
    let mut by_consumer: BTreeMap<ConsumerKey, u64> = BTreeMap::new();
    for (path, count) in after {
        let delta = count.saturating_sub(before.get(path).copied().unwrap_or(0));
        if delta > 0 {
            *by_consumer.entry(resolver.consumer(path)).or_default() += delta;
        }
    }
    by_consumer
}

/// Turns cumulative per-cgroup counts from the probe into recent rates per consumer.
#[derive(Debug, Default)]
pub struct RateTracker {
    previous: Option<(u64, BTreeMap<CgroupPath, u64>)>,
    /// Per-tick counts, newest last, covering about [`RATE_WINDOW`].
    recent: VecDeque<(Duration, BTreeMap<ConsumerKey, u64>)>,
}

impl RateTracker {
    /// Adds one reading (`generation` and cumulative counts by cgroup) taken `elapsed` after
    /// the previous one. A new generation (the probe restarted) only sets a new baseline.
    pub fn observe(
        &mut self,
        generation: u64,
        counts: BTreeMap<CgroupPath, u64>,
        elapsed: Duration,
        resolver: &Resolver<'_>,
    ) {
        let previous = self.previous.replace((generation, counts));
        let (Some((was_generation, before)), Some((_, after))) = (previous, self.previous.as_ref())
        else {
            return;
        };
        if was_generation != generation {
            return;
        }
        self.recent
            .push_back((elapsed, deltas_by_consumer(&before, after, resolver)));
        while self.recent.len() > 1
            && self
                .recent
                .iter()
                .skip(1)
                .map(|(d, _)| *d)
                .sum::<Duration>()
                >= RATE_WINDOW
        {
            self.recent.pop_front();
        }
    }

    /// Forgets everything, e.g. after a suspend.
    pub fn reset(&mut self) {
        self.previous = None;
        self.recent.clear();
    }

    /// Counts per second over the recent window, largest first.
    #[must_use]
    pub fn rates(&self) -> Vec<(ConsumerKey, f64)> {
        let seconds: f64 = self.recent.iter().map(|(d, _)| d.as_secs_f64()).sum();
        if seconds <= 0.0 {
            return Vec::new();
        }
        let mut totals: BTreeMap<&ConsumerKey, u64> = BTreeMap::new();
        for (_, counts) in &self.recent {
            for (key, count) in counts {
                *totals.entry(key).or_default() += count;
            }
        }
        // Counts per minute are far below 2^52.
        #[allow(clippy::cast_precision_loss)]
        let mut rates: Vec<(ConsumerKey, f64)> = totals
            .into_iter()
            .map(|(key, count)| (key.clone(), count as f64 / seconds))
            .collect();
        rates.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-org.mozilla.firefox-1.scope";
    const NM: &str = "system.slice/NetworkManager.service";

    fn counts(entries: &[(&str, u64)]) -> BTreeMap<CgroupPath, u64> {
        entries
            .iter()
            .map(|(p, c)| (CgroupPath::new(p), *c))
            .collect()
    }

    fn rate(tracker: &RateTracker, key: &ConsumerKey) -> f64 {
        tracker
            .rates()
            .into_iter()
            .find(|(k, _)| k == key)
            .map_or(0.0, |(_, r)| r)
    }

    #[test]
    fn rates_follow_count_increases() {
        let labels = BTreeMap::new();
        let resolver = Resolver {
            own_uid: 1000,
            terminal_labels: &labels,
        };
        let firefox = ConsumerKey::App("org.mozilla.firefox".into());
        let nm = ConsumerKey::SystemUnit("NetworkManager.service".into());
        let five = Duration::from_secs(5);
        let mut tracker = RateTracker::default();
        tracker.observe(1, counts(&[(FIREFOX, 100), (NM, 10)]), five, &resolver);
        assert!(
            tracker.rates().is_empty(),
            "the first reading is a baseline"
        );
        tracker.observe(
            1,
            counts(&[(FIREFOX, 150), (NM, 15), ("init.scope", 5)]),
            five,
            &resolver,
        );
        assert!((rate(&tracker, &firefox) - 10.0).abs() < 1e-9);
        assert!((rate(&tracker, &nm) - 1.0).abs() < 1e-9);
        assert_eq!(tracker.rates()[0].0, firefox);
    }

    #[test]
    fn a_new_generation_is_a_new_baseline() {
        let labels = BTreeMap::new();
        let resolver = Resolver {
            own_uid: 1000,
            terminal_labels: &labels,
        };
        let mut tracker = RateTracker::default();
        let five = Duration::from_secs(5);
        tracker.observe(1, counts(&[(NM, 1000)]), five, &resolver);
        tracker.observe(2, counts(&[(NM, 3)]), five, &resolver);
        assert_eq!(tracker.rates(), Vec::new());
        tracker.observe(2, counts(&[(NM, 8)]), five, &resolver);
        assert_eq!(tracker.rates().len(), 1);
        assert!((tracker.rates()[0].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn keeps_about_a_minute() {
        let labels = BTreeMap::new();
        let resolver = Resolver {
            own_uid: 1000,
            terminal_labels: &labels,
        };
        let mut tracker = RateTracker::default();
        let five = Duration::from_secs(5);
        let mut total = 0;
        tracker.observe(1, counts(&[(NM, total)]), five, &resolver);
        // A busy minute, then a quiet one: only the quiet one remains.
        for _ in 0..12 {
            total += 500;
            tracker.observe(1, counts(&[(NM, total)]), five, &resolver);
        }
        for _ in 0..12 {
            total += 5;
            tracker.observe(1, counts(&[(NM, total)]), five, &resolver);
        }
        assert!((tracker.rates()[0].1 - 1.0).abs() < 1e-9);
        tracker.reset();
        assert_eq!(tracker.rates(), Vec::new());
    }
}

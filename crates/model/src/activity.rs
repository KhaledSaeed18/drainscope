//! Who was busy during an interval: CPU and GPU time grouped by consumer.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::cgroup::{CgroupIdentity, CgroupPath, classify};
use crate::consumer::ConsumerKey;
use crate::delta::IntervalDelta;

/// Label for a terminal tab whose processes couldn't be named.
const UNLABELLED_TERMINAL: &str = "terminal";

/// Maps cgroups to consumers for one user, with process-derived labels supplied by the
/// caller (terminal tabs are named by UUID; the caller knows which process is working there).
#[derive(Debug, Clone, Copy)]
pub struct Resolver<'a> {
    pub own_uid: u32,
    pub terminal_labels: &'a BTreeMap<CgroupPath, String>,
}

impl Resolver<'_> {
    /// The consumer to charge for time measured in `path` itself.
    #[must_use]
    pub fn consumer(&self, path: &CgroupPath) -> ConsumerKey {
        match classify(path, self.own_uid) {
            CgroupIdentity::Consumer(key) => key,
            CgroupIdentity::Terminal { .. } => ConsumerKey::Terminal(
                self.terminal_labels
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| UNLABELLED_TERMINAL.to_owned()),
            ),
            // A slice runs nothing itself: its own time is from children that exited.
            CgroupIdentity::Slice(name) => ConsumerKey::Exited(name),
        }
    }
}

/// CPU and GPU time per consumer for one interval.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub cpu_usec: BTreeMap<ConsumerKey, u64>,
    pub gpu_ns: BTreeMap<ConsumerKey, u64>,
}

impl Activity {
    #[must_use]
    pub fn from_delta(delta: &IntervalDelta, resolver: &Resolver<'_>) -> Self {
        let mut activity = Self::default();
        for (path, own) in &delta.cpu.own_usec {
            if own.usec > 0 {
                *activity
                    .cpu_usec
                    .entry(resolver.consumer(path))
                    .or_default() += own.usec;
            }
        }
        for (path, &busy) in &delta.gpu_ns {
            if busy > 0 {
                *activity.gpu_ns.entry(resolver.consumer(path)).or_default() += busy;
            }
        }
        activity
    }

    /// Average number of busy CPUs over `duration`.
    #[must_use]
    // CPU microseconds per interval stay far below 2^52.
    #[allow(clippy::cast_precision_loss)]
    pub fn busy_cpus(&self, duration: Duration) -> f64 {
        let secs = duration.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.cpu_usec.values().sum::<u64>() as f64 / 1e6 / secs
    }

    /// Fraction of `duration` the GPU engines were busy, summed over clients.
    #[must_use]
    // GPU nanoseconds per interval stay far below 2^52.
    #[allow(clippy::cast_precision_loss)]
    pub fn gpu_busy(&self, duration: Duration) -> f64 {
        let secs = duration.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.gpu_ns.values().sum::<u64>() as f64 / 1e9 / secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{CpuDelta, OwnTime};

    const MANAGER: &str = "user.slice/user-1000.slice/user@1000.service";

    fn own(usec: u64, has_children: bool) -> OwnTime {
        OwnTime { usec, has_children }
    }

    #[test]
    fn groups_time_by_consumer() {
        let terminal = CgroupPath::new(&format!("{MANAGER}/app.slice/ptyxis-spawn-1.scope"));
        let unnamed = CgroupPath::new(&format!("{MANAGER}/app.slice/ptyxis-spawn-2.scope"));
        let firefox_a = CgroupPath::new(&format!(
            "{MANAGER}/app.slice/app-gnome-org.mozilla.firefox-1.scope"
        ));
        let firefox_b = CgroupPath::new(&format!(
            "{MANAGER}/app.slice/app-gnome-org.mozilla.firefox-2.scope"
        ));
        let app_slice = CgroupPath::new(&format!("{MANAGER}/app.slice"));
        let delta = IntervalDelta {
            duration: Duration::from_secs(2),
            rapl: BTreeMap::new(),
            cpu: CpuDelta {
                own_usec: BTreeMap::from([
                    (CgroupPath::root(), own(300, true)),
                    (app_slice, own(50, true)),
                    (firefox_a.clone(), own(1_000, false)),
                    (firefox_b, own(500, false)),
                    (terminal.clone(), own(700, false)),
                    (unnamed, own(10, false)),
                ]),
                clamped_usec: 0,
            },
            gpu_ns: BTreeMap::from([(firefox_a, 4_000)]),
            battery: None,
        };
        let labels = BTreeMap::from([(terminal, "pnpm".to_owned())]);
        let resolver = Resolver {
            own_uid: 1000,
            terminal_labels: &labels,
        };
        let activity = Activity::from_delta(&delta, &resolver);
        let firefox = ConsumerKey::App("org.mozilla.firefox".into());
        assert_eq!(
            activity.cpu_usec,
            BTreeMap::from([
                (ConsumerKey::Kernel, 300),
                (ConsumerKey::Exited("app.slice".into()), 50),
                (firefox.clone(), 1_500),
                (ConsumerKey::Terminal("pnpm".into()), 700),
                (ConsumerKey::Terminal("terminal".into()), 10),
            ])
        );
        assert_eq!(activity.gpu_ns, BTreeMap::from([(firefox, 4_000)]));
        assert!((activity.busy_cpus(delta.duration) - 0.00128).abs() < 1e-12);
    }
}

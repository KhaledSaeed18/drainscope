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

    /// Model v2 (ADR 0007): moves the kernel's network-softirq time, `network_usec`, from
    /// Kernel to the consumers that caused the traffic, in proportion to their `bytes`. Never
    /// moves more than Kernel has, and keeps the total CPU time exact (the integer remainder
    /// goes to the consumer with the most bytes). A no-op without traffic.
    pub fn charge_network(&mut self, network_usec: u64, bytes: &BTreeMap<ConsumerKey, u64>) {
        let total_bytes: u128 = bytes.values().map(|&b| u128::from(b)).sum();
        let kernel = self
            .cpu_usec
            .get(&ConsumerKey::Kernel)
            .copied()
            .unwrap_or(0);
        let moved = network_usec.min(kernel);
        if moved == 0 || total_bytes == 0 {
            return;
        }
        let mut given = 0;
        for (key, &count) in bytes {
            // moved × count / total ≤ moved, so it fits in u64.
            let share =
                u64::try_from(u128::from(moved) * u128::from(count) / total_bytes).unwrap_or(moved);
            if share > 0 {
                *self.cpu_usec.entry(key.clone()).or_default() += share;
                given += share;
            }
        }
        if let Some((key, _)) = bytes.iter().max_by_key(|(_, count)| **count) {
            *self.cpu_usec.entry(key.clone()).or_default() += moved - given;
        }
        if kernel == moved {
            self.cpu_usec.remove(&ConsumerKey::Kernel);
        } else if let Some(left) = self.cpu_usec.get_mut(&ConsumerKey::Kernel) {
            *left = kernel - moved;
        }
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

    fn firefox() -> ConsumerKey {
        ConsumerKey::App("org.mozilla.firefox".into())
    }

    #[test]
    fn network_time_moves_from_kernel_by_bytes() {
        let dnf = ConsumerKey::SystemUnit("dnf.service".into());
        let mut activity = Activity {
            cpu_usec: BTreeMap::from([(ConsumerKey::Kernel, 1_000), (firefox(), 500)]),
            gpu_ns: BTreeMap::new(),
        };
        activity.charge_network(
            300,
            &BTreeMap::from([(firefox(), 2_000), (dnf.clone(), 1_000)]),
        );
        assert_eq!(activity.cpu_usec[&ConsumerKey::Kernel], 700);
        assert_eq!(activity.cpu_usec[&firefox()], 700);
        assert_eq!(activity.cpu_usec[&dnf], 100);
    }

    #[test]
    fn network_time_is_capped_by_kernel_time() {
        let mut activity = Activity {
            cpu_usec: BTreeMap::from([(ConsumerKey::Kernel, 100)]),
            gpu_ns: BTreeMap::new(),
        };
        activity.charge_network(5_000, &BTreeMap::from([(firefox(), 10)]));
        assert_eq!(activity.cpu_usec, BTreeMap::from([(firefox(), 100)]));

        let unchanged = activity.clone();
        activity.charge_network(5_000, &BTreeMap::new());
        assert_eq!(activity, unchanged);
    }

    proptest::proptest! {
        #[test]
        fn charging_network_conserves_cpu_time(
            kernel in 0_u64..10_000_000,
            other in 0_u64..10_000_000,
            network in 0_u64..20_000_000,
            bytes in proptest::collection::vec(0_u64..u64::MAX / 4, 0..6),
        ) {
            let mut activity = Activity {
                cpu_usec: BTreeMap::from([(ConsumerKey::Kernel, kernel), (firefox(), other)]),
                gpu_ns: BTreeMap::new(),
            };
            let before: u64 = activity.cpu_usec.values().sum();
            let bytes: BTreeMap<ConsumerKey, u64> = bytes
                .into_iter()
                .enumerate()
                .map(|(i, b)| (ConsumerKey::SystemUnit(format!("u{i}.service")), b))
                .collect();
            activity.charge_network(network, &bytes);
            proptest::prop_assert_eq!(activity.cpu_usec.values().sum::<u64>(), before);
            let kernel_after = activity.cpu_usec.get(&ConsumerKey::Kernel).copied().unwrap_or(0);
            proptest::prop_assert!(kernel_after <= kernel);
        }
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

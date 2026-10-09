//! Reads everything except RAPL (which comes from the sampler) into a snapshot.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use drainscope_model::{CgroupPath, Snapshot};
use drainscope_sys::{
    DrmScanner, SysError, SysRoot, irq_threads, network_irqs, read_batteries, read_cpu_usage,
    read_runtimes, terminal_labels,
};

/// How often the network IRQ threads are looked up again; they only change when a driver
/// loads or a device appears.
const IRQ_THREADS_RESCAN: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct Collector {
    root: SysRoot,
    drm: DrmScanner,
    /// The network devices' threaded interrupt handlers, and when they were looked up.
    irq_threads: Option<(Duration, Vec<u32>)>,
}

#[derive(Debug, Clone)]
pub struct Collected {
    /// Without RAPL counters.
    pub snapshot: Snapshot,
    pub terminal_labels: BTreeMap<CgroupPath, String>,
    /// Time of the network devices' threaded interrupt handlers, by thread id.
    pub network_irq_ns: BTreeMap<u32, u64>,
}

impl Collector {
    #[must_use]
    pub fn new(root: SysRoot) -> Self {
        Self::new_with_scanner(root, DrmScanner::default())
    }

    /// Reuses a DRM scanner and its cache, e.g. when replaying recorded snapshots that each
    /// live under a different root.
    #[must_use]
    pub fn new_with_scanner(root: SysRoot, drm: DrmScanner) -> Self {
        Self {
            root,
            drm,
            irq_threads: None,
        }
    }

    /// Hands back the DRM scanner, leaving a fresh one.
    pub fn take_scanner(&mut self) -> DrmScanner {
        std::mem::take(&mut self.drm)
    }

    /// # Errors
    /// Unexpected errors from the sysfs, procfs and cgroupfs readers.
    pub fn collect(&mut self, taken_at: Duration) -> Result<Collected, SysError> {
        let cgroup_cpu_usec = read_cpu_usage(&self.root)?;
        let terminal_labels = terminal_labels(&self.root, cgroup_cpu_usec.keys())?;
        let drm_clients = self.drm.scan(&self.root, taken_at)?;
        let batteries = read_batteries(&self.root)?;
        let network_irq_ns = self.network_irq_ns(taken_at)?;
        Ok(Collected {
            snapshot: Snapshot {
                taken_at,
                rapl: BTreeMap::new(),
                cgroup_cpu_usec,
                drm_clients,
                batteries,
            },
            terminal_labels,
            network_irq_ns,
        })
    }

    /// The network IRQ threads' runtimes, rescanning for the threads when the list is stale
    /// or one of them exited.
    fn network_irq_ns(&mut self, taken_at: Duration) -> Result<BTreeMap<u32, u64>, SysError> {
        let fresh = |at: Duration| taken_at.saturating_sub(at) < IRQ_THREADS_RESCAN;
        if let Some((at, threads)) = &self.irq_threads
            && fresh(*at)
        {
            let runtimes = read_runtimes(&self.root, threads)?;
            if runtimes.len() == threads.len() {
                return Ok(runtimes);
            }
        }
        let threads = irq_threads(&self.root, &network_irqs(&self.root)?)?;
        let runtimes = read_runtimes(&self.root, &threads)?;
        self.irq_threads = Some((taken_at, threads));
        Ok(runtimes)
    }
}

/// `CLOCK_MONOTONIC`: interval lengths, excluding suspend.
#[must_use]
pub fn monotonic_now() -> Duration {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(
        u64::try_from(now.tv_sec).unwrap_or_default(),
        u32::try_from(now.tv_nsec).unwrap_or_default(),
    )
}

/// Wall clock in Unix milliseconds, for stored timestamps.
#[must_use]
pub fn wall_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_a_recorded_snapshot() {
        let trace = drainscope_testkit::trace("m0-battery-2min");
        let dir = tempfile::tempdir().unwrap();
        drainscope_testkit::materialize(&trace.snapshots[0], dir.path());
        let mut collector = Collector::new(SysRoot::at(dir.path()));
        let collected = collector.collect(Duration::from_secs(1)).unwrap();
        assert!(collected.snapshot.rapl.is_empty());
        assert!(collected.snapshot.cgroup_cpu_usec.len() > 100);
        assert_eq!(collected.snapshot.batteries.len(), 2);
        assert!(!collected.snapshot.drm_clients.is_empty());
    }

    #[test]
    fn caches_the_network_irq_threads_and_rescans_when_one_exits() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        std::fs::create_dir_all(base.join("sys/class/net/wlp2s0/device/msi_irqs/135")).unwrap();
        let thread = |pid: u32, ns: u64| {
            std::fs::create_dir_all(base.join(format!("proc/{pid}"))).unwrap();
            std::fs::write(base.join(format!("proc/{pid}/comm")), "irq/135-iwlwifi\n").unwrap();
            std::fs::write(
                base.join(format!("proc/{pid}/schedstat")),
                format!("{ns} 1 1\n"),
            )
            .unwrap();
        };
        thread(664, 1_000);
        let mut collector = Collector::new(SysRoot::at(base));
        let at = Duration::from_secs(10);
        assert_eq!(
            collector.network_irq_ns(at).unwrap(),
            BTreeMap::from([(664, 1_000)])
        );

        // Cached: a thread that appears is only found at the next rescan.
        thread(665, 50);
        assert_eq!(
            collector
                .network_irq_ns(at + Duration::from_secs(5))
                .unwrap()
                .len(),
            1
        );
        // The driver reloaded: the old thread is gone, so the threads are looked up again.
        std::fs::remove_dir_all(base.join("proc/664")).unwrap();
        assert_eq!(
            collector
                .network_irq_ns(at + Duration::from_secs(10))
                .unwrap(),
            BTreeMap::from([(665, 50)])
        );
    }

    #[test]
    fn clocks_move_forward() {
        let a = monotonic_now();
        let b = monotonic_now();
        assert!(b >= a);
        assert!(wall_now_ms() > 1_700_000_000_000);
    }
}

//! Reads everything except RAPL (which comes from the sampler) into a snapshot.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use drainscope_model::{CgroupPath, Snapshot};
use drainscope_sys::{
    DrmScanner, SysError, SysRoot, read_batteries, read_cpu_usage, terminal_labels,
};

#[derive(Debug)]
pub struct Collector {
    root: SysRoot,
    drm: DrmScanner,
}

#[derive(Debug, Clone)]
pub struct Collected {
    /// Without RAPL counters.
    pub snapshot: Snapshot,
    pub terminal_labels: BTreeMap<CgroupPath, String>,
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
        Self { root, drm }
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
        Ok(Collected {
            snapshot: Snapshot {
                taken_at,
                rapl: BTreeMap::new(),
                cgroup_cpu_usec,
                drm_clients,
                batteries,
            },
            terminal_labels,
        })
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
    fn clocks_move_forward() {
        let a = monotonic_now();
        let b = monotonic_now();
        assert!(b >= a);
        assert!(wall_now_ms() > 1_700_000_000_000);
    }
}

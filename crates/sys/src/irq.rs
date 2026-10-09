//! Network devices' threaded interrupt handlers: kernel threads named `irq/<n>-<driver>`, such
//! as `irq/135-iwlwifi`. Their CPU time is Kernel's own time; model v3 charges it to the
//! consumers causing the traffic, like the network softirqs (ADR 0008).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use crate::error::SysError;
use crate::process::list_pids;
use crate::root::{SysRoot, is_gone};

const NET: &str = "sys/class/net";

/// Interrupt numbers of the physical network devices: their MSI vectors
/// (`device/msi_irqs/<n>`), or the legacy line (`device/irq`) without MSI. Virtual interfaces
/// (no `device`) have none.
///
/// # Errors
/// If `/sys/class/net` exists but can't be listed.
pub fn network_irqs(root: &SysRoot) -> Result<BTreeSet<u32>, SysError> {
    let dir = root.path(NET);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if is_gone(&err) => return Ok(BTreeSet::new()),
        Err(err) => return Err(SysError::io(&dir, err)),
    };
    let mut irqs = BTreeSet::new();
    for entry in entries.filter_map(Result::ok) {
        let device = entry.path().join("device");
        let vectors: BTreeSet<u32> = fs::read_dir(device.join("msi_irqs"))
            .map(|vectors| {
                vectors
                    .filter_map(Result::ok)
                    .filter_map(|v| v.file_name().to_str()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        if vectors.is_empty() {
            let legacy = root
                .read_optional(&device.join("irq"))?
                .and_then(|text| text.trim().parse::<u32>().ok())
                .filter(|&irq| irq > 0);
            irqs.extend(legacy);
        } else {
            irqs.extend(vectors);
        }
    }
    Ok(irqs)
}

/// The interrupt number in a threaded handler's name, `irq/<n>-<name>`.
#[must_use]
pub fn irq_of_thread(comm: &str) -> Option<u32> {
    let rest = comm.strip_prefix("irq/")?;
    rest.get(..rest.find('-')?)?.parse().ok()
}

/// Kernel threads handling `irqs`, by pid. Scans every process's `comm`, so callers should
/// cache the result and rescan only occasionally.
///
/// # Errors
/// If `/proc` can't be listed.
pub fn irq_threads(root: &SysRoot, irqs: &BTreeSet<u32>) -> Result<Vec<u32>, SysError> {
    if irqs.is_empty() {
        return Ok(Vec::new());
    }
    let mut threads = Vec::new();
    for pid in list_pids(root)? {
        let comm = root.read_optional(&root.path(&format!("proc/{pid}/comm")))?;
        if comm
            .and_then(|c| irq_of_thread(c.trim_end()))
            .is_some_and(|irq| irqs.contains(&irq))
        {
            threads.push(pid);
        }
    }
    Ok(threads)
}

/// A thread's time on the CPU in nanoseconds, from the first field of `/proc/<pid>/schedstat`.
/// `Ok(None)` when the thread is gone.
///
/// # Errors
/// Unexpected I/O errors or a malformed file.
pub fn read_runtime_ns(root: &SysRoot, pid: u32) -> Result<Option<u64>, SysError> {
    let path = root.path(&format!("proc/{pid}/schedstat"));
    let Some(text) = root.read_optional(&path)? else {
        return Ok(None);
    };
    text.split_whitespace()
        .next()
        .and_then(|field| field.parse().ok())
        .map(Some)
        .ok_or_else(|| SysError::parse(&path, "unexpected schedstat format"))
}

/// Runtime of each of `threads` that still exists, by pid.
///
/// # Errors
/// Unexpected I/O errors or a malformed `schedstat`.
pub fn read_runtimes(root: &SysRoot, threads: &[u32]) -> Result<BTreeMap<u32, u64>, SysError> {
    let mut runtimes = BTreeMap::new();
    for &pid in threads {
        if let Some(ns) = read_runtime_ns(root, pid)? {
            runtimes.insert(pid, ns);
        }
    }
    Ok(runtimes)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn write(base: &Path, relative: &str, text: &str) {
        let path = base.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Wi-Fi with one MSI vector and a threaded handler, Ethernet on a legacy line without
    /// one, loopback and a bridge without devices, and an unrelated IRQ thread. Laid out like
    /// the dev machine (i7-8550U, iwlwifi).
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        fs::create_dir_all(base.join("sys/class/net/wlp2s0/device/msi_irqs/135")).unwrap();
        write(base, "sys/class/net/wlp2s0/device/irq", "135\n");
        write(base, "sys/class/net/enp3s0/device/irq", "17\n");
        fs::create_dir_all(base.join("sys/class/net/lo")).unwrap();
        fs::create_dir_all(base.join("sys/class/net/docker0")).unwrap();
        for (pid, comm, schedstat) in [
            (664, "irq/135-iwlwifi", "7734224915 1201 5120"),
            (98, "irq/121-aerdrv", "100645701 10 20"),
            (17, "kworker/0:1", "5 1 1"),
            (1, "systemd", "123 4 5"),
        ] {
            write(base, &format!("proc/{pid}/comm"), &format!("{comm}\n"));
            write(
                base,
                &format!("proc/{pid}/schedstat"),
                &format!("{schedstat}\n"),
            );
        }
        dir
    }

    #[test]
    fn finds_the_network_devices_interrupts() {
        let dir = fixture();
        let irqs = network_irqs(&SysRoot::at(dir.path())).unwrap();
        assert_eq!(irqs, BTreeSet::from([17, 135]));
    }

    #[test]
    fn no_network_class_means_no_interrupts() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            network_irqs(&SysRoot::at(dir.path())).unwrap(),
            BTreeSet::new()
        );
    }

    #[test]
    fn parses_handler_thread_names() {
        assert_eq!(irq_of_thread("irq/135-iwlwifi"), Some(135));
        assert_eq!(irq_of_thread("irq/136-iwlwifi:queue_1"), Some(136));
        assert_eq!(irq_of_thread("irq/9-acpi"), Some(9));
        assert_eq!(irq_of_thread("irq_work/0"), None);
        assert_eq!(irq_of_thread("kworker/0:1"), None);
        assert_eq!(irq_of_thread("irq/-x"), None);
    }

    #[test]
    fn finds_only_the_network_handler_threads() {
        let dir = fixture();
        let root = SysRoot::at(dir.path());
        let irqs = network_irqs(&root).unwrap();
        assert_eq!(irq_threads(&root, &irqs).unwrap(), vec![664]);
        assert_eq!(
            irq_threads(&root, &BTreeSet::new()).unwrap(),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn reads_runtimes_of_threads_that_still_exist() {
        let dir = fixture();
        let root = SysRoot::at(dir.path());
        assert_eq!(
            read_runtimes(&root, &[664, 4242]).unwrap(),
            BTreeMap::from([(664, 7_734_224_915)])
        );
    }

    #[test]
    fn malformed_schedstat_is_an_error() {
        let dir = fixture();
        write(dir.path(), "proc/664/schedstat", "garbage\n");
        assert!(read_runtime_ns(&SysRoot::at(dir.path()), 664).is_err());
    }
}

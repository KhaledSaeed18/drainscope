//! `drainscope doctor`: checks every link from the kernel to the daemon and says how to fix
//! what's missing. The one CLI command that reads the system directly (read-only).

use std::fmt::Write as _;
use std::fs;
use std::time::Duration;

use drainscope_dbus::monitor::Monitor1Proxy;
use drainscope_dbus::probe::{Probe1Proxy, ProbeError};
use drainscope_dbus::sampler::{Sampler1Proxy, SamplerError};
use drainscope_model::CgroupPath;
use drainscope_sys::{
    DrmScanner, EngineTime, SysRoot, gpu_drivers, irq_threads, network_irqs, read_batteries,
    read_cpu_usage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub message: String,
    pub hint: Option<String>,
}

fn check(level: Level, message: impl Into<String>, hint: Option<&str>) -> Check {
    Check {
        level,
        message: message.into(),
        hint: hint.map(str::to_owned),
    }
}

#[must_use]
pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        let mark = match c.level {
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail => "✗",
        };
        let _ = writeln!(out, "{mark} {}", c.message);
        if let Some(hint) = &c.hint {
            let _ = writeln!(out, "    {hint}");
        }
    }
    out
}

fn cgroups(root: &SysRoot) -> Check {
    match read_cpu_usage(root) {
        Ok(usage) if usage.contains_key(&CgroupPath::root()) => check(
            Level::Ok,
            format!("cgroup v2 CPU accounting ({} cgroups)", usage.len()),
            None,
        ),
        Ok(_) => check(
            Level::Fail,
            "no cgroup v2 CPU accounting at /sys/fs/cgroup",
            Some(
                "drainscope needs the unified cgroup v2 hierarchy (the default on current distributions)",
            ),
        ),
        Err(err) => check(Level::Fail, format!("reading cgroups failed: {err}"), None),
    }
}

fn batteries(root: &SysRoot) -> Check {
    match read_batteries(root) {
        Ok(batteries) if !batteries.is_empty() => {
            let list: Vec<String> = batteries
                .iter()
                .map(|b| format!("{} ({:?})", b.name, b.status))
                .collect();
            check(Level::Ok, format!("batteries: {}", list.join(", ")), None)
        }
        Ok(_) => check(
            Level::Warn,
            "no system battery found",
            Some("energy is still measured with RAPL, but there is no battery usage to report"),
        ),
        Err(err) => check(
            Level::Fail,
            format!("reading batteries failed: {err}"),
            None,
        ),
    }
}

fn gpu(root: &SysRoot) -> Check {
    let drivers = gpu_drivers(root);
    let names: Vec<&str> = drivers.iter().map(|(_, driver)| driver.as_str()).collect();
    let listed = if names.is_empty() {
        "no GPU found".to_owned()
    } else {
        format!("GPU driver: {}", names.join(", "))
    };
    gpu_check(
        &listed,
        &names
            .iter()
            .map(|d| EngineTime::of_driver(d))
            .collect::<Vec<_>>(),
        DrmScanner::default()
            .scan(root, Duration::ZERO)
            .map(|c| c.len()),
    )
}

fn gpu_check(
    listed: &str,
    support: &[EngineTime],
    clients: Result<usize, drainscope_sys::SysError>,
) -> Check {
    if support.contains(&EngineTime::Cycles) {
        return check(
            Level::Warn,
            format!("{listed}: reports GPU cycles, not busy time"),
            Some("per-app GPU energy isn't supported for xe yet; it counts as idle"),
        );
    }
    if !support.is_empty() && support.iter().all(|s| *s == EngineTime::None) {
        return check(
            Level::Warn,
            format!("{listed}: no per-app GPU statistics"),
            Some("GPU energy counts as idle"),
        );
    }
    match clients {
        Ok(0) => check(
            Level::Warn,
            format!("{listed}: no GPU clients visible (DRM fdinfo)"),
            Some("GPU energy counts as idle until an app uses the GPU"),
        ),
        Ok(count) => check(
            Level::Ok,
            format!("{listed}: GPU time visible for {count} DRM clients"),
            None,
        ),
        Err(err) => check(
            Level::Warn,
            format!("{listed}: scanning GPU clients failed: {err}"),
            None,
        ),
    }
}

/// The network devices' threaded interrupt handlers, whose time model v3 charges to the apps
/// causing the traffic (ADR 0008).
fn network_interrupts(root: &SysRoot) -> Check {
    let threads = network_irqs(root).and_then(|irqs| irq_threads(root, &irqs));
    match threads {
        Ok(pids) if pids.is_empty() => check(
            Level::Ok,
            "network interrupts: no handler threads (handled directly, counted with the softirqs)",
            None,
        ),
        Ok(pids) => {
            let names: Vec<String> = pids
                .iter()
                .filter_map(|&pid| drainscope_sys::process::read_stat(root, pid).ok().flatten())
                .map(|stat| stat.comm)
                .collect();
            check(
                Level::Ok,
                format!(
                    "network interrupt threads: {} (charged to apps by traffic, with the probe)",
                    names.join(", ")
                ),
                None,
            )
        }
        Err(err) => check(
            Level::Warn,
            format!("finding network interrupt threads failed: {err}"),
            None,
        ),
    }
}

/// RAPL zone names are world-readable even though the counters aren't.
fn rapl_zones(root: &SysRoot) -> Check {
    let dir = root.path("sys/class/powercap");
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with("intel-rapl-mmio")
        })
        .filter_map(|entry| fs::read_to_string(entry.path().join("name")).ok())
        .map(|name| name.trim().to_owned())
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        check(
            Level::Warn,
            "no RAPL energy counters on this machine",
            Some("drainscope falls back to battery readings split by CPU time"),
        )
    } else if names.iter().any(|n| n.starts_with("package")) && !names.iter().any(|n| n == "uncore")
    {
        // AMD and some Intel parts: integrated-GPU energy is inside the package (ADR 0005).
        check(
            Level::Ok,
            format!("RAPL domains: {}", names.join(", ")),
            Some("no uncore domain: integrated-GPU energy is split by CPU time"),
        )
    } else {
        check(
            Level::Ok,
            format!("RAPL domains: {}", names.join(", ")),
            None,
        )
    }
}

async fn sampler(system: Option<&zbus::Connection>) -> Check {
    let Some(bus) = system else {
        return check(Level::Fail, "no system bus", None);
    };
    let proxy = match Sampler1Proxy::new(bus).await {
        Ok(proxy) => proxy,
        Err(err) => return check(Level::Fail, format!("sampler proxy: {err}"), None),
    };
    match proxy.read_counters().await {
        Ok((_, _, counters)) => check(
            Level::Ok,
            format!("sampler: {} RAPL counters readable", counters.len()),
            None,
        ),
        Err(SamplerError::RateLimited(_)) => check(Level::Ok, "sampler reachable", None),
        Err(SamplerError::NotAuthorized(_)) => check(
            Level::Fail,
            "polkit refused access to the energy counters",
            Some(
                "only the user at the machine's own seat may read them; remote sessions are refused by design",
            ),
        ),
        Err(SamplerError::ZBus(zbus::Error::MethodError(name, ..)))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            check(
                Level::Fail,
                "the RAPL sampler isn't installed",
                Some(
                    "install the drainscope-sampler package, or for development: sudo target/release/xtask install-dev",
                ),
            )
        }
        Err(err) => check(Level::Fail, format!("sampler failed: {err}"), None),
    }
}

/// The eBPF probe is optional: problems are warnings.
async fn probe(system: Option<&zbus::Connection>) -> Check {
    let Some(bus) = system else {
        return check(Level::Warn, "no system bus: no wakeup counts", None);
    };
    let proxy = match Probe1Proxy::new(bus).await {
        Ok(proxy) => proxy,
        Err(err) => return check(Level::Warn, format!("probe proxy: {err}"), None),
    };
    match proxy.read_wakeups().await {
        Ok((_, _, wakeups)) => check(
            Level::Ok,
            format!("probe: wakeups counted for {} cgroups", wakeups.len()),
            None,
        ),
        Err(ProbeError::RateLimited(_)) => check(Level::Ok, "probe reachable", None),
        Err(ProbeError::NotAuthorized(_)) => check(
            Level::Warn,
            "polkit refused access to wakeup counts",
            Some("only the user at the machine's own seat may read them"),
        ),
        Err(ProbeError::ZBus(zbus::Error::MethodError(name, ..)))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            check(
                Level::Warn,
                "the eBPF probe isn't installed: no wakeup counts",
                Some("optional; install the drainscope-probe package"),
            )
        }
        Err(err) => check(
            Level::Warn,
            format!("probe failed: {err}"),
            Some("see: journalctl -u drainscope-probe.service"),
        ),
    }
}

async fn daemon(session: Option<&zbus::Connection>) -> Check {
    let not_running = || {
        check(
            Level::Fail,
            "the drainscope daemon isn't running",
            Some("systemctl --user enable --now drainscope.service"),
        )
    };
    let Some(bus) = session else {
        return check(Level::Fail, "no session bus", None);
    };
    let Ok(proxy) = Monitor1Proxy::new(bus).await else {
        return not_running();
    };
    match proxy.status().await {
        Ok(status) => check(
            Level::Ok,
            format!("daemon running (measurement: {status})"),
            None,
        ),
        Err(_) => not_running(),
    }
}

/// Runs every check and prints them. Returns whether nothing failed.
pub async fn run() -> bool {
    let root = SysRoot::host();
    let system = zbus::Connection::system().await.ok();
    let session = zbus::Connection::session().await.ok();
    let checks = vec![
        cgroups(&root),
        batteries(&root),
        gpu(&root),
        rapl_zones(&root),
        network_interrupts(&root),
        sampler(system.as_ref()).await,
        probe(system.as_ref()).await,
        daemon(session.as_ref()).await,
    ];
    print!("{}", render(&checks));
    checks.iter().all(|c| c.level != Level::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_gpu_support() {
        let xe = gpu_check("GPU driver: xe", &[EngineTime::Cycles], Ok(3));
        assert_eq!(xe.level, Level::Warn);
        assert!(xe.message.contains("cycles"));
        let nvidia = gpu_check("GPU driver: nvidia", &[EngineTime::None], Ok(0));
        assert!(nvidia.message.contains("no per-app GPU statistics"));
        let amd = gpu_check("GPU driver: amdgpu", &[EngineTime::Nanoseconds], Ok(4));
        assert_eq!(amd.level, Level::Ok);
        assert_eq!(
            amd.message,
            "GPU driver: amdgpu: GPU time visible for 4 DRM clients"
        );
    }

    #[test]
    fn renders_marks_and_hints() {
        let rendered = render(&[
            check(Level::Ok, "fine", None),
            check(Level::Fail, "broken", Some("fix it")),
        ]);
        assert_eq!(rendered, "✓ fine\n✗ broken\n    fix it\n");
    }

    #[test]
    fn names_the_network_interrupt_threads() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        assert!(
            network_interrupts(&root)
                .message
                .contains("no handler threads")
        );
        fs::create_dir_all(root.path("sys/class/net/wlp2s0/device/msi_irqs/135")).unwrap();
        fs::create_dir_all(root.path("proc/664")).unwrap();
        fs::write(root.path("proc/664/comm"), "irq/135-iwlwifi\n").unwrap();
        fs::write(
            root.path("proc/664/stat"),
            "664 (irq/135-iwlwifi) S 2 0 0 0 -1 2129984 0 0 0 0 0 773 0 0 -51 0 1 0 412 0 0 18446744073709551615",
        )
        .unwrap();
        let found = network_interrupts(&root);
        assert_eq!(found.level, Level::Ok);
        assert!(
            found.message.contains("irq/135-iwlwifi"),
            "{}",
            found.message
        );
    }

    #[test]
    fn reads_rapl_zone_names_without_counters() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        for (zone, name) in [
            ("intel-rapl:0", "package-0"),
            ("intel-rapl-mmio:0", "package-0"),
            ("intel-rapl:1", "psys"),
        ] {
            let path = root.path(&format!("sys/class/powercap/{zone}"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("name"), format!("{name}\n")).unwrap();
        }
        assert_eq!(rapl_zones(&root).message, "RAPL domains: package-0, psys");
    }
}

//! M0 spike: a deliberately throwaway attribution over a recorded trace.
//!
//! It exists only to answer "are the numbers sane on real hardware?" before the real model
//! is built in `drainscope-model` (M1). Don't grow this file; port what survives into M1.

// Energies, durations and counters here stay far below 2^52, and percentile indexes are
// small non-negative values, so these float conversions are exact enough for a spike.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Result, bail};

use crate::record::TERMINAL_SCOPE_PREFIXES;
use crate::trace::{self, Snapshot};

type Files = BTreeMap<String, String>;
type Ledger = BTreeMap<String, f64>;

const POWERCAP: &str = "sys/class/powercap/";
const POWER_SUPPLY: &str = "sys/class/power_supply/";
const CGROUP_ROOT: &str = "sys/fs/cgroup";
const IDLE_PERCENTILE: f64 = 0.05;

/// How the idle floor is treated.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Model {
    /// Per-domain idle floor (5th percentile of the trace) goes to `idle`; apps get the rest.
    Marginal,
    /// No idle floor: all RAPL energy is split by activity share.
    Proportional,
}

pub fn run(path: &Path, top: usize, model: Model) -> Result<()> {
    let trace = trace::read(path)?;
    if trace.snapshots.len() < 2 {
        bail!("need at least two snapshots, got {}", trace.snapshots.len());
    }
    let intervals: Vec<Interval> = trace
        .snapshots
        .windows(2)
        .filter_map(|pair| match pair {
            [a, b] => Some(Interval::between(a, b)),
            _ => None,
        })
        .collect();
    let floors = match model {
        Model::Marginal => Floors::from(&intervals),
        Model::Proportional => Floors::default(),
    };
    let calibration = Calibration {
        floors,
        psys: PsysCheck::from(&intervals),
    };

    let mut ledger = Ledger::new();
    let mut stats = Stats::default();
    for interval in &intervals {
        attribute(
            interval,
            &calibration.floors,
            calibration.psys.trusted,
            &mut ledger,
            &mut stats,
        );
    }

    println!(
        "{}",
        report(
            path,
            &trace.header,
            &intervals,
            &calibration,
            &ledger,
            &stats,
            top
        )
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Interval deltas
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Interval {
    dt_s: f64,
    /// Joules per RAPL domain name (`package-0`, `core`, `uncore`, `dram`, `psys`).
    rapl_j: Ledger,
    /// Average discharge power from `power_now`, if any battery is discharging.
    battery_w: Option<f64>,
    /// Discharged energy from `energy_now` deltas, if any battery is discharging.
    battery_energy_j: Option<f64>,
    /// CPU microseconds per consumer key.
    cpu_us: Ledger,
    /// GPU engine nanoseconds per consumer key.
    gpu_ns: Ledger,
    cpu_clamped: u32,
    cpu_clamped_us: f64,
    cpu_root_us: f64,
    cpu_exited_us: f64,
}

impl Interval {
    fn between(a: &Snapshot, b: &Snapshot) -> Self {
        let cpu = cpu_by_consumer(&a.files, &b.files);
        let (battery_w, battery_energy_j) = battery(&a.files, &b.files);
        Self {
            dt_s: b.mono_ns.saturating_sub(a.mono_ns) as f64 / 1e9,
            rapl_j: rapl_delta(&a.files, &b.files),
            battery_w,
            battery_energy_j,
            gpu_ns: gpu_by_consumer(&a.files, &b.files),
            cpu_clamped: cpu.clamped,
            cpu_clamped_us: cpu.clamped_us,
            cpu_root_us: cpu.root_us,
            cpu_exited_us: cpu.exited_us,
            cpu_us: cpu.by_consumer,
        }
    }

    fn domain(&self, name: &str) -> Option<f64> {
        self.rapl_j.get(name).copied()
    }

    /// Package minus core minus uncore: caches, memory controller, rest of the `SoC`.
    fn soc_rest(&self) -> Option<f64> {
        self.domain("package-0").map(|pkg| {
            (pkg - self.domain("core").unwrap_or(0.0) - self.domain("uncore").unwrap_or(0.0))
                .max(0.0)
        })
    }
}

fn parse_u64(text: Option<&String>) -> Option<u64> {
    text.and_then(|t| t.trim().parse().ok())
}

/// Device directory → (cumulative µJ, wrap range µJ, domain name).
fn rapl_counters(files: &Files) -> BTreeMap<&str, (u64, u64, &str)> {
    let mut out = BTreeMap::new();
    for (path, value) in files {
        let Some(device) = path
            .strip_prefix(POWERCAP)
            .and_then(|rest| rest.strip_suffix("/energy_uj"))
        else {
            continue;
        };
        // The MMIO interface duplicates package-0 on recent Intel parts.
        if device.starts_with("intel-rapl-mmio") {
            continue;
        }
        let (Some(energy), Some(name)) = (
            value.trim().parse::<u64>().ok(),
            files.get(&format!("{POWERCAP}{device}/name")),
        ) else {
            continue;
        };
        let range =
            parse_u64(files.get(&format!("{POWERCAP}{device}/max_energy_range_uj"))).unwrap_or(0);
        out.insert(device, (energy, range, name.trim()));
    }
    out
}

fn rapl_delta(a: &Files, b: &Files) -> Ledger {
    let before = rapl_counters(a);
    let mut out = Ledger::new();
    for (device, (after, range, name)) in rapl_counters(b) {
        let Some((start, _, _)) = before.get(device) else {
            continue;
        };
        let delta = if after >= *start {
            after - start
        } else {
            range.saturating_sub(*start) + after
        };
        *out.entry(name.to_owned()).or_default() += delta as f64 / 1e6;
    }
    out
}

#[derive(Debug)]
struct BatteryState {
    discharging: bool,
    power_w: Option<f64>,
    energy_wh: Option<f64>,
}

fn batteries(files: &Files) -> BTreeMap<&str, BatteryState> {
    let mut out = BTreeMap::new();
    for (path, value) in files {
        let Some(device) = path
            .strip_prefix(POWER_SUPPLY)
            .and_then(|rest| rest.strip_suffix("/type"))
        else {
            continue;
        };
        if value.trim() != "Battery" {
            continue;
        }
        let get = |name: &str| parse_u64(files.get(&format!("{POWER_SUPPLY}{device}/{name}")));
        let status = files
            .get(&format!("{POWER_SUPPLY}{device}/status"))
            .map_or("", |s| s.trim());
        let power_w = get("power_now")
            .map(|uw| uw as f64 / 1e6)
            .or_else(|| Some(get("current_now")? as f64 * get("voltage_now")? as f64 / 1e12));
        let energy_wh = get("energy_now")
            .map(|uwh| uwh as f64 / 1e6)
            .or_else(|| Some(get("charge_now")? as f64 * get("voltage_now")? as f64 / 1e12));
        out.insert(
            device,
            BatteryState {
                discharging: status == "Discharging",
                power_w,
                energy_wh,
            },
        );
    }
    out
}

fn battery(a: &Files, b: &Files) -> (Option<f64>, Option<f64>) {
    let before = batteries(a);
    let after = batteries(b);
    let mut power = None;
    let mut energy = None;
    for (device, now) in &after {
        let Some(then) = before.get(device) else {
            continue;
        };
        if !(now.discharging && then.discharging) {
            continue;
        }
        if let (Some(p0), Some(p1)) = (then.power_w, now.power_w) {
            *power.get_or_insert(0.0) += f64::midpoint(p0, p1);
        }
        if let (Some(e0), Some(e1)) = (then.energy_wh, now.energy_wh) {
            *energy.get_or_insert(0.0) += (e0 - e1) * 3600.0;
        }
    }
    (power, energy)
}

/// cgroup path relative to the cgroupfs root (`""` for the root) → cumulative `usage_usec`.
fn cgroup_usage(files: &Files) -> BTreeMap<&str, u64> {
    let mut out = BTreeMap::new();
    for (path, value) in files {
        let Some(dir) = path
            .strip_prefix(CGROUP_ROOT)
            .and_then(|rest| rest.strip_suffix("cpu.stat"))
        else {
            continue;
        };
        let usage = value
            .lines()
            .find_map(|line| line.strip_prefix("usage_usec "))
            .and_then(|v| v.trim().parse().ok());
        if let Some(usage) = usage {
            out.insert(dir.trim_matches('/'), usage);
        }
    }
    out
}

fn parent(rel: &str) -> Option<&str> {
    if rel.is_empty() {
        None
    } else {
        Some(rel.rsplit_once('/').map_or("", |(p, _)| p))
    }
}

/// Splits CPU time exactly: each cgroup's "self" time is its delta minus its live children's
/// deltas. Leaves → their consumer; root → kernel threads; inner nodes → descendants that
/// exited during the interval (their usage stays accounted in the parent).
#[derive(Debug, Default)]
struct CpuSplit {
    by_consumer: Ledger,
    clamped: u32,
    clamped_us: f64,
    root_us: f64,
    exited_us: f64,
}

fn cpu_by_consumer(a: &Files, b: &Files) -> CpuSplit {
    let before = cgroup_usage(a);
    let after = cgroup_usage(b);
    let delta = |rel: &str| -> i128 {
        let now = after.get(rel).copied().unwrap_or(0);
        i128::from(now.saturating_sub(before.get(rel).copied().unwrap_or(0)))
    };
    let mut children_sum: BTreeMap<&str, i128> = BTreeMap::new();
    let mut has_children: BTreeMap<&str, bool> = BTreeMap::new();
    for rel in after.keys() {
        if let Some(p) = parent(rel) {
            *children_sum.entry(p).or_default() += delta(rel);
            has_children.insert(p, true);
        }
    }

    let mut split = CpuSplit {
        root_us: delta("") as f64,
        ..CpuSplit::default()
    };
    for rel in after.keys() {
        let mut own = delta(rel) - children_sum.get(rel).copied().unwrap_or(0);
        if own < 0 {
            // Files are read one by one, so a child can be read "later" than its parent.
            split.clamped += 1;
            split.clamped_us += -own as f64;
            own = 0;
        }
        let own = own as f64;
        let consumer = if rel.is_empty() {
            "kernel".to_owned()
        } else if has_children.contains_key(rel) {
            split.exited_us += own;
            format!("exited:{}", unescape(rel.rsplit('/').next().unwrap_or(rel)))
        } else {
            identity(rel, b)
        };
        *split.by_consumer.entry(consumer).or_default() += own;
    }
    split
}

/// DRM client id → (pid, total engine ns). Several fds can share one client; count it once.
fn drm_clients(files: &Files) -> BTreeMap<&str, (&str, u64)> {
    let mut out: BTreeMap<&str, (&str, u64)> = BTreeMap::new();
    for (path, text) in files {
        let Some((pid, tail)) = path
            .strip_prefix("proc/")
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        if !tail.starts_with("fdinfo/") {
            continue;
        }
        let mut client = None;
        let mut engine_ns = 0;
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            if key == "drm-client-id" {
                client = Some(value);
            } else if key.starts_with("drm-engine-") && !key.starts_with("drm-engine-capacity") {
                engine_ns += value
                    .trim_end_matches("ns")
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(0);
            }
        }
        let Some(client) = client else {
            continue;
        };
        // PID 1 (fd store) and logind hold duplicates of the compositor's DRM fd; the same
        // client must be charged to the process actually using it.
        match out.get(client) {
            Some((holder, _)) if !is_fd_broker(files, holder) => {}
            _ => {
                out.insert(client, (pid, engine_ns));
            }
        }
    }
    out
}

fn pid_cgroup<'a>(files: &'a Files, pid: &str) -> &'a str {
    files
        .get(&format!("proc/{pid}/cgroup"))
        .and_then(|text| text.lines().find_map(|l| l.strip_prefix("0::")))
        .map_or("", |p| p.trim().trim_matches('/'))
}

fn is_fd_broker(files: &Files, pid: &str) -> bool {
    let cgroup = pid_cgroup(files, pid);
    cgroup == "init.scope" || cgroup.ends_with("/systemd-logind.service")
}

fn gpu_by_consumer(a: &Files, b: &Files) -> Ledger {
    let before = drm_clients(a);
    let mut ledger = Ledger::new();
    for (client, (pid, ns)) in drm_clients(b) {
        // A client missing from the previous snapshot is skipped rather than charged its
        // whole lifetime in one interval.
        let Some((_, previous)) = before.get(client) else {
            continue;
        };
        *ledger.entry(identity(pid_cgroup(b, pid), b)).or_default() +=
            ns.saturating_sub(*previous) as f64;
    }
    ledger
}

// ---------------------------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------------------------

fn identity(rel: &str, files: &Files) -> String {
    if rel.is_empty() {
        return "kernel".to_owned();
    }
    let raw = rel.rsplit('/').next().unwrap_or(rel);
    if let Some(app) = app_id(raw) {
        return format!("app:{}", unescape(app));
    }
    let leaf = unescape(raw);
    if TERMINAL_SCOPE_PREFIXES.iter().any(|p| leaf.starts_with(p)) {
        return format!("term:{}", terminal_leader(rel, files).unwrap_or("?"));
    }
    if leaf.starts_with("org.gnome.Shell@") {
        return "shell".to_owned();
    }
    if rel.starts_with("system.slice/") {
        return format!("unit:{leaf}");
    }
    if rel.contains("/user@") {
        return format!("user-unit:{leaf}");
    }
    format!("cg:{leaf}")
}

/// `app-gnome-<id>-<n>.scope`, `app-flatpak-<id>-<n>.scope`, `app-gnome-<id>@<x>.service`,
/// and D-Bus-activated apps: `dbus-:<bus address>-<bus name>@<n>.service`.
/// Dashes inside the id are escaped as `\x2d`, so the literal dashes are separators.
fn app_id(leaf: &str) -> Option<&str> {
    if let Some(rest) = leaf.strip_prefix("dbus-:")
        && let Some((_, name)) = rest.split_once('-')
        && let Some((bus_name, _)) = name.rsplit_once('@')
    {
        return Some(bus_name);
    }
    for prefix in ["app-gnome-", "app-flatpak-"] {
        let Some(rest) = leaf.strip_prefix(prefix) else {
            continue;
        };
        if let Some(scope) = rest.strip_suffix(".scope") {
            return Some(scope.rsplit_once('-').map_or(scope, |(id, _)| id));
        }
        if let Some(service) = rest.strip_suffix(".service") {
            return service.split('@').next();
        }
    }
    None
}

fn terminal_leader<'a>(rel: &str, files: &'a Files) -> Option<&'a str> {
    let procs = files.get(&format!("{CGROUP_ROOT}/{rel}/cgroup.procs"))?;
    let pid = procs.lines().map(str::trim).find(|l| !l.is_empty())?;
    files.get(&format!("proc/{pid}/comm")).map(|c| c.trim())
}

/// Reverses systemd's `\xHH` unit-name escaping (ASCII only; enough for a spike).
fn unescape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(i) = rest.find("\\x") {
        out.push_str(&rest[..i]);
        let byte = rest
            .get(i + 2..i + 4)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = byte {
            out.push(char::from(byte));
            rest = &rest[i + 4..];
        } else {
            out.push_str("\\x");
            rest = &rest[i + 2..];
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------
// Attribution
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Floors {
    core: f64,
    uncore: f64,
    soc_rest: f64,
    dram: f64,
}

impl Floors {
    fn from(intervals: &[Interval]) -> Self {
        let floor = |f: &dyn Fn(&Interval) -> Option<f64>| {
            let mut watts: Vec<f64> = intervals
                .iter()
                .filter(|i| i.dt_s > 0.0)
                .filter_map(|i| f(i).map(|j| j / i.dt_s))
                .collect();
            percentile(&mut watts, IDLE_PERCENTILE)
        };
        Self {
            core: floor(&|i| i.domain("core")),
            uncore: floor(&|i| i.domain("uncore")),
            soc_rest: floor(&Interval::soc_rest),
            dram: floor(&|i| i.domain("dram")),
        }
    }
}

fn percentile(values: &mut [f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * q).round() as usize;
    values.get(index).copied().unwrap_or(0.0)
}

/// What the trace says about its own counters, derived once before attribution.
#[derive(Debug)]
struct Calibration {
    floors: Floors,
    psys: PsysCheck,
}

/// `psys` should cover the whole platform, so it can never be below `package`. Some firmware
/// reports something else under that name; reject it when it's physically implausible.
#[derive(Debug)]
struct PsysCheck {
    trusted: bool,
    median_ratio: Option<f64>,
}

impl PsysCheck {
    fn from(intervals: &[Interval]) -> Self {
        let mut ratios: Vec<f64> = intervals
            .iter()
            .filter_map(|i| Some(i.domain("psys")? / i.domain("package-0")?))
            .filter(|r| r.is_finite())
            .collect();
        if ratios.is_empty() {
            return Self {
                trusted: false,
                median_ratio: None,
            };
        }
        let median = percentile(&mut ratios, 0.5);
        Self {
            trusted: median >= 1.0,
            median_ratio: Some(median),
        }
    }
}

#[derive(Debug, Default)]
struct Stats {
    measured_j: f64,
    attributed_j: f64,
    negative_devices: u32,
    negative_devices_j: f64,
    psys_below_package: u32,
}

fn add(ledger: &mut Ledger, key: &str, joules: f64) {
    if joules > 0.0 {
        *ledger.entry(key.to_owned()).or_default() += joules;
    }
}

/// Splits `joules` across `weights`, or gives it all to `fallback` when nothing was active.
fn share(ledger: &mut Ledger, joules: f64, weights: &Ledger, fallback: &str) -> f64 {
    if joules <= 0.0 {
        return 0.0;
    }
    let total: f64 = weights.values().sum();
    if total <= 0.0 {
        add(ledger, fallback, joules);
        return joules;
    }
    for (key, weight) in weights {
        add(ledger, key, joules * weight / total);
    }
    joules
}

/// Idle floor → `idle`; the remainder → consumers by `weights`.
fn split_domain(
    ledger: &mut Ledger,
    joules: f64,
    floor_w: f64,
    dt_s: f64,
    weights: &Ledger,
    fallback: &str,
) -> f64 {
    let base = (floor_w * dt_s).clamp(0.0, joules.max(0.0));
    add(ledger, "idle", base);
    base + share(ledger, joules - base, weights, fallback)
}

fn attribute(
    interval: &Interval,
    floors: &Floors,
    trust_psys: bool,
    ledger: &mut Ledger,
    stats: &mut Stats,
) {
    let dt = interval.dt_s;
    let cpu = &interval.cpu_us;
    let mut attributed = 0.0;

    if let Some(core) = interval.domain("core") {
        attributed += split_domain(ledger, core, floors.core, dt, cpu, "kernel");
    }
    if let Some(uncore) = interval.domain("uncore") {
        attributed += split_domain(
            ledger,
            uncore,
            floors.uncore,
            dt,
            &interval.gpu_ns,
            "gpu:unattributed",
        );
    }
    if let Some(rest) = interval.soc_rest() {
        attributed += split_domain(ledger, rest, floors.soc_rest, dt, cpu, "kernel");
    }
    if let Some(dram) = interval.domain("dram") {
        attributed += split_domain(ledger, dram, floors.dram, dt, cpu, "kernel");
    }

    let package = interval.domain("package-0").unwrap_or(0.0);
    let dram = interval.domain("dram").unwrap_or(0.0);
    let soc = if let Some(psys) = interval.domain("psys").filter(|_| trust_psys) {
        let platform = psys - package - dram;
        if platform < 0.0 {
            stats.psys_below_package += 1;
        }
        add(ledger, "platform", platform);
        attributed += platform.max(0.0);
        psys
    } else {
        package + dram
    };

    let measured = match interval.battery_w {
        Some(watts) => {
            let battery = watts * dt;
            let devices = battery - soc;
            if devices < 0.0 {
                stats.negative_devices += 1;
                stats.negative_devices_j += -devices;
            }
            add(ledger, "devices", devices);
            attributed += devices.max(0.0);
            battery
        }
        None => soc,
    };
    stats.measured_j += measured;
    stats.attributed_j += attributed;
}

// ---------------------------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------------------------

fn report(
    path: &Path,
    header: &trace::Header,
    intervals: &[Interval],
    calibration: &Calibration,
    ledger: &Ledger,
    stats: &Stats,
    top: usize,
) -> String {
    let Calibration { floors, psys } = calibration;
    let total_s: f64 = intervals.iter().map(|i| i.dt_s).sum();
    let on_battery: Vec<&Interval> = intervals.iter().filter(|i| i.battery_w.is_some()).collect();
    let mut out = String::new();
    // `write!` into a String cannot fail.
    let _ = writeln!(
        out,
        "trace {}  kernel {}  root {}",
        path.display(),
        header.kernel,
        header.recorded_as_root
    );
    let _ = writeln!(
        out,
        "duration {total_s:.0} s, {} intervals ({} discharging)\n",
        intervals.len(),
        on_battery.len()
    );

    let _ = writeln!(out, "RAPL domains (avg W | idle floor W)");
    for name in ["package-0", "core", "uncore", "dram", "psys"] {
        let joules: f64 = intervals.iter().filter_map(|i| i.domain(name)).sum();
        let floor = match name {
            "core" => format!("{:.2}", floors.core),
            "uncore" => format!("{:.2}", floors.uncore),
            "dram" => format!("{:.2}", floors.dram),
            _ => "-".to_owned(),
        };
        if intervals.iter().any(|i| i.domain(name).is_some()) {
            let _ = writeln!(out, "  {name:<10} {:>7.2} | {floor}", joules / total_s);
        } else {
            let _ = writeln!(out, "  {name:<10} missing");
        }
    }
    let _ = writeln!(out, "  soc-rest   floor {:.2}", floors.soc_rest);
    let _ = writeln!(
        out,
        "  psys {} (median psys/package {})\n",
        if psys.trusted { "trusted" } else { "REJECTED" },
        psys.median_ratio
            .map_or("n/a".to_owned(), |r| format!("{r:.2}"))
    );

    report_battery(&mut out, &on_battery);

    let root_s: f64 = intervals.iter().map(|i| i.cpu_root_us).sum::<f64>() / 1e6;
    let exited_s: f64 = intervals.iter().map(|i| i.cpu_exited_us).sum::<f64>() / 1e6;
    let clamped: u32 = intervals.iter().map(|i| i.cpu_clamped).sum();
    let clamped_s: f64 = intervals.iter().map(|i| i.cpu_clamped_us).sum::<f64>() / 1e6;
    let gpu_s: f64 = intervals
        .iter()
        .flat_map(|i| i.gpu_ns.values())
        .sum::<f64>()
        / 1e9;
    let _ = writeln!(
        out,
        "CPU busy {root_s:.1} s (exited processes {exited_s:.1} s, clamped reads {clamped} = {clamped_s:.2} s); GPU busy {gpu_s:.1} s\n"
    );

    report_activity(&mut out, intervals, top);

    let grand: f64 = ledger.values().sum();
    let mut rows: Vec<(&String, &f64)> = ledger.iter().collect();
    rows.sort_by(|a, b| b.1.total_cmp(a.1));
    let _ = writeln!(
        out,
        "{:<48} {:>9} {:>8} {:>7}",
        "consumer", "J", "avg W", "share"
    );
    for (consumer, joules) in rows.into_iter().take(top) {
        let _ = writeln!(
            out,
            "{consumer:<48} {joules:>9.1} {:>8.3} {:>6.1}%",
            joules / total_s,
            joules / grand * 100.0
        );
    }

    let _ = writeln!(
        out,
        "\nconservation: attributed {:.1} J vs measured {:.1} J ({:+.2}%)",
        stats.attributed_j,
        stats.measured_j,
        (stats.attributed_j / stats.measured_j - 1.0) * 100.0
    );
    let _ = writeln!(
        out,
        "battery < psys in {} intervals ({:.1} J); psys < package+dram in {} intervals",
        stats.negative_devices, stats.negative_devices_j, stats.psys_below_package
    );
    out
}

/// Cross-checks the battery's own readings against RAPL while discharging.
fn report_battery(out: &mut String, on_battery: &[&Interval]) {
    if on_battery.is_empty() {
        return;
    }
    let secs: f64 = on_battery.iter().map(|i| i.dt_s).sum();
    let power_j: f64 = on_battery
        .iter()
        .filter_map(|i| i.battery_w.map(|w| w * i.dt_s))
        .sum();
    let energy_j: f64 = on_battery.iter().filter_map(|i| i.battery_energy_j).sum();
    let psys_j: f64 = on_battery.iter().filter_map(|i| i.domain("psys")).sum();
    let pkg_j: f64 = on_battery
        .iter()
        .filter_map(|i| i.domain("package-0"))
        .sum();
    let _ = writeln!(out, "Battery while discharging ({secs:.0} s)");
    let _ = writeln!(out, "  power_now integrated  {:>7.2} W", power_j / secs);
    let _ = writeln!(
        out,
        "  energy_now deltas     {:>7.2} W  ({:+.1}% vs power_now)",
        energy_j / secs,
        (energy_j / power_j - 1.0) * 100.0
    );
    let _ = writeln!(
        out,
        "  psys                  {:>7.2} W  ({:.0}% of battery)",
        psys_j / secs,
        psys_j / power_j * 100.0
    );
    let _ = writeln!(
        out,
        "  package-0             {:>7.2} W  ({:.0}% of battery)\n",
        pkg_j / secs,
        pkg_j / power_j * 100.0
    );
}

/// CPU and GPU time per consumer: checks identity rules even without energy counters.
fn report_activity(out: &mut String, intervals: &[Interval], top: usize) {
    let mut activity: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
    for interval in intervals {
        for (consumer, us) in &interval.cpu_us {
            activity.entry(consumer).or_default().0 += us / 1e6;
        }
        for (consumer, ns) in &interval.gpu_ns {
            activity.entry(consumer).or_default().1 += ns / 1e9;
        }
    }
    let mut rows: Vec<(&str, (f64, f64))> = activity.into_iter().collect();
    rows.sort_by(|a, b| (b.1.0 + b.1.1).total_cmp(&(a.1.0 + a.1.1)));
    let _ = writeln!(out, "{:<48} {:>9} {:>9}", "activity", "CPU s", "GPU s");
    for (consumer, (cpu_s, gpu_s)) in rows.into_iter().take(top) {
        let _ = writeln!(out, "{consumer:<48} {cpu_s:>9.2} {gpu_s:>9.2}");
    }
    let _ = writeln!(out);
}

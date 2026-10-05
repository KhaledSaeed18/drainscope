//! `validate`: the accuracy harness (PLAN.md task 1.22, ADR 0001).
//!
//! Runs known CPU loads in their own transient scopes while sampling RAPL (through the
//! sampler, as the daemon does), battery power and the scopes' CPU time, then reports per
//! phase: the increase over idle at the battery and in RAPL, the conversion factor
//! `k = Δbattery / Δ(package + dram)`, a linear fit of package power against load, and, if
//! the daemon is running, how much of each phase's active energy it gave to the load's scope.
//!
//! With `--activity`, the loads are instead timers waking the CPU at known rates and downloads
//! at known rates, measured through the eBPF probe as well (ADR 0006): what a wakeup and a
//! megabyte cost, and how much of it model v1 charges to the app causing it.

// Sample counts, seconds and microjoules are far below 2^52.
#![allow(clippy::cast_precision_loss)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use drainscope_dbus::monitor::{Monitor1Proxy, UsageRow};
use drainscope_dbus::probe::{Probe1Proxy, ProbeError};
use drainscope_dbus::sampler::{Sampler1Proxy, SamplerError};

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// Battery power lags load changes by 6–8 s (ADR 0001): skip the start of each phase.
const SETTLE: f64 = 8.0;
const REST: Duration = Duration::from_secs(20);
/// Shorter phases leave too few settled samples to compute a rate.
const MIN_PHASE_SECS: u64 = 20;
/// Below this RAPL increase, k is dominated by measurement noise.
const MIN_K_RAPL_W: f64 = 1.0;
#[derive(Debug)]
pub struct Options {
    pub idle_secs: u64,
    pub phase_secs: u64,
    /// Run on AC too (battery figures are then unavailable); for checking the harness.
    pub allow_ac: bool,
    /// Measure wakeups and network traffic instead of CPU load.
    pub activity: bool,
}

/// One load phase, run in its own transient scope.
#[derive(Debug, Clone, Copy)]
enum Workload {
    /// `stress-ng --cpu <cpus> --cpu-load <load_percent>`.
    Cpu { cpus: u32, load_percent: u32 },
    /// `stress-ng --timer 1 --timer-freq <hz>`: wakes a CPU `hz` times a second for almost no
    /// work.
    Timer { hz: u32 },
    /// A download limited to `kbytes_per_second` kB/s.
    Download { kbytes_per_second: u32 },
}

const CPU_LOADS: [Workload; 4] = [
    Workload::Cpu {
        cpus: 1,
        load_percent: 100,
    },
    Workload::Cpu {
        cpus: 2,
        load_percent: 100,
    },
    Workload::Cpu {
        cpus: 4,
        load_percent: 100,
    },
    Workload::Cpu {
        cpus: 1,
        load_percent: 50,
    },
];

/// Download rates stay below what a home connection reliably delivers from the test server
/// (about 0.75 MB/s here), so each phase reaches its rate and the points spread out.
const ACTIVITY_LOADS: [Workload; 6] = [
    Workload::Timer { hz: 250 },
    Workload::Timer { hz: 1000 },
    Workload::Timer { hz: 4000 },
    Workload::Download {
        kbytes_per_second: 250,
    },
    Workload::Download {
        kbytes_per_second: 500,
    },
    Workload::Download {
        kbytes_per_second: 750,
    },
];

/// Serves downloads without authentication (Cloudflare's speed test), up to just under 100 MB
/// per request; phases repeat requests of this size.
const DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=50000000";
/// Name of the rest phases between loads, which serve as local baselines.
const REST_PHASE: &str = "rest";

impl Workload {
    /// Stable names, so repeated runs add up under the same consumers.
    fn unit(self) -> String {
        match self {
            Self::Cpu { cpus, load_percent } => {
                format!("drainscope-validate-cpu{cpus}x{load_percent}")
            }
            Self::Timer { hz } => format!("drainscope-validate-timer{hz}"),
            Self::Download { kbytes_per_second } => {
                format!("drainscope-validate-net{kbytes_per_second}k")
            }
        }
    }

    fn label(self) -> String {
        match self {
            Self::Cpu { cpus, load_percent } => format!("{cpus} CPU × {load_percent}%"),
            Self::Timer { hz } => format!("Timer {hz} Hz"),
            Self::Download { kbytes_per_second } => {
                format!("Download {:.1} MB/s", f64::from(kbytes_per_second) / 1000.0)
            }
        }
    }

    fn command(self, secs: u64) -> Vec<String> {
        let timeout = format!("{secs}s");
        match self {
            Self::Cpu { cpus, load_percent } => [
                "stress-ng",
                "--cpu",
                &cpus.to_string(),
                "--cpu-load",
                &load_percent.to_string(),
                "--timeout",
                &timeout,
                "--quiet",
            ]
            .map(str::to_owned)
            .to_vec(),
            Self::Timer { hz } => [
                "stress-ng",
                "--timer",
                "1",
                "--timer-freq",
                &hz.to_string(),
                "--timeout",
                &timeout,
                "--quiet",
            ]
            .map(str::to_owned)
            .to_vec(),
            // Requests are capped below 100 MB, so repeat them until the phase ends.
            Self::Download { kbytes_per_second } => vec![
                "timeout".into(),
                timeout,
                "sh".into(),
                "-c".into(),
                format!(
                    "while curl --silent --fail --output /dev/null --limit-rate {kbytes_per_second}k \
                     '{DOWNLOAD_URL}'; do :; done"
                ),
            ],
        }
    }
}

#[derive(Debug, Clone)]
struct Sample {
    /// Seconds since the run started.
    t: f64,
    generation: u64,
    rapl_uj: BTreeMap<String, u64>,
    /// Summed discharge power; `None` when not discharging.
    battery_w: Option<f64>,
    /// Display backlight, to rule out dimming between phases (the display is outside RAPL).
    backlight: Option<u64>,
    root_usec: u64,
    /// `usage_usec` per transient scope that exists at this moment.
    scope_usec: BTreeMap<String, u64>,
    /// Cumulative idle exits and (received, sent) bytes per transient scope, from the probe
    /// (`--activity` only), with the probe's generation.
    probe: Option<ProbeSample>,
}

#[derive(Debug, Clone, Default)]
struct ProbeSample {
    generation: u64,
    wakeups: BTreeMap<String, u64>,
    bytes: BTreeMap<String, (u64, u64)>,
}

#[derive(Debug, Clone)]
struct Phase {
    name: String,
    /// The load's transient unit (`None` for idle).
    unit: Option<String>,
    start: f64,
    end: f64,
    wall_start: i64,
    wall_end: i64,
}

fn run_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// One system-bus and one session-bus connection for the whole run: a new connection per
/// sample would make the sampler ask polkit every time.
struct Buses {
    runtime: tokio::runtime::Runtime,
    sampler: Sampler1Proxy<'static>,
    /// `None` if the daemon isn't running.
    monitor: Option<Monitor1Proxy<'static>>,
    probe: Probe1Proxy<'static>,
}

impl Buses {
    fn connect() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (sampler, monitor, probe) = runtime.block_on(async {
            let system = zbus::Connection::system().await?;
            let sampler = Sampler1Proxy::new(&system).await?;
            let probe = Probe1Proxy::new(&system).await?;
            let monitor = match zbus::Connection::session().await {
                Ok(session) => Monitor1Proxy::new(&session).await.ok(),
                Err(_) => None,
            };
            anyhow::Ok((sampler, monitor, probe))
        })?;
        Ok(Self {
            runtime,
            sampler,
            monitor,
            probe,
        })
    }

    /// `ReadCounters`, retrying briefly when the daemon's own calls share the per-user limit.
    fn read_rapl(&self) -> Result<(u64, BTreeMap<String, u64>)> {
        for _ in 0..5 {
            match self.runtime.block_on(self.sampler.read_counters()) {
                Ok((_, generation, counters)) => {
                    return Ok((generation, counters.into_iter().collect()));
                }
                Err(SamplerError::RateLimited(_)) => sleep(Duration::from_millis(300)),
                Err(err) => bail!("reading RAPL from the sampler: {err}"),
            }
        }
        bail!("the sampler kept rate-limiting; is something else polling it every second?")
    }

    /// Wakeups and traffic of the transient scopes in `units`, retrying briefly when the
    /// daemon's own calls (same user) hit the per-user limit.
    fn read_probe(&self, units: &[String]) -> Result<ProbeSample> {
        let scope_of = |path: &str| {
            units
                .iter()
                .find(|unit| path.ends_with(&format!("/{unit}.scope")))
                .cloned()
        };
        let mut sample = ProbeSample::default();
        for _ in 0..5 {
            match self.runtime.block_on(self.probe.read_wakeups()) {
                Ok((_, generation, rows)) => {
                    sample.generation = generation;
                    sample.wakeups = rows
                        .into_iter()
                        .filter_map(|(path, count)| Some((scope_of(&path)?, count)))
                        .collect();
                    break;
                }
                Err(ProbeError::RateLimited(_)) => sleep(Duration::from_millis(300)),
                Err(err) => bail!("reading wakeups from the probe: {err}"),
            }
        }
        for _ in 0..5 {
            match self.runtime.block_on(self.probe.read_network()) {
                Ok((_, _, rows)) => {
                    sample.bytes = rows
                        .into_iter()
                        .filter_map(|(path, rx, tx)| Some((scope_of(&path)?, (rx, tx))))
                        .collect();
                    return Ok(sample);
                }
                Err(ProbeError::RateLimited(_)) => sleep(Duration::from_millis(300)),
                Err(err) => bail!("reading traffic from the probe: {err}"),
            }
        }
        bail!("the probe kept rate-limiting")
    }

    /// The daemon's usage rows for `[since, until)`, if it's running.
    fn usage(&self, since: i64, until: i64) -> Option<Vec<UsageRow>> {
        let monitor = self.monitor.as_ref()?;
        self.runtime
            .block_on(monitor.get_usage(since, until, "consumer", "any"))
            .ok()
    }
}

fn read_battery_w() -> Option<f64> {
    let mut total = None;
    for entry in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let path = entry.path();
        let read = |name: &str| fs::read_to_string(path.join(name)).ok();
        if read("type").as_deref().map(str::trim) != Some("Battery")
            || read("status").as_deref().map(str::trim) != Some("Discharging")
        {
            continue;
        }
        let microwatts: f64 = read("power_now")?.trim().parse().ok()?;
        *total.get_or_insert(0.0) += microwatts / 1e6;
    }
    total
}

/// `actual_brightness` of the first backlight device.
fn read_backlight() -> Option<u64> {
    let device = fs::read_dir("/sys/class/backlight")
        .ok()?
        .flatten()
        .next()?;
    fs::read_to_string(device.path().join("actual_brightness"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn usage_usec(cgroup_dir: &Path) -> Option<u64> {
    fs::read_to_string(cgroup_dir.join("cpu.stat"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("usage_usec "))?
        .trim()
        .parse()
        .ok()
}

fn app_slice() -> String {
    let uid = rustix::process::getuid().as_raw();
    format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/app.slice")
}

fn sample(buses: &Buses, start: Instant, units: &[String], activity: bool) -> Result<Sample> {
    let (generation, rapl_uj) = buses.read_rapl()?;
    let probe = if activity {
        Some(buses.read_probe(units)?)
    } else {
        None
    };
    let scope_usec = units
        .iter()
        .filter_map(|unit| {
            let usec = usage_usec(&Path::new(&app_slice()).join(format!("{unit}.scope")))?;
            Some((unit.clone(), usec))
        })
        .collect();
    Ok(Sample {
        t: start.elapsed().as_secs_f64(),
        generation,
        rapl_uj,
        battery_w: read_battery_w(),
        backlight: read_backlight(),
        root_usec: usage_usec(Path::new("/sys/fs/cgroup")).context("root cpu.stat")?,
        scope_usec,
        probe,
    })
}

/// What every sample of a run reads.
struct Recorder<'a> {
    buses: &'a Buses,
    start: Instant,
    units: &'a [String],
    activity: bool,
}

impl Recorder<'_> {
    fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Samples every `SAMPLE_EVERY` for `length`, or until `child` exits.
    fn record(
        &self,
        length: Duration,
        samples: &mut Vec<Sample>,
        mut child: Option<&mut Child>,
    ) -> Result<()> {
        let until = Instant::now() + length;
        while Instant::now() < until {
            // Check before sampling, so a load phase's last sample is taken while it still runs
            // (its scope, and so its CPU counter, disappears when it exits).
            if let Some(child) = child.as_deref_mut()
                && child.try_wait()?.is_some()
            {
                break;
            }
            samples.push(sample(self.buses, self.start, self.units, self.activity)?);
            sleep(SAMPLE_EVERY);
        }
        Ok(())
    }
}

/// Refuses runs that can't produce meaningful numbers.
fn preflight(options: &Options) -> Result<()> {
    ensure!(
        !rustix::process::geteuid().is_root(),
        "run as your normal user: polkit only grants RAPL access to the active session"
    );
    ensure!(
        options.allow_ac || read_battery_w().is_some(),
        "unplug the charger first (or pass --allow-ac to check the harness without battery data)"
    );
    ensure!(
        options.phase_secs >= MIN_PHASE_SECS && options.idle_secs >= MIN_PHASE_SECS,
        "phases must last at least {MIN_PHASE_SECS} s: each skips {SETTLE:.0} s of battery lag and \
         needs several samples ({} s apart) after that",
        SAMPLE_EVERY.as_secs()
    );
    run_output("stress-ng", &["--version"]).context("stress-ng is required")?;
    if options.activity {
        run_output("curl", &["--version"]).context("curl is required")?;
    }
    Ok(())
}

pub fn run(repo_root: &Path, options: &Options) -> Result<()> {
    preflight(options)?;
    let buses = Buses::connect().context("connecting to D-Bus")?;
    let workloads: &[Workload] = if options.activity {
        &ACTIVITY_LOADS
    } else {
        &CPU_LOADS
    };
    let run_id = unix_now();
    let units: Vec<String> = workloads.iter().map(|w| w.unit()).collect();
    let recorder = Recorder {
        buses: &buses,
        start: Instant::now(),
        units: &units,
        activity: options.activity,
    };
    let mut samples = Vec::new();
    let mut phases = Vec::new();

    eprintln!(
        "idle for {} s: please don't use the machine",
        options.idle_secs
    );
    let (t0, w0) = (recorder.elapsed(), unix_now());
    recorder.record(Duration::from_secs(options.idle_secs), &mut samples, None)?;
    phases.push(Phase {
        name: "idle".into(),
        unit: None,
        start: t0,
        end: recorder.elapsed(),
        wall_start: w0,
        wall_end: unix_now(),
    });

    for (workload, unit) in workloads.iter().zip(&units) {
        eprintln!("{} for {} s", workload.label(), options.phase_secs);
        let mut child = Command::new("systemd-run")
            .args([
                "--user",
                "--scope",
                "--quiet",
                "--unit",
                unit.as_str(),
                "--",
            ])
            .args(workload.command(options.phase_secs))
            .stdout(Stdio::null())
            .spawn()
            .context("starting the load in a transient scope")?;
        let (t0, w0) = (recorder.elapsed(), unix_now());
        recorder.record(
            Duration::from_secs(options.phase_secs + 2),
            &mut samples,
            Some(&mut child),
        )?;
        child.wait()?;
        phases.push(Phase {
            name: workload.label(),
            unit: Some(unit.clone()),
            start: t0,
            end: recorder.elapsed(),
            wall_start: w0,
            wall_end: unix_now(),
        });
        let (t0, w0) = (recorder.elapsed(), unix_now());
        recorder.record(REST, &mut samples, None)?;
        phases.push(Phase {
            name: REST_PHASE.into(),
            unit: None,
            start: t0,
            end: recorder.elapsed(),
            wall_start: w0,
            wall_end: unix_now(),
        });
    }

    let raw = write_raw(repo_root, run_id, &samples, &phases)?;
    let (report, name) = if options.activity {
        (
            activity_report(&buses, &phases, &samples, &raw)?,
            "validation-activity.md",
        )
    } else {
        (report(&buses, &phases, &samples, &raw)?, "validation.md")
    };
    let path = repo_root.join("docs").join(name);
    fs::write(&path, &report).with_context(|| format!("writing {}", path.display()))?;
    print!("{report}");
    eprintln!("\nwrote {}", path.display());
    Ok(())
}

/// Writes the samples, and the phase boundaries next to them, for later re-analysis.
fn write_raw(
    repo_root: &Path,
    run_id: i64,
    samples: &[Sample],
    phases: &[Phase],
) -> Result<String> {
    let dir = repo_root.join("testdata/local");
    fs::create_dir_all(&dir)?;
    let name = format!("validation-{run_id}.csv");
    let mut phase_csv = String::from("name,unit,start,end,wall_start,wall_end\n");
    for p in phases {
        let _ = writeln!(
            phase_csv,
            "{},{},{:.3},{:.3},{},{}",
            p.name,
            p.unit.as_deref().unwrap_or(""),
            p.start,
            p.end,
            p.wall_start,
            p.wall_end
        );
    }
    fs::write(
        dir.join(format!("validation-{run_id}.phases.csv")),
        phase_csv,
    )?;
    let mut csv = String::from(
        "t,generation,battery_w,backlight,root_usec,rapl_uj,scope_usec,probe_generation,\
         scope_wakeups,scope_bytes\n",
    );
    for s in samples {
        let rapl: Vec<String> = s.rapl_uj.iter().map(|(d, v)| format!("{d}={v}")).collect();
        let scopes: Vec<String> = s
            .scope_usec
            .iter()
            .map(|(u, v)| format!("{u}={v}"))
            .collect();
        let probe = s.probe.clone().unwrap_or_default();
        let wakeups: Vec<String> = probe
            .wakeups
            .iter()
            .map(|(u, v)| format!("{u}={v}"))
            .collect();
        let bytes: Vec<String> = probe
            .bytes
            .iter()
            .map(|(u, (rx, tx))| format!("{u}={rx}/{tx}"))
            .collect();
        let _ = writeln!(
            csv,
            "{:.3},{},{},{},{},{},{},{},{},{}",
            s.t,
            s.generation,
            s.battery_w.map_or(String::new(), |w| format!("{w:.3}")),
            s.backlight.map_or(String::new(), |b| b.to_string()),
            s.root_usec,
            rapl.join(";"),
            scopes.join(";"),
            s.probe
                .as_ref()
                .map_or(String::new(), |p| p.generation.to_string()),
            wakeups.join(";"),
            bytes.join(";")
        );
    }
    fs::write(dir.join(&name), csv)?;
    Ok(format!("testdata/local/{name}"))
}

/// Averages over one phase, after the battery has settled.
#[derive(Debug, Clone, Copy, Default)]
struct PhaseStats {
    battery_w: Option<f64>,
    package_w: f64,
    dram_w: f64,
    busy_cpus: f64,
    scope_cpus: f64,
    /// The scope's idle exits per second (`--activity`).
    scope_wakeups: f64,
    /// The scope's received and sent bytes per second (`--activity`).
    scope_rx: f64,
    scope_tx: f64,
}

/// Per-second rate of a per-scope probe counter over the samples that saw the scope (it may
/// appear late or vanish early), within the first probe generation seen.
fn scope_rate(window: &[&Sample], value: impl Fn(&ProbeSample) -> Option<u64>) -> f64 {
    let seen: Vec<(f64, u64, u64)> = window
        .iter()
        .filter_map(|s| {
            let probe = s.probe.as_ref()?;
            Some((s.t, probe.generation, value(probe)?))
        })
        .collect();
    let Some(&(t0, generation, v0)) = seen.first() else {
        return 0.0;
    };
    match seen.iter().rev().find(|(_, g, _)| *g == generation) {
        Some(&(t1, _, v1)) if t1 > t0 => v1.saturating_sub(v0) as f64 / (t1 - t0),
        _ => 0.0,
    }
}

fn stats(phase: &Phase, samples: &[Sample]) -> Option<PhaseStats> {
    let window: Vec<&Sample> = samples
        .iter()
        .filter(|s| {
            s.t >= phase.start + SETTLE.min((phase.end - phase.start) / 2.0) && s.t <= phase.end
        })
        .collect();
    let (first, last) = (window.first()?, window.last()?);
    let dt = last.t - first.t;
    if dt <= 0.0 || first.generation != last.generation {
        return None;
    }
    let rate = |domain: &str| {
        let a = first.rapl_uj.get(domain).copied().unwrap_or(0);
        let b = last.rapl_uj.get(domain).copied().unwrap_or(0);
        b.saturating_sub(a) as f64 / 1e6 / dt
    };
    let cpus = |a: u64, b: u64| b.saturating_sub(a) as f64 / 1e6 / dt;
    // Only samples that saw the scope count: it may appear late or vanish early.
    let scope_cpus = phase.unit.as_ref().map_or(0.0, |unit| {
        let seen: Vec<(f64, u64)> = window
            .iter()
            .filter_map(|s| Some((s.t, *s.scope_usec.get(unit)?)))
            .collect();
        match (seen.first(), seen.last()) {
            (Some(&(t0, u0)), Some(&(t1, u1))) if t1 > t0 => {
                u1.saturating_sub(u0) as f64 / 1e6 / (t1 - t0)
            }
            _ => 0.0,
        }
    });
    let batteries: Vec<f64> = window.iter().filter_map(|s| s.battery_w).collect();
    Some(PhaseStats {
        battery_w: (batteries.len() == window.len())
            .then(|| batteries.iter().sum::<f64>() / batteries.len() as f64),
        package_w: rate("package"),
        dram_w: rate("dram"),
        busy_cpus: cpus(first.root_usec, last.root_usec),
        scope_cpus,
        scope_wakeups: phase.unit.as_ref().map_or(0.0, |unit| {
            scope_rate(&window, |p| p.wakeups.get(unit).copied())
        }),
        scope_rx: phase.unit.as_ref().map_or(0.0, |unit| {
            scope_rate(&window, |p| p.bytes.get(unit).map(|b| b.0))
        }),
        scope_tx: phase.unit.as_ref().map_or(0.0, |unit| {
            scope_rate(&window, |p| p.bytes.get(unit).map(|b| b.1))
        }),
    })
}

/// Least-squares fit y = a·x + b; returns (a, b, R²).
fn fit(points: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let syy: f64 = points.iter().map(|p| (p.1 - mean_y).powi(2)).sum();
    if sxx <= 0.0 || syy <= 0.0 {
        return None;
    }
    let a = sxy / sxx;
    Some((a, mean_y - a * mean_x, sxy * sxy / (sxx * syy)))
}

/// The daemon's attribution for a phase: the load scope's share of active energy. `None` if
/// the daemon isn't running.
fn daemon_share(buses: &Buses, phase: &Phase) -> Option<f64> {
    let unit = phase.unit.as_ref()?;
    let rows = buses.usage(phase.wall_start, phase.wall_end + 1)?;
    let key = format!("user-unit:{unit}.scope");
    let mut scope = 0.0;
    let mut active = 0.0;
    for (name, _, joules, ..) in rows {
        if !matches!(name.as_str(), "idle" | "devices" | "platform") {
            active += joules;
        }
        if name == key {
            scope = joules;
        }
    }
    (active > 0.0).then(|| scope / active * 100.0)
}

fn report(buses: &Buses, phases: &[Phase], samples: &[Sample], raw: &str) -> Result<String> {
    let idle = phases
        .first()
        .and_then(|p| stats(p, samples))
        .context("no idle measurement")?;
    let date = run_output("date", &["-u", "+%Y-%m-%d %H:%M UTC"]).unwrap_or_default();
    let machine = fs::read_to_string("/sys/class/dmi/id/product_version").unwrap_or_default();
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("model name"))
        .map(|l| l.trim_start_matches([' ', '\t', ':']).to_owned())
        .unwrap_or_default();

    let mut out = String::new();
    let _ = writeln!(out, "# Validation\n");
    let _ = writeln!(
        out,
        "Generated by `cargo xtask validate` on {} ({}), {}.\nRaw samples: `{raw}` (not committed).\n",
        machine.trim(),
        cpu,
        date.trim()
    );
    let _ = writeln!(
        out,
        "Each phase runs `stress-ng` in its own transient scope. Averages skip the first {SETTLE:.0} s \
         of each phase (battery lag). Δ values are relative to idle. k = Δbattery / Δ(package + dram) \
         is ADR 0001's conversion-overhead factor. \"Daemon share\" is the fraction of the phase's \
         active (non-idle, non-devices) energy the running daemon attributed to the load's scope.\n"
    );
    let watts = |w: Option<f64>| w.map_or("n/a".to_owned(), |w| format!("{w:.2}"));
    let _ = writeln!(
        out,
        "| Phase | Load CPUs | Busy CPUs | Battery W | Package+DRAM W | ΔBattery W | ΔRAPL W | k | Daemon share |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
    let mut fit_points = Vec::new();
    let mut ks = Vec::new();
    for phase in phases.iter().filter(|p| p.name != REST_PHASE) {
        let Some(s) = stats(phase, samples) else {
            let _ = writeln!(out, "| {} | – | – | – | – | – | – | – | – |", phase.name);
            continue;
        };
        let rapl = s.package_w + s.dram_w;
        let d_rapl = rapl - (idle.package_w + idle.dram_w);
        let d_battery = s.battery_w.zip(idle.battery_w).map(|(b, i)| b - i);
        let k = d_battery
            .filter(|_| d_rapl >= MIN_K_RAPL_W)
            .map(|d| d / d_rapl);
        if phase.unit.is_some() {
            fit_points.push((s.scope_cpus, s.package_w - idle.package_w));
            ks.extend(k);
        }
        let share = daemon_share(buses, phase).map_or("n/a".to_owned(), |p| format!("{p:.0}%"));
        let _ = writeln!(
            out,
            "| {} | {:.2} | {:.2} | {} | {:.2} | {} | {:.2} | {} | {} |",
            phase.name,
            s.scope_cpus,
            s.busy_cpus,
            watts(s.battery_w),
            rapl,
            watts(d_battery),
            d_rapl,
            k.map_or("n/a".to_owned(), |k| format!("{k:.2}")),
            share
        );
    }
    let _ = writeln!(out);
    conclusions(&mut out, &fit_points, ks, samples);
    Ok(out)
}

/// Energy the running daemon attributed to the phase's scope, as average watts over the phase.
fn daemon_watts(buses: &Buses, phase: &Phase) -> Option<f64> {
    let unit = phase.unit.as_ref()?;
    let rows = buses.usage(phase.wall_start, phase.wall_end + 1)?;
    let key = format!("user-unit:{unit}.scope");
    let joules: f64 = rows
        .iter()
        .filter(|(name, ..)| *name == key)
        .map(|(_, _, joules, ..)| joules)
        .sum();
    let seconds = (phase.wall_end - phase.wall_start).max(1) as f64;
    Some(joules / seconds)
}

/// The average of the phases just before and after `phases[index]` (the rests around it, or
/// the idle phase), so slow drift during the run cancels out.
fn local_baseline(phases: &[Phase], index: usize, samples: &[Sample]) -> Option<PhaseStats> {
    let neighbours: Vec<PhaseStats> = [index.checked_sub(1), Some(index + 1)]
        .into_iter()
        .flatten()
        .filter_map(|i| phases.get(i))
        .filter(|p| p.unit.is_none())
        .filter_map(|p| stats(p, samples))
        .collect();
    if neighbours.is_empty() {
        return None;
    }
    let n = neighbours.len() as f64;
    let mean = |f: fn(&PhaseStats) -> f64| neighbours.iter().map(f).sum::<f64>() / n;
    let batteries: Option<Vec<f64>> = neighbours.iter().map(|s| s.battery_w).collect();
    Some(PhaseStats {
        battery_w: batteries.map(|b| b.iter().sum::<f64>() / n),
        package_w: mean(|s| s.package_w),
        dram_w: mean(|s| s.dram_w),
        busy_cpus: mean(|s| s.busy_cpus),
        ..PhaseStats::default()
    })
}

fn activity_report(
    buses: &Buses,
    phases: &[Phase],
    samples: &[Sample],
    raw: &str,
) -> Result<String> {
    let idle = phases
        .first()
        .and_then(|p| stats(p, samples))
        .context("no idle measurement")?;
    let date = run_output("date", &["-u", "+%Y-%m-%d %H:%M UTC"]).unwrap_or_default();
    let mut out = String::new();
    let _ = writeln!(out, "# Activity validation (ADR 0006)\n");
    let _ = writeln!(
        out,
        "Generated by `cargo xtask validate --activity` on {}.\nRaw samples: `{raw}` (not committed).\n",
        date.trim()
    );
    let _ = writeln!(
        out,
        "Timer phases wake a CPU at a fixed rate doing almost nothing (`stress-ng --timer`); \
         download phases fetch at a capped rate (`curl --limit-rate`). Wakeups and bytes are the \
         eBPF probe's counts for the load's scope. Averages skip the first {SETTLE:.0} s of each \
         phase; Δ values are relative to the rests just before and after the phase, which cancels \
         slow drift in the battery's readings. \"Outside RAPL\" is ΔBattery − ΔRAPL: the radio, \
         chipset and other devices. \"Charged\" is what the running daemon (model v1) attributed to \
         the load's scope, as average watts.\n"
    );
    let _ = writeln!(
        out,
        "| Phase | CPUs | Wakeups/s | In MB/s | Out MB/s | Battery W | ΔBattery W | ΔRAPL W | Outside RAPL W | Charged W |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
    let watts = |w: Option<f64>| w.map_or("n/a".to_owned(), |w| format!("{w:.2}"));
    let mut wake_points = Vec::new();
    let mut net_points = Vec::new();
    let mut charge_ratios = Vec::new();
    for (index, phase) in phases.iter().enumerate() {
        if phase.name == REST_PHASE {
            continue;
        }
        let Some(s) = stats(phase, samples) else {
            let _ = writeln!(
                out,
                "| {} | – | – | – | – | – | – | – | – | – |",
                phase.name
            );
            continue;
        };
        let baseline = local_baseline(phases, index, samples).unwrap_or(idle);
        let d_rapl = (s.package_w + s.dram_w) - (baseline.package_w + baseline.dram_w);
        let d_battery = s.battery_w.zip(baseline.battery_w).map(|(b, i)| b - i);
        let outside = d_battery.map(|d| d - d_rapl);
        let megabytes = (s.scope_rx + s.scope_tx) / 1e6;
        let charged = daemon_watts(buses, phase);
        if phase.name.starts_with("Timer") {
            wake_points.push((s.scope_wakeups, d_rapl, d_battery));
        } else if phase.name.starts_with("Download") {
            net_points.push((megabytes, d_rapl, outside));
            // Below this the ratio is noise.
            if let Some(charged) = charged.filter(|_| d_rapl >= 0.05) {
                charge_ratios.push(charged / d_rapl);
            }
        }
        let _ = writeln!(
            out,
            "| {} | {:.3} | {:.0} | {:.2} | {:.3} | {} | {} | {:.2} | {} | {} |",
            phase.name,
            s.scope_cpus,
            s.scope_wakeups,
            s.scope_rx / 1e6,
            s.scope_tx / 1e6,
            watts(s.battery_w),
            watts(d_battery),
            d_rapl,
            watts(outside),
            watts(charged),
        );
    }
    let _ = writeln!(out);
    activity_conclusions(&mut out, &wake_points, &net_points);
    if !charge_ratios.is_empty() {
        charge_ratios.sort_by(f64::total_cmp);
        let _ = writeln!(
            out,
            "- Downloads were charged {:.0}% (median; {:.0}–{:.0}%) of their RAPL increase by the \
             running daemon's model. Model v1 charges about 5%; v2 should approach the 79–90% \
             CPU loads get.",
            charge_ratios[charge_ratios.len() / 2] * 100.0,
            charge_ratios[0] * 100.0,
            charge_ratios[charge_ratios.len() - 1] * 100.0
        );
    }
    conclusions_backlight(&mut out, samples);
    Ok(out)
}

/// Costs fitted from the timer phases (wakeups/s, Δ RAPL, Δ battery) and the download phases
/// (MB/s, Δ RAPL, power outside RAPL).
fn activity_conclusions(
    out: &mut String,
    wake_points: &[(f64, f64, Option<f64>)],
    net_points: &[(f64, f64, Option<f64>)],
) {
    let rapl_fit: Vec<(f64, f64)> = wake_points.iter().map(|&(w, r, _)| (w, r)).collect();
    if let Some((a, b, r2)) = fit(&rapl_fit) {
        let _ = writeln!(
            out,
            "- RAPL power vs. wakeups: {:.1} mW per 100 wakeups/s + {b:.2} W (R² = {r2:.3}).",
            a * 100.0 * 1000.0
        );
    }
    let battery_fit: Vec<(f64, f64)> = wake_points
        .iter()
        .filter_map(|&(w, _, d)| Some((w, d?)))
        .collect();
    if let Some((a, b, r2)) = fit(&battery_fit) {
        let _ = writeln!(
            out,
            "- Battery power vs. wakeups: {:.1} mW per 100 wakeups/s + {b:.2} W (R² = {r2:.3}).",
            a * 100.0 * 1000.0
        );
    }
    let net_rapl_fit: Vec<(f64, f64)> = net_points.iter().map(|&(m, r, _)| (m, r)).collect();
    if let Some((a, b, r2)) = fit(&net_rapl_fit) {
        let _ = writeln!(
            out,
            "- RAPL power vs. traffic (the network stack's CPU work): {:.0} mW per MB/s + {b:.2} W \
             (R² = {r2:.3}).",
            a * 1000.0
        );
    }
    let outside_fit: Vec<(f64, f64)> = net_points
        .iter()
        .filter_map(|&(m, _, o)| Some((m, o?)))
        .collect();
    if let Some((a, b, r2)) = fit(&outside_fit) {
        let _ = writeln!(
            out,
            "- Power outside RAPL vs. traffic: {:.0} mW per MB/s + {b:.2} W (R² = {r2:.3}).",
            a * 1000.0
        );
    }
}

/// The fit, the conversion factor and the backlight check, below the table.
fn conclusions(out: &mut String, fit_points: &[(f64, f64)], mut ks: Vec<f64>, samples: &[Sample]) {
    if let Some((a, b, r2)) = fit(fit_points) {
        let _ = writeln!(
            out,
            "- Package power vs. load: ΔP ≈ {a:.2} W per busy CPU + {b:.2} W (R² = {r2:.3})."
        );
    }
    if ks.is_empty() {
        let _ = writeln!(
            out,
            "- k unavailable: no battery data (run unplugged) or no phase above {MIN_K_RAPL_W:.0} W."
        );
    } else {
        ks.sort_by(f64::total_cmp);
        let median = ks[ks.len() / 2];
        let _ = writeln!(
            out,
            "- k (phases with ΔRAPL ≥ {MIN_K_RAPL_W:.0} W): median {median:.2}, range {:.2}–{:.2}. \
             k < 1 means the battery reported a smaller increase than RAPL, which is physically \
             impossible; one of the two sensors is then miscalibrated.",
            ks[0],
            ks[ks.len() - 1]
        );
    }
    conclusions_backlight(out, samples);
}

/// Whether the display's backlight stayed constant (the display is outside RAPL).
fn conclusions_backlight(out: &mut String, samples: &[Sample]) {
    let levels: Vec<u64> = samples.iter().filter_map(|s| s.backlight).collect();
    match (levels.iter().min(), levels.iter().max()) {
        (Some(low), Some(high)) if low == high => {
            let _ = writeln!(out, "- Backlight constant at {low} throughout: no dimming.");
        }
        (Some(low), Some(high)) => {
            let _ = writeln!(
                out,
                "- **Backlight changed during the run ({low}–{high})**: battery deltas include display changes."
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_a_line() {
        let (a, b, r2) = fit(&[(1.0, 3.0), (2.0, 5.0), (4.0, 9.0)]).unwrap();
        assert!((a - 2.0).abs() < 1e-9 && (b - 1.0).abs() < 1e-9 && (r2 - 1.0).abs() < 1e-9);
        assert_eq!(fit(&[(1.0, 1.0)]), None);
    }

    fn sample(t: f64, package_uj: u64, battery_w: Option<f64>, scope_usec: u64) -> Sample {
        Sample {
            t,
            generation: 1,
            rapl_uj: BTreeMap::from([("package".to_owned(), package_uj)]),
            battery_w,
            backlight: Some(100),
            root_usec: scope_usec * 2,
            scope_usec: BTreeMap::from([("u".to_owned(), scope_usec)]),
            probe: None,
        }
    }

    #[test]
    fn scope_rates_stay_within_one_probe_generation() {
        let with_probe = |t: f64, generation: u64, wakeups: Option<u64>| {
            let mut s = sample(t, 0, None, 0);
            s.probe = Some(ProbeSample {
                generation,
                wakeups: wakeups.map(|w| ("u".to_owned(), w)).into_iter().collect(),
                bytes: BTreeMap::new(),
            });
            s
        };
        let samples = [
            with_probe(10.0, 1, None),
            with_probe(12.0, 1, Some(100)),
            with_probe(14.0, 1, Some(300)),
            with_probe(16.0, 1, Some(500)),
            // The probe restarted: its counts start over and must not be compared.
            with_probe(18.0, 2, Some(20)),
        ];
        let window: Vec<&Sample> = samples.iter().collect();
        let rate = scope_rate(&window, |p| p.wakeups.get("u").copied());
        assert!((rate - 100.0).abs() < 1e-9, "{rate}");
        assert!(scope_rate(&window[..2], |p| p.wakeups.get("u").copied()).abs() < 1e-12);
    }

    #[test]
    fn phase_stats_skip_the_settling_time() {
        let phase = Phase {
            name: "x".into(),
            unit: Some("u".into()),
            start: 0.0,
            end: 20.0,
            wall_start: 0,
            wall_end: 20,
        };
        let samples = [
            sample(2.0, 0, Some(99.0), 0), // settling: ignored
            sample(10.0, 10_000_000, Some(8.0), 1_000_000),
            sample(20.0, 40_000_000, Some(10.0), 11_000_000),
        ];
        let s = stats(&phase, &samples).unwrap();
        assert!((s.package_w - 3.0).abs() < 1e-9);
        assert!((s.scope_cpus - 1.0).abs() < 1e-9);
        assert!((s.busy_cpus - 2.0).abs() < 1e-9);
        assert!((s.battery_w.unwrap() - 9.0).abs() < 1e-9);
    }
}

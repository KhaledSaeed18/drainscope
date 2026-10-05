//! `validate`: the accuracy harness (PLAN.md task 1.22, ADR 0001).
//!
//! Runs known CPU loads in their own transient scopes while sampling RAPL (through the
//! sampler, as the daemon does), battery power and the scopes' CPU time, then reports per
//! phase: the increase over idle at the battery and in RAPL, the conversion factor
//! `k = Δbattery / Δ(package + dram)`, a linear fit of package power against load, and, if
//! the daemon is running, how much of each phase's active energy it gave to the load's scope.

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

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// Battery power lags load changes by 6–8 s (ADR 0001): skip the start of each phase.
const SETTLE: f64 = 8.0;
const REST: Duration = Duration::from_secs(20);
const SAMPLER: [&str; 3] = [
    "io.github.khaledsaeed18.Drainscope.Sampler",
    "/io/github/khaledsaeed18/Drainscope/Sampler",
    "io.github.khaledsaeed18.Drainscope.Sampler1",
];
const MONITOR: [&str; 3] = [
    "io.github.khaledsaeed18.Drainscope.Monitor",
    "/io/github/khaledsaeed18/Drainscope/Monitor",
    "io.github.khaledsaeed18.Drainscope.Monitor1",
];

#[derive(Debug)]
pub struct Options {
    pub idle_secs: u64,
    pub phase_secs: u64,
    /// Run on AC too (battery figures are then unavailable); for checking the harness.
    pub allow_ac: bool,
}

/// One load phase: `stress-ng --cpu <cpus> --cpu-load <load>`.
#[derive(Debug, Clone, Copy)]
struct Load {
    cpus: u32,
    load_percent: u32,
}

const LOADS: [Load; 4] = [
    Load {
        cpus: 1,
        load_percent: 100,
    },
    Load {
        cpus: 2,
        load_percent: 100,
    },
    Load {
        cpus: 4,
        load_percent: 100,
    },
    Load {
        cpus: 1,
        load_percent: 50,
    },
];

#[derive(Debug, Clone)]
struct Sample {
    /// Seconds since the run started.
    t: f64,
    generation: u64,
    rapl_uj: BTreeMap<String, u64>,
    /// Summed discharge power; `None` when not discharging.
    battery_w: Option<f64>,
    root_usec: u64,
    /// `usage_usec` per transient scope that exists at this moment.
    scope_usec: BTreeMap<String, u64>,
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

/// `ReadCounters` through busctl's JSON output, retrying briefly when the daemon's own calls
/// share the per-user rate limit.
fn read_rapl() -> Result<(u64, BTreeMap<String, u64>)> {
    for _ in 0..5 {
        let mut args = vec!["--system", "--json=short", "call"];
        args.extend(SAMPLER);
        args.push("ReadCounters");
        match run_output("busctl", &args) {
            Ok(json) => {
                let value: serde_json::Value = serde_json::from_str(&json)?;
                let data = &value["data"];
                let generation = data[1].as_u64().context("generation")?;
                let counters = data[2]
                    .as_array()
                    .context("counters")?
                    .iter()
                    .filter_map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_u64()?)))
                    .collect();
                return Ok((generation, counters));
            }
            Err(err) if err.to_string().contains("retry in") => sleep(Duration::from_millis(300)),
            Err(err) => return Err(err),
        }
    }
    bail!("the sampler kept rate-limiting; is something else polling it every second?")
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

fn sample(start: Instant, units: &[String]) -> Result<Sample> {
    let (generation, rapl_uj) = read_rapl()?;
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
        root_usec: usage_usec(Path::new("/sys/fs/cgroup")).context("root cpu.stat")?,
        scope_usec,
    })
}

/// Samples every `SAMPLE_EVERY` for `length`, or until `child` exits.
fn record(
    start: Instant,
    length: Duration,
    units: &[String],
    samples: &mut Vec<Sample>,
    mut child: Option<&mut Child>,
) -> Result<()> {
    let until = Instant::now() + length;
    while Instant::now() < until {
        samples.push(sample(start, units)?);
        if let Some(child) = child.as_deref_mut()
            && child.try_wait()?.is_some()
        {
            break;
        }
        sleep(SAMPLE_EVERY);
    }
    Ok(())
}

pub fn run(repo_root: &Path, options: &Options) -> Result<()> {
    ensure!(
        !rustix::process::geteuid().is_root(),
        "run as your normal user: polkit only grants RAPL access to the active session"
    );
    ensure!(
        options.allow_ac || read_battery_w().is_some(),
        "unplug the charger first (or pass --allow-ac to check the harness without battery data)"
    );
    run_output("stress-ng", &["--version"]).context("stress-ng is required")?;

    let run_id = unix_now();
    let units: Vec<String> = LOADS
        .iter()
        .map(|l| format!("validate-{run_id}-cpu{}x{}", l.cpus, l.load_percent))
        .collect();
    let start = Instant::now();
    let mut samples = Vec::new();
    let mut phases = Vec::new();

    eprintln!(
        "idle for {} s: please don't use the machine",
        options.idle_secs
    );
    let (t0, w0) = (start.elapsed().as_secs_f64(), unix_now());
    record(
        start,
        Duration::from_secs(options.idle_secs),
        &units,
        &mut samples,
        None,
    )?;
    phases.push(Phase {
        name: "idle".into(),
        unit: None,
        start: t0,
        end: start.elapsed().as_secs_f64(),
        wall_start: w0,
        wall_end: unix_now(),
    });

    for (load, unit) in LOADS.iter().zip(&units) {
        eprintln!(
            "{} CPU(s) at {}% for {} s",
            load.cpus, load.load_percent, options.phase_secs
        );
        let timeout = format!("{}s", options.phase_secs);
        let cpus = load.cpus.to_string();
        let percent = load.load_percent.to_string();
        let mut child = Command::new("systemd-run")
            .args([
                "--user",
                "--scope",
                "--quiet",
                "--unit",
                unit.as_str(),
                "--",
            ])
            .args([
                "stress-ng",
                "--cpu",
                &cpus,
                "--cpu-load",
                &percent,
                "--timeout",
                &timeout,
                "--quiet",
            ])
            .stdout(Stdio::null())
            .spawn()
            .context("starting stress-ng in a transient scope")?;
        let (t0, w0) = (start.elapsed().as_secs_f64(), unix_now());
        record(
            start,
            Duration::from_secs(options.phase_secs + 2),
            &units,
            &mut samples,
            Some(&mut child),
        )?;
        child.wait()?;
        phases.push(Phase {
            name: format!("{} CPU × {}%", load.cpus, load.load_percent),
            unit: Some(unit.clone()),
            start: t0,
            end: start.elapsed().as_secs_f64(),
            wall_start: w0,
            wall_end: unix_now(),
        });
        record(start, REST, &units, &mut samples, None)?;
    }

    let raw = write_raw(repo_root, run_id, &samples)?;
    let report = report(&phases, &samples, &raw)?;
    let path = repo_root.join("docs/validation.md");
    fs::write(&path, &report).with_context(|| format!("writing {}", path.display()))?;
    print!("{report}");
    eprintln!("\nwrote {}", path.display());
    Ok(())
}

fn write_raw(repo_root: &Path, run_id: i64, samples: &[Sample]) -> Result<String> {
    let dir = repo_root.join("testdata/local");
    fs::create_dir_all(&dir)?;
    let name = format!("validation-{run_id}.csv");
    let mut csv = String::from("t,generation,battery_w,root_usec,rapl_uj,scope_usec\n");
    for s in samples {
        let rapl: Vec<String> = s.rapl_uj.iter().map(|(d, v)| format!("{d}={v}")).collect();
        let scopes: Vec<String> = s
            .scope_usec
            .iter()
            .map(|(u, v)| format!("{u}={v}"))
            .collect();
        let _ = writeln!(
            csv,
            "{:.3},{},{},{},{},{}",
            s.t,
            s.generation,
            s.battery_w.map_or(String::new(), |w| format!("{w:.3}")),
            s.root_usec,
            rapl.join(";"),
            scopes.join(";")
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
    let scope_cpus = phase.unit.as_ref().map_or(0.0, |unit| {
        cpus(
            first.scope_usec.get(unit).copied().unwrap_or(0),
            last.scope_usec.get(unit).copied().unwrap_or(0),
        )
    });
    let batteries: Vec<f64> = window.iter().filter_map(|s| s.battery_w).collect();
    Some(PhaseStats {
        battery_w: (batteries.len() == window.len())
            .then(|| batteries.iter().sum::<f64>() / batteries.len() as f64),
        package_w: rate("package"),
        dram_w: rate("dram"),
        busy_cpus: cpus(first.root_usec, last.root_usec),
        scope_cpus,
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
fn daemon_share(phase: &Phase) -> Option<f64> {
    let unit = phase.unit.as_ref()?;
    let (since, until) = (
        phase.wall_start.to_string(),
        (phase.wall_end + 1).to_string(),
    );
    let mut args = vec!["--user", "--json=short", "call"];
    args.extend(MONITOR);
    args.extend(["GetUsage", "xxss", &since, &until, "consumer", "any"]);
    let json: serde_json::Value = serde_json::from_str(&run_output("busctl", &args).ok()?).ok()?;
    let rows = json["data"][0].as_array()?;
    let key = format!("user-unit:{unit}.scope");
    let mut scope = 0.0;
    let mut active = 0.0;
    for row in rows {
        let (name, joules) = (row[0].as_str()?, row[2].as_f64()?);
        if !matches!(name, "idle" | "devices" | "platform") {
            active += joules;
        }
        if name == key {
            scope = joules;
        }
    }
    (active > 0.0).then(|| scope / active * 100.0)
}

fn report(phases: &[Phase], samples: &[Sample], raw: &str) -> Result<String> {
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
    for phase in phases {
        let Some(s) = stats(phase, samples) else {
            let _ = writeln!(out, "| {} | – | – | – | – | – | – | – | – |", phase.name);
            continue;
        };
        let rapl = s.package_w + s.dram_w;
        let d_rapl = rapl - (idle.package_w + idle.dram_w);
        let d_battery = s.battery_w.zip(idle.battery_w).map(|(b, i)| b - i);
        let k = d_battery.filter(|_| d_rapl > 0.05).map(|d| d / d_rapl);
        if phase.unit.is_some() {
            fit_points.push((s.scope_cpus, s.package_w - idle.package_w));
            ks.extend(k);
        }
        let share = daemon_share(phase).map_or("n/a".to_owned(), |p| format!("{p:.0}%"));
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
    if let Some((a, b, r2)) = fit(&fit_points) {
        let _ = writeln!(
            out,
            "- Package power vs. load: ΔP ≈ {a:.2} W per busy CPU + {b:.2} W (R² = {r2:.3})."
        );
    }
    if ks.is_empty() {
        let _ = writeln!(out, "- k unavailable: no battery data (run unplugged).");
    } else {
        let mean = ks.iter().sum::<f64>() / ks.len() as f64;
        let _ = writeln!(
            out,
            "- Mean k = {mean:.2}: each watt measured by RAPL costs {mean:.2} W at the battery."
        );
    }
    Ok(out)
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
            root_usec: scope_usec * 2,
            scope_usec: BTreeMap::from([("u".to_owned(), scope_usec)]),
        }
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

//! `xtask cost`: CPU and memory of drainscope's processes over a period, and the database's
//! size. Reads `/proc` directly; meant for the 1.0 soak test.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// `comm` is truncated to 15 bytes by the kernel.
const COMMS: [&str; 4] = [
    "drainscope-daem",
    "drainscope-samp",
    "drainscope-prob",
    "dbus-broker",
];
/// Clock ticks per second in `/proc/<pid>/stat`: `USER_HZ`, fixed at 100 by the Linux ABI on
/// every architecture we run on (rustix's `param` feature isn't enabled in this workspace).
const USER_HZ: f64 = 100.0;
const DAEMON: &str = "drainscope-daem";

pub struct Options {
    pub secs: u64,
    pub interval_secs: u64,
}

#[derive(Debug, Default)]
struct Proc {
    first_ticks: u64,
    last_ticks: u64,
    first_at: Option<Instant>,
    last_at: Option<Instant>,
    peak_hwm_kb: u64,
    last_rss_kb: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DbSizes {
    db: u64,
    wal: u64,
    shm: u64,
}

/// User plus system CPU time (clock ticks) from a `/proc/<pid>/stat` line. The command name is
/// in parentheses and may itself contain spaces and parentheses, so parse after the last `)`.
fn utime_stime(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // After the comm: state is field 3, so utime (field 14) is the 12th word here.
    let mut words = rest.split_whitespace();
    let utime: u64 = words.nth(11)?.parse().ok()?;
    let stime: u64 = words.next()?.parse().ok()?;
    Some(utime + stime)
}

/// The value of a `Key:   123 kB` line in `/proc/<pid>/status`.
fn kb(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn comm_of(pid_dir: &Path) -> Option<String> {
    Some(
        std::fs::read_to_string(pid_dir.join("comm"))
            .ok()?
            .trim_end()
            .to_owned(),
    )
}

fn sample(procs: &mut BTreeMap<(String, u32), Proc>) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    let now = Instant::now();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();
        let Some(comm) = comm_of(&dir) else { continue };
        if !COMMS.contains(&comm.as_str()) {
            continue;
        }
        let ticks = std::fs::read_to_string(dir.join("stat"))
            .ok()
            .and_then(|stat| utime_stime(&stat));
        let status = std::fs::read_to_string(dir.join("status")).ok();
        let proc = procs.entry((comm, pid)).or_default();
        if let Some(ticks) = ticks {
            if proc.first_at.is_none() {
                proc.first_ticks = ticks;
                proc.first_at = Some(now);
            }
            proc.last_ticks = ticks;
            proc.last_at = Some(now);
        }
        if let Some(status) = status {
            if let Some(hwm) = kb(&status, "VmHWM:") {
                proc.peak_hwm_kb = proc.peak_hwm_kb.max(hwm);
            }
            if let Some(rss) = kb(&status, "VmRSS:") {
                proc.last_rss_kb = rss;
            }
        }
    }
}

fn cpu_percent(proc: &Proc, ticks_per_sec: f64) -> Option<f64> {
    let elapsed = proc.last_at?.duration_since(proc.first_at?).as_secs_f64();
    if elapsed <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)] // tick counts stay far below 2^52
    let cpu_secs = proc.last_ticks.saturating_sub(proc.first_ticks) as f64 / ticks_per_sec;
    Some(cpu_secs / elapsed * 100.0)
}

fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("drainscope");
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".local/state/drainscope")
}

fn db_sizes(dir: &Path) -> DbSizes {
    let size = |name: &str| std::fs::metadata(dir.join(name)).map_or(0, |m| m.len());
    DbSizes {
        db: size("drainscope.db"),
        wal: size("drainscope.db-wal"),
        shm: size("drainscope.db-shm"),
    }
}

#[allow(clippy::cast_precision_loss)] // sizes in display units
fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

#[allow(clippy::cast_precision_loss)] // kB counts in display units
fn kb_to_mb(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

fn report(
    procs: &BTreeMap<(String, u32), Proc>,
    ticks_per_sec: f64,
    elapsed: Duration,
    start: DbSizes,
    end: DbSizes,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nSampled for {:.0} s.\n\n| process | pid | avg CPU (% of one CPU) | peak RSS (MB) | last RSS (MB) |\n|---|---|---|---|---|",
        elapsed.as_secs_f64()
    );
    for ((comm, pid), proc) in procs {
        let cpu = cpu_percent(proc, ticks_per_sec)
            .map_or_else(|| "n/a".to_owned(), |cpu| format!("{cpu:.3}"));
        let _ = writeln!(
            out,
            "| {comm} | {pid} | {cpu} | {:.1} | {:.1} |",
            kb_to_mb(proc.peak_hwm_kb),
            kb_to_mb(proc.last_rss_kb)
        );
    }
    let _ = writeln!(
        out,
        "\n| file | start (MB) | end (MB) | growth (MB) |\n|---|---|---|---|"
    );
    for (name, a, b) in [
        ("drainscope.db", start.db, end.db),
        ("drainscope.db-wal", start.wal, end.wal),
        ("drainscope.db-shm", start.shm, end.shm),
    ] {
        let _ = writeln!(
            out,
            "| {name} | {:.2} | {:.2} | {:+.2} |",
            mb(a),
            mb(b),
            mb(b) - mb(a)
        );
    }
    out
}

/// Samples every `interval_secs` for `secs`, then prints the tables.
///
/// # Errors
/// Fails on a zero interval.
pub fn run(options: &Options) -> Result<()> {
    if options.interval_secs == 0 {
        bail!("--interval must be at least 1 second");
    }
    let ticks_per_sec = USER_HZ;
    let dir = state_dir();
    let db_start = db_sizes(&dir);
    let started = Instant::now();
    let total = Duration::from_secs(options.secs);
    let interval = Duration::from_secs(options.interval_secs);
    let mut procs = BTreeMap::new();
    loop {
        sample(&mut procs);
        let elapsed = started.elapsed();
        let daemon_cpu = procs
            .iter()
            .filter(|((comm, _), _)| comm == DAEMON)
            .filter_map(|(_, proc)| cpu_percent(proc, ticks_per_sec))
            .fold(0.0, f64::max);
        eprintln!(
            "[{:>8.0} s] {DAEMON} CPU so far: {daemon_cpu:.3}%",
            elapsed.as_secs_f64()
        );
        if elapsed >= total {
            break;
        }
        sleep(interval.min(total.saturating_sub(elapsed)));
    }
    let elapsed = started.elapsed();
    print!(
        "{}",
        report(&procs, ticks_per_sec, elapsed, db_start, db_sizes(&dir))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_ticks_sum_user_and_system() {
        let line = "1234 (drainscope-daem) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 5 0 99 1000 200 18446744073709551615";
        assert_eq!(utime_stime(line), Some(300));
    }

    #[test]
    fn stat_comm_with_spaces_and_parens() {
        let line =
            "1234 (a (b) c) S 1 1234 1234 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 5 0 99 1000 200";
        assert_eq!(utime_stime(line), Some(10));
    }

    #[test]
    fn stat_truncated_is_none() {
        assert_eq!(utime_stime("1234 (x) S 1 2 3"), None);
        assert_eq!(utime_stime("garbage"), None);
    }

    #[test]
    fn status_kb_values() {
        let status = "Name:\tx\nVmHWM:\t   12345 kB\nVmRSS:\t    9000 kB\n";
        assert_eq!(kb(status, "VmHWM:"), Some(12345));
        assert_eq!(kb(status, "VmRSS:"), Some(9000));
        assert_eq!(kb(status, "VmSwap:"), None);
    }
}

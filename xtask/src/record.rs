//! `record-fixture`: snapshot the raw kernel interfaces drainscope reads, for replay in
//! tests and spikes.
//!
//! Records only what attribution needs: powercap counters, `power_supply` state, `/proc/stat`,
//! every cgroup's `cpu.stat`, and DRM fdinfo of processes holding `/dev/dri` fds. Process
//! names (`comm`) are recorded; command lines and environments never are.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;

use crate::trace::{FORMAT, Header, Snapshot, VERSION};

/// VTE terminals (GNOME Console, Terminal) and Ptyxis run each tab in its own scope.
pub const TERMINAL_SCOPE_PREFIXES: &[&str] = &["vte-spawn-", "ptyxis-spawn-"];

const POWERCAP_FILES: &[&str] = &["name", "energy_uj", "max_energy_range_uj"];
const POWER_SUPPLY_FILES: &[&str] = &[
    "type",
    "status",
    "online",
    "capacity",
    "power_now",
    "energy_now",
    "energy_full",
    "energy_full_design",
    "charge_now",
    "charge_full",
    "charge_full_design",
    "current_now",
    "voltage_now",
    "cycle_count",
];

#[derive(Debug)]
pub struct Options {
    pub name: String,
    pub secs: u64,
    pub interval_ms: u64,
    pub local: bool,
}

pub fn run(repo_root: &Path, options: &Options) -> Result<PathBuf> {
    let dir = repo_root.join(if options.local {
        "testdata/local"
    } else {
        "testdata/traces"
    });
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.jsonl.gz", options.name));
    let file = fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    let mut out = GzEncoder::new(BufWriter::new(file), Compression::default());

    let as_root = rustix::process::geteuid().is_root();
    if !as_root {
        eprintln!("note: not running as root, so RAPL counters will be missing (see --help)");
    }
    let header = Header {
        format: FORMAT.to_owned(),
        version: VERSION,
        interval_ms: options.interval_ms,
        recorded_as_root: as_root,
        kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
            .to_owned(),
    };
    write_line(&mut out, &header)?;

    let mut warnings = Warnings::default();
    let interval = Duration::from_millis(options.interval_ms);
    let deadline = Instant::now() + Duration::from_secs(options.secs);
    let mut next = Instant::now();
    let mut count: u64 = 0;
    eprintln!(
        "recording {}s every {}ms to {}",
        options.secs,
        options.interval_ms,
        path.display()
    );
    while Instant::now() < deadline {
        let snapshot = capture(&mut warnings)?;
        write_line(&mut out, &snapshot)?;
        count += 1;
        if count.is_multiple_of(30) {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs();
            eprintln!("  {count} snapshots, {left}s left");
        }
        next += interval;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    out.finish()?.flush()?;
    chown_to_invoking_user(&[&dir, &path])?;
    eprintln!("wrote {count} snapshots to {}", path.display());
    Ok(path)
}

fn write_line<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Prints each distinct (category, error kind) once instead of on every tick.
#[derive(Debug, Default)]
struct Warnings(BTreeSet<String>);

impl Warnings {
    fn note(&mut self, category: &str, err: &io::Error) {
        let message = format!("{category}: {}", err.kind());
        if self.0.insert(message.clone()) {
            eprintln!("warning: {message}");
        }
    }
}

fn capture(warnings: &mut Warnings) -> Result<Snapshot> {
    let mono_ns = monotonic_ns()?;
    let real_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let mut files = BTreeMap::new();
    capture_class(
        Path::new("/sys/class/powercap"),
        POWERCAP_FILES,
        &mut files,
        warnings,
    );
    capture_class(
        Path::new("/sys/class/power_supply"),
        POWER_SUPPLY_FILES,
        &mut files,
        warnings,
    );
    read_into(Path::new("/proc/stat"), "/proc/stat", &mut files, warnings);
    capture_cgroups(Path::new("/sys/fs/cgroup"), &mut files, warnings);
    capture_drm_clients(&mut files, warnings);
    Ok(Snapshot {
        mono_ns,
        real_ms,
        files,
    })
}

fn monotonic_ns() -> Result<u64> {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Ok(u64::try_from(now.tv_sec)? * 1_000_000_000 + u64::try_from(now.tv_nsec)?)
}

fn key(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches('/').to_owned()
}

/// Reads `path` into `files`. Missing files are normal (optional attributes, exited
/// processes, removed cgroups) and are skipped silently; other errors warn once.
fn read_into(
    path: &Path,
    category: &str,
    files: &mut BTreeMap<String, String>,
    warnings: &mut Warnings,
) {
    match fs::read_to_string(path) {
        Ok(text) => {
            files.insert(key(path), text);
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => warnings.note(category, &err),
    }
}

fn capture_class(
    class_dir: &Path,
    names: &[&str],
    files: &mut BTreeMap<String, String>,
    warnings: &mut Warnings,
) {
    let Ok(entries) = fs::read_dir(class_dir) else {
        return;
    };
    for entry in entries.flatten() {
        for name in names {
            let category = format!("{}/*/{name}", class_dir.display());
            read_into(&entry.path().join(name), &category, files, warnings);
        }
    }
}

fn capture_cgroups(root: &Path, files: &mut BTreeMap<String, String>, warnings: &mut Warnings) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        read_into(&dir.join("cpu.stat"), "cgroup cpu.stat", files, warnings);
        let is_terminal = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| TERMINAL_SCOPE_PREFIXES.iter().any(|p| n.starts_with(p)));
        if is_terminal {
            capture_cgroup_comms(&dir, files, warnings);
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(entry.path());
            }
        }
    }
}

/// Terminal scopes are named by UUID; record their processes' names so they can be labelled.
fn capture_cgroup_comms(dir: &Path, files: &mut BTreeMap<String, String>, warnings: &mut Warnings) {
    let procs_path = dir.join("cgroup.procs");
    let Ok(procs) = fs::read_to_string(&procs_path) else {
        return;
    };
    for pid in procs.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let comm = Path::new("/proc").join(pid).join("comm");
        read_into(&comm, "terminal process comm", files, warnings);
    }
    files.insert(key(&procs_path), procs);
}

fn capture_drm_clients(files: &mut BTreeMap<String, String>, warnings: &mut Warnings) {
    let Ok(procs) = fs::read_dir("/proc") else {
        return;
    };
    for entry in procs.flatten() {
        let is_pid = entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()));
        if !is_pid {
            continue;
        }
        let proc_dir = entry.path();
        // Unreadable for other users' processes when not running as root; that's expected.
        let Ok(fds) = fs::read_dir(proc_dir.join("fd")) else {
            continue;
        };
        let mut found = false;
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            if !target.starts_with("/dev/dri") {
                continue;
            }
            let info = proc_dir.join("fdinfo").join(fd.file_name());
            if let Ok(text) = fs::read_to_string(&info)
                && text.contains("drm-client-id")
            {
                files.insert(key(&info), text);
                found = true;
            }
        }
        if found {
            for name in ["comm", "cgroup", "stat"] {
                read_into(&proc_dir.join(name), "drm process info", files, warnings);
            }
        }
    }
}

/// When run via sudo, hand the outputs back to the invoking user so the repo stays theirs.
fn chown_to_invoking_user(paths: &[&Path]) -> Result<()> {
    let (Ok(uid), Ok(gid)) = (std::env::var("SUDO_UID"), std::env::var("SUDO_GID")) else {
        return Ok(());
    };
    let uid: u32 = uid.parse().context("parsing SUDO_UID")?;
    let gid: u32 = gid.parse().context("parsing SUDO_GID")?;
    for path in paths {
        std::os::unix::fs::chown(path, Some(uid), Some(gid))
            .with_context(|| format!("chown {}", path.display()))?;
    }
    Ok(())
}

//! `install-dev` / `uninstall-dev`: put the sampler and its system integration files where a
//! development machine's systemd, D-Bus broker and polkit find them, without touching `/usr`.
//! Packages install the same files under `/usr` instead (packaging/).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

const SAMPLER_BINARY: &str = "target/release/drainscope-sampler";

/// (source relative to the repo, destination, mode). Sources referring to `/usr/libexec` are
/// rewritten to `/usr/local/libexec`.
const FILES: &[(&str, &str, u32)] = &[
    (
        SAMPLER_BINARY,
        "/usr/local/libexec/drainscope-sampler",
        0o755,
    ),
    (
        "data/systemd/drainscope-sampler.service",
        "/etc/systemd/system/drainscope-sampler.service",
        0o644,
    ),
    (
        "data/dbus/system-services/io.github.khaledsaeed18.Drainscope.Sampler.service",
        "/usr/local/share/dbus-1/system-services/io.github.khaledsaeed18.Drainscope.Sampler.service",
        0o644,
    ),
    (
        "data/dbus/system.d/io.github.khaledsaeed18.Drainscope.Sampler.conf",
        "/etc/dbus-1/system.d/io.github.khaledsaeed18.Drainscope.Sampler.conf",
        0o644,
    ),
    (
        "data/polkit/io.github.khaledsaeed18.Drainscope.policy",
        "/etc/polkit-1/actions/io.github.khaledsaeed18.Drainscope.policy",
        0o644,
    ),
    (
        "data/sysusers/drainscope.conf",
        "/etc/sysusers.d/drainscope.conf",
        0o644,
    ),
];

fn require_root() -> Result<()> {
    ensure!(
        rustix::process::geteuid().is_root(),
        "needs root: run `cargo build --release -p drainscope-sampler -p xtask` first, then \
         `sudo target/release/xtask install-dev`"
    );
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("running {program}"))?;
    if !status.success() {
        bail!("{program} {} failed: {status}", args.join(" "));
    }
    Ok(())
}

fn reload() -> Result<()> {
    run("systemctl", &["daemon-reload"])?;
    run(
        "busctl",
        &[
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ReloadConfig",
        ],
    )
}

/// Stops a running sampler so reinstalling takes effect immediately; not running is fine.
fn stop_sampler() {
    let _ = Command::new("systemctl")
        .args(["stop", "drainscope-sampler.service"])
        .status();
}

pub fn install(repo_root: &Path) -> Result<()> {
    require_root()?;
    stop_sampler();
    let binary = repo_root.join(SAMPLER_BINARY);
    ensure!(
        binary.exists(),
        "{} is missing: run `cargo build --release -p drainscope-sampler` first",
        binary.display()
    );
    for (source, destination, mode) in FILES {
        let source = repo_root.join(source);
        let destination = Path::new(destination);
        let mut contents =
            fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
        if mode & 0o111 == 0 {
            let text = String::from_utf8(contents).context("config files are UTF-8")?;
            contents = text
                .replace("/usr/libexec/", "/usr/local/libexec/")
                .into_bytes();
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        // Writing (not copying with metadata) gives each file the SELinux label of its
        // destination directory.
        fs::write(destination, contents)
            .with_context(|| format!("writing {}", destination.display()))?;
        fs::set_permissions(destination, fs::Permissions::from_mode(*mode))?;
        println!("installed {}", destination.display());
    }
    run("systemd-sysusers", &["/etc/sysusers.d/drainscope.conf"])?;
    reload()?;
    println!(
        "\nThe sampler starts on first use. Try, as your normal user:\n  busctl call \
         io.github.khaledsaeed18.Drainscope.Sampler /io/github/khaledsaeed18/Drainscope/Sampler \
         io.github.khaledsaeed18.Drainscope.Sampler1 ReadCounters"
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    require_root()?;
    stop_sampler();
    for (_, destination, _) in FILES {
        match fs::remove_file(destination) {
            Ok(()) => println!("removed {destination}"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {destination}")),
        }
    }
    reload()?;
    println!("\nThe drainscope-sampler system user is kept (sysusers never deletes users).");
    Ok(())
}

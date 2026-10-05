//! `install-dev` / `uninstall-dev`: put the binaries and their system integration files where
//! a development machine's systemd, D-Bus brokers and polkit find them, without touching
//! `/usr`. Packages install the same files under `/usr` instead (packaging/).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

const BUILD: &str = "cargo build --release -p drainscope-sampler -p drainscope-probe \
                     -p drainscope-daemon -p drainscope-cli -p xtask";

/// (source relative to the repo, destination, mode). Config files referring to `/usr/libexec`
/// or `/usr/bin` are rewritten to their `/usr/local` equivalents.
const FILES: &[(&str, &str, u32)] = &[
    // Binaries.
    (
        "target/release/drainscope-sampler",
        "/usr/local/libexec/drainscope-sampler",
        0o755,
    ),
    (
        "target/release/drainscope-probe",
        "/usr/local/libexec/drainscope-probe",
        0o755,
    ),
    (
        "target/release/drainscope-daemon",
        "/usr/local/bin/drainscope-daemon",
        0o755,
    ),
    (
        "target/release/drainscope",
        "/usr/local/bin/drainscope",
        0o755,
    ),
    // The privileged sampler (system bus).
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
    // The privileged eBPF probe (system bus).
    (
        "data/systemd/drainscope-probe.service",
        "/etc/systemd/system/drainscope-probe.service",
        0o644,
    ),
    (
        "data/dbus/system-services/io.github.khaledsaeed18.Drainscope.Probe.service",
        "/usr/local/share/dbus-1/system-services/io.github.khaledsaeed18.Drainscope.Probe.service",
        0o644,
    ),
    (
        "data/dbus/system.d/io.github.khaledsaeed18.Drainscope.Probe.conf",
        "/etc/dbus-1/system.d/io.github.khaledsaeed18.Drainscope.Probe.conf",
        0o644,
    ),
    (
        "data/polkit/io.github.khaledsaeed18.Drainscope.Probe.policy",
        "/etc/polkit-1/actions/io.github.khaledsaeed18.Drainscope.Probe.policy",
        0o644,
    ),
    (
        "data/sysusers/drainscope-probe.conf",
        "/etc/sysusers.d/drainscope-probe.conf",
        0o644,
    ),
    // The user daemon (session bus).
    (
        "data/systemd/user/drainscope.service",
        "/etc/systemd/user/drainscope.service",
        0o644,
    ),
    (
        "data/dbus/services/io.github.khaledsaeed18.Drainscope.Monitor.service",
        "/usr/local/share/dbus-1/services/io.github.khaledsaeed18.Drainscope.Monitor.service",
        0o644,
    ),
];

fn require_root() -> Result<()> {
    ensure!(
        rustix::process::geteuid().is_root(),
        "needs root: run `{BUILD}` first, then `sudo target/release/xtask install-dev`"
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

const SELINUX_MAKEFILE: &str = "/usr/share/selinux/devel/Makefile";
const SELINUX_MODULES: [&str; 2] = ["drainscope_sampler", "drainscope_probe"];
const SYSTEM_SERVICES: [&str; 2] = ["drainscope-sampler.service", "drainscope-probe.service"];
const SYSUSERS: [&str; 2] = [
    "/etc/sysusers.d/drainscope.conf",
    "/etc/sysusers.d/drainscope-probe.conf",
];

/// Builds and loads the privileged services' `SELinux` modules (data/selinux/), if `SELinux` is
/// enabled and the policy development files are installed. Returns whether they were loaded.
fn install_selinux(repo_root: &Path) -> Result<bool> {
    if !Path::new("/sys/fs/selinux/enforce").exists() {
        return Ok(false);
    }
    if !Path::new(SELINUX_MAKEFILE).exists() {
        println!("skipped the SELinux modules: install selinux-policy-devel to build them");
        return Ok(false);
    }
    // Built outside the repo so no root-owned files end up in the user's tree.
    let build = std::env::temp_dir().join("drainscope-selinux");
    if build.exists() {
        fs::remove_dir_all(&build)?;
    }
    fs::create_dir_all(&build)?;
    for module in SELINUX_MODULES {
        for extension in ["te", "fc", "if"] {
            let name = format!("{module}.{extension}");
            fs::copy(
                repo_root.join("data/selinux").join(&name),
                build.join(&name),
            )
            .with_context(|| format!("copying {name}"))?;
        }
    }
    let packages: Vec<String> = SELINUX_MODULES.iter().map(|m| format!("{m}.pp")).collect();
    let status = Command::new("make")
        .args(["-f", SELINUX_MAKEFILE])
        .args(&packages)
        .current_dir(&build)
        .stdout(Stdio::null())
        .status()
        .context("running make")?;
    ensure!(
        status.success(),
        "building the SELinux modules failed: {status}"
    );
    for package in &packages {
        run("semodule", &["-i", &build.join(package).to_string_lossy()])?;
    }
    fs::remove_dir_all(&build)?;
    println!("loaded SELinux modules {}", SELINUX_MODULES.join(", "));
    Ok(true)
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

/// Stops the running privileged services so reinstalling takes effect immediately (they are
/// D-Bus activated again on the next call); not running is fine.
fn stop_services() {
    let _ = Command::new("systemctl")
        .arg("stop")
        .args(SYSTEM_SERVICES)
        .status();
}

/// Writes `contents` next to `destination` and renames it into place: atomic, and works for
/// binaries that are running (writing over them fails with "text file busy"). Writing (not
/// copying with metadata) gives the file the `SELinux` label of its directory.
fn replace(destination: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let parent = destination
        .parent()
        .context("destination has no directory")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let staging = destination.with_extension("drainscope-new");
    fs::write(&staging, contents).with_context(|| format!("writing {}", staging.display()))?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(mode))?;
    fs::rename(&staging, destination)
        .with_context(|| format!("replacing {}", destination.display()))
}

pub fn install(repo_root: &Path) -> Result<()> {
    require_root()?;
    for (source, _, mode) in FILES {
        if mode & 0o111 != 0 {
            let binary = repo_root.join(source);
            ensure!(
                binary.exists(),
                "{} is missing: run `{BUILD}` first",
                binary.display()
            );
        }
    }
    stop_services();
    for (source, destination, mode) in FILES {
        let source = repo_root.join(source);
        let mut contents =
            fs::read(&source).with_context(|| format!("reading {}", source.display()))?;
        if mode & 0o111 == 0 {
            let text = String::from_utf8(contents).context("config files are UTF-8")?;
            contents = text
                .replace("/usr/libexec/", "/usr/local/libexec/")
                .replace("/usr/bin/", "/usr/local/bin/")
                .into_bytes();
        }
        // Unchanged files are left alone, so systemd doesn't report units changed on disk.
        if fs::read(destination).is_ok_and(|current| current == contents) {
            println!("unchanged {destination}");
            continue;
        }
        replace(Path::new(destination), &contents, *mode)?;
        println!("installed {destination}");
    }
    if install_selinux(repo_root)? {
        // Files renamed into place keep the label they were created with.
        let destinations: Vec<&str> = FILES
            .iter()
            .map(|(_, destination, _)| *destination)
            .collect();
        run("restorecon", &[&["-F"], destinations.as_slice()].concat())?;
    }
    run("systemd-sysusers", &SYSUSERS)?;
    reload()?;
    println!(
        "\nThe sampler starts on demand. Now, as your normal user (not root):\n  \
         systemctl --user daemon-reload\n  \
         systemctl --user enable --now drainscope.service   # or `restart` if already enabled\n  \
         drainscope doctor"
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    require_root()?;
    stop_services();
    for (_, destination, _) in FILES {
        match fs::remove_file(destination) {
            Ok(()) => println!("removed {destination}"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {destination}")),
        }
    }
    for module in SELINUX_MODULES {
        // Not loaded (or no SELinux) is fine.
        let removed = Command::new("semodule")
            .args(["-r", module])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if removed {
            println!("removed SELinux module {module}");
        }
    }
    reload()?;
    println!(
        "\nIf the daemon was enabled, also run as your normal user:\n  \
         systemctl --user disable --now drainscope.service\n\
         The drainscope-sampler and drainscope-probe system users are kept (sysusers never deletes users), and so is \
         your history in ~/.local/state/drainscope/."
    );
    Ok(())
}

//! cgroup v2 paths and how they map to consumers.
//!
//! systemd lays out the hierarchy predictably: system services under `system.slice`, each
//! user's processes under `user.slice/user-<uid>.slice`, and inside the user manager GNOME
//! starts every application in its own scope (`app-gnome-<app id>-<pid>.scope`). The rules
//! here turn a path into a [`CgroupIdentity`] without any I/O.

use std::fmt;

use crate::consumer::ConsumerKey;
use crate::unit_name::unescape;

/// A cgroup path relative to the cgroupfs root, without leading or trailing `/`.
/// The root cgroup is the empty path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CgroupPath(String);

impl CgroupPath {
    #[must_use]
    pub fn new(path: &str) -> Self {
        Self(path.trim_matches('/').to_owned())
    }

    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The parent cgroup, or `None` for the root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(Self(
            self.0
                .rsplit_once('/')
                .map_or_else(String::new, |(parent, _)| parent.to_owned()),
        ))
    }

    /// Path components from the top; empty for the root.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|s| !s.is_empty())
    }
}

impl fmt::Display for CgroupPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.0)
    }
}

/// What a cgroup represents, before any process-level information is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupIdentity {
    /// The cgroup belongs to a known consumer.
    Consumer(ConsumerKey),
    /// A terminal tab scope, named by a UUID. The caller labels it with the name of the
    /// process doing the work, which needs process information.
    Terminal { scope: String },
    /// Only a grouping (a slice, or the user manager itself). CPU time measured here belongs
    /// to descendants that exited during the interval.
    Slice(String),
}

/// VTE terminals (GNOME Console, GNOME Terminal) and Ptyxis run each tab in its own scope.
pub const TERMINAL_SCOPE_PREFIXES: &[&str] = &["vte-spawn-", "ptyxis-spawn-"];

const UNIT_SUFFIXES: &[&str] = &[".service", ".scope", ".socket", ".mount", ".swap"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Manager {
    System,
    User,
}

/// Classifies `path` from the point of view of the user with uid `own_uid`.
#[must_use]
pub fn classify(path: &CgroupPath, own_uid: u32) -> CgroupIdentity {
    let segments: Vec<&str> = path.segments().collect();
    match segments.as_slice() {
        [] => CgroupIdentity::Consumer(ConsumerKey::Kernel),
        ["user.slice", user, rest @ ..] => match user_slice_uid(user) {
            Some(uid) => classify_user(uid, user, rest, own_uid),
            None => classify_units(&segments, Manager::System),
        },
        ["machine.slice", machine, ..] => {
            CgroupIdentity::Consumer(ConsumerKey::Container(machine_name(machine)))
        }
        _ => classify_units(&segments, Manager::System),
    }
}

/// `user-<uid>.slice` → uid.
fn user_slice_uid(segment: &str) -> Option<u32> {
    segment
        .strip_prefix("user-")?
        .strip_suffix(".slice")?
        .parse()
        .ok()
}

fn classify_user(uid: u32, user_slice: &str, rest: &[&str], own_uid: u32) -> CgroupIdentity {
    if uid != own_uid {
        let key = if uid == 0 {
            ConsumerKey::Root
        } else {
            ConsumerKey::OtherUsers
        };
        return CgroupIdentity::Consumer(key);
    }
    match rest {
        [] => CgroupIdentity::Slice(user_slice.to_owned()),
        [first, tail @ ..] => {
            if let Some(id) = first
                .strip_prefix("session-")
                .and_then(|s| s.strip_suffix(".scope"))
            {
                CgroupIdentity::Consumer(ConsumerKey::Session(unescape(id)))
            } else if first.starts_with("user@") {
                if tail.is_empty() {
                    CgroupIdentity::Slice((*first).to_owned())
                } else {
                    classify_units(tail, Manager::User)
                }
            } else {
                classify_units(rest, Manager::User)
            }
        }
    }
}

fn is_unit(segment: &str) -> bool {
    UNIT_SUFFIXES.iter().any(|suffix| segment.ends_with(suffix))
}

/// The owning unit is the deepest segment that is a unit. Deeper segments without a unit
/// suffix are sub-cgroups a service manages itself (e.g. `systemd-udevd.service/udev`).
fn classify_units(segments: &[&str], manager: Manager) -> CgroupIdentity {
    match segments.iter().rev().find(|s| is_unit(s)) {
        Some(unit) => unit_identity(unit, manager),
        None => CgroupIdentity::Slice(unescape(segments.last().copied().unwrap_or_default())),
    }
}

fn unit_identity(unit: &str, manager: Manager) -> CgroupIdentity {
    if let Some(id) = app_id(unit) {
        return CgroupIdentity::Consumer(ConsumerKey::App(unescape(id)));
    }
    if TERMINAL_SCOPE_PREFIXES.iter().any(|p| unit.starts_with(p)) {
        return CgroupIdentity::Terminal {
            scope: unescape(unit),
        };
    }
    if unit.starts_with("org.gnome.Shell@") && unit.ends_with(".service") {
        return CgroupIdentity::Consumer(ConsumerKey::Shell);
    }
    if let Some(id) = container_id(unit) {
        return CgroupIdentity::Consumer(ConsumerKey::Container(id.to_owned()));
    }
    let key = match (manager, unit) {
        (Manager::System, "drainscope-sampler.service") | (Manager::User, "drainscope.service") => {
            ConsumerKey::Drainscope
        }
        (Manager::System, _) => ConsumerKey::SystemUnit(stable_unit_name(unit)),
        (Manager::User, _) => ConsumerKey::UserUnit(stable_unit_name(unit)),
    };
    CgroupIdentity::Consumer(key)
}

/// Application ids, still escaped, from the unit names desktops use:
///
/// - `app[-<launcher>]-<app id>-<random>.scope` and `app[-<launcher>]-<app id>[@<random>].service`
///   (systemd's desktop-environment conventions; GNOME uses launcher `gnome`, Flatpak `flatpak`)
/// - `dbus-:<bus address>-<bus name>@<n>.service` for D-Bus-activated applications
///
/// Literal dashes are separators; dashes inside an app id are escaped as `\x2d`.
fn app_id(unit: &str) -> Option<&str> {
    if let Some(rest) = unit.strip_prefix("dbus-:") {
        let (_, named) = rest.split_once('-')?;
        let (bus_name, _) = named.rsplit_once('@')?;
        return Some(bus_name);
    }
    let rest = unit.strip_prefix("app-")?;
    let body = if let Some(scope) = rest.strip_suffix(".scope") {
        scope.rsplit_once('-')?.0
    } else {
        let service = rest.strip_suffix(".service")?;
        service.split_once('@').map_or(service, |(id, _)| id)
    };
    // A launcher is a short word without dots; app ids are reverse-DNS names or desktop-file
    // basenames, so a first part containing a dot is already the app id.
    match body.split_once('-') {
        Some((launcher, id)) if !launcher.contains('.') && !id.is_empty() => Some(id),
        _ => Some(body),
    }
}

/// Container scopes: `libpod-<id>.scope`, `libpod-conmon-<id>.scope`, `docker-<id>.scope`.
/// Returns the 12-character short id, as `podman ps` and `docker ps` show it.
fn container_id(unit: &str) -> Option<&str> {
    let body = unit.strip_suffix(".scope")?;
    let id = body
        .strip_prefix("libpod-conmon-")
        .or_else(|| body.strip_prefix("libpod-"))
        .or_else(|| body.strip_prefix("docker-"))?;
    if id.len() >= 12 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        id.get(..12)
    } else {
        None
    }
}

fn machine_name(segment: &str) -> String {
    if let Some(id) = container_id(segment) {
        return id.to_owned();
    }
    unescape(segment.strip_suffix(".scope").unwrap_or(segment))
}

/// Collapses unit names that embed per-run identifiers, so each run doesn't become a new
/// consumer: `systemd-run`'s `run-u123.service` / `run-p1-i2.scope`, Podman's
/// `podman-pause-<hex>.scope`.
fn stable_unit_name(unit: &str) -> String {
    let (stem, suffix) = unit.rsplit_once('.').unwrap_or((unit, ""));
    if let Some(id) = stem.strip_prefix("run-")
        && is_run_id(id)
    {
        return format!("systemd-run.{suffix}");
    }
    if let Some(id) = stem.strip_prefix("podman-pause-")
        && id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return format!("podman-pause.{suffix}");
    }
    unescape(unit)
}

/// `u123`, `r1a2b`, `p1234-i5678`.
fn is_run_id(id: &str) -> bool {
    let mut parts = id.split('-');
    let first_ok = parts.next().is_some_and(|p| {
        let mut chars = p.chars();
        matches!(chars.next(), Some('u' | 'p' | 'r'))
            && !chars.as_str().is_empty()
            && chars.all(|c| c.is_ascii_hexdigit())
    });
    first_ok
        && parts.all(|p| {
            p.strip_prefix('i')
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: u32 = 1000;
    const MANAGER: &str = "user.slice/user-1000.slice/user@1000.service";
    const POD_ID: &str = "4f2a9c3b1d7e8f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c5d4e3f2a";

    /// Compact rendering of an identity for table-driven tests.
    fn describe(identity: &CgroupIdentity) -> String {
        match identity {
            CgroupIdentity::Consumer(key) => key.to_string(),
            CgroupIdentity::Terminal { scope } => format!("terminal-scope:{scope}"),
            CgroupIdentity::Slice(name) => format!("slice:{name}"),
        }
    }

    fn check(cases: &[(&str, &str)]) {
        for (path, expected) in cases {
            let got = describe(&classify(&CgroupPath::new(path), UID));
            assert_eq!(&got, expected, "classify({path:?})");
        }
    }

    #[test]
    fn root_is_the_kernel() {
        check(&[("", "kernel"), ("/", "kernel")]);
    }

    #[test]
    fn system_units() {
        check(&[
            ("init.scope", "unit:init.scope"),
            ("dev-hugepages.mount", "unit:dev-hugepages.mount"),
            ("system.slice", "slice:system.slice"),
            (
                "system.slice/NetworkManager.service",
                "unit:NetworkManager.service",
            ),
            ("system.slice/dev-zram0.swap", "unit:dev-zram0.swap"),
            (
                "system.slice/system-cups.slice/cups.service",
                "unit:cups.service",
            ),
            (
                r"system.slice/system-systemd\x2dbacklight.slice",
                "slice:system-systemd-backlight.slice",
            ),
            // A service's own sub-cgroups belong to the service.
            (
                "system.slice/systemd-udevd.service/udev",
                "unit:systemd-udevd.service",
            ),
            ("system.slice/drainscope-sampler.service", "drainscope"),
            ("system.slice/run-u123.service", "unit:systemd-run.service"),
        ]);
    }

    #[test]
    fn other_users_and_root() {
        check(&[
            ("user.slice/user-0.slice/session-c8.scope", "root"),
            (
                "user.slice/user-0.slice/user@0.service/app.slice/dbus.socket",
                "root",
            ),
            (
                "user.slice/user-1001.slice/user@1001.service/app.slice/app-gnome-org.mozilla.firefox-77.scope",
                "other-users",
            ),
        ]);
    }

    #[test]
    fn own_sessions_and_manager() {
        check(&[
            ("user.slice", "slice:user.slice"),
            ("user.slice/user-1000.slice", "slice:user-1000.slice"),
            ("user.slice/user-1000.slice/session-3.scope", "session:3"),
            (MANAGER, "slice:user@1000.service"),
        ]);
        let manager_cases = [
            ("init.scope", "user-unit:init.scope"),
            ("app.slice", "slice:app.slice"),
            ("uresourced.service", "user-unit:uresourced.service"),
            (
                "session.slice/pipewire.service",
                "user-unit:pipewire.service",
            ),
            (
                "background.slice/localsearch-3.service",
                "user-unit:localsearch-3.service",
            ),
            ("session.slice/org.gnome.Shell@user.service", "shell"),
            ("session.slice/org.gnome.Shell@wayland.service", "shell"),
            ("app.slice/drainscope.service", "drainscope"),
            ("app.slice/load-2cpu.scope", "user-unit:load-2cpu.scope"),
            (
                "app.slice/run-p1234-i1235.scope",
                "user-unit:systemd-run.scope",
            ),
            (
                r"app.slice/app-gnome\x2dsession\x2dmanager.slice",
                "slice:app-gnome-session-manager.slice",
            ),
            (
                r"app.slice/app-gnome\x2dsession\x2dmanager.slice/gnome-session-manager@gnome.service",
                "user-unit:gnome-session-manager@gnome.service",
            ),
        ];
        for (tail, expected) in manager_cases {
            check(&[(&format!("{MANAGER}/{tail}"), expected)]);
        }
    }

    #[test]
    fn applications() {
        let cases = [
            (
                "app-gnome-org.mozilla.firefox-5177.scope",
                "app:org.mozilla.firefox",
            ),
            ("app-gnome-dev.zed.Zed-18901.scope", "app:dev.zed.Zed"),
            (
                "app-gnome-org.gnome.SettingsDaemon.DiskUtilityNotify-3564.scope",
                "app:org.gnome.SettingsDaemon.DiskUtilityNotify",
            ),
            (
                "app-flatpak-com.spotify.Client-12345.scope",
                "app:com.spotify.Client",
            ),
            // Launcher is optional.
            ("app-org.kde.dolphin-4242.scope", "app:org.kde.dolphin"),
            // Dashes inside the app id are escaped.
            (r"app-gnome-code\x2doss-99.scope", "app:code-oss"),
            // Desktop-file basenames without dots.
            ("app-gnome-firefox-123.scope", "app:firefox"),
            (
                "app-gnome-org.gnome.Settings@autostart.service",
                "app:org.gnome.Settings",
            ),
            (
                "dbus-:1.2-org.gnome.Ptyxis@0.service",
                "app:org.gnome.Ptyxis",
            ),
            (
                "dbus-:1.18-org.a11y.atspi.Registry@0.service",
                "app:org.a11y.atspi.Registry",
            ),
        ];
        for (unit, expected) in cases {
            check(&[(&format!("{MANAGER}/app.slice/{unit}"), expected)]);
        }
    }

    #[test]
    fn terminals() {
        let scope = "ptyxis-spawn-033dff48-0663-4c9e-b689-232e4e424326.scope";
        check(&[
            (
                &format!("{MANAGER}/app.slice/{scope}"),
                &format!("terminal-scope:{scope}"),
            ),
            (
                &format!("{MANAGER}/app.slice/vte-spawn-1b2c.scope"),
                "terminal-scope:vte-spawn-1b2c.scope",
            ),
        ]);
    }

    #[test]
    fn containers_and_machines() {
        let short = &POD_ID[..12];
        let expected = format!("container:{short}");
        check(&[
            (
                &format!("{MANAGER}/user.slice/libpod-{POD_ID}.scope"),
                &expected,
            ),
            (
                &format!("{MANAGER}/user.slice/libpod-conmon-{POD_ID}.scope"),
                &expected,
            ),
            (&format!("system.slice/docker-{POD_ID}.scope"), &expected),
            (&format!("machine.slice/libpod-{POD_ID}.scope"), &expected),
            (
                r"machine.slice/machine-qemu\x2d1\x2dwin11.scope",
                "container:machine-qemu-1-win11",
            ),
            (
                &format!("{MANAGER}/user.slice/podman-pause-651d8c4c.scope"),
                "user-unit:podman-pause.scope",
            ),
        ]);
    }

    #[test]
    fn paths_normalize_and_navigate() {
        let path = CgroupPath::new("/system.slice/cups.service/");
        assert_eq!(path.as_str(), "system.slice/cups.service");
        assert_eq!(path.parent(), Some(CgroupPath::new("system.slice")));
        assert_eq!(
            CgroupPath::new("system.slice").parent(),
            Some(CgroupPath::root())
        );
        assert_eq!(CgroupPath::root().parent(), None);
        assert_eq!(path.to_string(), "/system.slice/cups.service");
    }
}

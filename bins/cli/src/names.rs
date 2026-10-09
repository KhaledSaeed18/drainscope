//! Human names for consumer keys. Applications are named from their desktop entries.

use std::collections::HashMap;
use std::path::PathBuf;

use drainscope_model::ConsumerKey;

/// The `Name=` of a desktop entry's `[Desktop Entry]` group (untranslated).
#[must_use]
pub fn desktop_entry_name(contents: &str) -> Option<String> {
    let mut in_entry = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some(name) = line.strip_prefix("Name=") {
            return Some(name.trim().to_owned()).filter(|n| !n.is_empty());
        }
    }
    None
}

/// Directories holding `.desktop` files, most specific first (XDG base directories plus
/// Flatpak exports).
fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    let mut dirs: Vec<PathBuf> = data_home.into_iter().collect();
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    );
    if let Some(home) = &home {
        dirs.push(home.join(".local/share/flatpak/exports/share"));
    }
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// Caches desktop-entry lookups for one run.
#[derive(Debug, Default)]
pub struct Names {
    cache: HashMap<String, Option<String>>,
}

impl Names {
    fn app_name(&mut self, id: &str) -> Option<String> {
        self.cache
            .entry(id.to_owned())
            .or_insert_with(|| {
                application_dirs().iter().find_map(|dir| {
                    let contents =
                        std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok()?;
                    desktop_entry_name(&contents)
                })
            })
            .clone()
    }

    /// A readable label for a storage key; unknown keys are shown as they are.
    pub fn label(&mut self, key: &str) -> String {
        match key.parse::<ConsumerKey>() {
            Ok(key) => describe(&key, |id| self.app_name(id)),
            Err(_) => key.to_owned(),
        }
    }
}

/// Processes whose cgroup came and went between two measurements, by the slice they ran in
/// (ADR 0010). Matches the app's and extension's labels.
fn short_lived(slice: &str) -> String {
    // Slice names are case-sensitive: `system-<name>.slice` holds a template's services.
    let system_template = slice
        .strip_prefix("system-")
        .is_some_and(|rest| rest.strip_suffix(".slice").is_some());
    if slice == "app.slice" {
        "Short-lived apps and commands".to_owned()
    } else if slice == "system.slice" || system_template {
        "Short-lived system services".to_owned()
    } else {
        format!("Short-lived processes ({slice})")
    }
}

/// Describes a consumer, naming apps through `app_name`.
pub fn describe(key: &ConsumerKey, mut app_name: impl FnMut(&str) -> Option<String>) -> String {
    let unit = |name: &str| name.strip_suffix(".service").unwrap_or(name).to_owned();
    match key {
        ConsumerKey::App(id) => app_name(id).unwrap_or_else(|| id.clone()),
        ConsumerKey::Terminal(label) => format!("Terminal: {label}"),
        ConsumerKey::Session(id) => format!("Login session {id}"),
        ConsumerKey::Shell => "GNOME Shell".to_owned(),
        ConsumerKey::SystemUnit(name) => format!("System: {}", unit(name)),
        ConsumerKey::UserUnit(name) => format!("Service: {}", unit(name)),
        ConsumerKey::Container(id) => format!("Container {id}"),
        ConsumerKey::OtherUsers => "Other users".to_owned(),
        ConsumerKey::Root => "Root (sudo, admin sessions)".to_owned(),
        ConsumerKey::Kernel => "Kernel".to_owned(),
        ConsumerKey::Exited(slice) => short_lived(slice),
        ConsumerKey::Idle => "Idle".to_owned(),
        ConsumerKey::Platform => "Chipset & platform".to_owned(),
        ConsumerKey::Devices => "Display & devices".to_owned(),
        ConsumerKey::Drainscope => "drainscope".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_untranslated_entry_name() {
        let entry = "[Desktop Entry]\nName[de]=Feuerfuchs\nName=Firefox\nExec=firefox\n\n[Desktop Action new]\nName=New Window\n";
        assert_eq!(desktop_entry_name(entry).as_deref(), Some("Firefox"));
        assert_eq!(desktop_entry_name("[Desktop Action x]\nName=Nope\n"), None);
    }

    #[test]
    fn describes_consumers() {
        let named = |id: &str| (id == "org.mozilla.firefox").then(|| "Firefox".to_owned());
        assert_eq!(
            describe(&ConsumerKey::App("org.mozilla.firefox".into()), named),
            "Firefox"
        );
        assert_eq!(
            describe(&ConsumerKey::App("org.unknown".into()), named),
            "org.unknown"
        );
        assert_eq!(
            describe(
                &ConsumerKey::SystemUnit("NetworkManager.service".into()),
                named
            ),
            "System: NetworkManager"
        );
        assert_eq!(describe(&ConsumerKey::Devices, named), "Display & devices");
        let exited = |slice: &str| describe(&ConsumerKey::Exited(slice.into()), named);
        assert_eq!(exited("app.slice"), "Short-lived apps and commands");
        assert_eq!(
            exited("system-systemd-coredump.slice"),
            "Short-lived system services"
        );
        assert_eq!(exited("user.slice"), "Short-lived processes (user.slice)");
    }
}

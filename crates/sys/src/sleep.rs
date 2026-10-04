//! Suspend: logind's sleep notifications and the kernel's sleep mode.

use drainscope_model::snapshot::BatteryStatus;

use crate::error::SysError;
use crate::root::SysRoot;

/// The subset of `org.freedesktop.login1.Manager` drainscope uses.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
pub trait Login1Manager {
    /// Takes an inhibitor lock; it is held until the returned fd is closed. A `delay` lock on
    /// `sleep` gives the holder time to act on `PrepareForSleep(true)` before suspending.
    fn inhibit(
        &self,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedFd>;

    /// `true` just before suspending, `false` after resuming.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// The active mode in `/sys/power/mem_sleep` (the bracketed entry, e.g. `deep` in
/// `s2idle [deep]`), if the file exists.
///
/// # Errors
/// Unexpected I/O errors.
pub fn read_mem_sleep(root: &SysRoot) -> Result<Option<String>, SysError> {
    let path = root.path("sys/power/mem_sleep");
    Ok(root.read_optional(&path)?.and_then(|text| {
        text.split_whitespace()
            .find_map(|mode| mode.strip_prefix('[')?.strip_suffix(']'))
            .map(str::to_owned)
    }))
}

/// Whether any battery is discharging: the machine is on battery power.
#[must_use]
pub fn on_battery(statuses: impl IntoIterator<Item = BatteryStatus>) -> bool {
    statuses
        .into_iter()
        .any(|status| status == BatteryStatus::Discharging)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_the_active_sleep_mode() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        assert_eq!(read_mem_sleep(&root).unwrap(), None);
        fs::create_dir_all(root.path("sys/power")).unwrap();
        fs::write(root.path("sys/power/mem_sleep"), "s2idle [deep]\n").unwrap();
        assert_eq!(read_mem_sleep(&root).unwrap().as_deref(), Some("deep"));
    }

    #[test]
    fn any_discharging_battery_means_on_battery() {
        use BatteryStatus::{Discharging, NotCharging};
        assert!(on_battery([NotCharging, Discharging]));
        assert!(!on_battery([NotCharging]));
    }
}

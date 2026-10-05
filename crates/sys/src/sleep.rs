//! Suspend: logind's sleep notifications and the kernel's sleep mode.

use std::fs;

use drainscope_model::snapshot::BatteryStatus;
use drainscope_model::{WakeupIrq, WakeupSource, WakeupSources};

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

/// Every wakeup source in `/sys/class/wakeup` (all world-readable). Unreadable entries are
/// skipped: sources come and go with devices.
#[must_use]
pub fn read_wakeup_sources(root: &SysRoot) -> WakeupSources {
    let Ok(entries) = fs::read_dir(root.path("sys/class/wakeup")) else {
        return WakeupSources::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            let name = fs::read_to_string(dir.join("name")).ok()?.trim().to_owned();
            let wakeup_count = fs::read_to_string(dir.join("wakeup_count"))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            Some((
                entry.file_name().to_string_lossy().into_owned(),
                WakeupSource { name, wakeup_count },
            ))
        })
        .collect()
}

/// The IRQ that ended the last suspend (`/sys/power/pm_wakeup_irq`, which fails with ENODATA
/// when there was none), named from `/proc/interrupts`.
#[must_use]
pub fn read_wakeup_irq(root: &SysRoot) -> Option<WakeupIrq> {
    let irq: u32 = fs::read_to_string(root.path("sys/power/pm_wakeup_irq"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let interrupts = fs::read_to_string(root.path("proc/interrupts")).unwrap_or_default();
    let action = interrupts
        .lines()
        .find_map(|line| {
            let (label, rest) = line.split_once(':')?;
            if label.trim().parse::<u32>().ok()? != irq {
                return None;
            }
            // Per-CPU counts, then the chip, the hardware IRQ and the handlers.
            let fields: Vec<&str> = rest
                .split_whitespace()
                .skip_while(|f| f.bytes().all(|b| b.is_ascii_digit()))
                .skip(2)
                .collect();
            Some(fields.join(" "))
        })
        .unwrap_or_default();
    Some(WakeupIrq { irq, action })
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
    use drainscope_model::WakeupIrq;

    #[test]
    fn reads_wakeup_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        assert!(read_wakeup_sources(&root).is_empty());
        for (entry, name, count) in [("wakeup3", "PNP0C0D:00", "2"), ("wakeup7", "rtc0", "0")] {
            let path = root.path(&format!("sys/class/wakeup/{entry}"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("name"), format!("{name}\n")).unwrap();
            fs::write(path.join("wakeup_count"), format!("{count}\n")).unwrap();
        }
        fs::create_dir_all(root.path("sys/class/wakeup/wakeup9")).unwrap();
        let sources = read_wakeup_sources(&root);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources["wakeup3"].name, "PNP0C0D:00");
        assert_eq!(sources["wakeup3"].wakeup_count, 2);
    }

    #[test]
    fn names_the_wakeup_irq() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        assert_eq!(read_wakeup_irq(&root), None);
        fs::create_dir_all(root.path("sys/power")).unwrap();
        fs::create_dir_all(root.path("proc")).unwrap();
        fs::write(root.path("sys/power/pm_wakeup_irq"), "9\n").unwrap();
        fs::write(
            root.path("proc/interrupts"),
            "            CPU0       CPU1\n   1:          0       3317 IR-IO-APIC    1-edge      i8042\n   9:       5485          0 IR-IO-APIC    9-fasteoi   acpi\n  16:          0    1081716 IR-IO-APIC   16-fasteoi   i2c_designware.0, idma64.0\n NMI:          0          0   Non-maskable interrupts\n",
        )
        .unwrap();
        assert_eq!(
            read_wakeup_irq(&root),
            Some(WakeupIrq {
                irq: 9,
                action: "acpi".into()
            })
        );
        fs::write(root.path("sys/power/pm_wakeup_irq"), "16\n").unwrap();
        assert_eq!(
            read_wakeup_irq(&root).unwrap().action,
            "i2c_designware.0, idma64.0"
        );
    }

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

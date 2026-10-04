//! Laptop batteries from `/sys/class/power_supply`.

use std::fs;

use drainscope_model::snapshot::{BatteryReading, BatteryStatus};
use drainscope_model::{Joules, Watts};

use crate::error::SysError;
use crate::root::{SysRoot, is_gone};

const POWER_SUPPLY: &str = "sys/class/power_supply";
/// 1 µWh = 3.6 mJ.
const JOULES_PER_MICROWATT_HOUR: f64 = 3.6e-3;

/// Every system battery (peripherals like wireless mice are excluded), sorted by name.
///
/// # Errors
/// Unexpected I/O errors on `type`, `scope` or `status`.
pub fn read_batteries(root: &SysRoot) -> Result<Vec<BatteryReading>, SysError> {
    let dir = root.path(POWER_SUPPLY);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if is_gone(&err) => return Ok(Vec::new()),
        Err(err) => return Err(SysError::io(&dir, err)),
    };
    let mut batteries = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let supply = entry.path();
        let text = |name: &str| root.read_optional(&supply.join(name));
        if text("type")?.as_deref().map(str::trim) != Some("Battery") {
            continue;
        }
        if text("scope")?.as_deref().map(str::trim) == Some("Device") {
            continue;
        }
        // Numeric attributes are optional, and some drivers fail reads (ENODATA) instead
        // of omitting them; either way the value is unknown.
        let number = |name: &str| -> Option<u64> {
            fs::read_to_string(supply.join(name))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        let status =
            text("status")?.map_or(BatteryStatus::Unknown, |s| BatteryStatus::from_sysfs(&s));
        batteries.push(BatteryReading {
            name: entry.file_name().to_string_lossy().into_owned(),
            status,
            power: power(&number),
            energy: energy(&number),
        });
    }
    batteries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(batteries)
}

// Battery magnitudes (µW, µWh, µA, µV) are far below 2^52.
#[allow(clippy::cast_precision_loss)]
fn power(number: &dyn Fn(&str) -> Option<u64>) -> Option<Watts> {
    if let Some(microwatts) = number("power_now") {
        return Some(Watts(microwatts as f64 / 1e6));
    }
    let microamps = number("current_now")? as f64;
    let microvolts = number("voltage_now")? as f64;
    Some(Watts(microamps * microvolts / 1e12))
}

// Battery magnitudes (µW, µWh, µA, µV) are far below 2^52.
#[allow(clippy::cast_precision_loss)]
fn energy(number: &dyn Fn(&str) -> Option<u64>) -> Option<Joules> {
    if let Some(microwatt_hours) = number("energy_now") {
        return Some(Joules(microwatt_hours as f64 * JOULES_PER_MICROWATT_HOUR));
    }
    // Charge-reporting batteries: µAh × µV = 1e-12 Wh, approximated at the present voltage.
    let microamp_hours = number("charge_now")? as f64;
    let microvolts = number("voltage_now")? as f64;
    Some(Joules(microamp_hours * microvolts / 1e12 * 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &SysRoot, name: &str, attrs: &[(&str, &str)]) {
        let dir = root.path(&format!("{POWER_SUPPLY}/{name}"));
        fs::create_dir_all(&dir).unwrap();
        for (attr, value) in attrs {
            fs::write(dir.join(attr), format!("{value}\n")).unwrap();
        }
    }

    #[test]
    fn reads_system_batteries_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        supply(&root, "ADP1", &[("type", "Mains"), ("online", "0")]);
        supply(
            &root,
            "BAT1",
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("power_now", "5561000"),
                ("energy_now", "3230000"),
            ],
        );
        supply(
            &root,
            "BAT0",
            &[
                ("type", "Battery"),
                ("status", "Not charging"),
                ("power_now", "0"),
                ("energy_now", "24980000"),
            ],
        );
        supply(
            &root,
            "hidpp_battery_0",
            &[
                ("type", "Battery"),
                ("scope", "Device"),
                ("status", "Discharging"),
            ],
        );

        let batteries = read_batteries(&root).unwrap();
        let names: Vec<&str> = batteries.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["BAT0", "BAT1"]);
        let bat1 = &batteries[1];
        assert_eq!(bat1.status, BatteryStatus::Discharging);
        assert_eq!(bat1.power, Some(Watts(5.561)));
        assert!((bat1.energy.unwrap().0 - 11_628.0).abs() < 1e-6); // 3.23 Wh
    }

    #[test]
    fn falls_back_to_current_and_charge() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        supply(
            &root,
            "BAT0",
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("current_now", "500000"),
                ("voltage_now", "12000000"),
                ("charge_now", "2000000"),
            ],
        );
        let battery = &read_batteries(&root).unwrap()[0];
        assert_eq!(battery.power, Some(Watts(6.0))); // 0.5 A × 12 V
        assert!((battery.energy.unwrap().0 - 86_400.0).abs() < 1e-6); // 2 Ah × 12 V = 24 Wh
    }

    #[test]
    fn missing_attributes_are_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        supply(&root, "BAT0", &[("type", "Battery")]);
        let battery = &read_batteries(&root).unwrap()[0];
        assert_eq!(battery.status, BatteryStatus::Unknown);
        assert_eq!((battery.power, battery.energy), (None, None));
    }
}

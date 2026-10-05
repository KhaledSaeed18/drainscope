//! Battery wear: full-charge capacity against design capacity.

use crate::Joules;
use crate::snapshot::BatteryReading;

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryHealth {
    /// Supply name, e.g. `BAT0`.
    pub battery: String,
    pub energy_full: Joules,
    pub energy_full_design: Option<Joules>,
    pub cycle_count: Option<u32>,
}

impl BatteryHealth {
    /// Full-charge capacity as a percentage of design capacity. New batteries often report a
    /// little over 100%.
    #[must_use]
    pub fn percent(&self) -> Option<f64> {
        let design = self.energy_full_design?;
        (design.0 > 0.0).then(|| self.energy_full.0 / design.0 * 100.0)
    }
}

/// Health of every battery that reports its full-charge capacity.
#[must_use]
pub fn health(batteries: &[BatteryReading]) -> Vec<BatteryHealth> {
    batteries
        .iter()
        .filter_map(|b| {
            Some(BatteryHealth {
                battery: b.name.clone(),
                energy_full: b.energy_full?,
                energy_full_design: b.energy_full_design,
                cycle_count: b.cycle_count,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::BatteryStatus;

    fn reading(name: &str, full: Option<f64>, design: Option<f64>) -> BatteryReading {
        BatteryReading {
            name: name.to_owned(),
            status: BatteryStatus::Discharging,
            power: None,
            energy: None,
            energy_full: full.map(Joules),
            energy_full_design: design.map(Joules),
            cycle_count: Some(12),
        }
    }

    #[test]
    fn compares_full_charge_with_design() {
        let found = health(&[
            reading("BAT0", Some(90.0), Some(100.0)),
            reading("BAT1", None, Some(100.0)),
            reading("BAT2", Some(50.0), None),
        ]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].percent(), Some(90.0));
        assert_eq!(found[0].cycle_count, Some(12));
        assert_eq!(found[1].percent(), None);
    }

    #[test]
    fn zero_design_capacity_is_unknown() {
        let found = health(&[reading("BAT0", Some(90.0), Some(0.0))]);
        assert_eq!(found[0].percent(), None);
    }
}

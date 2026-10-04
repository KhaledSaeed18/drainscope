//! Reconciling RAPL attribution with the battery over windows.
//!
//! Battery `power_now` lags load changes by 6–8 s (ADR 0001), so per-tick comparisons are
//! meaningless. Over a window of at least [`MIN_WINDOW`], the battery's energy minus what
//! RAPL measured is the display, Wi-Fi, storage and conversion losses: `devices`.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::attribution::{Cause, Ledger, TickAttribution, credit, share};
use crate::consumer::ConsumerKey;
use crate::delta::BatteryDelta;
use crate::units::Joules;

/// Shortest window over which battery energy is compared with RAPL.
pub const MIN_WINDOW: Duration = Duration::from_secs(10);

/// What a closed window's total was measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measurement {
    /// Discharging, with RAPL: the battery is the ground truth.
    Battery,
    /// On AC (or mixed): only RAPL energy is known.
    Rapl,
    /// Discharging without RAPL (sampler unavailable): battery energy split by CPU time.
    BatteryOnly,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Window {
    duration: Duration,
    ledger: Ledger,
    rapl: Joules,
    battery: BatteryState,
    cpu_usec: BTreeMap<ConsumerKey, u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum BatteryState {
    #[default]
    Empty,
    Discharging(Joules),
    /// At least one tick wasn't discharging: no battery truth for this window.
    Mixed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosedWindow {
    pub duration: Duration,
    pub ledger: Ledger,
    /// The window's total energy; equals the sum of the ledger.
    pub measured: Joules,
    pub measurement: Measurement,
    /// RAPL energy in excess of the battery's (measurement noise), not attributed as devices.
    pub shortfall: Joules,
}

impl Window {
    pub fn push(&mut self, tick: TickAttribution, battery: Option<BatteryDelta>) {
        self.duration += tick.duration;
        self.rapl += tick.measured;
        for (key, split) in &tick.ledger {
            self.ledger.entry(key.clone()).or_default().merge(split);
        }
        for (key, usec) in tick.activity.cpu_usec {
            *self.cpu_usec.entry(key).or_default() += usec;
        }
        let tick_battery = battery.map(|b| b.mean_power.for_duration(tick.duration));
        self.battery = match (self.battery, tick_battery) {
            (BatteryState::Empty, Some(energy)) => BatteryState::Discharging(energy),
            (BatteryState::Discharging(sum), Some(energy)) => {
                BatteryState::Discharging(sum + energy)
            }
            _ => BatteryState::Mixed,
        };
    }

    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.duration >= MIN_WINDOW
    }

    #[must_use]
    pub fn close(self) -> ClosedWindow {
        let Self {
            duration,
            mut ledger,
            rapl,
            battery,
            cpu_usec,
        } = self;
        let mut shortfall = Joules::ZERO;
        let (measured, measurement) = match battery {
            BatteryState::Discharging(battery) if rapl.0 > 0.0 => {
                let devices = battery - rapl;
                if devices.0 >= 0.0 {
                    credit(&mut ledger, ConsumerKey::Devices, Cause::Other, devices);
                    (battery, Measurement::Battery)
                } else {
                    shortfall = Joules(-devices.0);
                    (rapl, Measurement::Battery)
                }
            }
            BatteryState::Discharging(battery) => {
                share(&mut ledger, battery, &cpu_usec, Cause::Cpu);
                (battery.non_negative(), Measurement::BatteryOnly)
            }
            BatteryState::Empty | BatteryState::Mixed => (rapl, Measurement::Rapl),
        };
        ClosedWindow {
            duration,
            ledger,
            measured,
            measurement,
            shortfall,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::attribution::EnergySplit;
    use crate::units::Watts;
    use proptest::prelude::*;

    fn tick(rapl_joules: f64, cpu: &[(&str, u64)]) -> TickAttribution {
        let mut ledger = Ledger::new();
        if rapl_joules > 0.0 {
            ledger.insert(
                ConsumerKey::Idle,
                EnergySplit {
                    cpu: Joules(rapl_joules),
                    ..EnergySplit::default()
                },
            );
        }
        TickAttribution {
            duration: Duration::from_secs(2),
            ledger,
            measured: Joules(rapl_joules),
            activity: Activity {
                cpu_usec: cpu
                    .iter()
                    .map(|(k, v)| (ConsumerKey::App((*k).into()), *v))
                    .collect(),
                gpu_ns: BTreeMap::new(),
            },
        }
    }

    fn discharging(watts: f64) -> BatteryDelta {
        BatteryDelta {
            mean_power: Watts(watts),
            energy_drop: None,
        }
    }

    fn sum(ledger: &Ledger) -> f64 {
        ledger.values().map(|s| s.total().0).sum()
    }

    #[test]
    fn battery_beyond_rapl_is_devices() {
        let mut window = Window::default();
        for _ in 0..5 {
            window.push(tick(4.0, &[]), Some(discharging(5.5))); // 11 J battery, 4 J RAPL per tick
        }
        assert!(window.is_ready());
        let closed = window.close();
        assert_eq!(closed.measurement, Measurement::Battery);
        assert!((closed.ledger[&ConsumerKey::Devices].other.0 - 35.0).abs() < 1e-9);
        assert!((closed.measured.0 - 55.0).abs() < 1e-9);
        assert!((sum(&closed.ledger) - closed.measured.0).abs() < 1e-9);
    }

    #[test]
    fn rapl_above_battery_is_a_shortfall_not_negative_devices() {
        let mut window = Window::default();
        window.push(tick(12.0, &[]), Some(discharging(5.0)));
        let closed = window.close();
        assert!(!closed.ledger.contains_key(&ConsumerKey::Devices));
        assert!((closed.shortfall.0 - 2.0).abs() < 1e-9);
        assert!((closed.measured.0 - 12.0).abs() < 1e-9);
    }

    #[test]
    fn plugging_in_mid_window_drops_battery_truth() {
        let mut window = Window::default();
        window.push(tick(4.0, &[]), Some(discharging(6.0)));
        window.push(tick(4.0, &[]), None);
        let closed = window.close();
        assert_eq!(closed.measurement, Measurement::Rapl);
        assert!((closed.measured.0 - 8.0).abs() < 1e-9);
    }

    #[test]
    fn without_rapl_battery_energy_follows_cpu_time() {
        let mut window = Window::default();
        window.push(tick(0.0, &[("a", 300), ("b", 100)]), Some(discharging(4.0)));
        let closed = window.close();
        assert_eq!(closed.measurement, Measurement::BatteryOnly);
        assert!((closed.ledger[&ConsumerKey::App("a".into())].cpu.0 - 6.0).abs() < 1e-9);
        assert!((closed.ledger[&ConsumerKey::App("b".into())].cpu.0 - 2.0).abs() < 1e-9);
    }

    proptest! {
        #[test]
        fn closed_windows_conserve_energy(
            ticks in proptest::collection::vec((0.0f64..20.0, proptest::option::of(0.0f64..15.0)), 1..12),
        ) {
            let mut window = Window::default();
            for (rapl, watts) in &ticks {
                window.push(tick(*rapl, &[("a", 1)]), watts.map(discharging));
            }
            let closed = window.close();
            prop_assert!((sum(&closed.ledger) - closed.measured.0).abs() <= 1e-9 * closed.measured.0.max(1.0));
            prop_assert!(closed.shortfall.0 >= 0.0);
        }
    }
}

//! Plug/unplug transitions and sleep sessions, from battery readings.

use drainscope_model::snapshot::{BatteryReading, BatteryStatus};
use drainscope_model::{Joules, WakeupIrq, WakeupSources, wake_reason};
use drainscope_store::{PowerEvent, PowerEventKind, SleepSession};

/// Combined state of all batteries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryLevel {
    pub on_battery: bool,
    pub energy: Option<Joules>,
    pub capacity: Option<Joules>,
}

impl BatteryLevel {
    #[must_use]
    pub fn of(batteries: &[BatteryReading]) -> Self {
        let sum = |field: fn(&BatteryReading) -> Option<Joules>| {
            batteries
                .iter()
                .map(field)
                .try_fold(Joules::ZERO, |total, j| Some(total + j?))
                .filter(|_| !batteries.is_empty())
        };
        Self {
            on_battery: batteries
                .iter()
                .any(|b| b.status == BatteryStatus::Discharging),
            energy: sum(|b| b.energy),
            capacity: sum(|b| b.energy_full),
        }
    }

    #[must_use]
    pub fn percent(&self) -> Option<f64> {
        match (self.energy, self.capacity) {
            (Some(energy), Some(capacity)) if capacity.0 > 0.0 => {
                Some(energy.0 / capacity.0 * 100.0)
            }
            _ => None,
        }
    }
}

/// Emits plug and unplug events when the machine switches power source.
#[derive(Debug, Default)]
pub struct PowerTracker {
    on_battery: Option<bool>,
}

impl PowerTracker {
    /// The first call only records the state; later calls report transitions.
    pub fn update(&mut self, level: &BatteryLevel, wall_ms: i64) -> Option<PowerEvent> {
        let previous = self.on_battery.replace(level.on_battery)?;
        if previous == level.on_battery {
            return None;
        }
        Some(PowerEvent {
            ts_ms: wall_ms,
            kind: if level.on_battery {
                PowerEventKind::Unplug
            } else {
                PowerEventKind::Plug
            },
            battery_percent: level.percent(),
            energy_wh: level.energy.map(|j| j.0 / 3600.0),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct BeforeSleep {
    wall_ms: i64,
    level: BatteryLevel,
    mem_sleep: Option<String>,
    wakeups: WakeupSources,
}

/// Pairs the readings taken just before suspend and just after resume.
#[derive(Debug, Default)]
pub struct SleepTracker {
    before: Option<BeforeSleep>,
}

impl SleepTracker {
    pub fn before_sleep(
        &mut self,
        wall_ms: i64,
        level: BatteryLevel,
        mem_sleep: Option<String>,
        wakeups: WakeupSources,
    ) {
        self.before = Some(BeforeSleep {
            wall_ms,
            level,
            mem_sleep,
            wakeups,
        });
    }

    /// The finished session, if a matching "before" reading exists.
    pub fn after_resume(
        &mut self,
        wall_ms: i64,
        level: BatteryLevel,
        wakeups: &WakeupSources,
        irq: Option<&WakeupIrq>,
    ) -> Option<SleepSession> {
        let before = self.before.take()?;
        let wh_lost = match (before.level.energy, level.energy) {
            (Some(was), Some(now)) => Some((was.0 - now.0) / 3600.0),
            _ => None,
        };
        let percent_lost = match (before.level.percent(), level.percent()) {
            (Some(was), Some(now)) => Some(was - now),
            _ => None,
        };
        Some(SleepSession {
            start_ms: before.wall_ms,
            end_ms: wall_ms.max(before.wall_ms),
            wh_lost,
            percent_lost,
            wake_reason: wake_reason(&before.wakeups, wakeups, irq, before.mem_sleep.as_deref()),
            mem_sleep: before.mem_sleep,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(name: &str, status: BatteryStatus, wh: f64, full_wh: f64) -> BatteryReading {
        BatteryReading {
            name: name.into(),
            status,
            power: None,
            energy: Some(Joules(wh * 3600.0)),
            energy_full: Some(Joules(full_wh * 3600.0)),
            energy_full_design: None,
            cycle_count: None,
        }
    }

    #[test]
    fn levels_sum_batteries() {
        let level = BatteryLevel::of(&[
            battery("BAT0", BatteryStatus::NotCharging, 25.0, 30.0),
            battery("BAT1", BatteryStatus::Discharging, 5.0, 10.0),
        ]);
        assert!(level.on_battery);
        assert!((level.percent().unwrap() - 75.0).abs() < 1e-9);
        assert_eq!(BatteryLevel::of(&[]).percent(), None);
    }

    #[test]
    fn reports_transitions_only() {
        let charging = BatteryLevel::of(&[battery("BAT0", BatteryStatus::Charging, 20.0, 40.0)]);
        let discharging =
            BatteryLevel::of(&[battery("BAT0", BatteryStatus::Discharging, 20.0, 40.0)]);
        let mut tracker = PowerTracker::default();
        assert_eq!(tracker.update(&charging, 1), None);
        assert_eq!(tracker.update(&charging, 2), None);
        let unplug = tracker.update(&discharging, 3).unwrap();
        assert_eq!(unplug.kind, PowerEventKind::Unplug);
        assert_eq!(unplug.battery_percent, Some(50.0));
        assert_eq!(unplug.energy_wh, Some(20.0));
        assert_eq!(
            tracker.update(&charging, 4).unwrap().kind,
            PowerEventKind::Plug
        );
    }

    #[test]
    fn sleep_sessions_measure_what_was_lost() {
        let before = BatteryLevel::of(&[battery("BAT0", BatteryStatus::Discharging, 30.0, 40.0)]);
        let after = BatteryLevel::of(&[battery("BAT0", BatteryStatus::Discharging, 29.2, 40.0)]);
        let mut tracker = SleepTracker::default();
        let lid = |count| {
            WakeupSources::from([(
                "wakeup1".to_owned(),
                drainscope_model::WakeupSource {
                    name: "PNP0C0D:00".into(),
                    wakeup_count: count,
                },
            )])
        };
        assert_eq!(tracker.after_resume(5, after, &lid(0), None), None);
        tracker.before_sleep(1_000, before, Some("deep".into()), lid(0));
        let session = tracker
            .after_resume(3_601_000, after, &lid(1), None)
            .unwrap();
        assert_eq!(session.wake_reason.as_deref(), Some("Lid (PNP0C0D:00)"));
        assert_eq!((session.start_ms, session.end_ms), (1_000, 3_601_000));
        assert!((session.wh_lost.unwrap() - 0.8).abs() < 1e-9);
        assert!((session.percent_lost.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(session.mem_sleep.as_deref(), Some("deep"));
    }
}

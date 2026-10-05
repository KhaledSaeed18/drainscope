//! Battery health history, kept indefinitely (one small row per battery per day).

use drainscope_model::{BatteryHealth, Joules};
use rusqlite::params;

use crate::{Store, StoreError};

const JOULES_PER_WH: f64 = 3600.0;
const MS_PER_DAY: i64 = 86_400_000;

#[derive(Debug, Clone, PartialEq)]
pub struct HealthRecord {
    pub ts_ms: i64,
    pub health: BatteryHealth,
}

impl Store {
    /// Records the batteries' health at `ts_ms`, replacing earlier readings from the same
    /// (UTC) day.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn record_battery_health(
        &mut self,
        ts_ms: i64,
        batteries: &[BatteryHealth],
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO battery_health
                     (battery, day, ts_ms, energy_full_wh, energy_full_design_wh, cycle_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for battery in batteries {
                insert.execute(params![
                    battery.battery,
                    ts_ms.div_euclid(MS_PER_DAY),
                    ts_ms,
                    battery.energy_full.0 / JOULES_PER_WH,
                    battery.energy_full_design.map(|j| j.0 / JOULES_PER_WH),
                    battery.cycle_count,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Daily readings since `since_ms`, oldest first, then by battery.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn battery_health(&self, since_ms: i64) -> Result<Vec<HealthRecord>, StoreError> {
        let mut query = self.conn.prepare_cached(
            "SELECT ts_ms, battery, energy_full_wh, energy_full_design_wh, cycle_count
             FROM battery_health WHERE ts_ms >= ?1 ORDER BY day, battery",
        )?;
        let rows = query.query_map(params![since_ms], |row| {
            let full: f64 = row.get(2)?;
            let design: Option<f64> = row.get(3)?;
            Ok(HealthRecord {
                ts_ms: row.get(0)?,
                health: BatteryHealth {
                    battery: row.get(1)?,
                    energy_full: Joules(full * JOULES_PER_WH),
                    energy_full_design: design.map(|wh| Joules(wh * JOULES_PER_WH)),
                    cycle_count: row.get(4)?,
                },
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(name: &str, full_wh: f64, cycles: Option<u32>) -> BatteryHealth {
        BatteryHealth {
            battery: name.to_owned(),
            energy_full: Joules(full_wh * JOULES_PER_WH),
            energy_full_design: Some(Joules(40.0 * JOULES_PER_WH)),
            cycle_count: cycles,
        }
    }

    #[test]
    fn keeps_the_last_reading_of_each_day() {
        let mut store = Store::open_in_memory().unwrap();
        let day = MS_PER_DAY * 20_000;
        store
            .record_battery_health(
                day + 1_000,
                &[battery("BAT0", 31.0, Some(10)), battery("BAT1", 30.0, None)],
            )
            .unwrap();
        store
            .record_battery_health(day + 60_000, &[battery("BAT0", 30.9, Some(11))])
            .unwrap();
        store
            .record_battery_health(day + MS_PER_DAY, &[battery("BAT0", 30.8, Some(12))])
            .unwrap();

        let history = store.battery_health(0).unwrap();
        let summary: Vec<(i64, &str, Option<u32>)> = history
            .iter()
            .map(|r| (r.ts_ms, r.health.battery.as_str(), r.health.cycle_count))
            .collect();
        assert_eq!(
            summary,
            [
                (day + 60_000, "BAT0", Some(11)),
                (day + 1_000, "BAT1", None),
                (day + MS_PER_DAY, "BAT0", Some(12)),
            ]
        );
        assert!((history[0].health.energy_full.0 - 30.9 * JOULES_PER_WH).abs() < 1e-6);
        assert_eq!(store.battery_health(day + MS_PER_DAY).unwrap().len(), 1);
    }
}

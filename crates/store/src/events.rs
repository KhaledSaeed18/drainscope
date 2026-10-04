//! Power events (plug, unplug, suspend, …) and sleep sessions.

use rusqlite::{OptionalExtension, params};

use crate::{Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEventKind {
    Boot,
    Plug,
    Unplug,
    Suspend,
    Resume,
}

impl PowerEventKind {
    const ALL: [Self; 5] = [
        Self::Boot,
        Self::Plug,
        Self::Unplug,
        Self::Suspend,
        Self::Resume,
    ];

    fn wire_name(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Plug => "plug",
            Self::Unplug => "unplug",
            Self::Suspend => "suspend",
            Self::Resume => "resume",
        }
    }

    fn from_wire_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.wire_name() == name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PowerEvent {
    pub ts_ms: i64,
    pub kind: PowerEventKind,
    pub battery_percent: Option<f64>,
    pub energy_wh: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SleepSession {
    pub start_ms: i64,
    pub end_ms: i64,
    pub wh_lost: Option<f64>,
    pub percent_lost: Option<f64>,
    /// `/sys/power/mem_sleep` mode, e.g. `deep` or `s2idle`.
    pub mem_sleep: Option<String>,
    pub wake_reason: Option<String>,
}

impl Store {
    /// # Errors
    /// SQLite errors.
    pub fn record_power_event(&self, event: &PowerEvent) -> Result<(), StoreError> {
        self.conn
            .prepare_cached(
                "INSERT INTO power_events (ts_ms, kind, battery_percent, energy_wh) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                event.ts_ms,
                event.kind.wire_name(),
                event.battery_percent,
                event.energy_wh
            ])?;
        Ok(())
    }

    /// The most recent event of `kind`, e.g. the last unplug for "since unplugged".
    ///
    /// # Errors
    /// SQLite errors.
    pub fn last_power_event(&self, kind: PowerEventKind) -> Result<Option<PowerEvent>, StoreError> {
        let row = self
            .conn
            .prepare_cached(
                "SELECT ts_ms, kind, battery_percent, energy_wh FROM power_events
                 WHERE kind = ?1 ORDER BY ts_ms DESC, id DESC LIMIT 1",
            )?
            .query_row(params![kind.wire_name()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })
            .optional()?;
        row.map(|(ts_ms, kind, battery_percent, energy_wh)| {
            let kind = PowerEventKind::from_wire_name(&kind)
                .ok_or_else(|| StoreError::Corrupt(format!("power event kind {kind:?}")))?;
            Ok(PowerEvent {
                ts_ms,
                kind,
                battery_percent,
                energy_wh,
            })
        })
        .transpose()
    }

    /// # Errors
    /// SQLite errors, e.g. a session ending before it started.
    pub fn record_sleep_session(&self, session: &SleepSession) -> Result<(), StoreError> {
        self.conn
            .prepare_cached(
                "INSERT INTO sleep_sessions (start_ms, end_ms, wh_lost, percent_lost, mem_sleep, wake_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                session.start_ms,
                session.end_ms,
                session.wh_lost,
                session.percent_lost,
                session.mem_sleep,
                session.wake_reason
            ])?;
        Ok(())
    }

    /// Sessions that started at or after `since_ms`, oldest first.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn sleep_sessions(&self, since_ms: i64) -> Result<Vec<SleepSession>, StoreError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT start_ms, end_ms, wh_lost, percent_lost, mem_sleep, wake_reason
             FROM sleep_sessions WHERE start_ms >= ?1 ORDER BY start_ms",
        )?;
        let sessions = statement
            .query_map(params![since_ms], |row| {
                Ok(SleepSession {
                    start_ms: row.get(0)?,
                    end_ms: row.get(1)?,
                    wh_lost: row.get(2)?,
                    percent_lost: row.get(3)?,
                    mem_sleep: row.get(4)?,
                    wake_reason: row.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_last_event_of_a_kind() {
        let store = Store::open_in_memory().unwrap();
        let event = |ts_ms, kind, percent| PowerEvent {
            ts_ms,
            kind,
            battery_percent: Some(percent),
            energy_wh: None,
        };
        store
            .record_power_event(&event(100, PowerEventKind::Unplug, 90.0))
            .unwrap();
        store
            .record_power_event(&event(200, PowerEventKind::Plug, 70.0))
            .unwrap();
        store
            .record_power_event(&event(300, PowerEventKind::Unplug, 95.0))
            .unwrap();
        assert_eq!(
            store.last_power_event(PowerEventKind::Unplug).unwrap(),
            Some(event(300, PowerEventKind::Unplug, 95.0))
        );
        assert_eq!(
            store.last_power_event(PowerEventKind::Suspend).unwrap(),
            None
        );
    }

    #[test]
    fn stores_sleep_sessions_in_order() {
        let store = Store::open_in_memory().unwrap();
        let session = |start_ms| SleepSession {
            start_ms,
            end_ms: start_ms + 3_600_000,
            wh_lost: Some(0.4),
            percent_lost: Some(1.0),
            mem_sleep: Some("deep".into()),
            wake_reason: None,
        };
        store.record_sleep_session(&session(5_000)).unwrap();
        store.record_sleep_session(&session(1_000)).unwrap();
        let starts: Vec<i64> = store
            .sleep_sessions(0)
            .unwrap()
            .iter()
            .map(|s| s.start_ms)
            .collect();
        assert_eq!(starts, [1_000, 5_000]);
        assert_eq!(store.sleep_sessions(2_000).unwrap(), [session(5_000)]);

        let backwards = SleepSession {
            end_ms: 0,
            ..session(10)
        };
        assert!(store.record_sleep_session(&backwards).is_err());
    }
}

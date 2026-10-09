//! Foreground and background energy per app (ADR 0011).
//!
//! The Shell extension reports which app has focus; [`FocusLog`] keeps those changes on the
//! `CLOCK_MONOTONIC` timeline, and [`split`] divides each app's energy in a tick into the part
//! spent while one of its windows had focus (foreground), the part while focus wasn't known,
//! and the rest (background). Attribution itself is unchanged: this only classifies energy
//! already charged to an app.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use crate::attribution::Ledger;
use crate::consumer::ConsumerKey;

/// What has focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusState {
    /// Not reported: no extension, it was turned off, or the Shell restarted.
    Unknown,
    /// Nothing: the desktop, the Overview or the lock screen.
    Nothing,
    /// An app, by desktop ID without `.desktop` (`org.mozilla.firefox`), as in [`ConsumerKey::App`].
    App(String),
}

/// How long each app had focus in an interval, and how long focus was unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusSpans {
    pub focused: BTreeMap<String, Duration>,
    pub unknown: Duration,
}

impl FocusSpans {
    fn add(&mut self, state: &FocusState, time: Duration) {
        if time.is_zero() {
            return;
        }
        match state {
            FocusState::Unknown => self.unknown += time,
            FocusState::Nothing => {}
            FocusState::App(id) => *self.focused.entry(id.clone()).or_default() += time,
        }
    }
}

/// Focus changes on the monotonic timeline. Starts [`FocusState::Unknown`].
#[derive(Debug, Clone)]
pub struct FocusLog {
    /// (when, state from then on), oldest first; the first entry covers everything before it.
    changes: VecDeque<(Duration, FocusState)>,
}

impl Default for FocusLog {
    fn default() -> Self {
        Self {
            changes: VecDeque::from([(Duration::ZERO, FocusState::Unknown)]),
        }
    }
}

impl FocusLog {
    /// Records that focus became `state` at `at`. Changes arrive in order; one dated before
    /// the latest is taken as happening at the latest's time.
    pub fn set(&mut self, at: Duration, state: FocusState) {
        let latest = self.changes.back().map_or(Duration::ZERO, |(t, _)| *t);
        if self.changes.back().is_some_and(|(_, s)| *s == state) {
            return;
        }
        self.changes.push_back((at.max(latest), state));
    }

    /// The state now.
    #[must_use]
    pub fn current(&self) -> &FocusState {
        self.changes.back().map_or(&FocusState::Unknown, |(_, s)| s)
    }

    /// Time per app with focus, and time unknown, in `[from, to)`.
    #[must_use]
    pub fn spans(&self, from: Duration, to: Duration) -> FocusSpans {
        let mut spans = FocusSpans::default();
        if to <= from {
            return spans;
        }
        // Before the first kept change, its state applies too.
        if let Some((first, state)) = self.changes.front() {
            spans.add(state, (*first).min(to).saturating_sub(from));
        }
        for (i, (start, state)) in self.changes.iter().enumerate() {
            let end = self.changes.get(i + 1).map_or(to, |(t, _)| *t);
            spans.add(state, end.min(to).saturating_sub((*start).max(from)));
        }
        spans
    }

    /// Forgets changes no longer needed for intervals starting at or after `before`, keeping
    /// the state in force at `before`.
    pub fn forget_before(&mut self, before: Duration) {
        while self.changes.len() > 1 && self.changes.get(1).is_some_and(|(t, _)| *t <= before) {
            self.changes.pop_front();
        }
    }
}

/// One app's energy in a tick or window, by focus.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FocusSplit {
    /// Joules while one of the app's windows had focus.
    pub foreground_j: f64,
    /// Joules while focus wasn't known.
    pub unknown_j: f64,
    /// How long the app had focus.
    pub focused: Duration,
}

impl FocusSplit {
    pub fn add(&mut self, other: &Self) {
        self.foreground_j += other.foreground_j;
        self.unknown_j += other.unknown_j;
        self.focused += other.focused;
    }
}

/// Splits each app's energy in a tick of length `duration` by focus, assuming energy evenly
/// spread over the tick. Apps that had focus but no energy still get their focused time.
#[must_use]
pub fn split(
    ledger: &Ledger,
    spans: &FocusSpans,
    duration: Duration,
) -> BTreeMap<ConsumerKey, FocusSplit> {
    let seconds = duration.as_secs_f64();
    let mut out = BTreeMap::new();
    if seconds <= 0.0 {
        return out;
    }
    let unknown = (spans.unknown.as_secs_f64() / seconds).min(1.0);
    for (key, energy) in ledger {
        let ConsumerKey::App(id) = key else {
            continue;
        };
        let focused = spans.focused.get(id).copied().unwrap_or_default();
        let joules = energy.total().0;
        out.insert(
            key.clone(),
            FocusSplit {
                foreground_j: joules * (focused.as_secs_f64() / seconds).min(1.0),
                unknown_j: joules * unknown,
                focused,
            },
        );
    }
    for (id, focused) in &spans.focused {
        out.entry(ConsumerKey::App(id.clone()))
            .or_insert(FocusSplit {
                focused: *focused,
                ..FocusSplit::default()
            });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::EnergySplit;
    use crate::units::Joules;

    const S: Duration = Duration::from_secs(1);

    fn app(id: &str) -> FocusState {
        FocusState::App(id.into())
    }

    #[test]
    fn starts_unknown() {
        let log = FocusLog::default();
        assert_eq!(log.current(), &FocusState::Unknown);
        let spans = log.spans(10 * S, 15 * S);
        assert_eq!(spans.unknown, 5 * S);
        assert!(spans.focused.is_empty());
    }

    #[test]
    fn spans_follow_changes_inside_the_interval() {
        let mut log = FocusLog::default();
        log.set(100 * S, app("firefox"));
        log.set(103 * S, app("editor"));
        log.set(104 * S, FocusState::Nothing);
        log.set(105 * S, app("firefox"));
        let spans = log.spans(101 * S, 106 * S);
        assert_eq!(spans.focused["firefox"], 3 * S); // 101–103, 105–106
        assert_eq!(spans.focused["editor"], S);
        assert_eq!(spans.unknown, Duration::ZERO);
        // Before the first report, focus was unknown.
        assert_eq!(log.spans(98 * S, 101 * S).unknown, 2 * S);
    }

    #[test]
    fn repeated_states_and_late_reports_are_harmless() {
        let mut log = FocusLog::default();
        log.set(10 * S, app("firefox"));
        log.set(12 * S, app("firefox"));
        log.set(9 * S, FocusState::Nothing); // late: counted from 10 s
        assert_eq!(log.spans(10 * S, 14 * S).focused.get("firefox"), None);
    }

    #[test]
    fn forgetting_keeps_the_state_in_force() {
        let mut log = FocusLog::default();
        log.set(10 * S, app("firefox"));
        log.set(20 * S, app("editor"));
        log.forget_before(25 * S);
        assert_eq!(log.spans(25 * S, 30 * S).focused["editor"], 5 * S);
        log.forget_before(15 * S);
        assert_eq!(log.current(), &app("editor"));
    }

    #[test]
    fn splits_app_energy_by_focused_time() {
        let mut log = FocusLog::default();
        log.set(Duration::ZERO, app("org.mozilla.firefox"));
        log.set(3 * S, FocusState::Unknown);
        log.set(4 * S, app("org.gnome.TextEditor"));
        let spans = log.spans(Duration::ZERO, 5 * S);
        let joules = |j: f64| EnergySplit {
            cpu: Joules(j),
            ..EnergySplit::default()
        };
        let ledger = Ledger::from([
            (ConsumerKey::App("org.mozilla.firefox".into()), joules(10.0)),
            (ConsumerKey::App("org.gnome.Software".into()), joules(5.0)),
            (ConsumerKey::Kernel, joules(7.0)),
        ]);
        let out = split(&ledger, &spans, 5 * S);
        let firefox = out[&ConsumerKey::App("org.mozilla.firefox".into())];
        assert!((firefox.foreground_j - 6.0).abs() < 1e-9); // 3 of 5 s
        assert!((firefox.unknown_j - 2.0).abs() < 1e-9); // 1 of 5 s unknown
        let software = out[&ConsumerKey::App("org.gnome.Software".into())];
        assert!(software.foreground_j.abs() < 1e-12 && (software.unknown_j - 1.0).abs() < 1e-9);
        // Focused without energy: time only.
        let editor = out[&ConsumerKey::App("org.gnome.TextEditor".into())];
        assert_eq!((editor.foreground_j, editor.focused), (0.0, S));
        assert!(!out.contains_key(&ConsumerKey::Kernel));
    }
}

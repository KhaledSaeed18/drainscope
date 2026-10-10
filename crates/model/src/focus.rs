//! Foreground and background energy per app (ADR 0011).
//!
//! The Shell extension reports which app has focus; [`FocusLog`] keeps those changes on the
//! `CLOCK_MONOTONIC` timeline, and [`FocusWindow`] divides each app's energy in a window into the part
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

/// How much of an app's activity happened with focus, and with focus unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Shares {
    total: f64,
    foreground: f64,
    unknown: f64,
}

impl Shares {
    fn add(&mut self, weight: f64, focused: f64, unknown: f64) {
        self.total += weight;
        self.foreground += weight * focused;
        self.unknown += weight * unknown;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct AppFocus {
    /// Weighted by the app's energy in each tick.
    energy: Shares,
    /// Weighted by its CPU time in each tick: how a window without RAPL shares the battery's
    /// energy out when it closes.
    cpu: Shares,
    focused: Duration,
}

/// Focus over a window, tick by tick, applied to apps' energy once the window closes.
///
/// A window's final energy per app isn't always the sum of its ticks: without RAPL, the battery's
/// energy is shared out by CPU time at close. So each tick records what fraction of each app's
/// activity (its energy, and its CPU time) fell while it had focus or while focus was unknown,
/// and [`Self::finish`] applies those fractions to the final energy. Energy within a tick is
/// assumed even over it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FocusWindow {
    apps: BTreeMap<String, AppFocus>,
    duration: Duration,
    unknown: Duration,
}

impl FocusWindow {
    /// Adds one tick of length `duration`: its energy ledger, CPU time by consumer, and focus.
    pub fn add_tick(
        &mut self,
        ledger: &Ledger,
        cpu_usec: &BTreeMap<ConsumerKey, u64>,
        spans: &FocusSpans,
        duration: Duration,
    ) {
        let seconds = duration.as_secs_f64();
        if seconds <= 0.0 {
            return;
        }
        let unknown = (spans.unknown.as_secs_f64() / seconds).min(1.0);
        let focused = |id: &str| {
            spans
                .focused
                .get(id)
                .map_or(0.0, |d| (d.as_secs_f64() / seconds).min(1.0))
        };
        for (key, energy) in ledger {
            if let ConsumerKey::App(id) = key {
                let shares = &mut self.apps.entry(id.clone()).or_default().energy;
                shares.add(energy.total().0, focused(id), unknown);
            }
        }
        for (key, usec) in cpu_usec {
            if let ConsumerKey::App(id) = key {
                #[allow(clippy::cast_precision_loss)] // CPU microseconds per tick, far below 2^52
                let usec = *usec as f64;
                let shares = &mut self.apps.entry(id.clone()).or_default().cpu;
                shares.add(usec, focused(id), unknown);
            }
        }
        for (id, time) in &spans.focused {
            self.apps.entry(id.clone()).or_default().focused += *time;
        }
        self.duration += duration;
        self.unknown += spans.unknown.min(duration);
    }

    /// Each app's energy in the closed window's `ledger`, split by focus, plus the apps that
    /// had focus without using energy (time only).
    #[must_use]
    pub fn finish(self, ledger: &Ledger) -> BTreeMap<ConsumerKey, FocusSplit> {
        let window = self.duration.as_secs_f64();
        let mut out = BTreeMap::new();
        for (key, energy) in ledger {
            let ConsumerKey::App(id) = key else {
                continue;
            };
            let app = self.apps.get(id).cloned().unwrap_or_default();
            // The weights that match how the energy was assigned; by time if there were none.
            let (focused, unknown) = [app.energy, app.cpu]
                .into_iter()
                .find(|shares| shares.total > 0.0)
                .map_or_else(
                    || {
                        if window > 0.0 {
                            (
                                app.focused.as_secs_f64() / window,
                                self.unknown.as_secs_f64() / window,
                            )
                        } else {
                            (0.0, 1.0)
                        }
                    },
                    |shares| {
                        (
                            shares.foreground / shares.total,
                            shares.unknown / shares.total,
                        )
                    },
                );
            let joules = energy.total().0;
            out.insert(
                key.clone(),
                FocusSplit {
                    foreground_j: joules * focused.clamp(0.0, 1.0),
                    unknown_j: joules * unknown.clamp(0.0, 1.0),
                    focused: app.focused,
                },
            );
        }
        for (id, app) in self.apps {
            if !app.focused.is_zero() {
                out.entry(ConsumerKey::App(id)).or_insert(FocusSplit {
                    focused: app.focused,
                    ..FocusSplit::default()
                });
            }
        }
        out
    }
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

    fn joules(j: f64) -> EnergySplit {
        EnergySplit {
            cpu: Joules(j),
            ..EnergySplit::default()
        }
    }

    #[test]
    fn splits_app_energy_by_focused_time() {
        let mut log = FocusLog::default();
        log.set(Duration::ZERO, app("org.mozilla.firefox"));
        log.set(3 * S, FocusState::Unknown);
        log.set(4 * S, app("org.gnome.TextEditor"));
        let ledger = Ledger::from([
            (ConsumerKey::App("org.mozilla.firefox".into()), joules(10.0)),
            (ConsumerKey::App("org.gnome.Software".into()), joules(5.0)),
            (ConsumerKey::Kernel, joules(7.0)),
        ]);
        let mut window = FocusWindow::default();
        window.add_tick(
            &ledger,
            &BTreeMap::new(),
            &log.spans(Duration::ZERO, 5 * S),
            5 * S,
        );
        let out = window.finish(&ledger);
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

    #[test]
    fn energy_assigned_at_close_follows_cpu_time() {
        // Without RAPL, ticks carry no energy; the window's battery energy is shared by CPU
        // time at close. Firefox ran 3 s with focus and 1 s without.
        let firefox = ConsumerKey::App("org.mozilla.firefox".into());
        let mut log = FocusLog::default();
        log.set(Duration::ZERO, app("org.mozilla.firefox"));
        log.set(5 * S, FocusState::Nothing);
        let mut window = FocusWindow::default();
        let cpu = |usec| BTreeMap::from([(firefox.clone(), usec)]);
        window.add_tick(
            &Ledger::new(),
            &cpu(3_000_000),
            &log.spans(Duration::ZERO, 5 * S),
            5 * S,
        );
        window.add_tick(
            &Ledger::new(),
            &cpu(1_000_000),
            &log.spans(5 * S, 10 * S),
            5 * S,
        );
        let closed = Ledger::from([(firefox.clone(), joules(40.0))]);
        let out = window.finish(&closed);
        assert!(
            (out[&firefox].foreground_j - 30.0).abs() < 1e-9,
            "{:?}",
            out[&firefox]
        );
        assert!(out[&firefox].unknown_j.abs() < 1e-12);
        assert_eq!(out[&firefox].focused, 5 * S);
    }

    proptest::proptest! {
        /// Foreground and unknown never exceed an app's energy, whatever the weights.
        #[test]
        fn splits_stay_within_the_energy(
            ticks in proptest::collection::vec((0.0f64..20.0, 0u64..5_000_000, 0u64..6, 0u64..6), 1..8),
            closed_j in 0.0f64..200.0,
        ) {
            let firefox = ConsumerKey::App("org.mozilla.firefox".into());
            let mut window = FocusWindow::default();
            for (energy, usec, focused_s, unknown_s) in ticks {
                let mut spans = FocusSpans::default();
                spans.focused.insert("org.mozilla.firefox".into(), Duration::from_secs(focused_s.min(5)));
                spans.unknown = Duration::from_secs(unknown_s.min(5 - focused_s.min(5)));
                window.add_tick(
                    &Ledger::from([(firefox.clone(), joules(energy))]),
                    &BTreeMap::from([(firefox.clone(), usec)]),
                    &spans,
                    5 * S,
                );
            }
            let out = window.finish(&Ledger::from([(firefox.clone(), joules(closed_j))]));
            let split = out[&firefox];
            proptest::prop_assert!(split.foreground_j >= 0.0 && split.unknown_j >= 0.0);
            proptest::prop_assert!(split.foreground_j + split.unknown_j <= closed_j + 1e-9);
        }
    }
}

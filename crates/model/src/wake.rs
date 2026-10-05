//! Why the machine woke from suspend, from wakeup-source counters and the wakeup IRQ.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeupSource {
    /// e.g. `PNP0C0D:00` (the lid) or `rtc0`.
    pub name: String,
    /// Times this source woke the machine or aborted a suspend.
    pub wakeup_count: u64,
}

/// Wakeup sources by sysfs entry (`wakeup0`, …); names alone aren't unique.
pub type WakeupSources = BTreeMap<String, WakeupSource>;

/// `/sys/power/pm_wakeup_irq`, named from `/proc/interrupts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeupIrq {
    pub irq: u32,
    /// The IRQ's handlers, e.g. `acpi` or `i8042`.
    pub action: String,
}

/// Readable names for well-known wakeup sources and IRQ handlers.
fn label(name: &str) -> Option<&'static str> {
    let known: &[(&str, &str)] = &[
        ("PNP0C0D", "Lid"),
        ("PNP0C0C", "Power button"),
        ("PNP0C0E", "Sleep button"),
        ("ACPI0003", "Charger"),
        ("PNP0C0A", "Battery"),
        ("PNP0C09", "Embedded controller"),
        ("rtc", "Alarm"),
        ("alarmtimer", "Alarm"),
        ("i8042", "Keyboard or touchpad"),
        ("serio", "Keyboard or touchpad"),
    ];
    known
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, label)| *label)
}

fn describe(name: &str) -> String {
    label(name).map_or_else(|| name.to_owned(), |label| format!("{label} ({name})"))
}

/// At most this many sources are named when several fired.
const MAX_SOURCES: usize = 2;

/// The sources whose wakeup count rose during sleep (most first), else the wakeup IRQ.
#[must_use]
pub fn wake_reason(
    before: &WakeupSources,
    after: &WakeupSources,
    irq: Option<&WakeupIrq>,
) -> Option<String> {
    let mut fired: Vec<(u64, &str)> = after
        .iter()
        .filter_map(|(entry, source)| {
            let was = before.get(entry).map_or(0, |b| b.wakeup_count);
            let delta = source.wakeup_count.saturating_sub(was);
            (delta > 0).then_some((delta, source.name.as_str()))
        })
        .collect();
    fired.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut names: Vec<String> = Vec::new();
    for (_, name) in fired {
        let described = describe(name);
        if !names.contains(&described) {
            names.push(described);
        }
    }
    if !names.is_empty() {
        names.truncate(MAX_SOURCES);
        return Some(names.join(", "));
    }
    irq.map(|irq| match label(&irq.action) {
        Some(label) => format!("{label} (IRQ {})", irq.irq),
        None => format!("IRQ {} ({})", irq.irq, irq.action),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(entries: &[(&str, &str, u64)]) -> WakeupSources {
        entries
            .iter()
            .map(|(entry, name, count)| {
                (
                    (*entry).to_owned(),
                    WakeupSource {
                        name: (*name).to_owned(),
                        wakeup_count: *count,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn names_the_sources_that_fired() {
        let before = sources(&[
            ("wakeup1", "PNP0C0D:00", 3),
            ("wakeup2", "rtc0", 0),
            ("wakeup3", "XHC", 1),
        ]);
        let after = sources(&[
            ("wakeup1", "PNP0C0D:00", 4),
            ("wakeup2", "rtc0", 0),
            ("wakeup3", "XHC", 3),
        ]);
        assert_eq!(
            wake_reason(&before, &after, None).as_deref(),
            Some("XHC, Lid (PNP0C0D:00)")
        );
    }

    #[test]
    fn duplicate_names_are_listed_once() {
        let before = sources(&[("wakeup1", "PNP0C0D:00", 0), ("wakeup2", "PNP0C0D:00", 0)]);
        let after = sources(&[("wakeup1", "PNP0C0D:00", 1), ("wakeup2", "PNP0C0D:00", 1)]);
        assert_eq!(
            wake_reason(&before, &after, None).as_deref(),
            Some("Lid (PNP0C0D:00)")
        );
    }

    #[test]
    fn falls_back_to_the_irq() {
        let none = WakeupSources::new();
        let keyboard = WakeupIrq {
            irq: 1,
            action: "i8042".into(),
        };
        let acpi = WakeupIrq {
            irq: 9,
            action: "acpi".into(),
        };
        assert_eq!(
            wake_reason(&none, &none, Some(&keyboard)).as_deref(),
            Some("Keyboard or touchpad (IRQ 1)")
        );
        assert_eq!(
            wake_reason(&none, &none, Some(&acpi)).as_deref(),
            Some("IRQ 9 (acpi)")
        );
        assert_eq!(wake_reason(&none, &none, None), None);
    }

    #[test]
    fn new_sources_count_from_zero() {
        let after = sources(&[("wakeup9", "ACPI0003:00", 1)]);
        assert_eq!(
            wake_reason(&WakeupSources::new(), &after, None).as_deref(),
            Some("Charger (ACPI0003:00)")
        );
    }
}

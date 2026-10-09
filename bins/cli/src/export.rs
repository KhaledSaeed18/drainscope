//! `--format json` and `--format csv`: the data behind each view, complete and in raw units
//! (joules, watts, Unix seconds), for other tools. Text output is unchanged.

use std::fmt::Write as _;

use serde::Serialize;

/// Output format of the data commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// Tables for people.
    #[default]
    Text,
    /// One JSON document: the rows plus the period and context they cover.
    Json,
    /// A header line, then one line per row.
    Csv,
}

/// One consumer's energy over a period (`report`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageRow {
    /// Stable identifier, e.g. `app:org.mozilla.firefox`; the kind itself with `--by kind`.
    pub key: String,
    pub kind: String,
    pub label: String,
    pub joules: f64,
    pub cpu_joules: f64,
    pub gpu_joules: f64,
    /// Display and devices: the battery minus RAPL, shared out.
    pub other_joules: f64,
    /// Average over the measured time.
    pub watts: f64,
    /// Fraction of the period's total, 0–1.
    pub share: f64,
    /// Apps only, from the GNOME Shell extension's focus reports (ADR 0011); `null` otherwise.
    /// Joules while one of the app's windows had focus.
    pub foreground_joules: Option<f64>,
    /// The rest of `joules`, outside `foreground_joules` and `unknown_joules`.
    pub background_joules: Option<f64>,
    /// While focus wasn't reported (no extension, or before drainscope tracked it).
    pub unknown_joules: Option<f64>,
    pub focused_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Usage {
    /// Unix seconds, `[since, until)`.
    pub since: i64,
    pub until: i64,
    /// `any`, `battery` or `ac`.
    pub power_source: String,
    /// `consumer` or `kind`.
    pub group_by: String,
    /// How much of the period the daemon measured.
    pub measured_seconds: u64,
    pub model_version: u32,
    pub total_joules: f64,
    pub rows: Vec<UsageRow>,
}

/// One consumer since the charger was unplugged (`summary`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryRow {
    pub key: String,
    pub label: String,
    pub joules: f64,
    /// Percent of the battery's capacity.
    pub battery_percent: f64,
    /// Percent of the energy caused by something (not idle, display or platform); 0 for those.
    pub active_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub on_battery: bool,
    /// Unix seconds of the last unplug; `null` if none was ever recorded.
    pub since_unplug: Option<i64>,
    /// Percent of the battery used since then.
    pub battery_percent_used: f64,
    pub model_version: u32,
    pub rows: Vec<SummaryRow>,
}

/// One suspend (`sleep`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SleepRow {
    /// Unix seconds.
    pub start: i64,
    pub end: i64,
    /// `null` when unknown.
    pub wh_lost: Option<f64>,
    pub percent_lost: Option<f64>,
    /// e.g. `deep`; empty when unknown.
    pub mode: String,
    /// e.g. `Lid (PNP0C0D:00)`; empty when unknown.
    pub wake_reason: String,
}

/// One daily battery reading (`health`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HealthRow {
    pub battery: String,
    /// Unix seconds.
    pub time: i64,
    pub full_wh: f64,
    /// `null` when unknown.
    pub design_wh: Option<f64>,
    /// `null` when unknown.
    pub cycles: Option<u32>,
}

/// One consumer's idle exits per second over the last minute (`wakeups`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WakeupRow {
    pub key: String,
    pub label: String,
    pub per_second: f64,
}

/// One consumer's network traffic over the last minute (`network`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetworkRow {
    pub key: String,
    pub label: String,
    pub received_bytes_per_second: f64,
    pub sent_bytes_per_second: f64,
}

/// Rows of a live metric, which the probe may not provide.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Live<T> {
    /// False without the eBPF probe (`rows` is then empty).
    pub available: bool,
    pub rows: Vec<T>,
}

/// Unknown amounts arrive as NaN over D-Bus; JSON has no NaN, so they become `null`.
#[must_use]
pub fn known(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

/// Pretty JSON and a final newline.
///
/// # Errors
/// Never in practice: the documents hold only strings, numbers and booleans.
pub fn json<T: Serialize>(document: &T) -> serde_json::Result<String> {
    serde_json::to_string_pretty(document).map(|text| text + "\n")
}

/// A CSV field, quoted when it contains a comma, quote or line break (RFC 4180).
fn field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// CSV with `header` and one line per row; `None` cells are empty.
#[must_use]
pub fn csv(header: &[&str], rows: &[Vec<Option<String>>]) -> String {
    let mut out = header.join(",");
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|cell| cell.as_deref().map(field).unwrap_or_default())
            .collect();
        let _ = writeln!(out, "{}", cells.join(","));
    }
    out
}

/// A number with full precision, for CSV.
#[must_use]
pub fn num(value: f64) -> Option<String> {
    known(value).map(|v| v.to_string())
}

#[must_use]
pub fn usage_csv(usage: &Usage) -> String {
    let rows: Vec<Vec<Option<String>>> = usage
        .rows
        .iter()
        .map(|r| {
            vec![
                Some(r.key.clone()),
                Some(r.kind.clone()),
                Some(r.label.clone()),
                num(r.joules),
                num(r.joules / 3600.0),
                num(r.cpu_joules),
                num(r.gpu_joules),
                num(r.other_joules),
                num(r.watts),
                num(r.share),
                r.foreground_joules.and_then(num),
                r.background_joules.and_then(num),
                r.unknown_joules.and_then(num),
                r.focused_seconds.map(|s| s.to_string()),
            ]
        })
        .collect();
    csv(
        &[
            "key",
            "kind",
            "label",
            "joules",
            "wh",
            "cpu_joules",
            "gpu_joules",
            "other_joules",
            "watts",
            "share",
            "foreground_joules",
            "background_joules",
            "unknown_joules",
            "focused_seconds",
        ],
        &rows,
    )
}

#[must_use]
pub fn summary_csv(summary: &Summary) -> String {
    let rows: Vec<Vec<Option<String>>> = summary
        .rows
        .iter()
        .map(|r| {
            vec![
                Some(r.key.clone()),
                Some(r.label.clone()),
                num(r.joules),
                num(r.joules / 3600.0),
                num(r.battery_percent),
                num(r.active_percent),
            ]
        })
        .collect();
    csv(
        &[
            "key",
            "label",
            "joules",
            "wh",
            "battery_percent",
            "active_percent",
        ],
        &rows,
    )
}

#[must_use]
pub fn sleep_csv(rows: &[SleepRow]) -> String {
    let rows: Vec<Vec<Option<String>>> = rows
        .iter()
        .map(|r| {
            vec![
                Some(r.start.to_string()),
                Some(r.end.to_string()),
                r.wh_lost.and_then(num),
                r.percent_lost.and_then(num),
                Some(r.mode.clone()),
                Some(r.wake_reason.clone()),
            ]
        })
        .collect();
    csv(
        &[
            "start",
            "end",
            "wh_lost",
            "percent_lost",
            "mode",
            "wake_reason",
        ],
        &rows,
    )
}

#[must_use]
pub fn health_csv(rows: &[HealthRow]) -> String {
    let rows: Vec<Vec<Option<String>>> = rows
        .iter()
        .map(|r| {
            vec![
                Some(r.battery.clone()),
                Some(r.time.to_string()),
                num(r.full_wh),
                r.design_wh.and_then(num),
                r.cycles.map(|c| c.to_string()),
            ]
        })
        .collect();
    csv(
        &["battery", "time", "full_wh", "design_wh", "cycles"],
        &rows,
    )
}

#[must_use]
pub fn wakeups_csv(rows: &[WakeupRow]) -> String {
    let rows: Vec<Vec<Option<String>>> = rows
        .iter()
        .map(|r| {
            vec![
                Some(r.key.clone()),
                Some(r.label.clone()),
                num(r.per_second),
            ]
        })
        .collect();
    csv(&["key", "label", "per_second"], &rows)
}

#[must_use]
pub fn network_csv(rows: &[NetworkRow]) -> String {
    let rows: Vec<Vec<Option<String>>> = rows
        .iter()
        .map(|r| {
            vec![
                Some(r.key.clone()),
                Some(r.label.clone()),
                num(r.received_bytes_per_second),
                num(r.sent_bytes_per_second),
            ]
        })
        .collect();
    csv(
        &[
            "key",
            "label",
            "received_bytes_per_second",
            "sent_bytes_per_second",
        ],
        &rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn firefox() -> UsageRow {
        UsageRow {
            key: "app:org.mozilla.firefox".into(),
            kind: "app".into(),
            label: "Firefox".into(),
            joules: 3600.0,
            cpu_joules: 3000.0,
            gpu_joules: 500.0,
            other_joules: 100.0,
            watts: 1.25,
            share: 0.4,
            foreground_joules: Some(2400.0),
            background_joules: Some(1000.0),
            unknown_joules: Some(200.0),
            focused_seconds: Some(1800),
        }
    }

    #[test]
    fn quotes_csv_fields_only_when_needed() {
        let out = csv(
            &["a", "b", "c"],
            &[vec![
                Some("plain".into()),
                Some("Terminal: make, test".into()),
                Some("say \"hi\"".into()),
            ]],
        );
        assert_eq!(
            out,
            "a,b,c\nplain,\"Terminal: make, test\",\"say \"\"hi\"\"\"\n"
        );
    }

    #[test]
    fn usage_csv_has_every_unit() {
        let usage = Usage {
            since: 100,
            until: 200,
            power_source: "any".into(),
            group_by: "consumer".into(),
            measured_seconds: 100,
            model_version: 3,
            total_joules: 9000.0,
            rows: vec![firefox()],
        };
        assert_eq!(
            usage_csv(&usage),
            "key,kind,label,joules,wh,cpu_joules,gpu_joules,other_joules,watts,share,\
             foreground_joules,background_joules,unknown_joules,focused_seconds\n\
             app:org.mozilla.firefox,app,Firefox,3600,1,3000,500,100,1.25,0.4,2400,1000,200,1800\n"
        );
        let value: serde_json::Value = serde_json::from_str(&json(&usage).unwrap()).unwrap();
        assert_eq!(value["rows"][0]["key"], "app:org.mozilla.firefox");
        assert_eq!(value["measured_seconds"], 100);
        assert_eq!(value["model_version"], 3);
        assert_eq!(value["rows"][0]["focused_seconds"], 1800);
    }

    #[test]
    fn unknown_amounts_are_null_or_empty() {
        let rows = [SleepRow {
            start: 1,
            end: 2,
            wh_lost: known(f64::NAN),
            percent_lost: known(0.5),
            mode: String::new(),
            wake_reason: "Lid (PNP0C0D:00)".into(),
        }];
        assert_eq!(
            sleep_csv(&rows),
            "start,end,wh_lost,percent_lost,mode,wake_reason\n1,2,,0.5,,Lid (PNP0C0D:00)\n"
        );
        let value: serde_json::Value = serde_json::from_str(&json(&rows).unwrap()).unwrap();
        assert!(value[0]["wh_lost"].is_null());
        assert_eq!(
            health_csv(&[HealthRow {
                battery: "BAT0".into(),
                time: 5,
                full_wh: 31.4,
                design_wh: None,
                cycles: None,
            }]),
            "battery,time,full_wh,design_wh,cycles\nBAT0,5,31.4,,\n"
        );
    }
}

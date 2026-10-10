//! `status`, `report`, `top` and `sleep`: views over the daemon's `Monitor1` interface.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use drainscope_dbus::monitor::Monitor1Proxy;
use futures_util::StreamExt;

use crate::export::{self, Format, known};
use crate::format::{byte_rate, duration, energy, percent, table, table_aligned, truncate, watts};
use crate::names::{Names, kind_label};

const LABEL_WIDTH: usize = 40;

/// One consumer's energy, already labelled.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub joules: f64,
    /// Apps' energy by focus (ADR 0011), when the daemon knows it.
    pub focus: Option<Focus>,
}

/// An app's energy split by focus; the rest of its energy was used in the background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Focus {
    pub foreground_j: f64,
    pub unknown_j: f64,
    pub focused_secs: u64,
}

impl Focus {
    #[must_use]
    pub fn background_j(&self, total_j: f64) -> f64 {
        (total_j - self.foreground_j - self.unknown_j).max(0.0)
    }

    /// Whether anything about the app's focus is known: it had focus, or some of its energy
    /// (at least 1 mWh) was classified. History from before 0.1.4 is all unknown.
    #[must_use]
    pub fn is_known(&self, total_j: f64) -> bool {
        self.focused_secs > 0 || self.foreground_j + self.background_j(total_j) >= 3.6
    }
}

/// Apps' focus split by consumer key; empty from daemons older than `GetFocus`.
async fn focus_by_key(
    monitor: &Monitor1Proxy<'_>,
    since: i64,
    until: i64,
    source: &str,
) -> anyhow::Result<HashMap<String, Focus>> {
    match monitor.get_focus(since, until, source).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|(key, foreground_j, unknown_j, focused_secs)| {
                (
                    key,
                    Focus {
                        foreground_j,
                        unknown_j,
                        focused_secs,
                    },
                )
            })
            .collect()),
        Err(zbus::Error::MethodError(name, ..))
            if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" =>
        {
            Ok(HashMap::new())
        }
        Err(err) => Err(daemon_hint(err)),
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn seconds(secs: i64) -> Duration {
    Duration::from_secs(u64::try_from(secs).unwrap_or(0))
}

pub async fn connect(bus: &zbus::Connection) -> anyhow::Result<Monitor1Proxy<'_>> {
    Monitor1Proxy::new(bus)
        .await
        .context("connecting to the drainscope daemon")
}

/// Prints a JSON or CSV export: `csv` turns the document into CSV.
fn print_export<T: serde::Serialize>(
    format: Format,
    document: &T,
    csv: impl FnOnce(&T) -> String,
) -> anyhow::Result<()> {
    match format {
        Format::Json => print!("{}", export::json(document)?),
        Format::Csv => print!("{}", csv(document)),
        Format::Text => {}
    }
    Ok(())
}

/// What to do about a failed call to the daemon, by D-Bus error name.
fn hint_for(error_name: Option<&str>) -> &'static str {
    match error_name {
        Some("org.freedesktop.DBus.Error.UnknownMethod") => {
            "the running daemon is older than this command; restart it after updating \
             (systemctl --user restart drainscope.service)"
        }
        Some("org.freedesktop.DBus.Error.ServiceUnknown") => {
            "the drainscope daemon isn't running (systemctl --user enable --now drainscope.service)"
        }
        // Installed but failing to start, or not answering: the reason is in its log.
        _ => {
            "the drainscope daemon didn't answer; see why: journalctl --user -u drainscope.service -n 20"
        }
    }
}

/// Explains a failed call to the daemon.
fn daemon_hint(err: zbus::Error) -> anyhow::Error {
    let hint = hint_for(match &err {
        zbus::Error::MethodError(name, ..) => Some(name.as_str()),
        _ => None,
    });
    anyhow::Error::new(err).context(hint)
}

/// The top `limit` rows, the rest folded into one line, as table rows with shares of the
/// listed total. `measured` is how much of `span` the daemon recorded (see `GetCoverage`);
/// averages are over measured time, not the whole span.
#[must_use]
pub fn render_usage(rows: &[Row], span: Duration, measured: Duration, limit: usize) -> String {
    let total: f64 = rows.iter().map(|r| r.joules).sum();
    let mut shown: Vec<Row> = rows.iter().take(limit).cloned().collect();
    let rest: f64 = rows.iter().skip(limit).map(|r| r.joules).sum();
    if rest > 0.0 {
        shown.push(Row {
            label: format!("{} more", rows.len() - limit),
            joules: rest,
            focus: None,
        });
    }
    let secs = measured.as_secs_f64().max(1.0);
    // Rows whose focus is all unknown show none, and the columns only appear for a split.
    for row in &mut shown {
        row.focus = row.focus.filter(|focus| focus.is_known(row.joules));
    }
    let with_focus = shown.iter().any(|row| row.focus.is_some());
    let cells: Vec<Vec<String>> = shown
        .iter()
        .map(|row| {
            let mut cells = vec![
                truncate(&row.label, LABEL_WIDTH),
                energy(row.joules),
                watts(row.joules / secs),
                percent(if total > 0.0 {
                    row.joules / total * 100.0
                } else {
                    0.0
                }),
            ];
            if with_focus {
                match row.focus {
                    Some(focus) => cells.extend([
                        energy(focus.foreground_j),
                        energy(focus.background_j(row.joules)),
                    ]),
                    None => cells.extend(["—".to_owned(), "—".to_owned()]),
                }
            }
            cells
        })
        .collect();
    let mut out = if with_focus {
        table(
            &[
                "Consumer",
                "Energy",
                "Average",
                "Share",
                "In use",
                "Background",
            ],
            &cells,
        )
    } else {
        table(&["Consumer", "Energy", "Average", "Share"], &cells)
    };
    // Within 5% counts as the whole span (window edges don't line up with it exactly).
    if measured.as_secs_f64() >= span.as_secs_f64() * 0.95 {
        let _ = writeln!(
            out,
            "\nTotal {} over {} (average {})",
            energy(total),
            duration(span),
            watts(total / secs)
        );
    } else {
        let _ = writeln!(
            out,
            "\nTotal {} in {} measured of the last {} (average {} while measured)",
            energy(total),
            duration(measured),
            duration(span),
            watts(total / secs)
        );
    }
    let unknown: f64 = shown
        .iter()
        .filter_map(|row| row.focus.map(|focus| focus.unknown_j))
        .sum();
    if with_focus && unknown >= 3.6 {
        let _ = writeln!(
            out,
            "In use and background leave out {} from while focus wasn't reported.",
            energy(unknown)
        );
    }
    out
}

/// One `GetSummary` row with its label resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryRow {
    pub label: String,
    pub joules: f64,
    pub of_battery: f64,
    pub of_active: f64,
}

#[must_use]
pub fn render_summary(
    on_battery: bool,
    since: Option<Duration>,
    battery_percent: f64,
    rows: &[SummaryRow],
) -> String {
    let Some(since) = since else {
        return if on_battery {
            "On battery, but drainscope hasn't seen an unplug yet; check back in a minute.\n"
        } else {
            "On AC power. No discharge recorded yet: unplug the charger to start measuring.\n"
        }
        .to_owned();
    };
    let mut out = if on_battery {
        format!(
            "On battery for {}: {} of the battery used.\n\n",
            duration(since),
            percent(battery_percent)
        )
    } else {
        format!(
            "On AC power. The last discharge, which started {} ago, used {} of the battery.\n\n",
            duration(since),
            percent(battery_percent)
        )
    };
    if rows.is_empty() {
        out.push_str("Nothing recorded on battery yet.\n");
        return out;
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                truncate(&row.label, LABEL_WIDTH),
                percent(row.of_battery),
                if row.of_active > 0.0 {
                    percent(row.of_active)
                } else {
                    "—".to_owned()
                },
                energy(row.joules),
            ]
        })
        .collect();
    out.push_str(&table(
        &["Consumer", "Battery", "Of active use", "Energy"],
        &cells,
    ));
    out.push_str(
        "\n\"Of active use\" leaves out idle power and display & devices: it is what apps and\nservices caused.\n",
    );
    out
}

#[must_use]
pub fn render_live(rows: &[Row], tick: Duration, limit: usize) -> String {
    let total: f64 = rows.iter().map(|r| r.joules).sum();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .take(limit)
        .map(|row| vec![truncate(&row.label, LABEL_WIDTH), watts(row.joules)])
        .collect();
    let mut out = format!(
        "drainscope top: CPU and GPU power, updated every {} (Ctrl-C to quit)\n\n",
        duration(tick)
    );
    out.push_str(&table(&["Consumer", "Power"], &cells));
    let _ = writeln!(out, "\nMeasured total {}", watts(total));
    out
}

/// A sleep session, relative to now.
#[derive(Debug, Clone, PartialEq)]
pub struct SleepRow {
    pub ago: Duration,
    pub slept: Duration,
    pub wh_lost: f64,
    pub percent_lost: f64,
    pub mode: String,
    /// Empty when unknown.
    pub woke_by: String,
}

#[must_use]
pub fn render_sleep(rows: &[SleepRow]) -> String {
    if rows.is_empty() {
        return "No sleep sessions recorded in that period.\n".to_owned();
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let hours = row.slept.as_secs_f64() / 3600.0;
            let rate = if hours > 0.0 && row.percent_lost.is_finite() {
                format!("{:.1}%/h", row.percent_lost / hours)
            } else {
                "?".to_owned()
            };
            vec![
                format!("{} ago", duration(row.ago)),
                duration(row.slept),
                percent(row.percent_lost),
                if row.wh_lost.is_finite() {
                    format!("{:.2} Wh", row.wh_lost)
                } else {
                    "?".to_owned()
                },
                rate,
                if row.mode.is_empty() {
                    "?".to_owned()
                } else {
                    row.mode.clone()
                },
                if row.woke_by.is_empty() {
                    "?".to_owned()
                } else {
                    row.woke_by.clone()
                },
            ]
        })
        .collect();
    table_aligned(
        &[
            "Started", "Slept", "Lost", "Energy", "Rate", "Mode", "Woke by",
        ],
        &cells,
        &[0, 6],
    )
}

/// One battery's readings over the requested period.
pub struct HealthRow {
    pub battery: String,
    pub full_wh: f64,
    /// NaN when unknown.
    pub design_wh: f64,
    /// 0 when unknown.
    pub cycles: u32,
    /// Change in full-charge capacity since the oldest reading in the period.
    pub change_wh: f64,
    pub since: Duration,
}

#[must_use]
pub fn render_health(rows: &[HealthRow]) -> String {
    if rows.is_empty() {
        return "No battery health recorded yet (the daemon records it once a day).\n".to_owned();
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let health = if row.design_wh > 0.0 {
                percent(row.full_wh / row.design_wh * 100.0)
            } else {
                "?".to_owned()
            };
            vec![
                row.battery.clone(),
                format!("{:.1} Wh", row.full_wh),
                if row.design_wh.is_finite() {
                    format!("{:.1} Wh", row.design_wh)
                } else {
                    "?".to_owned()
                },
                health,
                if row.cycles > 0 {
                    row.cycles.to_string()
                } else {
                    "?".to_owned()
                },
                if row.since.is_zero() {
                    "—".to_owned()
                } else {
                    format!("{:+.1} Wh in {}", row.change_wh, duration(row.since))
                },
            ]
        })
        .collect();
    table(
        &[
            "Battery",
            "Full charge",
            "Design",
            "Health",
            "Cycles",
            "Change",
        ],
        &cells,
    )
}

fn describe_status(status: &str) -> &str {
    match status {
        "full" => "full (RAPL energy counters, plus the battery when unplugged)",
        "battery-only" => "battery only: no RAPL sampler, so battery energy is split by CPU time",
        "time-only" => "CPU time only: on AC without the RAPL sampler, energy isn't measured",
        other => other,
    }
}

pub async fn status(bus: &zbus::Connection) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let status = monitor.status().await.map_err(daemon_hint)?;
    let domains = monitor.domains().await.map_err(daemon_hint)?;
    let version = monitor.model_version().await.map_err(daemon_hint)?;
    let (on_battery, since, battery_percent, _) =
        monitor.get_summary().await.map_err(daemon_hint)?;
    println!("Measurement: {}", describe_status(&status));
    println!(
        "RAPL domains: {}",
        if domains.is_empty() {
            "none".to_owned()
        } else {
            domains.join(", ")
        }
    );
    println!("Model:       v{version}");
    if on_battery && since > 0 {
        println!(
            "Power:       on battery for {} ({} used)",
            duration(seconds(now_secs() - since)),
            percent(battery_percent)
        );
    } else if on_battery {
        println!("Power:       on battery");
    } else {
        println!("Power:       on AC");
    }
    Ok(())
}

pub async fn summary(bus: &zbus::Connection, format: Format) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let (on_battery, since, battery_percent, top) =
        monitor.get_summary().await.map_err(daemon_hint)?;
    let mut names = Names::default();
    if format != Format::Text {
        let document = export::Summary {
            on_battery,
            since_unplug: (since > 0).then_some(since),
            battery_percent_used: battery_percent,
            model_version: monitor.model_version().await.map_err(daemon_hint)?,
            rows: top
                .into_iter()
                .map(|(key, joules, of_battery, of_active)| export::SummaryRow {
                    label: names.label(&key),
                    key,
                    joules,
                    battery_percent: of_battery,
                    active_percent: of_active,
                })
                .collect(),
        };
        return print_export(format, &document, export::summary_csv);
    }
    let rows: Vec<SummaryRow> = top
        .into_iter()
        .map(|(key, joules, of_battery, of_active)| SummaryRow {
            label: names.label(&key),
            joules,
            of_battery,
            of_active,
        })
        .collect();
    // 0 means the daemon has never seen an unplug.
    let since = (since > 0).then(|| seconds(now_secs() - since));
    print!(
        "{}",
        render_summary(on_battery, since, battery_percent, &rows)
    );
    Ok(())
}

pub async fn usage(
    bus: &zbus::Connection,
    since: Duration,
    by_kind: bool,
    source: &str,
    limit: usize,
    format: Format,
) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let now = now_secs();
    let start = now - i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    let group = if by_kind { "kind" } else { "consumer" };
    let usage = monitor
        .get_usage(start, now + 1, group, source)
        .await
        .map_err(daemon_hint)?;
    let measured = Duration::from_secs(
        monitor
            .get_coverage(start, now + 1, source)
            .await
            .map_err(daemon_hint)?,
    );
    let focus = if by_kind {
        HashMap::new()
    } else {
        focus_by_key(&monitor, start, now + 1, source).await?
    };
    let mut names = Names::default();
    if format != Format::Text {
        // Every row, not just the top ones; averages over the measured time, like the table.
        let total: f64 = usage.iter().map(|row| row.2).sum();
        let seconds = measured.as_secs_f64().max(1.0);
        let document = export::Usage {
            since: start,
            until: now + 1,
            power_source: source.to_owned(),
            group_by: group.to_owned(),
            measured_seconds: measured.as_secs(),
            model_version: monitor.model_version().await.map_err(daemon_hint)?,
            total_joules: total,
            rows: usage
                .into_iter()
                .map(|(key, kind, joules, cpu, gpu, other)| {
                    let app_focus = focus.get(&key).copied();
                    export::UsageRow {
                        label: if by_kind {
                            kind_label(&key)
                        } else {
                            names.label(&key)
                        },
                        key,
                        kind,
                        joules,
                        cpu_joules: cpu,
                        gpu_joules: gpu,
                        other_joules: other,
                        watts: joules / seconds,
                        share: if total > 0.0 { joules / total } else { 0.0 },
                        foreground_joules: app_focus.map(|f| f.foreground_j),
                        background_joules: app_focus.map(|f| f.background_j(joules)),
                        unknown_joules: app_focus.map(|f| f.unknown_j),
                        focused_seconds: app_focus.map(|f| f.focused_secs),
                    }
                })
                .collect(),
        };
        return print_export(format, &document, export::usage_csv);
    }
    let rows: Vec<Row> = usage
        .into_iter()
        .map(|(key, _, joules, ..)| Row {
            focus: focus.get(&key).copied(),
            label: if by_kind {
                kind_label(&key)
            } else {
                names.label(&key)
            },
            joules,
        })
        .collect();
    print!("{}", render_usage(&rows, since, measured, limit));
    Ok(())
}

pub async fn top(bus: &zbus::Connection, limit: usize) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    // Fail early with a useful message if the daemon isn't there.
    monitor.status().await.map_err(daemon_hint)?;
    let mut ticks = monitor.receive_tick().await?;
    let mut names = Names::default();
    println!("Waiting for the next measurement…");
    while let Some(tick) = ticks.next().await {
        let args = tick.args()?;
        let mut rows: Vec<Row> = args
            .usage()
            .iter()
            .map(|(key, w)| Row {
                label: names.label(key),
                joules: *w,
                focus: None,
            })
            .collect();
        rows.sort_by(|a, b| b.joules.total_cmp(&a.joules));
        let tick_length = Duration::from_millis(u64::from(*args.duration_ms()));
        // Clear the screen and redraw.
        print!("\x1b[2J\x1b[H{}", render_live(&rows, tick_length, limit));
    }
    Ok(())
}

/// Renders `(label, idle exits per second)` rows, largest first.
#[must_use]
pub fn render_wakeups(available: bool, rows: &[(String, f64)], limit: usize) -> String {
    if !available {
        return "Wakeup counts need the drainscope-probe service (eBPF); it isn't installed or \
                reachable.\n"
            .to_owned();
    }
    if rows.is_empty() {
        return "No wakeups measured yet; check again in a minute.\n".to_owned();
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .take(limit)
        .map(|(label, rate)| vec![truncate(label, 40), format!("{rate:.1}/s")])
        .collect();
    let mut out = table(&["Consumer", "Wakeups"], &cells);
    let _ = writeln!(
        out,
        "\nTimes per second each one woke the processor from idle, over the last minute."
    );
    out
}

pub async fn wakeups(bus: &zbus::Connection, limit: usize, format: Format) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let (available, wakeups) = monitor.get_wakeups().await.map_err(daemon_hint)?;
    let mut names = Names::default();
    if format != Format::Text {
        let document = export::Live {
            available,
            rows: wakeups
                .into_iter()
                .map(|(key, per_second)| export::WakeupRow {
                    label: names.label(&key),
                    key,
                    per_second,
                })
                .collect(),
        };
        return print_export(format, &document, |d| export::wakeups_csv(&d.rows));
    }
    let rows: Vec<(String, f64)> = wakeups
        .into_iter()
        .map(|(key, rate)| (names.label(&key), rate))
        .collect();
    print!("{}", render_wakeups(available, &rows, limit));
    Ok(())
}

/// Renders `(label, received B/s, sent B/s)` rows, busiest first.
#[must_use]
pub fn render_network(available: bool, rows: &[(String, f64, f64)], limit: usize) -> String {
    if !available {
        return "Network counts need the drainscope-probe service (eBPF) with network counting; \
                it isn't installed or reachable.\n"
            .to_owned();
    }
    if rows.is_empty() {
        return "No network traffic measured yet; check again in a minute.\n".to_owned();
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .take(limit)
        .map(|(label, received, sent)| {
            vec![truncate(label, 40), byte_rate(*received), byte_rate(*sent)]
        })
        .collect();
    let mut out = table(&["Consumer", "Received", "Sent"], &cells);
    let _ = writeln!(out, "\nAverage over the last minute, excluding loopback.");
    out
}

pub async fn network(bus: &zbus::Connection, limit: usize, format: Format) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let (available, traffic) = monitor.get_network().await.map_err(daemon_hint)?;
    let mut names = Names::default();
    if format != Format::Text {
        let document = export::Live {
            available,
            rows: traffic
                .into_iter()
                .map(|(key, received, sent)| export::NetworkRow {
                    label: names.label(&key),
                    key,
                    received_bytes_per_second: received,
                    sent_bytes_per_second: sent,
                })
                .collect(),
        };
        return print_export(format, &document, |d| export::network_csv(&d.rows));
    }
    let rows: Vec<(String, f64, f64)> = traffic
        .into_iter()
        .map(|(key, received, sent)| (names.label(&key), received, sent))
        .collect();
    print!("{}", render_network(available, &rows, limit));
    Ok(())
}

pub async fn sleep(bus: &zbus::Connection, since: Duration, format: Format) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let now = now_secs();
    let start = now - i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    let sessions = monitor
        .get_sleep_history(start)
        .await
        .map_err(daemon_hint)?;
    if format != Format::Text {
        let rows: Vec<export::SleepRow> = sessions
            .into_iter()
            .map(
                |(start, end, wh_lost, percent_lost, mode, wake_reason)| export::SleepRow {
                    start,
                    end,
                    wh_lost: known(wh_lost),
                    percent_lost: known(percent_lost),
                    mode,
                    wake_reason,
                },
            )
            .collect();
        return print_export(format, &rows, |rows| export::sleep_csv(rows));
    }
    let rows: Vec<SleepRow> = sessions
        .into_iter()
        .map(
            |(start, end, wh_lost, percent_lost, mode, woke_by)| SleepRow {
                ago: seconds(now - start),
                slept: seconds(end - start),
                wh_lost,
                percent_lost,
                mode,
                woke_by,
            },
        )
        .collect();
    print!("{}", render_sleep(&rows));
    Ok(())
}

pub async fn health(bus: &zbus::Connection, since: Duration, format: Format) -> anyhow::Result<()> {
    let monitor = connect(bus).await?;
    let start = now_secs() - i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    let readings = monitor
        .get_battery_health(start)
        .await
        .map_err(daemon_hint)?;
    if format != Format::Text {
        // Every daily reading, oldest first, not just the latest per battery.
        let rows: Vec<export::HealthRow> = readings
            .into_iter()
            .map(
                |(battery, time, full_wh, design_wh, cycles)| export::HealthRow {
                    battery,
                    time,
                    full_wh,
                    design_wh: known(design_wh).filter(|wh| *wh > 0.0),
                    cycles: (cycles > 0).then_some(cycles),
                },
            )
            .collect();
        return print_export(format, &rows, |rows| export::health_csv(rows));
    }
    print!("{}", render_health(&summarize_health(&readings)));
    Ok(())
}

/// The latest reading per battery, with the change since its oldest one (readings come
/// oldest first).
fn summarize_health(readings: &[drainscope_dbus::monitor::HealthRow]) -> Vec<HealthRow> {
    let mut rows: Vec<HealthRow> = Vec::new();
    let mut first: Vec<(i64, f64)> = Vec::new();
    for (battery, ts, full_wh, design_wh, cycles) in readings {
        let index = rows.iter().position(|r| &r.battery == battery);
        let index = index.unwrap_or_else(|| {
            rows.push(HealthRow {
                battery: battery.clone(),
                full_wh: *full_wh,
                design_wh: *design_wh,
                cycles: *cycles,
                change_wh: 0.0,
                since: Duration::ZERO,
            });
            first.push((*ts, *full_wh));
            rows.len() - 1
        });
        let (first_ts, first_wh) = first[index];
        let row = &mut rows[index];
        row.full_wh = *full_wh;
        row.design_wh = *design_wh;
        row.cycles = *cycles;
        row.change_wh = full_wh - first_wh;
        row.since = seconds(ts - first_ts);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_say_what_to_do_about_the_daemon() {
        assert!(
            hint_for(Some("org.freedesktop.DBus.Error.ServiceUnknown")).contains("enable --now")
        );
        assert!(hint_for(Some("org.freedesktop.DBus.Error.UnknownMethod")).contains("older"));
        // A daemon that fails to start (e.g. a database from a newer version) explains why in its log.
        assert!(
            hint_for(Some("org.freedesktop.DBus.Error.Spawn.ChildExited")).contains("journalctl")
        );
        assert!(hint_for(None).contains("journalctl"));
    }

    fn row(label: &str, joules: f64) -> Row {
        Row {
            label: label.into(),
            joules,
            focus: None,
        }
    }

    #[test]
    fn usage_leaves_out_focus_that_is_all_unknown() {
        let mut firefox = row("Firefox", 3600.0);
        firefox.focus = Some(Focus {
            foreground_j: 0.0,
            unknown_j: 3600.0,
            focused_secs: 0,
        });
        let rendered = render_usage(
            &[firefox],
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            5,
        );
        assert!(
            rendered.starts_with("Consumer   Energy  Average  Share\n"),
            "{rendered}"
        );
        assert!(!rendered.contains("focus wasn't reported"), "{rendered}");
    }

    #[test]
    fn usage_shows_in_use_and_background_for_apps() {
        let mut firefox = row("Firefox", 3600.0);
        firefox.focus = Some(Focus {
            foreground_j: 2520.0,
            unknown_j: 360.0,
            focused_secs: 1800,
        });
        let rendered = render_usage(
            &[firefox, row("Kernel", 360.0)],
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            5,
        );
        assert_eq!(
            rendered,
            "Consumer   Energy  Average  Share   In use  Background\n\
             ──────────────────────────────────────────────────────\n\
             Firefox   1.00 Wh   1.00 W    91%  0.70 Wh     0.20 Wh\n\
             Kernel    0.10 Wh   0.10 W     9%        —           —\n\
             \n\
             Total 1.10 Wh over 1 h (average 1.10 W)\n\
             In use and background leave out 0.10 Wh from while focus wasn't reported.\n"
        );
    }

    #[test]
    fn usage_folds_the_tail_and_totals() {
        let rows = [
            row("Display & devices", 2700.0),
            row("Firefox", 720.0),
            row("Kernel", 180.0),
            row("Zed", 0.36),
        ];
        assert_eq!(
            render_usage(
                &rows,
                Duration::from_secs(3600),
                Duration::from_secs(3600),
                2
            ),
            "Consumer             Energy  Average  Share\n\
             ───────────────────────────────────────────\n\
             Display & devices   0.75 Wh   0.75 W    75%\n\
             Firefox             0.20 Wh   0.20 W    20%\n\
             2 more             0.050 Wh   0.05 W     5%\n\
             \n\
             Total 1.00 Wh over 1 h (average 1.00 W)\n"
        );
    }

    #[test]
    fn usage_averages_over_measured_time_only() {
        let rendered = render_usage(
            &[row("Firefox", 1800.0)],
            Duration::from_secs(1800),
            Duration::from_secs(900),
            5,
        );
        assert!(
            rendered.ends_with(
                "Total 0.50 Wh in 15 min measured of the last 30 min (average 2.00 W while measured)\n"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn summary_explains_active_use() {
        let rows = [
            SummaryRow {
                label: "Display & devices".into(),
                joules: 3600.0,
                of_battery: 3.0,
                of_active: 0.0,
            },
            SummaryRow {
                label: "Firefox".into(),
                joules: 900.0,
                of_battery: 0.75,
                of_active: 62.0,
            },
        ];
        assert_eq!(
            render_summary(true, Some(Duration::from_secs(4320)), 4.2, &rows),
            "On battery for 1 h 12 min: 4% of the battery used.\n\
             \n\
             Consumer           Battery  Of active use   Energy\n\
             ──────────────────────────────────────────────────\n\
             Display & devices       3%              —  1.00 Wh\n\
             Firefox                <1%            62%  0.25 Wh\n\
             \n\
             \"Of active use\" leaves out idle power and display & devices: it is what apps and\n\
             services caused.\n"
        );
        assert_eq!(
            render_summary(false, Some(Duration::from_secs(60)), 0.0, &[]),
            "On AC power. The last discharge, which started 1 min ago, used 0% of the battery.\n\n\
             Nothing recorded on battery yet.\n"
        );
    }

    #[test]
    fn summary_without_any_discharge_says_so() {
        assert_eq!(
            render_summary(false, None, 0.0, &[]),
            "On AC power. No discharge recorded yet: unplug the charger to start measuring.\n"
        );
    }

    #[test]
    fn sleep_rows_show_drain_rate() {
        let rendered = render_sleep(&[SleepRow {
            ago: Duration::from_secs(9 * 3600),
            slept: Duration::from_secs(8 * 3600),
            wh_lost: 1.6,
            percent_lost: 4.0,
            mode: "deep".into(),
            woke_by: "Lid (PNP0C0D:00)".into(),
        }]);
        assert_eq!(
            rendered,
            "Started  Slept  Lost   Energy    Rate  Mode  Woke by\n\
             ─────────────────────────────────────────────────────────────\n\
             9 h ago    8 h    4%  1.60 Wh  0.5%/h  deep  Lid (PNP0C0D:00)\n"
        );
        assert_eq!(
            render_sleep(&[]),
            "No sleep sessions recorded in that period.\n"
        );
    }

    #[test]
    fn network_lists_traffic() {
        let rows = vec![("Firefox".to_owned(), 1_520_000.0, 41_000.0)];
        assert_eq!(
            render_network(true, &rows, 5),
            "Consumer  Received       Sent\n\
             ─────────────────────────────\n\
             Firefox   1.5 MB/s  41.0 kB/s\n\
             \nAverage over the last minute, excluding loopback.\n"
        );
        assert!(render_network(false, &rows, 5).contains("drainscope-probe"));
    }

    #[test]
    fn wakeups_list_rates() {
        let rows = vec![
            ("Firefox".to_owned(), 41.24),
            ("System: NetworkManager".to_owned(), 2.0),
        ];
        assert_eq!(
            render_wakeups(true, &rows, 1),
            "Consumer  Wakeups\n\
             ─────────────────\n\
             Firefox    41.2/s\n\
             \nTimes per second each one woke the processor from idle, over the last minute.\n"
        );
        assert!(render_wakeups(false, &rows, 5).contains("drainscope-probe"));
    }

    #[test]
    fn health_shows_the_latest_reading_and_the_change() {
        let day = 86_400;
        let readings = vec![
            ("BAT0".to_owned(), 0, 31.2, 39.0, 300),
            ("BAT1".to_owned(), 0, 30.0, f64::NAN, 0),
            ("BAT0".to_owned(), 30 * day, 30.9, 39.0, 312),
        ];
        assert_eq!(
            render_health(&summarize_health(&readings)),
            "Battery  Full charge   Design  Health  Cycles           Change\n\
             ──────────────────────────────────────────────────────────────\n\
             BAT0         30.9 Wh  39.0 Wh     79%     312  -0.3 Wh in 30 d\n\
             BAT1         30.0 Wh        ?       ?       ?                —\n"
        );
        assert_eq!(
            render_health(&[]),
            "No battery health recorded yet (the daemon records it once a day).\n"
        );
    }
}

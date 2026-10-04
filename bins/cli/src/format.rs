//! Units, durations and tables for terminal output.

use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Context, bail};

/// Parses `30s`, `90m`, `24h`, `7d`.
pub fn parse_duration(text: &str) -> anyhow::Result<Duration> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .context("expected a number with a unit, e.g. 24h")?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number
        .parse()
        .with_context(|| format!("invalid duration {text:?}"))?;
    let seconds = match unit {
        "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => bail!("unknown unit {unit:?} in {text:?} (use s, m, h or d)"),
    };
    Ok(Duration::from_secs(number.saturating_mul(seconds)))
}

/// `35 s`, `4 min`, `1 h 12 min`, `2 d 3 h`.
#[must_use]
pub fn duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (days, hours, minutes) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{secs} s"),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, _) => format!("{d} d"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

/// Energy in watt-hours with about three significant digits.
#[must_use]
pub fn energy(joules: f64) -> String {
    let wh = joules / 3600.0;
    if wh >= 10.0 {
        format!("{wh:.1} Wh")
    } else if wh >= 0.1 {
        format!("{wh:.2} Wh")
    } else {
        format!("{wh:.3} Wh")
    }
}

#[must_use]
pub fn watts(w: f64) -> String {
    if w >= 10.0 {
        format!("{w:.1} W")
    } else {
        format!("{w:.2} W")
    }
}

/// Whole percent; small non-zero shares read as `<1%`.
#[must_use]
pub fn percent(p: f64) -> String {
    if p.is_nan() {
        "?".to_owned()
    } else if p > 0.0 && p < 1.0 {
        "<1%".to_owned()
    } else {
        format!("{p:.0}%")
    }
}

/// Cuts `text` to `width` characters, ending in `…` if shortened.
#[must_use]
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// A left-aligned first column and right-aligned others.
#[must_use]
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let columns = header.len();
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    let line = |out: &mut String, cells: &mut dyn Iterator<Item = &str>| {
        for (i, (cell, width)) in cells.zip(&widths).enumerate() {
            let pad = width.saturating_sub(cell.chars().count());
            if i == 0 {
                out.push_str(cell);
                out.push_str(&" ".repeat(pad));
            } else {
                out.push_str("  ");
                out.push_str(&" ".repeat(pad));
                out.push_str(cell);
            }
        }
        let trimmed = out.trim_end().len();
        out.truncate(trimmed);
        out.push('\n');
    };
    line(&mut out, &mut header.iter().copied());
    let _ = writeln!(
        out,
        "{}",
        "─".repeat(widths.iter().sum::<usize>() + 2 * columns.saturating_sub(1))
    );
    for row in rows {
        line(&mut out, &mut row.iter().map(String::as_str));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("90m").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_duration("7d").unwrap(), Duration::from_secs(604_800));
        assert!(parse_duration("7").is_err());
        assert!(parse_duration("3w").is_err());
    }

    #[test]
    fn formats_units() {
        assert_eq!(duration(Duration::from_secs(4320)), "1 h 12 min");
        assert_eq!(duration(Duration::from_secs(35)), "35 s");
        assert_eq!(
            duration(Duration::from_secs(2 * 86_400 + 3 * 3600)),
            "2 d 3 h"
        );
        assert_eq!(energy(3600.0 * 12.34), "12.3 Wh");
        assert_eq!(energy(3600.0 * 0.5), "0.50 Wh");
        assert_eq!(energy(17.3), "0.005 Wh");
        assert_eq!(watts(0.734), "0.73 W");
        assert_eq!(percent(0.3), "<1%");
        assert_eq!(percent(f64::NAN), "?");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[test]
    fn tables_align_columns() {
        let rendered = table(
            &["Consumer", "Energy"],
            &[
                vec!["Firefox".into(), "1.20 Wh".into()],
                vec!["Kernel".into(), "0.005 Wh".into()],
            ],
        );
        assert_eq!(
            rendered,
            "Consumer    Energy\n──────────────────\nFirefox    1.20 Wh\nKernel    0.005 Wh\n"
        );
    }
}

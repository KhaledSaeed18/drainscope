//! Trace format shared by `record-fixture` and its consumers.
//!
//! A trace is gzipped JSON Lines: one [`Header`] line, then one [`Snapshot`] per line.
//! Each snapshot maps kernel file paths (without the leading `/`) to their raw contents,
//! so replays exercise the same parsers as live reads.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "drainscope-trace";
pub const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub version: u32,
    pub interval_ms: u64,
    pub recorded_as_root: bool,
    pub kernel: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// `CLOCK_MONOTONIC` at capture start, nanoseconds.
    pub mono_ns: u64,
    /// Wall-clock time at capture start, Unix milliseconds.
    pub real_ms: u64,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Trace {
    pub header: Header,
    pub snapshots: Vec<Snapshot>,
}

/// Reads a trace, tolerating a truncated tail (e.g. a recording interrupted with Ctrl-C).
pub fn read(path: &Path) -> Result<Trace> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut lines = BufReader::new(GzDecoder::new(file)).lines();

    let first = lines
        .next()
        .context("trace is empty")?
        .context("reading trace header")?;
    let header: Header = serde_json::from_str(&first).context("parsing trace header")?;
    if header.format != FORMAT || header.version != VERSION {
        bail!(
            "unsupported trace {} v{} (expected {FORMAT} v{VERSION})",
            header.format,
            header.version
        );
    }

    let mut snapshots = Vec::new();
    for line in lines {
        let parsed = line
            .map_err(anyhow::Error::from)
            .and_then(|l| serde_json::from_str::<Snapshot>(&l).map_err(anyhow::Error::from));
        match parsed {
            Ok(snapshot) => snapshots.push(snapshot),
            Err(err) => {
                eprintln!(
                    "warning: trace truncated after {} snapshots ({err})",
                    snapshots.len()
                );
                break;
            }
        }
    }
    Ok(Trace { header, snapshots })
}

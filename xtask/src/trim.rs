//! `trim-trace`: cut a time window out of a trace and drop every field the model doesn't
//! read, so curated traces stay small enough to commit.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;

use crate::trace::{self, Snapshot};

#[derive(Debug)]
pub struct Options {
    pub input: PathBuf,
    pub name: String,
    pub from_s: f64,
    pub to_s: f64,
}

pub fn run(repo_root: &Path, options: &Options) -> Result<PathBuf> {
    let trace = trace::read(&options.input)?;
    let Some(start) = trace.snapshots.first().map(|s| s.mono_ns) else {
        bail!("trace has no snapshots");
    };
    let in_window = |s: &Snapshot| {
        let elapsed = Duration::from_nanos(s.mono_ns.saturating_sub(start)).as_secs_f64();
        (options.from_s..=options.to_s).contains(&elapsed)
    };

    let path = repo_root
        .join("testdata/traces")
        .join(format!("{}.jsonl.gz", options.name));
    let file = fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    let mut out = GzEncoder::new(BufWriter::new(file), Compression::best());
    write_line(&mut out, &trace.header)?;

    let mut kept = 0usize;
    for snapshot in trace.snapshots.into_iter().filter(|s| in_window(s)) {
        write_line(&mut out, &trim(snapshot))?;
        kept += 1;
    }
    out.finish()?.flush()?;
    if kept < 2 {
        bail!("window kept {kept} snapshots; need at least 2");
    }
    eprintln!("kept {kept} snapshots in {}", path.display());
    Ok(path)
}

fn write_line<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn trim(mut snapshot: Snapshot) -> Snapshot {
    snapshot.files = snapshot
        .files
        .into_iter()
        .filter_map(|(path, text)| trim_file(&path, &text).map(|t| (path, t)))
        .collect();
    snapshot
}

/// Returns the trimmed contents to keep, or `None` to drop the file.
fn trim_file(path: &str, text: &str) -> Option<String> {
    if path == "proc/stat" {
        // CPU totals come from the root cgroup's cpu.stat instead.
        return None;
    }
    if path.ends_with("/cpu.stat") {
        return Some(keep_lines(text, |l| l.starts_with("usage_usec ")));
    }
    if path.contains("/fdinfo/") {
        return Some(keep_lines(text, |l| {
            l.starts_with("drm-driver:")
                || l.starts_with("drm-client-id:")
                || (l.starts_with("drm-engine-") && !l.starts_with("drm-engine-capacity"))
        }));
    }
    Some(text.to_owned())
}

fn keep_lines(text: &str, keep: impl Fn(&str) -> bool) -> String {
    let mut kept = String::new();
    for line in text.lines().filter(|l| keep(l)) {
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

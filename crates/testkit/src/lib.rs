//! Test support: the recorded traces in `testdata/traces/`, and materializing a snapshot as a
//! fixture tree laid out like `/` so the real readers can parse it.
//!
//! Only ever a dev-dependency. Failing loudly is the point, hence the panics.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

#[derive(Debug, serde::Deserialize)]
struct Header {
    recorded_as_root: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct TraceSnapshot {
    /// `CLOCK_MONOTONIC` at capture, nanoseconds.
    pub mono_ns: u64,
    /// Wall-clock time at capture, Unix milliseconds.
    pub real_ms: u64,
    /// Kernel file path (without the leading `/`) → contents.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct Trace {
    /// Root-only data (RAPL counters) is present.
    pub recorded_as_root: bool,
    pub snapshots: Vec<TraceSnapshot>,
}

/// The workspace's `testdata/traces/` directory.
#[must_use]
pub fn traces_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/traces")
}

/// Loads `testdata/traces/<name>.jsonl.gz`.
#[must_use]
pub fn trace(name: &str) -> Trace {
    let path = traces_dir().join(format!("{name}.jsonl.gz"));
    let file = fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines = BufReader::new(GzDecoder::new(file))
        .lines()
        .map(Result::unwrap);
    let header: Header = serde_json::from_str(&lines.next().unwrap()).unwrap();
    Trace {
        recorded_as_root: header.recorded_as_root,
        snapshots: lines.map(|l| serde_json::from_str(&l).unwrap()).collect(),
    }
}

/// Writes a snapshot's files under `dir`. Recorded fdinfo files belonged to DRM fds, so each
/// also gets the `/dev/dri` fd symlink the DRM scanner looks for.
pub fn materialize(snapshot: &TraceSnapshot, dir: &Path) {
    for (relative, contents) in &snapshot.files {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        if let Some((proc_dir, fd)) = relative.split_once("/fdinfo/") {
            let link = dir.join(proc_dir).join("fd").join(fd);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink("/dev/dri/renderD128", link).unwrap();
        }
    }
}

//! The whole pipeline on recorded hardware data: collector → engine → store.
//!
//! The attribution of `m0-battery-2min` is compared with a golden file, so any change to the
//! model's output shows up as a reviewable diff. Regenerate after an intended change with
//! `UPDATE_GOLDEN=1 cargo test -p drainscope-daemon --test pipeline`, and bump
//! `MODEL_VERSION` if stored meanings changed.

// Test helpers fail the test by panicking.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use drainscope_daemon::collector::Collector;
use drainscope_daemon::engine::{Engine, FinishedWindow, Reading};
use drainscope_model::{EnergySplit, Joules, MODEL_VERSION, PowerSource, PsysCheck};
use drainscope_store::{SourceFilter, Store, WindowRecord};
use drainscope_sys::{DrmScanner, SysRoot, read_zones};
use drainscope_testkit::{materialize, trace};

const OWN_UID: u32 = 1000;

/// Runs a trace through a fresh collector and `engine`, returning the finished windows.
fn run(name: &str, engine: &mut Engine) -> Vec<FinishedWindow> {
    let trace = trace(name);
    // One DRM scanner for the whole trace, as in the daemon; each snapshot has its own root.
    let mut scanner = DrmScanner::default();
    let mut finished = Vec::new();
    for recorded in &trace.snapshots {
        let dir = tempfile::tempdir().unwrap();
        materialize(recorded, dir.path());
        let root = SysRoot::at(dir.path());
        let mut collector = Collector::new_with_scanner(root.clone(), scanner);
        let mut collected = collector
            .collect(Duration::from_nanos(recorded.mono_ns))
            .unwrap();
        scanner = collector.take_scanner();
        if trace.recorded_as_root {
            collected.snapshot.rapl = read_zones(&root)
                .unwrap()
                .into_iter()
                .map(|zone| (zone.domain, zone.counter))
                .collect();
        }
        let reading = Reading {
            snapshot: collected.snapshot,
            wall_ms: i64::try_from(recorded.real_ms).unwrap(),
            rapl_generation: trace.recorded_as_root.then_some(1),
            terminal_labels: collected.terminal_labels,
            // Traces carry no probe data: model v2 then attributes exactly like v1.
            network: None,
        };
        if let Some(outcome) = engine.tick(reading) {
            finished.extend(outcome.finished);
        }
    }
    finished
}

fn calibrated_engine() -> Engine {
    let mut engine = Engine::new(OWN_UID, BTreeMap::new(), PsysCheck::default());
    run("m0-idle-2min", &mut engine);
    // Keep what was learned, drop the idle trace's state.
    Engine::new(OWN_UID, engine.floors().clone(), *engine.psys())
}

#[test]
fn battery_trace_matches_the_golden_attribution() {
    let mut engine = calibrated_engine();
    let windows = run("m0-battery-2min", &mut engine);
    assert!(
        !engine.psys_trusted(),
        "psys reads below the package on this machine"
    );
    assert!(windows.len() >= 10);

    let mut store = Store::open_in_memory().unwrap();
    for finished in &windows {
        let total: Joules = finished
            .window
            .ledger
            .values()
            .map(EnergySplit::total)
            .sum();
        assert!((total.0 - finished.window.measured.0).abs() < 1e-6 * finished.window.measured.0);
        store
            .record_window(&WindowRecord {
                start_ms: finished.start_ms,
                end_ms: finished.end_ms,
                window: &finished.window,
                model_version: MODEL_VERSION,
            })
            .unwrap();
    }
    let start = windows[0].start_ms;
    let end = windows.last().unwrap().end_ms;
    let usage = store
        .usage(start, end, SourceFilter::Only(PowerSource::Battery), end)
        .unwrap();

    let mut rendered = format!("# m0-battery-2min, model v{MODEL_VERSION}: top consumers (J)\n");
    for row in usage.iter().take(12) {
        let _ = writeln!(
            rendered,
            "{:<52} {:>9.1}",
            row.key.to_string(),
            row.split.total().0
        );
    }
    let golden =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden/m0-battery-2min.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &rendered).unwrap();
    }
    let expected = std::fs::read_to_string(&golden)
        .expect("golden file missing; run with UPDATE_GOLDEN=1 to create it");
    assert_eq!(rendered, expected);
}

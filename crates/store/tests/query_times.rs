//! Benchmark: times the history queries over 30 days of windows. Ignored by default; run with
//! `cargo test -p drainscope-store --release --test query_times -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use drainscope_model::{
    ClosedWindow, ConsumerKey, EnergySplit, FocusSplit, Joules, Ledger, Measurement, PowerSource,
};
use drainscope_store::{SourceFilter, Store, WindowRecord};

const NOW: i64 = 1_791_000_000_000;
const WINDOW_MS: i64 = 15_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
const DAYS: i64 = 30;
const APPS: usize = 20;

fn file_mb(path: &Path) -> f64 {
    let bytes = std::fs::metadata(path).map_or(0, |m| m.len());
    #[allow(clippy::cast_precision_loss)]
    let mb = bytes as f64 / 1_000_000.0;
    mb
}

/// The database plus its WAL, in MB.
fn db_mb(path: &Path) -> f64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    file_mb(path) + file_mb(Path::new(&wal))
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn history_query_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drainscope.db");
    let mut store = Store::open(&path).unwrap();

    let apps: Vec<ConsumerKey> = (0..APPS)
        .map(|i| ConsumerKey::App(format!("org.example.App{i}")))
        .collect();
    let others = [
        ConsumerKey::Idle,
        ConsumerKey::Devices,
        ConsumerKey::Kernel,
        ConsumerKey::SystemUnit("NetworkManager.service".into()),
        ConsumerKey::SystemUnit("systemd-journald.service".into()),
    ];

    let start = NOW - DAYS * DAY_MS;
    let count = (NOW - start) / WINDOW_MS;
    let seeding = Instant::now();
    for n in 0..count {
        let start_ms = start + n * WINDOW_MS;
        let day = n * WINDOW_MS / DAY_MS;
        // The last day is on battery, the one before on AC, and so on.
        let measurement = if (DAYS - 1 - day) % 2 == 0 {
            Measurement::Battery
        } else {
            Measurement::Rapl
        };
        let ledger: Ledger = apps
            .iter()
            .chain(&others)
            .enumerate()
            .map(|(i, key)| {
                let base = 0.05 + ((n * 7 + i as i64 * 13) % 50) as f64 * 0.02;
                (
                    key.clone(),
                    EnergySplit {
                        cpu: Joules(base),
                        gpu: Joules(base * 0.1),
                        other: Joules(base * 0.3),
                    },
                )
            })
            .collect();
        let measured = ledger.values().map(EnergySplit::total).sum();
        let closed = ClosedWindow {
            duration: Duration::from_secs(15),
            ledger,
            measured,
            measurement,
            shortfall: Joules(0.0),
        };
        let focus: BTreeMap<ConsumerKey, FocusSplit> = apps
            .iter()
            .take(10)
            .enumerate()
            .map(|(i, key)| {
                (
                    key.clone(),
                    FocusSplit {
                        foreground_j: 0.1 + i as f64 * 0.01,
                        unknown_j: 0.01,
                        focused: Duration::from_secs(5),
                    },
                )
            })
            .collect();
        store
            .record_window(&WindowRecord {
                start_ms,
                end_ms: start_ms + WINDOW_MS,
                window: &closed,
                model_version: 1,
                focus: &focus,
            })
            .unwrap();
    }
    println!(
        "\nSeeded {count} windows in {:.1} s; database + WAL {:.1} MB",
        seeding.elapsed().as_secs_f64(),
        db_mb(&path)
    );

    let pruning = Instant::now();
    let stats = store.prune(NOW).unwrap();
    println!(
        "Pruned in {:.1} s ({stats:?}); database + WAL {:.1} MB",
        pruning.elapsed().as_secs_f64(),
        db_mb(&path)
    );

    let ranges = [
        ("1 h", HOUR_MS),
        ("24 h", DAY_MS),
        ("7 d", 7 * DAY_MS),
        ("30 d", 30 * DAY_MS),
    ];
    let filters = [
        ("any", SourceFilter::Any),
        ("battery", SourceFilter::Only(PowerSource::Battery)),
    ];
    println!("\n| range | filter | query | median ms |\n|---|---|---|---|");
    for (range, width) in ranges {
        let since = NOW - width;
        for (filter_name, filter) in filters {
            let time = |query: &str, run: &mut dyn FnMut() -> usize| {
                let runs: Vec<Duration> = (0..5)
                    .map(|_| {
                        let t = Instant::now();
                        let size = run();
                        let took = t.elapsed();
                        assert!(size > 0, "{query} over {range} ({filter_name}) is empty");
                        took
                    })
                    .collect();
                println!(
                    "| {range} | {filter_name} | {query} | {:.2} |",
                    median(runs).as_secs_f64() * 1000.0
                );
            };
            time("usage", &mut || {
                store.usage(since, NOW, filter, NOW).unwrap().len()
            });
            time("focus", &mut || {
                store.focus(since, NOW, filter, NOW).unwrap().len()
            });
            time("covered_ms", &mut || {
                usize::try_from(store.covered_ms(since, NOW, filter, NOW).unwrap()).unwrap()
            });
        }
    }
}

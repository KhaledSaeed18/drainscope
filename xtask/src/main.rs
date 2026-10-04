//! Developer tooling for drainscope. Never shipped.

mod record;
mod spike;
mod trace;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "drainscope developer tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Record raw powercap, `power_supply`, procfs and cgroupfs snapshots into a gzipped JSONL trace.
    ///
    /// RAPL counters are root-only: build first, then run the binary with sudo, e.g.
    /// `cargo build -p xtask && sudo target/debug/xtask record-fixture m0-battery`.
    RecordFixture {
        /// Trace name; written to testdata/traces/<name>.jsonl.gz (or testdata/local/ with --local).
        name: String,
        /// Recording duration in seconds.
        #[arg(long, default_value_t = 600)]
        secs: u64,
        /// Interval between snapshots in milliseconds.
        #[arg(long, default_value_t = 2000)]
        interval_ms: u64,
        /// Write to the git-ignored testdata/local/ directory instead of testdata/traces/.
        #[arg(long)]
        local: bool,
    },
    /// M0 spike: throwaway per-consumer attribution over a recorded trace.
    SpikeAttribute {
        /// Path to a .jsonl.gz trace.
        trace: PathBuf,
        /// Number of consumers to list.
        #[arg(long, default_value_t = 25)]
        top: usize,
        /// Attribution model to compare.
        #[arg(long, value_enum, default_value_t = spike::Model::Marginal)]
        model: spike::Model,
    },
}

fn repo_root() -> Result<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask manifest has no parent directory")
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::RecordFixture {
            name,
            secs,
            interval_ms,
            local,
        } => {
            let options = record::Options {
                name,
                secs,
                interval_ms,
                local,
            };
            record::run(repo_root()?, &options).map(|_| ())
        }
        Command::SpikeAttribute { trace, top, model } => spike::run(&trace, top, model),
    }
}

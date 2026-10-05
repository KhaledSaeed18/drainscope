//! Developer tooling for drainscope. Never shipped.

mod install;
mod record;
mod spike;
mod trace;
mod trim;
mod validate;

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
    /// Install the sampler and its systemd, D-Bus, polkit and sysusers files for development
    /// (to /etc and /usr/local; needs root).
    InstallDev,
    /// Remove what install-dev installed (needs root).
    UninstallDev,
    /// Accuracy harness: known CPU loads in transient scopes, sampled against RAPL and the
    /// battery. Run unplugged, as your normal user; writes docs/validation.md.
    Validate {
        /// Idle baseline length in seconds.
        #[arg(long, default_value_t = 90)]
        idle: u64,
        /// Length of each load phase in seconds.
        #[arg(long, default_value_t = 45)]
        phase: u64,
        /// Allow running on AC (no battery figures), to check the harness itself.
        #[arg(long)]
        allow_ac: bool,
    },
    /// Cut a time window out of a trace into testdata/traces/, keeping only what the model reads.
    TrimTrace {
        /// Path to the source .jsonl.gz trace.
        input: PathBuf,
        /// Output name; written to testdata/traces/<name>.jsonl.gz.
        name: String,
        /// Window start, seconds from the first snapshot.
        #[arg(long, default_value_t = 0.0)]
        from: f64,
        /// Window end, seconds from the first snapshot.
        #[arg(long)]
        to: f64,
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
        Command::InstallDev => install::install(repo_root()?),
        Command::Validate {
            idle,
            phase,
            allow_ac,
        } => validate::run(
            repo_root()?,
            &validate::Options {
                idle_secs: idle,
                phase_secs: phase,
                allow_ac,
            },
        ),
        Command::UninstallDev => install::uninstall(),
        Command::TrimTrace {
            input,
            name,
            from,
            to,
        } => {
            let options = trim::Options {
                input,
                name,
                from_s: from,
                to_s: to,
            };
            trim::run(repo_root()?, &options).map(|_| ())
        }
    }
}

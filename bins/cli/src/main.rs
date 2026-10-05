//! drainscope command-line interface.
//!
//! Everything except `doctor` comes from the daemon over `Monitor1`; the CLI never reads the
//! database.

mod doctor;
mod format;
mod names;
mod report;

use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "drainscope",
    version,
    about = "Per-app battery and energy usage"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Battery used since the charger was unplugged, by consumer (the default).
    Summary,
    /// What drainscope can measure right now.
    Status,
    /// Energy per consumer over a period.
    Report {
        /// How far back: `unplug` (same as `summary`), or a duration like 90m, 24h, 7d.
        #[arg(long, default_value = "24h")]
        since: String,
        /// Group by consumer or by kind (app, unit, terminal, …).
        #[arg(long, value_enum, default_value_t = GroupBy::Consumer)]
        by: GroupBy,
        /// Only energy used on battery, or on AC.
        #[arg(long, value_enum, default_value_t = Source::Any)]
        source: Source,
        /// Rows to show before folding the rest.
        #[arg(long, default_value_t = 15)]
        top: usize,
    },
    /// Live power per consumer, updated every tick.
    Top {
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Which consumers keep waking the processor from idle (needs drainscope-probe).
    Wakeups {
        /// Rows to show.
        #[arg(long, default_value_t = 15)]
        top: usize,
    },
    /// Battery lost while asleep.
    Sleep {
        /// How far back, e.g. 7d.
        #[arg(long, default_value = "7d")]
        since: String,
    },
    /// Battery wear: full-charge capacity against design, and charge cycles.
    Health {
        /// How far back to look for the change in capacity, e.g. 365d.
        #[arg(long, default_value = "365d")]
        since: String,
    },
    /// Check that this machine is set up for drainscope.
    Doctor,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GroupBy {
    Consumer,
    Kind,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Source {
    Any,
    Battery,
    Ac,
}

impl Source {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Battery => "battery",
            Self::Ac => "ac",
        }
    }
}

async fn session_bus() -> anyhow::Result<zbus::Connection> {
    zbus::Connection::session()
        .await
        .context("connecting to the session bus")
}

async fn run(command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Summary => report::summary(&session_bus().await?).await?,
        Command::Status => report::status(&session_bus().await?).await?,
        Command::Report {
            since,
            by,
            source,
            top,
        } => {
            let bus = session_bus().await?;
            if since == "unplug" {
                report::summary(&bus).await?;
            } else {
                let since = format::parse_duration(&since)?;
                let by_kind = matches!(by, GroupBy::Kind);
                report::usage(&bus, since, by_kind, source.wire_name(), top).await?;
            }
        }
        Command::Top { top } => report::top(&session_bus().await?, top).await?,
        Command::Wakeups { top } => {
            report::wakeups(&session_bus().await?, top).await?;
        }
        Command::Sleep { since } => {
            report::sleep(&session_bus().await?, format::parse_duration(&since)?).await?;
        }
        Command::Health { since } => {
            report::health(&session_bus().await?, format::parse_duration(&since)?).await?;
        }
        Command::Doctor => {
            return Ok(if doctor::run().await {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command.unwrap_or(Command::Summary)).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("drainscope: {err:#}");
            ExitCode::FAILURE
        }
    }
}

//! drainscope command-line interface.
//!
//! Everything except `doctor` comes from the daemon over `Monitor1`; the CLI never reads the
//! database.

mod doctor;
mod export;
mod format;
mod names;
mod report;

use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

use crate::export::Format;

#[derive(Debug, Parser)]
#[command(
    name = "drainscope",
    version,
    about = "Per-app battery and energy usage"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Output format for summary, report, sleep, health, wakeups and network. JSON and CSV
    /// list every row, in joules, watts and Unix seconds.
    #[arg(long, value_enum, global = true, default_value_t = Format::Text)]
    format: Format,
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
    /// Network traffic by consumer (needs drainscope-probe).
    Network {
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

async fn run(command: Command, format: Format) -> anyhow::Result<ExitCode> {
    if format != Format::Text
        && matches!(
            command,
            Command::Status | Command::Top { .. } | Command::Doctor
        )
    {
        anyhow::bail!(
            "--format {} works with summary, report, sleep, health, wakeups and network",
            format
                .to_possible_value()
                .map_or_else(String::new, |v| v.get_name().to_owned())
        );
    }
    match command {
        Command::Summary => report::summary(&session_bus().await?, format).await?,
        Command::Status => report::status(&session_bus().await?).await?,
        Command::Report {
            since,
            by,
            source,
            top,
        } => {
            let bus = session_bus().await?;
            if since == "unplug" {
                report::summary(&bus, format).await?;
            } else {
                let since = format::parse_duration(&since)?;
                let by_kind = matches!(by, GroupBy::Kind);
                report::usage(&bus, since, by_kind, source.wire_name(), top, format).await?;
            }
        }
        Command::Top { top } => report::top(&session_bus().await?, top).await?,
        Command::Wakeups { top } => {
            report::wakeups(&session_bus().await?, top, format).await?;
        }
        Command::Network { top } => {
            report::network(&session_bus().await?, top, format).await?;
        }
        Command::Sleep { since } => {
            report::sleep(
                &session_bus().await?,
                format::parse_duration(&since)?,
                format,
            )
            .await?;
        }
        Command::Health { since } => {
            report::health(
                &session_bus().await?,
                format::parse_duration(&since)?,
                format,
            )
            .await?;
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
    match run(cli.command.unwrap_or(Command::Summary), cli.format).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("drainscope: {err:#}");
            ExitCode::FAILURE
        }
    }
}

//! Privileged, sandboxed RAPL sampler exposing Sampler1 on the system bus.
//!
//! Started by D-Bus activation and exits after [`IDLE_EXIT`] without calls, so it costs
//! nothing when unused. Runs as a dedicated system user with only `CAP_DAC_READ_SEARCH`
//! (see `data/systemd/drainscope-sampler.service`).

use std::time::Duration;

use anyhow::Context;
use drainscope_dbus::sampler::{BUS_NAME, OBJECT_PATH};
use drainscope_sampler::auth::Polkit;
use drainscope_sampler::{Config, Powercap, Sampler};
use drainscope_sys::SysRoot;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const IDLE_EXIT: Duration = Duration::from_secs(60);
const IDLE_CHECK: Duration = Duration::from_secs(5);

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let bus = zbus::Connection::system()
        .await
        .context("connecting to the system bus")?;
    let polkit = Polkit::new(&bus).await.context("connecting to polkit")?;
    let sampler = Sampler::new(
        Box::new(Powercap(SysRoot::host())),
        Box::new(polkit),
        Config::default(),
    )
    .context("reading RAPL counters")?;
    let last_call = sampler.last_call();
    bus.object_server()
        .at(OBJECT_PATH, sampler)
        .await
        .context("exporting Sampler1")?;
    bus.request_name(BUS_NAME)
        .await
        .with_context(|| format!("acquiring {BUS_NAME}"))?;
    tracing::info!("serving {BUS_NAME}");

    while last_call.idle_for() < IDLE_EXIT {
        tokio::time::sleep(IDLE_CHECK).await;
    }
    tracing::info!("idle for {}s, exiting", IDLE_EXIT.as_secs());
    Ok(())
}

/// Logs to the journal when running under systemd, to stderr otherwise.
fn init_logging() {
    let registry = tracing_subscriber::registry();
    match tracing_journald::layer() {
        Ok(journald) => registry.with(journald).init(),
        Err(_) => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
}

//! User daemon: collects, attributes, stores and serves Monitor1 on the session bus.
//!
//! Runs as a `systemd --user` service with no privileges. RAPL counters come from the
//! sampler on the system bus when available; everything else is read directly.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context;
use drainscope_daemon::collector::{Collector, monotonic_now, wall_now_ms};
use drainscope_daemon::engine::{Engine, NetworkCounters, Reading, TickOutcome};
use drainscope_daemon::lock::{EXIT_ALREADY_RUNNING, lock_database};
use drainscope_daemon::monitor::{Monitor, Shared, Status};
use drainscope_daemon::power::{BatteryLevel, PowerTracker, SleepTracker};
use drainscope_daemon::probe::{ProbeClient, ProbeReadings};
use drainscope_daemon::rapl::RaplClient;
use drainscope_daemon::ticker::Ticker;
use drainscope_dbus::monitor::{BUS_NAME, OBJECT_PATH};
use drainscope_model::{
    BatteryHealth, MODEL_VERSION, PowerSource, RaplDomain, RateTracker, Resolver, health,
};
use drainscope_store::{PowerEvent, PowerEventKind, Store, WindowRecord};
use drainscope_sys::sleep::{PrepareForSleep, PrepareForSleepStream};
use drainscope_sys::{
    Login1ManagerProxy, SysRoot, read_batteries, read_mem_sleep, read_wakeup_irq,
    read_wakeup_sources,
};
use futures_util::StreamExt;
use tokio::signal::unix::{SignalKind, signal};
use tracing::Level;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Battery readings lag 6–8 s and windows are 10 s, so faster ticks add no accuracy; at 5 s
/// the daemon costs ≈ 0.4% of one CPU, at 2 s it would exceed the 0.5% budget.
const TICK: Duration = Duration::from_secs(5);
const SAVE_EVERY: Duration = Duration::from_secs(300);
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

struct Daemon {
    shared: Arc<Shared>,
    session: zbus::Connection,
    /// `None` only while a collection runs on the blocking pool.
    collector: Option<Collector>,
    engine: Engine,
    rapl: RaplClient,
    probe: ProbeClient,
    /// Idle exits per consumer from the probe, and when it was last read.
    wakeups: RateTracker,
    last_probe_at: Option<Duration>,
    /// Bytes received and sent per consumer from the probe, and when it was last read.
    received: RateTracker,
    sent: RateTracker,
    last_network_at: Option<Duration>,
    own_uid: u32,
    power: PowerTracker,
    sleep: SleepTracker,
    logind: Option<Login1ManagerProxy<'static>>,
    /// Held to delay suspend until the battery level is recorded; closing it releases it.
    inhibitor: Option<zbus::zvariant::OwnedFd>,
    last_save: Instant,
    last_prune: Option<Instant>,
    /// From the latest snapshot; saved with the calibration.
    health: Vec<BatteryHealth>,
}

impl Daemon {
    fn store(&self) -> MutexGuard<'_, Store> {
        self.shared
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn tick(&mut self) -> anyhow::Result<()> {
        let taken_at = monotonic_now();
        let wall_ms = wall_now_ms();
        let mut collector = self.collector.take().context("collector busy")?;
        // Everything a tick reads, in flight together: replies that arrive close together cost
        // one wakeup instead of one each.
        let (rapl, readings, collection) = tokio::join!(
            self.rapl.read(),
            self.probe.read_all(),
            tokio::task::spawn_blocking(move || {
                let collected = collector.collect(taken_at);
                (collector, collected)
            }),
        );
        let ProbeReadings {
            wakeups: probe,
            traffic,
            network_time,
        } = readings;
        let (collector, collected) = collection?;
        self.collector = Some(collector);
        let mut collected = collected?;

        let level = BatteryLevel::of(&collected.snapshot.batteries);
        self.health = health(&collected.snapshot.batteries);
        if let Some(event) = self.power.update(&level, wall_ms) {
            tracing::info!(kind = ?event.kind, "power source changed");
            self.store().record_power_event(&event)?;
        }

        let generation = rapl.as_ref().map(|r| r.generation);
        if let Some(reading) = rapl {
            collected.snapshot.rapl = reading.counters;
        }
        if let Some(reading) = probe {
            let elapsed = self
                .last_probe_at
                .map_or(Duration::ZERO, |at| taken_at.saturating_sub(at));
            self.last_probe_at = Some(taken_at);
            let resolver = Resolver {
                own_uid: self.own_uid,
                terminal_labels: &collected.terminal_labels,
            };
            self.wakeups
                .observe(reading.generation, reading.wakeups, elapsed, &resolver);
        }
        // Model v2/v3 input: bytes and softirq time from the same probe generation, and the
        // network IRQ threads' time from procfs.
        let network = match (&traffic, network_time) {
            (Some(traffic), Some((generation, softirq_ns))) if traffic.generation == generation => {
                let mut bytes = traffic.received.clone();
                for (path, sent) in &traffic.sent {
                    *bytes.entry(path.clone()).or_default() += sent;
                }
                Some(NetworkCounters {
                    generation,
                    bytes,
                    softirq_ns,
                    irq_thread_ns: std::mem::take(&mut collected.network_irq_ns),
                })
            }
            _ => None,
        };
        if let Some(reading) = traffic {
            let elapsed = self
                .last_network_at
                .map_or(Duration::ZERO, |at| taken_at.saturating_sub(at));
            self.last_network_at = Some(taken_at);
            let resolver = Resolver {
                own_uid: self.own_uid,
                terminal_labels: &collected.terminal_labels,
            };
            self.received
                .observe(reading.generation, reading.received, elapsed, &resolver);
            self.sent
                .observe(reading.generation, reading.sent, elapsed, &resolver);
        }
        self.update_live_state(&level, &collected.snapshot.rapl);
        let outcome = self.engine.tick(Reading {
            snapshot: collected.snapshot,
            wall_ms,
            rapl_generation: generation,
            terminal_labels: collected.terminal_labels,
            network,
        });
        if let Some(outcome) = outcome {
            self.finish_tick(outcome).await?;
        }

        if self.last_save.elapsed() >= SAVE_EVERY {
            self.save_state();
        }
        if self.last_prune.is_none_or(|at| at.elapsed() >= PRUNE_EVERY) {
            let stats = self.store().prune(wall_ms)?;
            tracing::debug!(?stats, "pruned history");
            self.last_prune = Some(Instant::now());
        }
        Ok(())
    }

    fn update_live_state(
        &self,
        level: &BatteryLevel,
        rapl: &BTreeMap<RaplDomain, drainscope_model::snapshot::EnergyCounter>,
    ) {
        let status = match (self.rapl.available(), level.on_battery) {
            (true, _) => Status::Full,
            (false, true) => Status::BatteryOnly,
            (false, false) => Status::TimeOnly,
        };
        let psys_trusted = self.engine.psys_trusted();
        let domains = rapl
            .keys()
            .filter(|d| **d != RaplDomain::Psys || psys_trusted)
            .map(|d| d.wire_name().to_owned())
            .collect();
        let mut live = self
            .shared
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        live.on_battery = level.on_battery;
        live.capacity = level.capacity;
        live.status = status;
        live.domains = domains;
        live.wakeups = self.probe.available().then(|| {
            self.wakeups
                .rates()
                .into_iter()
                .map(|(key, rate)| (key.to_string(), rate))
                .collect()
        });
        live.network = self
            .probe
            .network_available()
            .then(|| merge_traffic(&self.received.rates(), &self.sent.rates()));
    }

    async fn finish_tick(&self, outcome: TickOutcome) -> anyhow::Result<()> {
        if let Some(finished) = &outcome.finished {
            self.store().record_window(&WindowRecord {
                start_ms: finished.start_ms,
                end_ms: finished.end_ms,
                window: &finished.window,
                model_version: MODEL_VERSION,
            })?;
        }
        let usage = outcome
            .live
            .into_iter()
            .filter(|(_, watts)| watts.0 > 0.0)
            .map(|(key, watts)| (key.to_string(), watts.0))
            .collect();
        let duration_ms = u32::try_from(outcome.duration.as_millis()).unwrap_or(u32::MAX);
        let monitor = self
            .session
            .object_server()
            .interface::<_, Monitor>(OBJECT_PATH)
            .await?;
        Monitor::tick(monitor.signal_emitter(), duration_ms, usage).await?;
        Ok(())
    }

    /// Persists what was learned (idle floors, psys verdict counts) and battery health.
    fn save_state(&mut self) {
        let result = (|| {
            let mut store = self.store();
            if !self.health.is_empty() {
                store.record_battery_health(wall_now_ms(), &self.health)?;
            }
            for source in [PowerSource::Battery, PowerSource::Ac] {
                if let Some(estimator) = self.engine.floors().get(&source) {
                    store.save_floor(source, estimator)?;
                }
            }
            store.save_psys_check(self.engine.psys())
        })();
        if let Err(err) = result {
            tracing::warn!(%err, "saving calibration failed");
        }
        self.last_save = Instant::now();
    }

    /// If the machine runs on battery but no unplug was recorded since the last plug-in (it
    /// was unplugged while the daemon wasn't running), "since unplugged" starts now.
    fn note_startup_power(&self, level: &BatteryLevel) -> anyhow::Result<()> {
        if !level.on_battery {
            return Ok(());
        }
        let store = self.store();
        let unplug = store.last_power_event(PowerEventKind::Unplug)?;
        let plug = store.last_power_event(PowerEventKind::Plug)?;
        let missing = match (unplug, plug) {
            (None, _) => true,
            (Some(unplug), Some(plug)) => plug.ts_ms >= unplug.ts_ms,
            (Some(_), None) => false,
        };
        if missing {
            store.record_power_event(&battery_event(PowerEventKind::Unplug, level))?;
        }
        Ok(())
    }

    async fn take_inhibitor(&mut self) {
        let Some(logind) = &self.logind else {
            return;
        };
        match logind
            .inhibit(
                "sleep",
                "drainscope",
                "Recording the battery level before sleep",
                "delay",
            )
            .await
        {
            Ok(fd) => self.inhibitor = Some(fd),
            Err(err) => {
                tracing::warn!(%err, "no sleep inhibitor; sleep sessions won't be recorded");
            }
        }
    }

    fn before_sleep(&mut self) {
        let level = BatteryLevel::of(&read_batteries(&SysRoot::host()).unwrap_or_default());
        let mode = read_mem_sleep(&SysRoot::host()).ok().flatten();
        let wakeups = read_wakeup_sources(&SysRoot::host());
        self.sleep.before_sleep(wall_now_ms(), level, mode, wakeups);
        if let Err(err) = self
            .store()
            .record_power_event(&battery_event(PowerEventKind::Suspend, &level))
        {
            tracing::warn!(%err, "recording suspend failed");
        }
        self.save_state();
        // Closing the fd lets the suspend proceed.
        self.inhibitor = None;
    }

    async fn after_resume(&mut self) {
        // An interval spanning the sleep would be meaningless.
        self.engine.reset();
        self.wakeups.reset();
        self.last_probe_at = None;
        self.received.reset();
        self.sent.reset();
        self.last_network_at = None;
        let level = BatteryLevel::of(&read_batteries(&SysRoot::host()).unwrap_or_default());
        let root = SysRoot::host();
        let session = self.sleep.after_resume(
            wall_now_ms(),
            level,
            &read_wakeup_sources(&root),
            read_wakeup_irq(&root).as_ref(),
        );
        let result = (|| {
            let store = self.store();
            store.record_power_event(&battery_event(PowerEventKind::Resume, &level))?;
            if let Some(session) = &session {
                tracing::info!(wh_lost = ?session.wh_lost, wake = ?session.wake_reason, "recorded sleep session");
                store.record_sleep_session(session)?;
            }
            anyhow::Ok(())
        })();
        if let Err(err) = result {
            tracing::warn!(%err, "recording resume failed");
        }
        self.take_inhibitor().await;
    }
}

fn battery_event(kind: PowerEventKind, level: &BatteryLevel) -> PowerEvent {
    PowerEvent {
        ts_ms: wall_now_ms(),
        kind,
        battery_percent: level.percent(),
        energy_wh: level.energy.map(|j| j.0 / 3600.0),
    }
}

/// `$XDG_STATE_HOME/drainscope/drainscope.db`, the directory created `0700`.
fn database_path() -> anyhow::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .context("neither XDG_STATE_HOME nor HOME is set")?;
    let dir = state.join("drainscope");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir.join("drainscope.db"))
}

async fn next_sleep_signal(stream: &mut Option<PrepareForSleepStream>) -> Option<PrepareForSleep> {
    match stream {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let path = database_path()?;
    let Some(_lock) =
        lock_database(&path).with_context(|| format!("locking {}", path.display()))?
    else {
        tracing::error!(database = %path.display(), "another drainscope daemon is using the database");
        std::process::exit(EXIT_ALREADY_RUNNING);
    };
    let store = Store::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let floors = BTreeMap::from([
        (
            PowerSource::Battery,
            store.load_floor(PowerSource::Battery)?,
        ),
        (PowerSource::Ac, store.load_floor(PowerSource::Ac)?),
    ]);
    let psys = store.load_psys_check()?;
    let engine = Engine::new(rustix::process::getuid().as_raw(), floors, psys);
    let shared = Shared::new(store);

    let session = zbus::Connection::session()
        .await
        .context("connecting to the session bus")?;
    session
        .object_server()
        .at(OBJECT_PATH, Monitor::new(Arc::clone(&shared), wall_now_ms))
        .await?;
    session
        .request_name(BUS_NAME)
        .await
        .with_context(|| format!("acquiring {BUS_NAME} (already running?)"))?;

    let system = zbus::Connection::system()
        .await
        .map_err(|err| tracing::warn!(%err, "no system bus: no RAPL, no sleep tracking"))
        .ok();
    let rapl = RaplClient::connect(system.as_ref()).await;
    let probe = ProbeClient::connect(system.as_ref()).await;
    let logind = match &system {
        // Only its signal and Inhibit are used; caching properties would subscribe to every
        // PropertiesChanged logind emits.
        Some(bus) => Login1ManagerProxy::builder(bus)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|err| tracing::warn!(%err, "no logind"))
            .ok(),
        None => None,
    };
    let sleep_signals = match &logind {
        Some(logind) => Some(logind.receive_prepare_for_sleep().await?),
        None => None,
    };

    let mut daemon = Daemon {
        shared,
        session,
        collector: Some(Collector::new(SysRoot::host())),
        engine,
        rapl,
        probe,
        wakeups: RateTracker::default(),
        last_probe_at: None,
        received: RateTracker::default(),
        sent: RateTracker::default(),
        last_network_at: None,
        own_uid: rustix::process::getuid().as_raw(),
        power: PowerTracker::default(),
        sleep: SleepTracker::default(),
        logind,
        inhibitor: None,
        last_save: Instant::now(),
        health: Vec::new(),
        last_prune: None,
    };
    daemon.take_inhibitor().await;
    let level = BatteryLevel::of(&read_batteries(&SysRoot::host()).unwrap_or_default());
    daemon.note_startup_power(&level)?;
    tracing::info!(database = %path.display(), "serving {BUS_NAME}");

    serve(&mut daemon, sleep_signals).await?;
    daemon.save_state();
    tracing::info!("stopped");
    Ok(())
}

/// Ticks, follows suspend and resume, and serves Monitor1 until asked to stop or the session
/// bus goes away.
async fn serve(
    daemon: &mut Daemon,
    mut sleep_signals: Option<PrepareForSleepStream>,
) -> anyhow::Result<()> {
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    // Started by D-Bus outside systemd (e.g. under dbus-run-session), nothing else stops us
    // when that bus goes away; there are no clients left to serve.
    let session_bus = daemon.session.clone();
    let ticker = Ticker::new(TICK).context("creating the tick timer")?;
    loop {
        tokio::select! {
            () = session_bus.closed() => {
                tracing::info!("session bus closed");
                break;
            }
            expired = ticker.tick() => {
                let result = match expired {
                    Ok(_) => daemon.tick().await,
                    Err(err) => Err(err.into()),
                };
                if let Err(err) = result {
                    tracing::warn!(%err, "tick failed");
                }
            }
            Some(signal) = next_sleep_signal(&mut sleep_signals) => match signal.args() {
                Ok(args) if args.start => daemon.before_sleep(),
                Ok(_) => daemon.after_resume().await,
                Err(err) => tracing::warn!(%err, "bad PrepareForSleep signal"),
            },
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }
    Ok(())
}

/// Whether stderr is the journal stream systemd connected us to. `JOURNAL_STREAM` is
/// inherited by children, so it only counts if its `device:inode` is stderr's (systemd.exec(5)).
fn stderr_is_journal() -> bool {
    let Some(stream) = std::env::var("JOURNAL_STREAM").ok() else {
        return false;
    };
    let Ok(stat) = rustix::fs::fstat(std::io::stderr()) else {
        return false;
    };
    stream == format!("{}:{}", stat.st_dev, stat.st_ino)
}

/// Logs to the journal when systemd runs us, to stderr otherwise.
fn init_logging() {
    let filter = Targets::new()
        .with_default(Level::INFO)
        .with_target("zbus", Level::WARN);
    let registry = tracing_subscriber::registry().with(filter);
    if stderr_is_journal()
        && let Ok(journald) = tracing_journald::layer()
    {
        registry.with(journald).init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}

/// (key, received B/s, sent B/s), busiest first.
fn merge_traffic(
    received: &[(drainscope_model::ConsumerKey, f64)],
    sent: &[(drainscope_model::ConsumerKey, f64)],
) -> Vec<(String, f64, f64)> {
    let mut by_key: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for (key, rate) in received {
        by_key.entry(key.to_string()).or_default().0 += rate;
    }
    for (key, rate) in sent {
        by_key.entry(key.to_string()).or_default().1 += rate;
    }
    let mut rows: Vec<(String, f64, f64)> = by_key
        .into_iter()
        .map(|(key, (received, sent))| (key, received, sent))
        .collect();
    rows.sort_by(|a, b| {
        (b.1 + b.2)
            .total_cmp(&(a.1 + a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    rows
}

//! The `Probe1` D-Bus interface.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use drainscope_access::{Authorizer, RateLimiter, caller_uid};
use drainscope_dbus::probe::ProbeError;
use drainscope_sys::{SysRoot, cgroup_ids};
use zbus::message::Header;

use crate::counters::{NetworkSource, WakeupSource};
use crate::visibility::visible;

/// Cgroup paths by ID: cgroupfs in production, fixtures in tests.
pub trait CgroupNames: Send + Sync {
    /// # Errors
    /// If the cgroup hierarchy can't be listed.
    fn ids(&self) -> anyhow::Result<BTreeMap<u64, String>>;
}

#[derive(Debug)]
pub struct Cgroupfs(pub SysRoot);

impl CgroupNames for Cgroupfs {
    fn ids(&self) -> anyhow::Result<BTreeMap<u64, String>> {
        Ok(cgroup_ids(&self.0)?
            .into_iter()
            .map(|(id, path)| (id, path.as_str().to_owned()))
            .collect())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub min_interval: Duration,
    /// The UID assumed for peer-to-peer callers (tests); bus callers are asked about.
    pub peer_uid: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(1),
            peer_uid: None,
        }
    }
}

/// When the service was last called, shared with the idle-exit loop.
#[derive(Debug, Clone)]
pub struct LastCall(Arc<Mutex<Instant>>);

impl LastCall {
    fn now() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }

    fn touch(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    #[must_use]
    pub fn idle_for(&self) -> Duration {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }
}

struct State {
    limiter: RateLimiter,
    /// Cgroup paths by ID, refreshed when a counter's cgroup is unknown.
    names: BTreeMap<u64, String>,
}

pub struct Probe {
    source: Box<dyn WakeupSource>,
    /// `None` when the network programs couldn't be attached.
    network: Option<Box<dyn NetworkSource>>,
    cgroups: Box<dyn CgroupNames>,
    authorizer: Box<dyn Authorizer>,
    config: Config,
    /// Changes on every start, so clients never compare totals across restarts.
    generation: u64,
    state: Mutex<State>,
    last_call: LastCall,
}

impl Probe {
    #[must_use]
    pub fn new(
        source: Box<dyn WakeupSource>,
        network: Option<Box<dyn NetworkSource>>,
        cgroups: Box<dyn CgroupNames>,
        authorizer: Box<dyn Authorizer>,
        config: Config,
    ) -> Self {
        Self {
            source,
            network,
            cgroups,
            authorizer,
            config,
            generation: monotonic_ns(),
            state: Mutex::new(State {
                limiter: RateLimiter::new(config.min_interval),
                names: BTreeMap::new(),
            }),
            last_call: LastCall::now(),
        }
    }

    #[must_use]
    pub fn last_call(&self) -> LastCall {
        self.last_call.clone()
    }

    /// Authorizes and rate-limits a call to `method`; returns the caller's UID. Each method
    /// has its own limit, so a client can read everything once per interval.
    async fn admit(
        &self,
        header: &Header<'_>,
        connection: &zbus::Connection,
        method: &str,
    ) -> Result<Option<u32>, ProbeError> {
        self.last_call.touch();
        let caller = header.sender().map(|name| name.as_str().to_owned());
        let uid = self.authorize(connection, caller.as_deref()).await?;
        let key = match uid {
            Some(uid) => format!("uid:{uid}:{method}"),
            None => format!("peer:{method}"),
        };
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .limiter
            .check(&key, Instant::now())
            .map_err(|wait| ProbeError::RateLimited(format!("retry in {} ms", wait.as_millis())))?;
        Ok(uid)
    }

    /// Sums `values` (by cgroup ID) per cgroup path the caller may see. Counts of removed
    /// cgroups (unknown IDs) are dropped: nothing can be attributed to them.
    fn by_path<T: Copy>(
        &self,
        values: &BTreeMap<u64, T>,
        uid: Option<u32>,
        mut add: impl FnMut(&mut T, T),
        zero: T,
    ) -> BTreeMap<String, T> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if values.keys().any(|id| !state.names.contains_key(id)) {
            match self.cgroups.ids() {
                Ok(names) => state.names = names,
                Err(err) => tracing::warn!(%err, "listing cgroups failed"),
            }
        }
        let mut by_path: BTreeMap<String, T> = BTreeMap::new();
        for (id, value) in values {
            if let Some(path) = state.names.get(id).filter(|path| visible(path, uid)) {
                add(by_path.entry(path.clone()).or_insert(zero), *value);
            }
        }
        by_path
    }

    async fn authorize(
        &self,
        connection: &zbus::Connection,
        caller: Option<&str>,
    ) -> Result<Option<u32>, ProbeError> {
        let authorized = self.authorizer.authorize(caller).await.map_err(|err| {
            // Logged here; callers only learn that the check failed.
            tracing::error!(%err, "polkit check failed");
            ProbeError::Failed("authorization check failed".into())
        })?;
        if !authorized {
            tracing::warn!(caller = caller.unwrap_or("peer"), "denied by polkit");
            return Err(ProbeError::NotAuthorized(
                "not allowed by polkit (active local sessions only)".into(),
            ));
        }
        match caller_uid(connection, caller).await {
            Ok(Some(uid)) => Ok(Some(uid)),
            Ok(None) => Ok(self.config.peer_uid),
            Err(err) => {
                tracing::error!(%err, caller, "looking up the caller's uid failed");
                Err(ProbeError::Failed("identifying the caller failed".into()))
            }
        }
    }
}

fn monotonic_ns() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let secs = u64::try_from(now.tv_sec).unwrap_or_default();
    let nanos = u64::try_from(now.tv_nsec).unwrap_or_default();
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}

#[zbus::interface(name = "io.github.khaledsaeed18.Drainscope.Probe1")]
impl Probe {
    // The tuple is spelled out so the macro emits three out arguments, as the contract requires.
    #[zbus(out_args("monotonic_ns", "generation", "wakeups"))]
    async fn read_wakeups(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(u64, u64, Vec<(String, u64)>), ProbeError> {
        let uid = self.admit(&header, connection, "ReadWakeups").await?;
        let counts = self
            .source
            .read()
            .map_err(|err| ProbeError::Failed(err.to_string()))?;
        let wakeups = self.by_path(&counts, uid, |sum, count| *sum += count, 0);
        Ok((
            monotonic_ns(),
            self.generation,
            wakeups.into_iter().collect(),
        ))
    }

    #[zbus(out_args("monotonic_ns", "generation", "traffic"))]
    async fn read_network(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(u64, u64, Vec<(String, u64, u64)>), ProbeError> {
        let uid = self.admit(&header, connection, "ReadNetwork").await?;
        let network = self
            .network
            .as_ref()
            .ok_or_else(|| ProbeError::Unsupported("network counting isn't available".into()))?;
        let traffic = network
            .read()
            .map_err(|err| ProbeError::Failed(err.to_string()))?;
        let mut both: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
        for (id, bytes) in &traffic.received {
            both.entry(*id).or_default().0 += bytes;
        }
        for (id, bytes) in &traffic.sent {
            both.entry(*id).or_default().1 += bytes;
        }
        let by_path = self.by_path(
            &both,
            uid,
            |sum, (received, sent)| {
                sum.0 += received;
                sum.1 += sent;
            },
            (0, 0),
        );
        Ok((
            monotonic_ns(),
            self.generation,
            by_path
                .into_iter()
                .map(|(path, (received, sent))| (path, received, sent))
                .collect(),
        ))
    }

    #[zbus(out_args("monotonic_ns", "generation", "tx_ns", "rx_ns"))]
    async fn read_network_time(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(u64, u64, u64, u64), ProbeError> {
        self.admit(&header, connection, "ReadNetworkTime").await?;
        let network = self
            .network
            .as_ref()
            .ok_or_else(|| ProbeError::Unsupported("network counting isn't available".into()))?;
        let (tx_ns, rx_ns) = network
            .read()
            .map_err(|err| ProbeError::Failed(err.to_string()))?
            .softirq_ns;
        Ok((monotonic_ns(), self.generation, tx_ns, rx_ns))
    }

    #[zbus(property)]
    fn min_interval_ms(&self) -> u32 {
        u32::try_from(self.config.min_interval.as_millis()).unwrap_or(u32::MAX)
    }
}

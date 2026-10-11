//! The `Sampler1` D-Bus interface.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use drainscope_dbus::sampler::SamplerError;
use drainscope_sys::{PowercapZone, SysError, SysRoot, read_zones};
use zbus::message::Header;

use crate::accumulator::Accumulator;
use drainscope_access::{Authorizer, DENIAL_WINDOW, DenialGate, RateLimiter, caller_uid, quantize};

/// Where counters come from: powercap in production, fixtures in tests.
pub trait CounterSource: Send + Sync {
    /// # Errors
    /// Reading the counters failed.
    fn read(&self) -> Result<Vec<PowercapZone>, SysError>;
}

#[derive(Debug)]
pub struct Powercap(pub SysRoot);

impl CounterSource for Powercap {
    fn read(&self) -> Result<Vec<PowercapZone>, SysError> {
        read_zones(&self.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub min_interval: Duration,
    pub quantum_uj: u64,
}

impl Default for Config {
    /// 1 s and 10 mJ: ≈ 0.1% error at 10 W, and far coarser than the Platypus attack needs.
    fn default() -> Self {
        Self {
            min_interval: Duration::from_secs(1),
            quantum_uj: 10_000,
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
    accumulator: Accumulator,
    limiter: RateLimiter,
    denials: DenialGate,
}

pub struct Sampler {
    source: Box<dyn CounterSource>,
    authorizer: Box<dyn Authorizer>,
    config: Config,
    /// Changes on every start, so clients never compare totals across restarts.
    generation: u64,
    domains: Vec<String>,
    state: Mutex<State>,
    last_call: LastCall,
}

impl Sampler {
    /// Takes a first reading as the zero point for the totals.
    ///
    /// # Errors
    /// If the counters can't be read.
    pub fn new(
        source: Box<dyn CounterSource>,
        authorizer: Box<dyn Authorizer>,
        config: Config,
    ) -> Result<Self, SysError> {
        let zones = source.read()?;
        let mut accumulator = Accumulator::default();
        let domains = accumulator
            .update(&zones)
            .keys()
            .map(|d| d.wire_name().to_owned())
            .collect();
        Ok(Self {
            source,
            authorizer,
            config,
            generation: monotonic_ns(),
            domains,
            state: Mutex::new(State {
                accumulator,
                limiter: RateLimiter::new(config.min_interval),
                denials: DenialGate::new(DENIAL_WINDOW),
            }),
            last_call: LastCall::now(),
        })
    }

    #[must_use]
    pub fn last_call(&self) -> LastCall {
        self.last_call.clone()
    }
}

fn monotonic_ns() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let secs = u64::try_from(now.tv_sec).unwrap_or_default();
    let nanos = u64::try_from(now.tv_nsec).unwrap_or_default();
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// The caller's UID, or `None` on peer-to-peer connections (tests), which have no bus to ask.
/// Rate limits and the denial gate apply per user, not per connection: every new connection
/// gets a new unique name, so keying by name would let a caller reconnect around them.
async fn caller_uid_or_failed(
    connection: &zbus::Connection,
    caller: Option<&str>,
) -> Result<Option<u32>, SamplerError> {
    caller_uid(connection, caller).await.map_err(|err| {
        tracing::error!(%err, caller, "looking up the caller's uid failed");
        SamplerError::Failed("identifying the caller failed".into())
    })
}

#[zbus::interface(name = "io.github.khaledsaeed18.Drainscope.Sampler1")]
impl Sampler {
    // The tuple is spelled out (not the `Counters` alias) so the macro emits three out
    // arguments, as the contract requires, instead of one struct.
    #[zbus(out_args("monotonic_ns", "generation", "counters"))]
    async fn read_counters(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(u64, u64, Vec<(String, u64)>), SamplerError> {
        let caller = header.sender().map(|name| name.as_str().to_owned());
        let uid = caller_uid_or_failed(connection, caller.as_deref()).await?;
        let not_authorized = || {
            SamplerError::NotAuthorized("not allowed by polkit (active local sessions only)".into())
        };
        if let Some(uid) = uid {
            let refused = self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .denials
                .refused(uid, Instant::now());
            if refused {
                tracing::debug!(uid, "refused without asking polkit");
                return Err(not_authorized());
            }
        }
        let authorized = self
            .authorizer
            .authorize(caller.as_deref())
            .await
            .map_err(|err| {
                // Logged here; callers only learn that the check failed.
                tracing::error!(%err, "polkit check failed");
                SamplerError::Failed("authorization check failed".into())
            })?;
        if !authorized {
            if let Some(uid) = uid {
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .denials
                    .deny(uid, Instant::now());
            }
            tracing::warn!(
                caller = caller.as_deref().unwrap_or("peer"),
                "denied by polkit"
            );
            return Err(not_authorized());
        }
        // Only authorized calls keep the service from exiting when idle.
        self.last_call.touch();

        let limit_key = uid.map_or_else(|| "peer".to_owned(), |uid| format!("uid:{uid}"));
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .limiter
            .check(&limit_key, Instant::now())
            .map_err(|wait| {
                SamplerError::RateLimited(format!("retry in {} ms", wait.as_millis()))
            })?;
        let zones = self
            .source
            .read()
            .map_err(|err| SamplerError::Failed(err.to_string()))?;
        if zones.is_empty() {
            return Err(SamplerError::Unsupported("no RAPL zones".into()));
        }
        let counters = state
            .accumulator
            .update(&zones)
            .iter()
            .map(|(domain, total)| {
                (
                    domain.wire_name().to_owned(),
                    quantize(*total, self.config.quantum_uj),
                )
            })
            .collect();
        Ok((monotonic_ns(), self.generation, counters))
    }

    #[zbus(property)]
    fn domains(&self) -> Vec<String> {
        self.domains.clone()
    }

    #[zbus(property)]
    fn min_interval_ms(&self) -> u32 {
        u32::try_from(self.config.min_interval.as_millis()).unwrap_or(u32::MAX)
    }

    #[zbus(property)]
    fn quantum_uj(&self) -> u64 {
        self.config.quantum_uj
    }
}

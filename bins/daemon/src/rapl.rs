//! RAPL counters from the sampler (Sampler1 on the system bus).
//!
//! Without the sampler the daemon still works in a degraded mode (ADR 0001), so failures are
//! states, not errors. Refusals back off instead of retrying every tick.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use drainscope_dbus::sampler::{Sampler1Proxy, SamplerError};
use drainscope_model::Microjoules;
use drainscope_model::snapshot::{EnergyCounter, RaplDomain};

/// Retry delay after polkit refused us or the machine has no RAPL: neither changes quickly.
const REFUSED_BACKOFF: Duration = Duration::from_secs(300);
/// Retry delay after other failures (sampler not installed, bus hiccups).
const FAILURE_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct RaplReading {
    /// Changes whenever the sampler's totals restart.
    pub generation: u64,
    pub counters: BTreeMap<RaplDomain, EnergyCounter>,
}

/// Turns `(domain, cumulative µJ)` pairs into counters. Unknown domains are ignored. The
/// sampler's totals never wrap within a generation.
#[must_use]
pub fn counters_from_wire(pairs: &[(String, u64)]) -> BTreeMap<RaplDomain, EnergyCounter> {
    pairs
        .iter()
        .filter_map(|(name, uj)| {
            let domain = RaplDomain::from_wire_name(name)?;
            Some((
                domain,
                EnergyCounter {
                    value: Microjoules(*uj),
                    range: Microjoules(u64::MAX),
                },
            ))
        })
        .collect()
}

#[derive(Debug)]
pub struct RaplClient {
    proxy: Option<Sampler1Proxy<'static>>,
    /// Why the last attempt failed; `None` while readings arrive.
    problem: Option<String>,
    retry_at: Option<Instant>,
}

impl RaplClient {
    /// A client on `system_bus`; without one, RAPL is simply unavailable.
    pub async fn connect(system_bus: Option<&zbus::Connection>) -> Self {
        let proxy = match system_bus {
            Some(bus) => Sampler1Proxy::builder(bus)
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await
                .map_err(|err| tracing::warn!(%err, "no Sampler1 proxy"))
                .ok(),
            None => None,
        };
        let problem = proxy.is_none().then(|| "no system bus".to_owned());
        Self {
            proxy,
            problem,
            retry_at: None,
        }
    }

    /// Whether the last attempt produced counters.
    #[must_use]
    pub fn available(&self) -> bool {
        self.problem.is_none()
    }

    pub async fn read(&mut self) -> Option<RaplReading> {
        let proxy = self.proxy.as_ref()?;
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return None;
        }
        match proxy.read_counters().await {
            Ok((_, generation, pairs)) => {
                if let Some(problem) = self.problem.take() {
                    tracing::info!(previously = problem, "RAPL counters available");
                }
                self.retry_at = None;
                Some(RaplReading {
                    generation,
                    counters: counters_from_wire(&pairs),
                })
            }
            // Ticks are slower than the limit, so this is only clock jitter.
            Err(SamplerError::RateLimited(_)) => None,
            Err(err) => {
                let backoff = match err {
                    SamplerError::NotAuthorized(_) | SamplerError::Unsupported(_) => {
                        REFUSED_BACKOFF
                    }
                    _ => FAILURE_BACKOFF,
                };
                let problem = err.to_string();
                if self.problem.as_deref() != Some(problem.as_str()) {
                    tracing::warn!(%problem, "RAPL unavailable, attributing without it");
                }
                self.problem = Some(problem);
                self.retry_at = Some(Instant::now() + backoff);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wire_counters_and_skips_unknown_domains() {
        let counters = counters_from_wire(&[
            ("package".into(), 1_230_000),
            ("psys".into(), 5),
            ("future-domain".into(), 9),
        ]);
        assert_eq!(counters.len(), 2);
        assert_eq!(counters[&RaplDomain::Package].value, Microjoules(1_230_000));
        assert_eq!(counters[&RaplDomain::Psys].range, Microjoules(u64::MAX));
    }

    #[tokio::test]
    async fn without_a_bus_rapl_is_unavailable() {
        let mut client = RaplClient::connect(None).await;
        assert!(!client.available());
        assert_eq!(client.read().await, None);
    }
}

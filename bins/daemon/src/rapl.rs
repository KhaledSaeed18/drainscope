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
/// Longest wait for a rate-limited read: the sampler's limit is 1 s.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(1);

/// How long the sampler asked to wait ("retry in 120 ms"), capped at [`MAX_RETRY_WAIT`];
/// the cap when the message can't be read.
fn retry_wait(message: &str) -> Duration {
    message
        .strip_prefix("retry in ")
        .and_then(|rest| rest.strip_suffix(" ms"))
        .and_then(|ms| ms.trim().parse::<u64>().ok())
        .map_or(MAX_RETRY_WAIT, |ms| {
            Duration::from_millis(ms).min(MAX_RETRY_WAIT)
        })
}

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
        let reply = match proxy.read_counters().await {
            // Another reader of this user (doctor, a second daemon) or a late tick used the
            // sampler's per-user limit; it is under a second, so wait it out once.
            Err(SamplerError::RateLimited(message)) => {
                tokio::time::sleep(retry_wait(&message)).await;
                proxy.read_counters().await
            }
            reply => reply,
        };
        match reply {
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
            // Still limited after waiting: skip this tick.
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use drainscope_dbus::sampler::{Counters, OBJECT_PATH};
    use zbus::connection::Builder;

    use super::*;

    #[test]
    fn reads_the_wait_from_the_sampler() {
        assert_eq!(retry_wait("retry in 120 ms"), Duration::from_millis(120));
        assert_eq!(retry_wait("retry in 5000 ms"), Duration::from_secs(1));
        assert_eq!(retry_wait("soon"), Duration::from_secs(1));
    }

    /// A sampler that refuses its first `limited_first` calls as rate-limited, counting calls.
    struct FakeSampler {
        calls: Arc<AtomicUsize>,
        limited_first: usize,
    }

    #[zbus::interface(name = "io.github.khaledsaeed18.Drainscope.Sampler1")]
    impl FakeSampler {
        #[zbus(out_args("monotonic_ns", "generation", "counters"))]
        fn read_counters(&self) -> Result<Counters, SamplerError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.limited_first {
                return Err(SamplerError::RateLimited("retry in 10 ms".into()));
            }
            Ok((1, 7, vec![("package".into(), 5_000_000)]))
        }
    }

    /// A client connected to a fake sampler over a peer-to-peer connection; keep the server alive.
    async fn client_of(limited_first: usize) -> (zbus::Connection, RaplClient, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let fake = FakeSampler {
            calls: calls.clone(),
            limited_first,
        };
        let (server_end, client_end) = tokio::net::UnixStream::pair().unwrap();
        let server = Builder::unix_stream(server_end)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(OBJECT_PATH, fake)
            .unwrap()
            .build();
        let client = Builder::unix_stream(client_end).p2p().build();
        let (server, client) = tokio::join!(server, client);
        let client = client.unwrap();
        (
            server.unwrap(),
            RaplClient::connect(Some(&client)).await,
            calls,
        )
    }

    #[tokio::test]
    async fn a_rate_limited_read_is_retried_once() {
        let (_server, mut client, calls) = client_of(1).await;
        let reading = client.read().await.unwrap();
        assert_eq!(reading.generation, 7);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(client.available());
    }

    #[tokio::test]
    async fn still_limited_after_waiting_skips_the_tick() {
        let (_server, mut client, calls) = client_of(2).await;
        assert_eq!(client.read().await, None);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(client.available());
    }

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

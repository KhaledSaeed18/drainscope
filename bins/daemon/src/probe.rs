//! Idle-exit counts and network bytes from the eBPF probe (Probe1 on the system bus, ADR 0006).
//!
//! Optional like the sampler: without it, wakeups and traffic are simply not reported. Each
//! kind of reading backs off on its own after refusals instead of retrying every tick.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use drainscope_dbus::probe::{Probe1Proxy, ProbeError};
use drainscope_model::CgroupPath;

/// Retry delay after polkit refused us: that doesn't change quickly.
const REFUSED_BACKOFF: Duration = Duration::from_secs(300);
/// Retry delay after other failures (probe not installed, bus hiccups).
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReading {
    /// Changes whenever the probe's totals restart.
    pub generation: u64,
    /// Cumulative idle exits by cgroup.
    pub wakeups: BTreeMap<CgroupPath, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficReading {
    /// Changes whenever the probe's totals restart.
    pub generation: u64,
    /// Cumulative bytes by cgroup.
    pub received: BTreeMap<CgroupPath, u64>,
    pub sent: BTreeMap<CgroupPath, u64>,
}

/// Failure state of one kind of reading.
#[derive(Debug, Default)]
struct Backoff {
    /// Why the last attempt failed; `None` while readings arrive.
    problem: Option<String>,
    retry_at: Option<Instant>,
}

impl Backoff {
    fn waiting(&self) -> bool {
        self.retry_at.is_some_and(|at| Instant::now() < at)
    }

    fn succeeded(&mut self, what: &str) {
        if let Some(problem) = self.problem.take() {
            tracing::info!(previously = problem, "{what} available");
        }
        self.retry_at = None;
    }

    /// Records a failure; rate limiting (clock jitter between ticks) isn't one.
    fn failed(&mut self, what: &str, err: &ProbeError) {
        let backoff = match err {
            ProbeError::RateLimited(_) => return,
            ProbeError::NotAuthorized(_) | ProbeError::Unsupported(_) => REFUSED_BACKOFF,
            _ => FAILURE_BACKOFF,
        };
        let problem = err.to_string();
        if self.problem.as_deref() != Some(problem.as_str()) {
            tracing::info!(%problem, "{what} unavailable");
        }
        self.problem = Some(problem);
        self.retry_at = Some(Instant::now() + backoff);
    }
}

#[derive(Debug)]
pub struct ProbeClient {
    proxy: Option<Probe1Proxy<'static>>,
    wakeups: Backoff,
    network: Backoff,
}

impl ProbeClient {
    /// A client on `system_bus`; without one, the probe is simply unavailable.
    pub async fn connect(system_bus: Option<&zbus::Connection>) -> Self {
        let proxy = match system_bus {
            Some(bus) => Probe1Proxy::builder(bus)
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await
                .map_err(|err| tracing::warn!(%err, "no Probe1 proxy"))
                .ok(),
            None => None,
        };
        let problem = proxy.is_none().then(|| "no system bus".to_owned());
        Self {
            proxy,
            wakeups: Backoff {
                problem: problem.clone(),
                retry_at: None,
            },
            network: Backoff {
                problem,
                retry_at: None,
            },
        }
    }

    /// Whether the last attempt produced wakeup counts.
    #[must_use]
    pub fn available(&self) -> bool {
        self.wakeups.problem.is_none()
    }

    /// Whether the last attempt produced traffic counts.
    #[must_use]
    pub fn network_available(&self) -> bool {
        self.network.problem.is_none()
    }

    pub async fn read(&mut self) -> Option<ProbeReading> {
        let proxy = self.proxy.as_ref()?;
        if self.wakeups.waiting() {
            return None;
        }
        match proxy.read_wakeups().await {
            Ok((_, generation, pairs)) => {
                self.wakeups.succeeded("wakeup counts");
                Some(ProbeReading {
                    generation,
                    wakeups: pairs
                        .into_iter()
                        .map(|(path, count)| (CgroupPath::new(&path), count))
                        .collect(),
                })
            }
            Err(err) => {
                self.wakeups.failed("wakeup counts", &err);
                None
            }
        }
    }

    pub async fn read_network(&mut self) -> Option<TrafficReading> {
        let proxy = self.proxy.as_ref()?;
        if self.network.waiting() {
            return None;
        }
        match proxy.read_network().await {
            Ok((_, generation, rows)) => {
                self.network.succeeded("network traffic");
                let mut reading = TrafficReading {
                    generation,
                    received: BTreeMap::new(),
                    sent: BTreeMap::new(),
                };
                for (path, received, sent) in rows {
                    let path = CgroupPath::new(&path);
                    reading.received.insert(path.clone(), received);
                    reading.sent.insert(path, sent);
                }
                Some(reading)
            }
            Err(err) => {
                self.network.failed("network traffic", &err);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_bus_the_probe_is_unavailable() {
        let mut client = ProbeClient::connect(None).await;
        assert!(!client.available());
        assert!(!client.network_available());
        assert_eq!(client.read().await, None);
        assert_eq!(client.read_network().await, None);
    }
}

//! Idle-exit counts from the eBPF probe (Probe1 on the system bus, ADR 0006).
//!
//! Optional like the sampler: without it, wakeups are simply not reported. Refusals back off
//! instead of retrying every tick.

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

#[derive(Debug)]
pub struct ProbeClient {
    proxy: Option<Probe1Proxy<'static>>,
    /// Why the last attempt failed; `None` while readings arrive.
    problem: Option<String>,
    retry_at: Option<Instant>,
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
            problem,
            retry_at: None,
        }
    }

    /// Whether the last attempt produced counts.
    #[must_use]
    pub fn available(&self) -> bool {
        self.problem.is_none()
    }

    pub async fn read(&mut self) -> Option<ProbeReading> {
        let proxy = self.proxy.as_ref()?;
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return None;
        }
        match proxy.read_wakeups().await {
            Ok((_, generation, pairs)) => {
                if let Some(problem) = self.problem.take() {
                    tracing::info!(previously = problem, "wakeup counts available");
                }
                self.retry_at = None;
                Some(ProbeReading {
                    generation,
                    wakeups: pairs
                        .into_iter()
                        .map(|(path, count)| (CgroupPath::new(&path), count))
                        .collect(),
                })
            }
            // Ticks are slower than the limit, so this is only clock jitter.
            Err(ProbeError::RateLimited(_)) => None,
            Err(err) => {
                let backoff = match err {
                    ProbeError::NotAuthorized(_) | ProbeError::Unsupported(_) => REFUSED_BACKOFF,
                    _ => FAILURE_BACKOFF,
                };
                let problem = err.to_string();
                if self.problem.as_deref() != Some(problem.as_str()) {
                    tracing::info!(%problem, "wakeup counts unavailable");
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

    #[tokio::test]
    async fn without_a_bus_the_probe_is_unavailable() {
        let mut client = ProbeClient::connect(None).await;
        assert!(!client.available());
        assert_eq!(client.read().await, None);
    }
}

//! Idle-exit counts and network bytes from the eBPF probe (Probe1 on the system bus, ADR 0006).
//!
//! Optional like the sampler: without it, wakeups and traffic are simply not reported. Each
//! kind of reading backs off on its own after refusals instead of retrying every tick.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use drainscope_dbus::probe::{All, Probe1Proxy, ProbeError, Traffic, Wakeups};
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

/// Everything one tick reads from the probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeReadings {
    pub wakeups: Option<ProbeReading>,
    pub traffic: Option<TrafficReading>,
    /// (generation, kernel nanoseconds in the network softirqs), cumulative.
    pub network_time: Option<(u64, u64)>,
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
    /// The probe's generation at the last wakeup reading.
    generation: Option<u64>,
    /// Whether to use `ReadAll`; off after a probe older than it answered `UnknownMethod`, until
    /// the probe restarts.
    combined: bool,
}

/// An older probe without the method (e.g. still running across an upgrade).
fn is_unknown_method(err: &ProbeError) -> bool {
    match err {
        ProbeError::ZBus(zbus::Error::MethodError(name, ..)) => {
            name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod"
        }
        ProbeError::ZBus(zbus::Error::FDO(fdo)) => {
            matches!(**fdo, zbus::fdo::Error::UnknownMethod(_))
        }
        _ => false,
    }
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
            generation: None,
            combined: true,
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

    /// Every reading a tick needs, in one `ReadAll` call; from a probe older than that, three
    /// calls in flight together.
    pub async fn read_all(&mut self) -> ProbeReadings {
        let Some(proxy) = self.proxy.clone() else {
            return ProbeReadings::default();
        };
        if self.combined {
            if self.wakeups.waiting() {
                return ProbeReadings::default();
            }
            match proxy.read_all().await {
                Ok(reply) => return self.readings_from(reply),
                Err(err) if is_unknown_method(&err) => {
                    tracing::info!("the probe predates ReadAll; reading its counters separately");
                    self.combined = false;
                }
                Err(err) => {
                    self.wakeups.failed("wakeup counts", &err);
                    return ProbeReadings::default();
                }
            }
        }
        self.read_separately(&proxy).await
    }

    /// The three readings with the calls in flight together (the probe limits each method
    /// separately), so replies that arrive together cost one wakeup.
    async fn read_separately(&mut self, proxy: &Probe1Proxy<'static>) -> ProbeReadings {
        let ask_wakeups = !self.wakeups.waiting();
        let ask_network = !self.network.waiting();
        let (wakeups, traffic, time) = tokio::join!(
            async {
                if ask_wakeups {
                    Some(proxy.read_wakeups().await)
                } else {
                    None
                }
            },
            async {
                if ask_network {
                    Some(proxy.read_network().await)
                } else {
                    None
                }
            },
            async {
                if ask_network {
                    Some(proxy.read_network_time().await)
                } else {
                    None
                }
            },
        );
        ProbeReadings {
            wakeups: wakeups.and_then(|reply| self.wakeups_from(reply)),
            traffic: traffic.and_then(|reply| self.traffic_from(reply)),
            network_time: time.and_then(|reply| self.network_time_from(reply)),
        }
    }

    pub async fn read(&mut self) -> Option<ProbeReading> {
        let proxy = self.proxy.as_ref()?;
        if self.wakeups.waiting() {
            return None;
        }
        let reply = proxy.read_wakeups().await;
        self.wakeups_from(reply)
    }

    pub async fn read_network(&mut self) -> Option<TrafficReading> {
        let proxy = self.proxy.as_ref()?;
        if self.network.waiting() {
            return None;
        }
        let reply = proxy.read_network().await;
        self.traffic_from(reply)
    }

    /// (generation, kernel nanoseconds in the network softirqs), cumulative. Shares the
    /// network backoff: both come from the same probe programs.
    pub async fn read_network_time(&mut self) -> Option<(u64, u64)> {
        let proxy = self.proxy.as_ref()?;
        if self.network.waiting() {
            return None;
        }
        let reply = proxy.read_network_time().await;
        self.network_time_from(reply)
    }

    fn readings_from(&mut self, reply: All) -> ProbeReadings {
        let (monotonic_ns, generation, wakeups, network, traffic, tx_ns, rx_ns) = reply;
        let wakeups = self.wakeups_from(Ok((monotonic_ns, generation, wakeups)));
        if !network {
            let err = ProbeError::Unsupported("network counting isn't available".into());
            self.network.failed("network traffic", &err);
            return ProbeReadings {
                wakeups,
                ..ProbeReadings::default()
            };
        }
        ProbeReadings {
            wakeups,
            traffic: self.traffic_from(Ok((monotonic_ns, generation, traffic))),
            network_time: self.network_time_from(Ok((monotonic_ns, generation, tx_ns, rx_ns))),
        }
    }

    fn wakeups_from(&mut self, reply: Result<Wakeups, ProbeError>) -> Option<ProbeReading> {
        match reply {
            Ok((_, generation, pairs)) => {
                self.wakeups.succeeded("wakeup counts");
                // A restarted (perhaps upgraded) probe may now count traffic: ask again
                // instead of waiting out a backoff from the old one.
                let previous = self.generation.replace(generation);
                if previous != Some(generation) {
                    self.network.retry_at = None;
                    // A restarted probe may be a newer one, with ReadAll.
                    if previous.is_some() {
                        self.combined = true;
                    }
                }
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

    fn traffic_from(&mut self, reply: Result<Traffic, ProbeError>) -> Option<TrafficReading> {
        match reply {
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

    fn network_time_from(
        &mut self,
        reply: Result<(u64, u64, u64, u64), ProbeError>,
    ) -> Option<(u64, u64)> {
        match reply {
            Ok((_, generation, tx_ns, rx_ns)) => Some((generation, tx_ns.saturating_add(rx_ns))),
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
        assert_eq!(client.read_all().await, ProbeReadings::default());
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use drainscope_dbus::probe::OBJECT_PATH;
    use zbus::connection::Builder;

    const FIREFOX: &str = "user.slice/user-1000.slice/app-firefox.scope";

    /// A probe from before `ReadAll`: the three methods only, counting calls.
    struct OldProbe(Arc<AtomicUsize>);

    #[zbus::interface(name = "io.github.khaledsaeed18.Drainscope.Probe1")]
    impl OldProbe {
        #[zbus(out_args("monotonic_ns", "generation", "wakeups"))]
        fn read_wakeups(&self) -> (u64, u64, Vec<(String, u64)>) {
            self.0.fetch_add(1, Ordering::SeqCst);
            (1, 7, vec![(FIREFOX.into(), 40)])
        }

        #[zbus(out_args("monotonic_ns", "generation", "traffic"))]
        fn read_network(&self) -> (u64, u64, Vec<(String, u64, u64)>) {
            self.0.fetch_add(1, Ordering::SeqCst);
            (1, 7, vec![(FIREFOX.into(), 1000, 50)])
        }

        #[zbus(out_args("monotonic_ns", "generation", "tx_ns", "rx_ns"))]
        fn read_network_time(&self) -> (u64, u64, u64, u64) {
            self.0.fetch_add(1, Ordering::SeqCst);
            (1, 7, 30, 70)
        }
    }

    /// A current probe answering `ReadAll`.
    struct NewProbe(Arc<AtomicUsize>);

    #[zbus::interface(name = "io.github.khaledsaeed18.Drainscope.Probe1")]
    impl NewProbe {
        // Spelled out so zbus emits seven out arguments, like the real probe.
        #[allow(clippy::type_complexity)]
        #[zbus(out_args(
            "monotonic_ns",
            "generation",
            "wakeups",
            "network_available",
            "traffic",
            "tx_ns",
            "rx_ns"
        ))]
        fn read_all(
            &self,
        ) -> (
            u64,
            u64,
            Vec<(String, u64)>,
            bool,
            Vec<(String, u64, u64)>,
            u64,
            u64,
        ) {
            self.0.fetch_add(1, Ordering::SeqCst);
            (
                1,
                7,
                vec![(FIREFOX.into(), 40)],
                true,
                vec![(FIREFOX.into(), 1000, 50)],
                30,
                70,
            )
        }
    }

    /// A client connected to `probe` over a peer-to-peer connection; keep the server alive.
    async fn client_of(
        probe: impl zbus::object_server::Interface,
    ) -> (zbus::Connection, ProbeClient) {
        let (server_end, client_end) = tokio::net::UnixStream::pair().unwrap();
        let server = Builder::unix_stream(server_end)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(OBJECT_PATH, probe)
            .unwrap()
            .build();
        let client = Builder::unix_stream(client_end).p2p().build();
        let (server, client) = tokio::join!(server, client);
        let client = client.unwrap();
        (server.unwrap(), ProbeClient::connect(Some(&client)).await)
    }

    fn expected() -> ProbeReadings {
        ProbeReadings {
            wakeups: Some(ProbeReading {
                generation: 7,
                wakeups: BTreeMap::from([(CgroupPath::new(FIREFOX), 40)]),
            }),
            traffic: Some(TrafficReading {
                generation: 7,
                received: BTreeMap::from([(CgroupPath::new(FIREFOX), 1000)]),
                sent: BTreeMap::from([(CgroupPath::new(FIREFOX), 50)]),
            }),
            network_time: Some((7, 100)),
        }
    }

    #[tokio::test]
    async fn reads_everything_in_one_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (_server, mut client) = client_of(NewProbe(calls.clone())).await;
        assert_eq!(client.read_all().await, expected());
        assert_eq!(client.read_all().await, expected());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(client.available() && client.network_available());
    }

    #[tokio::test]
    async fn falls_back_to_separate_calls_for_an_older_probe() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (_server, mut client) = client_of(OldProbe(calls.clone())).await;
        assert_eq!(client.read_all().await, expected());
        // Once it knows, it doesn't try ReadAll again: three calls per reading.
        assert_eq!(client.read_all().await, expected());
        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert!(!client.combined);
    }
}

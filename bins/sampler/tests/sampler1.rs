//! `Sampler1` over a peer-to-peer D-Bus connection: behaviour, and conformance with the
//! introspection XML in `data/dbus/interfaces/`.

// Test helpers fail the test by panicking.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use drainscope_dbus::sampler::{BUS_NAME, INTERFACE, OBJECT_PATH, Sampler1Proxy, SamplerError};
use drainscope_model::Microjoules;
use drainscope_model::snapshot::{EnergyCounter, RaplDomain};
use drainscope_sampler::auth::Fixed;
use drainscope_sampler::{Config, CounterSource, Sampler};
use drainscope_sys::{PowercapZone, SysError};
use zbus::connection::Builder;

/// A package counter the test can move; the core counter is a quarter of it.
#[derive(Clone)]
struct Counters(Arc<Mutex<u64>>);

impl Counters {
    fn set(&self, package_uj: u64) {
        *self.0.lock().unwrap() = package_uj;
    }
}

impl CounterSource for Counters {
    fn read(&self) -> Result<Vec<PowercapZone>, SysError> {
        let package = *self.0.lock().unwrap();
        let zone = |zone: &str, domain, value| PowercapZone {
            zone: zone.into(),
            domain,
            counter: EnergyCounter {
                value: Microjoules(value),
                range: Microjoules(u64::MAX),
            },
        };
        Ok(vec![
            zone("intel-rapl:0", RaplDomain::Package, package),
            zone("intel-rapl:0:0", RaplDomain::Core, package / 4),
        ])
    }
}

struct Peer {
    _server: zbus::Connection,
    client: zbus::Connection,
}

impl Peer {
    async fn proxy(&self) -> Sampler1Proxy<'_> {
        Sampler1Proxy::builder(&self.client)
            .path(OBJECT_PATH)
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

async fn serve(authorized: bool, config: Config, counters: &Counters) -> Peer {
    let sampler = Sampler::new(
        Box::new(counters.clone()),
        Box::new(Fixed(authorized)),
        config,
    )
    .unwrap();
    let (server_end, client_end) = tokio::net::UnixStream::pair().unwrap();
    let server = Builder::unix_stream(server_end)
        .server(zbus::Guid::generate())
        .unwrap()
        .p2p()
        .serve_at(OBJECT_PATH, sampler)
        .unwrap()
        .build();
    let client = Builder::unix_stream(client_end).p2p().build();
    let (server, client) = tokio::join!(server, client);
    Peer {
        _server: server.unwrap(),
        client: client.unwrap(),
    }
}

fn unlimited() -> Config {
    Config {
        min_interval: Duration::ZERO,
        quantum_uj: 10_000,
    }
}

#[tokio::test]
async fn counters_are_cumulative_quantized_and_same_generation() {
    let counters = Counters(Arc::new(Mutex::new(5_000_000)));
    let peer = serve(true, unlimited(), &counters).await;
    let proxy = peer.proxy().await;

    let (t1, generation1, first) = proxy.read_counters().await.unwrap();
    assert_eq!(first, [("package".into(), 0), ("core".into(), 0)]);

    counters.set(5_000_000 + 1_234_567);
    let (t2, generation2, second) = proxy.read_counters().await.unwrap();
    assert_eq!(
        second,
        [("package".into(), 1_230_000), ("core".into(), 300_000)]
    );
    assert_eq!(generation1, generation2);
    assert!(t2 > t1);
}

#[tokio::test]
async fn repeated_calls_are_rate_limited() {
    let counters = Counters(Arc::new(Mutex::new(0)));
    let config = Config {
        min_interval: Duration::from_secs(60),
        ..unlimited()
    };
    let peer = serve(true, config, &counters).await;
    let proxy = peer.proxy().await;
    proxy.read_counters().await.unwrap();
    assert!(matches!(
        proxy.read_counters().await,
        Err(SamplerError::RateLimited(_))
    ));
}

#[tokio::test]
async fn unauthorized_callers_are_denied() {
    let counters = Counters(Arc::new(Mutex::new(0)));
    let peer = serve(false, unlimited(), &counters).await;
    assert!(matches!(
        peer.proxy().await.read_counters().await,
        Err(SamplerError::NotAuthorized(_))
    ));
}

#[tokio::test]
async fn properties_describe_the_service() {
    let counters = Counters(Arc::new(Mutex::new(0)));
    let peer = serve(true, Config::default(), &counters).await;
    let proxy = peer.proxy().await;
    assert_eq!(proxy.domains().await.unwrap(), ["package", "core"]);
    assert_eq!(proxy.min_interval_ms().await.unwrap(), 1000);
    assert_eq!(proxy.quantum_uj().await.unwrap(), 10_000);
}

/// Methods, signals and properties as comparable lines.
fn describe(interface: &zbus_xml::Interface<'_>) -> Vec<String> {
    let args = |args: &[zbus_xml::Arg]| {
        args.iter()
            .map(|a| {
                format!(
                    "{:?} {}:{:?}",
                    a.direction(),
                    a.name().unwrap_or("_"),
                    a.ty()
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines: Vec<String> = interface
        .methods()
        .iter()
        .map(|m| format!("method {}({})", m.name(), args(m.args())))
        .chain(
            interface
                .signals()
                .iter()
                .map(|s| format!("signal {}({})", s.name(), args(s.args()))),
        )
        .chain(
            interface
                .properties()
                .iter()
                .map(|p| format!("property {}: {:?} {:?}", p.name(), p.ty(), p.access())),
        )
        .collect();
    lines.sort();
    lines
}

#[tokio::test]
async fn served_interface_matches_the_contract() {
    let contract_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/dbus/interfaces")
        .join(format!("{INTERFACE}.xml"));
    let contract =
        zbus_xml::Node::from_reader(std::fs::File::open(contract_path).unwrap()).unwrap();

    let counters = Counters(Arc::new(Mutex::new(0)));
    let peer = serve(true, unlimited(), &counters).await;
    let introspectable = zbus::fdo::IntrospectableProxy::builder(&peer.client)
        .destination(BUS_NAME)
        .unwrap()
        .path(OBJECT_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let served =
        zbus_xml::Node::from_reader(introspectable.introspect().await.unwrap().as_bytes()).unwrap();

    let find = |node: &zbus_xml::Node<'_>| {
        node.interfaces()
            .iter()
            .find(|i| i.name() == INTERFACE)
            .map(describe)
            .unwrap()
    };
    assert_eq!(find(&served), find(&contract));
}

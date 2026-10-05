//! `Probe1` over a peer-to-peer D-Bus connection: behaviour, and conformance with the
//! introspection XML in `data/dbus/interfaces/`.

// Test helpers fail the test by panicking.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use drainscope_access::Fixed;
use drainscope_dbus::probe::{BUS_NAME, INTERFACE, OBJECT_PATH, Probe1Proxy, ProbeError};
use drainscope_probe::service::CgroupNames;
use drainscope_probe::{Config, NetworkSource, Probe, Traffic, WakeupSource};
use zbus::connection::Builder;

const FIREFOX: &str =
    "user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-firefox-1.scope";
const OTHER_USER: &str = "user.slice/user-1001.slice/session-4.scope";
const NETWORK: &str = "system.slice/NetworkManager.service";

/// Idle-exit counts by cgroup ID that the test can change.
#[derive(Clone, Default)]
struct Counts(Arc<Mutex<BTreeMap<u64, u64>>>);

impl WakeupSource for Counts {
    fn read(&self) -> anyhow::Result<BTreeMap<u64, u64>> {
        Ok(self.0.lock().unwrap().clone())
    }
}

/// Cgroups the test can add, counting how often they were listed.
#[derive(Clone, Default)]
struct Names(Arc<Mutex<(BTreeMap<u64, String>, usize)>>);

impl CgroupNames for Names {
    fn ids(&self) -> anyhow::Result<BTreeMap<u64, String>> {
        let mut state = self.0.lock().unwrap();
        state.1 += 1;
        Ok(state.0.clone())
    }
}

/// Fixed traffic: 1000 bytes in and 50 out for Firefox, 500 in for the other user.
struct Network;

impl NetworkSource for Network {
    fn read(&self) -> anyhow::Result<Traffic> {
        Ok(Traffic {
            received: BTreeMap::from([(10, 1000), (11, 500)]),
            sent: BTreeMap::from([(10, 50)]),
            softirq_ns: (30_000, 70_000),
        })
    }
}

struct Peer {
    _server: zbus::Connection,
    client: zbus::Connection,
}

impl Peer {
    async fn proxy(&self) -> Probe1Proxy<'_> {
        Probe1Proxy::builder(&self.client)
            .path(OBJECT_PATH)
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

fn unlimited() -> Config {
    Config {
        min_interval: Duration::ZERO,
        peer_uid: Some(1000),
    }
}

async fn serve(authorized: bool, config: Config, counts: &Counts, names: &Names) -> Peer {
    let probe = Probe::new(
        Box::new(counts.clone()),
        Some(Box::new(Network)),
        Box::new(names.clone()),
        Box::new(Fixed(authorized)),
        config,
    );
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
    Peer {
        _server: server.unwrap(),
        client: client.unwrap(),
    }
}

fn fixtures() -> (Counts, Names) {
    let counts = Counts::default();
    *counts.0.lock().unwrap() = BTreeMap::from([(10, 7), (11, 3), (12, 5), (99, 4)]);
    let names = Names::default();
    names.0.lock().unwrap().0 = BTreeMap::from([
        (10, FIREFOX.to_owned()),
        (11, OTHER_USER.to_owned()),
        (12, NETWORK.to_owned()),
        (1, String::new()),
    ]);
    (counts, names)
}

#[tokio::test]
async fn reports_visible_cgroups_by_path() {
    let (counts, names) = fixtures();
    let peer = serve(true, unlimited(), &counts, &names).await;
    let (_, generation, wakeups) = peer.proxy().await.read_wakeups().await.unwrap();
    // The other user's slice is hidden; the removed cgroup (99) is dropped. Sorted by path.
    assert_eq!(wakeups, [(NETWORK.to_owned(), 5), (FIREFOX.to_owned(), 7)]);

    // A new cgroup triggers one refresh of the names.
    names.0.lock().unwrap().0.insert(13, "init.scope".into());
    counts.0.lock().unwrap().insert(13, 2);
    let (_, again, wakeups) = peer.proxy().await.read_wakeups().await.unwrap();
    assert_eq!(again, generation);
    assert!(wakeups.contains(&("init.scope".to_owned(), 2)));
    // Listed once at first, then once for the new ID (99 is still unknown, so every read
    // relists; that is bounded by the rate limit).
    assert!(names.0.lock().unwrap().1 >= 2);
}

#[tokio::test]
async fn reports_traffic_by_path() {
    let (counts, names) = fixtures();
    let peer = serve(true, unlimited(), &counts, &names).await;
    let (_, _, traffic) = peer.proxy().await.read_network().await.unwrap();
    assert_eq!(traffic, [(FIREFOX.to_owned(), 1000, 50)]);
}

#[tokio::test]
async fn reports_network_softirq_time() {
    let (counts, names) = fixtures();
    let peer = serve(true, unlimited(), &counts, &names).await;
    let (_, _, tx, rx) = peer.proxy().await.read_network_time().await.unwrap();
    assert_eq!((tx, rx), (30_000, 70_000));
}

#[tokio::test]
async fn methods_are_limited_separately() {
    let (counts, names) = fixtures();
    let config = Config {
        min_interval: Duration::from_secs(60),
        ..unlimited()
    };
    let peer = serve(true, config, &counts, &names).await;
    let proxy = peer.proxy().await;
    proxy.read_wakeups().await.unwrap();
    proxy.read_network().await.unwrap();
    assert!(matches!(
        proxy.read_network().await.unwrap_err(),
        ProbeError::RateLimited(_)
    ));
}

#[tokio::test]
async fn unknown_users_see_only_the_system() {
    let (counts, names) = fixtures();
    let config = Config {
        peer_uid: None,
        ..unlimited()
    };
    let peer = serve(true, config, &counts, &names).await;
    let (_, _, wakeups) = peer.proxy().await.read_wakeups().await.unwrap();
    assert_eq!(wakeups, [(NETWORK.to_owned(), 5)]);
}

#[tokio::test]
async fn refuses_unauthorized_callers() {
    let (counts, names) = fixtures();
    let peer = serve(false, unlimited(), &counts, &names).await;
    let err = peer.proxy().await.read_wakeups().await.unwrap_err();
    assert!(matches!(err, ProbeError::NotAuthorized(_)), "{err:?}");
}

#[tokio::test]
async fn rate_limits_callers() {
    let (counts, names) = fixtures();
    let config = Config {
        min_interval: Duration::from_secs(60),
        ..unlimited()
    };
    let peer = serve(true, config, &counts, &names).await;
    let proxy = peer.proxy().await;
    proxy.read_wakeups().await.unwrap();
    let err = proxy.read_wakeups().await.unwrap_err();
    assert!(matches!(err, ProbeError::RateLimited(_)), "{err:?}");
    assert_eq!(proxy.min_interval_ms().await.unwrap(), 60_000);
}

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

    let (counts, names) = fixtures();
    let peer = serve(true, unlimited(), &counts, &names).await;
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

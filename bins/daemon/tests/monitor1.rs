//! `Monitor1` over a peer-to-peer connection: answers from a populated store, and conformance
//! with the introspection XML in `data/dbus/interfaces/`.

// Test helpers fail the test by panicking.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use drainscope_daemon::monitor::{LiveState, Monitor, Shared, Status};
use drainscope_dbus::monitor::{BUS_NAME, INTERFACE, Monitor1Proxy, OBJECT_PATH};
use drainscope_model::{
    BatteryHealth, ClosedWindow, ConsumerKey, EnergySplit, Joules, Ledger, Measurement,
};
use drainscope_store::{PowerEvent, PowerEventKind, SleepSession, Store, WindowRecord};
use zbus::connection::Builder;

const T0: i64 = 1_791_000_000_000;

fn now() -> i64 {
    T0 + 600_000
}

fn split(cpu: f64, gpu: f64, other: f64) -> EnergySplit {
    EnergySplit {
        cpu: Joules(cpu),
        gpu: Joules(gpu),
        other: Joules(other),
    }
}

/// Unplugged at T0; one battery window: Firefox 300 J, its search provider 100 J, kernel
/// 100 J, idle 500 J, devices 1000 J. Capacity 100 kJ.
fn shared() -> Arc<Shared> {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_power_event(&PowerEvent {
            ts_ms: T0,
            kind: PowerEventKind::Unplug,
            battery_percent: Some(90.0),
            energy_wh: None,
        })
        .unwrap();
    let ledger: Ledger = [
        (
            ConsumerKey::App("org.mozilla.firefox".into()),
            split(250.0, 50.0, 0.0),
        ),
        (
            ConsumerKey::App("org.mozilla.firefox.SearchProvider".into()),
            split(100.0, 0.0, 0.0),
        ),
        (ConsumerKey::Kernel, split(100.0, 0.0, 0.0)),
        (ConsumerKey::Idle, split(500.0, 0.0, 0.0)),
        (ConsumerKey::Devices, split(0.0, 0.0, 1000.0)),
    ]
    .into_iter()
    .collect();
    let window = ClosedWindow {
        duration: Duration::from_secs(10),
        measured: Joules(2000.0),
        ledger,
        measurement: Measurement::Battery,
        shortfall: Joules(0.0),
    };
    store
        .record_window(&WindowRecord {
            start_ms: T0 + 1_000,
            end_ms: T0 + 11_000,
            window: &window,
            model_version: 1,
        })
        .unwrap();
    store
        .record_sleep_session(&SleepSession {
            start_ms: T0 + 20_000,
            end_ms: T0 + 80_000,
            wh_lost: Some(0.5),
            percent_lost: None,
            mem_sleep: Some("deep".into()),
            wake_reason: Some("Lid (PNP0C0D:00)".into()),
        })
        .unwrap();
    store
        .record_battery_health(
            T0,
            &[BatteryHealth {
                battery: "BAT0".into(),
                energy_full: Joules(31.0 * 3600.0),
                energy_full_design: None,
                cycle_count: Some(42),
            }],
        )
        .unwrap();
    let shared = Shared::new(store);
    *shared.live.lock().unwrap_or_else(PoisonError::into_inner) = LiveState {
        on_battery: true,
        capacity: Some(Joules(100_000.0)),
        status: Status::Full,
        domains: vec!["package".into(), "core".into()],
        wakeups: Some(vec![("app:org.mozilla.firefox".into(), 12.5)]),
    };
    shared
}

struct Peer {
    _server: zbus::Connection,
    client: zbus::Connection,
}

async fn serve() -> Peer {
    let monitor = Monitor::new(shared(), now);
    let (server_end, client_end) = tokio::net::UnixStream::pair().unwrap();
    let server = Builder::unix_stream(server_end)
        .server(zbus::Guid::generate())
        .unwrap()
        .p2p()
        .serve_at(OBJECT_PATH, monitor)
        .unwrap()
        .build();
    let client = Builder::unix_stream(client_end).p2p().build();
    let (server, client) = tokio::join!(server, client);
    Peer {
        _server: server.unwrap(),
        client: client.unwrap(),
    }
}

async fn proxy(peer: &Peer) -> Monitor1Proxy<'_> {
    Monitor1Proxy::builder(&peer.client)
        .path(OBJECT_PATH)
        .unwrap()
        .build()
        .await
        .unwrap()
}

#[tokio::test]
async fn summary_reports_battery_and_attributable_shares() {
    let peer = serve().await;
    let (on_battery, since, battery_percent, top) = proxy(&peer).await.get_summary().await.unwrap();
    assert!(on_battery);
    assert_eq!(since, T0 / 1000);
    assert!((battery_percent - 2.0).abs() < 1e-9, "2000 J of 100 kJ");
    assert_eq!(top[0].0, "devices");
    assert_eq!(top[0].3, 0.0, "devices aren't attributable");
    let firefox = top
        .iter()
        .find(|row| row.0 == "app:org.mozilla.firefox")
        .unwrap();
    assert!((firefox.2 - 0.3).abs() < 1e-9, "% of battery");
    assert!((firefox.3 - 60.0).abs() < 1e-9, "300 of 500 attributable J");
}

#[tokio::test]
async fn usage_groups_by_kind_and_validates_arguments() {
    let peer = serve().await;
    let proxy = proxy(&peer).await;
    let by_kind = proxy
        .get_usage(T0 / 1000, now() / 1000, "kind", "battery")
        .await
        .unwrap();
    let app = by_kind.iter().find(|row| row.0 == "app").unwrap();
    assert!((app.2 - 400.0).abs() < 1e-9);
    assert!((app.4 - 50.0).abs() < 1e-9, "gpu");
    assert_eq!(by_kind[0].0, "devices");

    let on_ac = proxy
        .get_usage(T0 / 1000, now() / 1000, "consumer", "ac")
        .await
        .unwrap();
    assert_eq!(on_ac, Vec::new());
    assert!(proxy.get_usage(0, 1, "colour", "any").await.is_err());
    assert!(proxy.get_usage(0, 1, "kind", "solar").await.is_err());
}

#[tokio::test]
async fn coverage_reports_measured_seconds() {
    let peer = serve().await;
    let proxy = proxy(&peer).await;
    // One 10 s window was recorded.
    assert_eq!(
        proxy
            .get_coverage(T0 / 1000, now() / 1000, "any")
            .await
            .unwrap(),
        10
    );
    assert_eq!(
        proxy
            .get_coverage(T0 / 1000, now() / 1000, "ac")
            .await
            .unwrap(),
        0
    );
    assert!(proxy.get_coverage(0, 1, "solar").await.is_err());
}

#[tokio::test]
async fn sleep_sessions_mark_unknowns_as_nan() {
    let peer = serve().await;
    let sessions = proxy(&peer)
        .await
        .get_sleep_sessions(T0 / 1000)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    let (start, end, wh, percent, mode) = &sessions[0];
    assert_eq!((*start, *end), ((T0 + 20_000) / 1000, (T0 + 80_000) / 1000));
    assert!((wh - 0.5).abs() < 1e-9);
    assert!(percent.is_nan());
    assert_eq!(mode, "deep");
}

#[tokio::test]
async fn sleep_history_adds_the_wake_reason() {
    let peer = serve().await;
    let sessions = proxy(&peer)
        .await
        .get_sleep_history(T0 / 1000)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].4, "deep");
    assert_eq!(sessions[0].5, "Lid (PNP0C0D:00)");
}

#[tokio::test]
async fn battery_health_marks_unknowns() {
    let peer = serve().await;
    let readings = proxy(&peer).await.get_battery_health(0).await.unwrap();
    assert_eq!(readings.len(), 1);
    let (battery, ts, full, design, cycles) = &readings[0];
    assert_eq!((battery.as_str(), *ts, *cycles), ("BAT0", T0 / 1000, 42));
    assert!((full - 31.0).abs() < 1e-9);
    assert!(design.is_nan());
    let later = proxy(&peer)
        .await
        .get_battery_health(T0 / 1000 + 1)
        .await
        .unwrap();
    assert_eq!(later, Vec::new());
}

#[tokio::test]
async fn properties_reflect_live_state() {
    let peer = serve().await;
    let proxy = proxy(&peer).await;
    assert_eq!(proxy.status().await.unwrap(), "full");
    assert_eq!(
        proxy.model_version().await.unwrap(),
        drainscope_model::MODEL_VERSION
    );
    assert_eq!(proxy.domains().await.unwrap(), ["package", "core"]);
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
    let peer = serve().await;
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

#[tokio::test]
async fn wakeups_come_from_live_state() {
    let peer = serve().await;
    let (available, wakeups) = proxy(&peer).await.get_wakeups().await.unwrap();
    assert!(available);
    assert_eq!(wakeups, [("app:org.mozilla.firefox".to_owned(), 12.5)]);
}

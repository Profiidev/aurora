use std::{sync::Arc, time::Duration};

use aurora_core::{
  engine::{Engine, Settings},
  ipc::{self, IpcRegistry, IpcServer, commands},
  pairing::{Target, hex},
  types::{Event, PairState},
};
use tokio::{sync::broadcast::Receiver, time::timeout};

fn settings(name: &str) -> Settings {
  Settings {
    name: name.into(),
    os: "Linux".into(),
    pairing_timeout: Duration::from_secs(60),
    // no mDNS or n0 in tests: devices meet through the link's direct addresses
    discovery: false,
    port: None,
    relay: false,
    usb: false,
    bluetooth: false,
    background: None,
  }
}

async fn engine(name: &str) -> (Arc<Engine>, tempfile::TempDir) {
  let dir = tempfile::tempdir().unwrap();
  (
    Engine::start(settings(name), dir.path()).await.unwrap(),
    dir,
  )
}

async fn next(rx: &mut Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
  timeout(Duration::from_secs(10), async {
    loop {
      let event = rx.recv().await.unwrap();
      if pred(&event) {
        return event;
      }
    }
  })
  .await
  .expect("timed out waiting for event")
}

#[tokio::test]
async fn pairs_confirms_and_unpairs() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let (mut dev, mut pev) = (desktop.events(), phone.events());

  let info = desktop.start_pairing().unwrap();
  assert!(!info.addrs.is_empty(), "no dialable addresses: {info:?}");
  assert!(
    desktop.start_pairing().is_err(),
    "second session must be refused"
  );
  phone.connect(&info.link).await.unwrap();

  let Event::CodeReady(a) = next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await else {
    unreachable!()
  };
  let Event::CodeReady(b) = next(&mut pev, |e| matches!(e, Event::CodeReady(_))).await else {
    unreachable!()
  };
  assert_eq!(a.code, b.code);
  assert_eq!(
    (a.peer_name.as_str(), b.peer_name.as_str()),
    ("phone", "desktop")
  );
  assert!(matches!(desktop.pair_state(), PairState::Confirm(_)));

  desktop.decide(true).unwrap();
  phone.decide(true).unwrap();
  next(&mut dev, |e| matches!(e, Event::Paired(_))).await;
  next(&mut pev, |e| matches!(e, Event::Paired(_))).await;
  assert_eq!(desktop.pair_state(), PairState::Idle);
  assert_eq!(desktop.devices().len(), 1);
  assert_eq!(phone.devices().len(), 1);

  // the phone knows the desktop's address, so it reconnects on its own
  next(&mut dev, |e| matches!(e, Event::DeviceConnected(_))).await;

  // pairing twice fails on the dialing side
  desktop.start_pairing().unwrap();
  let err = phone.connect(&info.link).await.unwrap_err();
  assert!(format!("{err:#}").contains("already paired"), "{err:#}");
  desktop.cancel_pairing();

  desktop
    .unpair(&desktop.devices()[0].device.id)
    .await
    .unwrap();
  next(&mut pev, |e| matches!(e, Event::Unpaired(_))).await;
  assert!(phone.devices().is_empty());
  assert!(desktop.devices().is_empty());
}

#[tokio::test]
async fn rejecting_pairs_nothing_and_closed_page_refuses() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let (mut dev, mut pev) = (desktop.events(), phone.events());

  let info = desktop.start_pairing().unwrap();
  phone.connect(&info.link).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;
  next(&mut pev, |e| matches!(e, Event::CodeReady(_))).await;
  phone.decide(false).unwrap();
  next(&mut dev, |e| matches!(e, Event::PairingFailed(_))).await;
  next(&mut pev, |e| matches!(e, Event::PairingFailed(_))).await;
  assert!(desktop.devices().is_empty() && phone.devices().is_empty());

  // the session ended and nobody reopened the pair page
  phone.connect(&info.link).await.unwrap();
  next(&mut pev, |e| matches!(e, Event::PairingFailed(_))).await;
}

#[tokio::test]
async fn waiting_never_expires() {
  let (desktop, _d) = engine("desktop").await;
  desktop.set_settings(Settings {
    pairing_timeout: Duration::from_millis(200),
    ..settings("desktop")
  });
  desktop.start_pairing().unwrap();
  tokio::time::sleep(Duration::from_millis(500)).await;
  assert!(matches!(desktop.pair_state(), PairState::Waiting(_)));
}

#[tokio::test]
async fn ipc_commands_and_event_stream() {
  let dir = tempfile::tempdir().unwrap();
  unsafe {
    std::env::set_var("AURORA_SOCKET", dir.path().join("t.sock"));
    std::env::set_var("XDG_STATE_HOME", dir.path());
  }
  let (engine, _s) = engine("desktop").await;
  let server = IpcServer::new().unwrap().unwrap();
  assert!(
    IpcServer::new().unwrap().is_none(),
    "second instance must yield"
  );
  let mut registry = IpcRegistry::default();
  commands::register_all(&mut registry);
  tokio::spawn(server.run(registry, engine.clone()));

  let stream = tokio::task::spawn_blocking(|| {
    let mut events = ipc::subscribe::<commands::Events>(()).unwrap();
    let info = ipc::send::<commands::PairStart>(()).unwrap();
    let err = ipc::send::<commands::PairStart>(()).unwrap_err();
    assert!(err.to_string().contains("pairing in progress"));
    let bad = ipc::client::send_raw("nope", serde_json::Value::Null).unwrap_err();
    assert!(bad.to_string().contains("Unknown command"));
    assert!(matches!(
      ipc::send::<commands::PairStatus>(()).unwrap(),
      PairState::Waiting(_)
    ));
    // settings go to the state dir, kept inside the temp dir here
    assert!(!ipc::send::<commands::SettingsGet>(()).unwrap().discovery);
    ipc::send::<commands::SettingsSetDiscovery>(true).unwrap();
    assert!(ipc::send::<commands::SettingsGet>(()).unwrap().discovery);
    assert!(ipc::send::<commands::DevicesList>(()).unwrap().is_empty());
    ipc::send::<commands::PairNearby>(()).unwrap();
    ipc::send::<commands::PairCancel>(()).unwrap();
    assert_eq!(
      ipc::send::<commands::PairStatus>(()).unwrap(),
      PairState::Idle
    );
    (info, events.next().unwrap().unwrap())
  })
  .await
  .unwrap();
  assert!(matches!(stream.1, Event::PairingStarted(i) if i.link == stream.0.link));
  let saved = std::fs::read_to_string(dir.path().join("aurora/settings.toml")).unwrap();
  assert!(saved.contains("discovery = true"), "{saved}");
}

#[tokio::test]
async fn refusal_tells_the_dialer_why() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let mut pev = phone.events();

  let info = desktop.start_pairing().unwrap();
  desktop.cancel_pairing();
  phone.connect(&info.link).await.unwrap();

  let Event::PairingFailed(why) = next(&mut pev, |e| matches!(e, Event::PairingFailed(_))).await
  else {
    unreachable!()
  };
  assert!(why.contains("not accepting pairing"), "{why}");
}

#[tokio::test]
async fn reconnects_after_a_restart_on_a_new_port() {
  let (ddir, pdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
  let desktop = Engine::start(settings("desktop"), ddir.path())
    .await
    .unwrap();
  let phone = Engine::start(settings("phone"), pdir.path()).await.unwrap();
  let (mut dev, mut pev) = (desktop.events(), phone.events());

  let info = desktop.start_pairing().unwrap();
  phone.connect(&info.link).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;
  next(&mut pev, |e| matches!(e, Event::CodeReady(_))).await;
  desktop.decide(true).unwrap();
  phone.decide(true).unwrap();
  next(&mut pev, |e| matches!(e, Event::Paired(_))).await;
  next(&mut dev, |e| matches!(e, Event::Paired(_))).await;
  let ports = (desktop.port(), phone.port());
  let stale = phone.devices()[0].device.addrs.clone();

  // same keys and stores, new ports: every stored address is stale, only
  // mDNS can bring them together again
  desktop.shutdown().await;
  phone.shutdown().await;
  drop((desktop, phone));
  let on = |name| Settings {
    discovery: true,
    ..settings(name)
  };
  let desktop = Engine::start(on("desktop"), ddir.path()).await.unwrap();
  let phone = Engine::start(on("phone"), pdir.path()).await.unwrap();
  assert!(desktop.port() != ports.0 && phone.port() != ports.1);
  let (mut dev, mut pev) = (desktop.events(), phone.events());

  timeout(Duration::from_secs(30), async {
    loop {
      if let Event::DeviceConnected(_) = dev.recv().await.unwrap() {
        break;
      }
    }
  })
  .await
  .expect("devices did not find each other");
  // the address that worked is remembered for the next reconnect
  timeout(Duration::from_secs(5), async {
    while phone.devices()[0].device.addrs == stale {
      tokio::time::sleep(Duration::from_millis(50)).await;
    }
  })
  .await
  .expect("hints not refreshed");
  let id = desktop.devices()[0].device.id.clone();
  desktop.unpair(&id).await.unwrap();
  next(&mut pev, |e| matches!(e, Event::Unpaired(_))).await;
  assert!(phone.devices().is_empty());
}

#[tokio::test]
async fn nearby_lists_names_over_mdns() {
  let on = |name| Settings {
    discovery: true,
    ..settings(name)
  };
  let (dd, pd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
  let desktop = Engine::start(on("desktop"), dd.path()).await.unwrap();
  let _phone = Engine::start(on("phone"), pd.path()).await.unwrap();
  timeout(Duration::from_secs(30), async {
    loop {
      if desktop
        .scan()
        .await
        .unwrap()
        .iter()
        .any(|n| n.name == "phone")
      {
        break;
      }
      tokio::time::sleep(Duration::from_millis(200)).await;
    }
  })
  .await
  .expect("phone not listed");
}

fn with(name: &str, edit: impl FnOnce(&mut Settings)) -> Settings {
  let mut s = settings(name);
  edit(&mut s);
  s
}

/// Pairs two running engines through the desktop's link
async fn pair(desktop: &Arc<Engine>, phone: &Arc<Engine>) {
  let (mut dev, mut pev) = (desktop.events(), phone.events());
  let info = desktop.start_pairing().unwrap();
  phone.connect(&info.link).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;
  next(&mut pev, |e| matches!(e, Event::CodeReady(_))).await;
  desktop.decide(true).unwrap();
  phone.decide(true).unwrap();
  next(&mut dev, |e| matches!(e, Event::Paired(_))).await;
  next(&mut pev, |e| matches!(e, Event::Paired(_))).await;
}

async fn failed(rx: &mut Receiver<Event>) -> String {
  match next(rx, |e| matches!(e, Event::PairingFailed(_))).await {
    Event::PairingFailed(why) => why,
    _ => unreachable!(),
  }
}

#[tokio::test]
async fn receiver_refuses_a_device_it_already_paired() {
  let (desktop, _d) = engine("desktop").await;
  let pdir = tempfile::tempdir().unwrap();
  let phone = Engine::start(settings("phone"), pdir.path()).await.unwrap();
  pair(&desktop, &phone).await;
  phone.shutdown().await;
  drop(phone);

  // the same key with an empty store: only the desktop remembers the pairing
  let fresh = tempfile::tempdir().unwrap();
  std::fs::copy(pdir.path().join("key"), fresh.path().join("key")).unwrap();
  let phone = Engine::start(settings("phone"), fresh.path())
    .await
    .unwrap();
  let mut pev = phone.events();
  let info = desktop.start_pairing().unwrap();
  phone.connect(&info.link).await.unwrap();
  assert!(failed(&mut pev).await.contains("not accepting pairing"));
  assert_eq!(desktop.devices().len(), 1);
}

#[tokio::test]
async fn a_second_dialer_is_refused_while_confirming() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let (tablet, _t) = engine("tablet").await;
  let (mut dev, mut tev) = (desktop.events(), tablet.events());
  let info = desktop.start_pairing().unwrap();
  phone.connect(&info.link).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;

  tablet.connect(&info.link).await.unwrap();
  assert!(failed(&mut tev).await.contains("not accepting pairing"));
  assert!(matches!(desktop.pair_state(), PairState::Confirm(_)));
}

#[tokio::test]
async fn confirming_times_out() {
  let quick = |name| with(name, |s| s.pairing_timeout = Duration::from_millis(500));
  let (dd, pd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
  let desktop = Engine::start(quick("desktop"), dd.path()).await.unwrap();
  let phone = Engine::start(quick("phone"), pd.path()).await.unwrap();
  let (mut dev, mut pev) = (desktop.events(), phone.events());
  let info = desktop.start_pairing().unwrap();
  phone.connect(&info.link).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;
  // the dialer's timer started first; its expiry also ends the other side
  next(&mut pev, |e| matches!(e, Event::PairingExpired)).await;
  next(&mut dev, |e| {
    matches!(e, Event::PairingExpired | Event::PairingFailed(_))
  })
  .await;
  assert_eq!(phone.pair_state(), PairState::Idle);
  assert_eq!(desktop.pair_state(), PairState::Idle);
  assert!(desktop.devices().is_empty());
}

#[tokio::test]
async fn cancelling_stops_a_dial() {
  let (phone, _p) = engine("phone").await;
  let (gone, _g) = engine("gone").await;
  let id = gone.id();
  gone.shutdown().await;
  let mut pev = phone.events();
  // an address nobody answers on keeps the dial pending
  let link = Target {
    id,
    relay: None,
    addrs: vec!["10.255.255.1:9".parse().unwrap()],
  }
  .link();
  phone.connect(&link).await.unwrap();
  assert_eq!(phone.pair_state(), PairState::Dialing);
  phone.cancel_pairing();
  next(&mut pev, |e| matches!(e, Event::PairingCancelled)).await;
  assert_eq!(phone.pair_state(), PairState::Idle);
}

#[tokio::test]
async fn an_unpair_made_while_offline_arrives_on_reconnect() {
  let (desktop, _d) = engine("desktop").await;
  let pdir = tempfile::tempdir().unwrap();
  let phone = Engine::start(settings("phone"), pdir.path()).await.unwrap();
  pair(&desktop, &phone).await;
  phone.shutdown().await;
  drop(phone);

  let id = desktop.devices()[0].device.id.clone();
  desktop.unpair(&id).await.unwrap();
  // the desktop kept its port, so the phone's stored address still works
  let phone = Engine::start(settings("phone"), pdir.path()).await.unwrap();
  let mut pev = phone.events();
  next(&mut pev, |e| matches!(e, Event::Unpaired(_))).await;
  assert!(phone.devices().is_empty());
}

#[tokio::test]
async fn a_bare_device_id_is_enough_on_the_local_network() {
  let on = |name| with(name, |s| s.discovery = true);
  let (dd, pd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
  let desktop = Engine::start(on("desktop"), dd.path()).await.unwrap();
  let phone = Engine::start(on("phone"), pd.path()).await.unwrap();
  let mut dev = desktop.events();
  desktop.start_pairing().unwrap();
  phone.connect(&hex(&desktop.id())).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;
}

#[tokio::test]
async fn connecting_to_ourselves_or_garbage_fails() {
  let (phone, _p) = engine("phone").await;
  let own = phone.connect(&hex(&phone.id())).await.unwrap_err();
  assert!(own.to_string().contains("this device"), "{own}");
  let junk = phone.connect("not a device").await.unwrap_err();
  assert!(format!("{junk:#}").contains("device ID"), "{junk:#}");
  assert_eq!(phone.pair_state(), PairState::Idle);
}

#[tokio::test]
async fn discovery_switches_on_and_off_while_running() {
  let (dd, pd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
  let desktop = Engine::start(settings("desktop"), dd.path()).await.unwrap();
  let phone = Engine::start(settings("phone"), pd.path()).await.unwrap();
  for e in [&desktop, &phone] {
    e.set_settings(with(&e.settings().name, |s| s.discovery = true));
  }
  timeout(Duration::from_secs(30), async {
    while !desktop
      .scan()
      .await
      .unwrap()
      .iter()
      .any(|n| n.name == "phone")
    {
      tokio::time::sleep(Duration::from_millis(200)).await;
    }
  })
  .await
  .expect("phone not listed");

  desktop.set_settings(settings("desktop"));
  assert!(desktop.scan().await.unwrap().is_empty());
  tokio::time::sleep(Duration::from_secs(2)).await;
  assert!(
    desktop.scan().await.unwrap().is_empty(),
    "mDNS still running"
  );
}

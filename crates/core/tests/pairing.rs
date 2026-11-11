use std::{sync::Arc, time::Duration};

use aurora_core::{
  engine::{Engine, Settings},
  ipc::{self, IpcRegistry, IpcServer, commands},
  types::{Event, PairState},
};
use tokio::{sync::broadcast::Receiver, time::timeout};

fn settings(name: &str) -> Settings {
  Settings {
    name: name.into(),
    os: "Linux".into(),
    pairing_timeout: Duration::from_secs(60),
    discovery: true,
    port: None,
    host_alias: None,
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

  desktop
    .unpair(&desktop.devices()[0].device.id)
    .await
    .unwrap();
  next(&mut pev, |e| matches!(e, Event::Unpaired(_))).await;
  assert!(phone.devices().is_empty());
  assert!(desktop.devices().is_empty());
}

#[tokio::test]
async fn rejecting_pairs_nothing_and_token_is_single_use() {
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

  // replaying the old QR fails: no session is waiting any more
  phone.connect(&info.link).await.unwrap();
  next(&mut pev, |e| matches!(e, Event::PairingFailed(_))).await;
}

#[tokio::test]
async fn session_expires() {
  let (desktop, _d) = engine("desktop").await;
  desktop.set_settings(Settings {
    pairing_timeout: Duration::from_millis(200),
    ..settings("desktop")
  });
  let mut events = desktop.events();
  desktop.start_pairing().unwrap();
  next(&mut events, |e| matches!(e, Event::PairingExpired)).await;
  assert_eq!(desktop.pair_state(), PairState::Idle);
}

#[tokio::test]
async fn ipc_commands_and_event_stream() {
  let dir = tempfile::tempdir().unwrap();
  unsafe { std::env::set_var("AURORA_SOCKET", dir.path().join("t.sock")) };
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
    (info, events.next().unwrap().unwrap())
  })
  .await
  .unwrap();
  assert!(matches!(stream.1, Event::PairingStarted(i) if i.link == stream.0.link));
}

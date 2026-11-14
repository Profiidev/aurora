#![allow(dead_code)]

use std::{sync::Arc, time::Duration};

use aurora_core::{
  engine::{Engine, Settings},
  types::Event,
};
use aurora_transport::LinkKind;
use tokio::{sync::broadcast::Receiver, task::JoinHandle, time::timeout};

pub fn settings(name: &str) -> Settings {
  Settings {
    name: name.into(),
    os: "Linux".into(),
    pairing_timeout: Duration::from_secs(60),
    discovery: false,
    port: None,
    relay: false,
    usb: false,
    bluetooth: false,
  }
}

pub async fn engine(name: &str) -> (Arc<Engine>, tempfile::TempDir) {
  let dir = tempfile::tempdir().unwrap();
  (
    Engine::start(settings(name), dir.path()).await.unwrap(),
    dir,
  )
}

pub async fn next(rx: &mut Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
  timeout(Duration::from_secs(15), async {
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

pub async fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
  for _ in 0..150 {
    if f() {
      return;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
  }
  panic!("timed out waiting for {what}");
}

/// Pairs two engines with `target` as the link (QR, nearby entry)
pub async fn pair_via(desktop: &Arc<Engine>, phone: &Arc<Engine>, target: &str) {
  let (mut dev, mut pev) = (desktop.events(), phone.events());
  phone.connect(target).await.unwrap();
  next(&mut dev, |e| matches!(e, Event::CodeReady(_))).await;
  next(&mut pev, |e| matches!(e, Event::CodeReady(_))).await;
  desktop.decide(true).unwrap();
  phone.decide(true).unwrap();
  next(&mut dev, |e| matches!(e, Event::Paired(_))).await;
  next(&mut pev, |e| matches!(e, Event::Paired(_))).await;
}

/// Pairs over the desktop's pairing link and waits until both are connected
pub async fn paired() -> (Arc<Engine>, Arc<Engine>, [tempfile::TempDir; 2]) {
  let (desktop, d) = engine("desktop").await;
  let (phone, p) = engine("phone").await;
  let mut dev = desktop.events();
  let info = desktop.start_pairing().unwrap();
  pair_via(&desktop, &phone, &info.link).await;
  next(&mut dev, |e| matches!(e, Event::DeviceConnected(_))).await;
  wait_for("phone connected", || {
    phone.devices().iter().all(|d| d.connected)
  })
  .await;
  (desktop, phone, [d, p])
}

/// An in-memory USB cable between two engines; abort to unplug
pub fn cable(a: &Engine, b: &Engine) -> JoinHandle<()> {
  link(LinkKind::Usb, a, b)
}

/// An in-memory link of `kind` between two engines; abort to cut it
pub fn link(kind: LinkKind, a: &Engine, b: &Engine) -> JoinHandle<()> {
  let (x, y) = tokio::io::duplex(1 << 16);
  let (xr, xw) = tokio::io::split(x);
  let (yr, yw) = tokio::io::split(y);
  let (la, lb) = (a.links().clone(), b.links().clone());
  tokio::spawn(async move {
    let _ = tokio::join!(la.attach(kind, xr, xw), lb.attach(kind, yr, yw));
  })
}

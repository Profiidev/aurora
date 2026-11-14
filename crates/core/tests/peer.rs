//! Messages between paired devices through the peer registry

mod common;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use aurora_core::{
  engine::Engine,
  pairing::hex,
  peer::{Framed, PeerCtx, PeerRequest, PeerStream, messages},
  types::Event,
};
use common::*;
use serde::{Deserialize, Serialize};

fn id_of(e: &Engine) -> String {
  hex(&e.id())
}

#[tokio::test(flavor = "multi_thread")]
async fn battery_status_reaches_the_other_side() {
  let (desktop, phone, _dirs) = paired().await;
  let mut dev = desktop.events();
  phone.set_local_status(Some(42), true);
  next(
    &mut dev,
    |e| matches!(e, Event::DeviceConnected(s) if s.battery == Some(42) && s.charging),
  )
  .await;
  let d = &desktop.devices()[0];
  assert_eq!((d.battery, d.charging), (Some(42), true));
}

#[tokio::test(flavor = "multi_thread")]
async fn ping_is_answered() {
  let (desktop, phone, _dirs) = paired().await;
  desktop
    .request::<messages::Ping>(&id_of(&phone), &())
    .await
    .unwrap();
  phone
    .request::<messages::Ping>(&id_of(&desktop), &())
    .await
    .unwrap();
}

/// Known to the sender only
struct Unknown;

impl PeerRequest for Unknown {
  const KIND: &'static str = "test:unknown";
  type Req = u32;
  type Resp = u32;

  async fn handle(req: u32, _ctx: PeerCtx) -> Result<u32> {
    Ok(req)
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_requests_are_unsupported_and_unknown_messages_ignored() {
  let (desktop, phone, _dirs) = paired().await;
  let err = desktop
    .request::<Unknown>(&id_of(&phone), &1)
    .await
    .unwrap_err();
  assert!(format!("{err:#}").contains("not supported"), "{err:#}");
  // the connection survives both
  desktop
    .open_stream::<Echo>(&id_of(&phone), &Prefix("x".into()))
    .await
    .err()
    .expect("phone has no Echo handler");
  desktop
    .request::<messages::Ping>(&id_of(&phone), &())
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sending_to_an_unconnected_device_fails() {
  let (desktop, _d) = engine("desktop").await;
  assert!(
    desktop
      .request::<messages::Ping>(&"00".repeat(32), &())
      .await
      .is_err()
  );
  assert!(
    desktop
      .notify::<messages::BatteryStatus>(
        &"00".repeat(32),
        &messages::Battery {
          battery: None,
          charging: false
        }
      )
      .await
      .is_err()
  );
}

#[derive(Serialize, Deserialize)]
struct Prefix(String);

/// Answers every line with the header's prefix in front
struct Echo;

impl PeerStream for Echo {
  const KIND: &'static str = "test:echo";
  type Header = Prefix;
  type Up = String;
  type Down = String;

  async fn handle(header: Prefix, _ctx: PeerCtx, mut ch: Framed<String, String>) -> Result<()> {
    while let Some(line) = ch.recv().await? {
      ch.send(&format!("{}{line}", header.0)).await?;
    }
    ch.finish()
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_carry_typed_frames_both_ways() {
  let start = |name: &'static str| async move {
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::start_with(settings(name), dir.path(), |r| {
      messages::register_all(r);
      r.register_stream::<Echo>();
    })
    .await
    .unwrap();
    (e, dir)
  };
  let (desktop, _d) = start("desktop").await;
  let (phone, _p) = start("phone").await;
  let info = desktop.start_pairing().unwrap();
  pair_via(&desktop, &phone, &info.link).await;
  wait_for("connected", || {
    desktop.devices().iter().all(|d| d.connected)
  })
  .await;

  let mut ch = phone
    .open_stream::<Echo>(&id_of(&desktop), &Prefix("> ".into()))
    .await
    .unwrap();
  for line in ["a", "bc", ""] {
    ch.send(&line.to_string()).await.unwrap();
    assert_eq!(ch.recv().await.unwrap().unwrap(), format!("> {line}"));
  }
  ch.finish().unwrap();
  assert!(ch.recv().await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn unpair_message_removes_the_device_on_both_sides() {
  let (desktop, phone, _dirs) = paired().await;
  let mut pev = phone.events();
  desktop.unpair(&id_of(&phone)).await.unwrap();
  next(&mut pev, |e| matches!(e, Event::Unpaired(_))).await;
  assert!(desktop.devices().is_empty());
  assert!(phone.devices().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn handlers_see_who_sent() {
  struct WhoAmI;
  impl PeerRequest for WhoAmI {
    const KIND: &'static str = "test:whoami";
    type Req = ();
    type Resp = String;
    async fn handle((): (), ctx: PeerCtx) -> Result<String> {
      Ok(ctx.device)
    }
  }
  let dir = tempfile::tempdir().unwrap();
  let desktop: Arc<Engine> = Engine::start_with(settings("desktop"), dir.path(), |r| {
    messages::register_all(r);
    r.register_request::<WhoAmI>();
  })
  .await
  .unwrap();
  let (phone, _p) = engine("phone").await;
  let info = desktop.start_pairing().unwrap();
  pair_via(&desktop, &phone, &info.link).await;
  wait_for("connected", || phone.devices().iter().all(|d| d.connected)).await;
  let seen = tokio::time::timeout(
    Duration::from_secs(10),
    phone.request::<WhoAmI>(&id_of(&desktop), &()),
  )
  .await
  .unwrap()
  .unwrap();
  assert_eq!(seen, id_of(&phone));
}

#[test]
#[should_panic(expected = "registered twice")]
fn registering_a_kind_twice_panics() {
  let mut r = aurora_core::peer::PeerRegistry::default();
  messages::register_all(&mut r);
  r.register_request::<messages::Ping>();
}

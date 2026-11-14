//! Paired and unpaired devices over an (in-memory) USB cable

mod common;

use std::time::Duration;

use aurora_core::{engine::Settings, pairing::hex, peer::messages, types::Event};
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn cabled_devices_show_up_nearby_with_discovery_off() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let _cable = cable(&desktop, &phone);
  for (e, other) in [(&desktop, "phone"), (&phone, "desktop")] {
    let e = e.clone();
    let mut found = Vec::new();
    for _ in 0..100 {
      found = e.scan().await.unwrap();
      if !found.is_empty() {
        break;
      }
      tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(found.len(), 1, "{other} not nearby");
    assert_eq!(found[0].name, other);
    assert!(found[0].usb);
  }
}

#[tokio::test(flavor = "multi_thread")]
async fn pairs_over_the_cable_and_shows_usb() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let _cable = cable(&desktop, &phone);
  let mut nearby = Vec::new();
  wait_for("nearby", || {
    nearby = futures_block(phone.scan());
    !nearby.is_empty()
  })
  .await;
  let (mut dev, mut pev) = (desktop.events(), phone.events());
  desktop.start_pairing().unwrap();
  phone.connect(&nearby[0].link).await.unwrap();
  for rx in [&mut dev, &mut pev] {
    let Event::CodeReady(view) = next(rx, |e| matches!(e, Event::CodeReady(_))).await else {
      unreachable!()
    };
    assert_eq!(view.via, "USB");
  }
  desktop.decide(true).unwrap();
  phone.decide(true).unwrap();
  next(&mut pev, |e| matches!(e, Event::Paired(_))).await;
  // the paired devices talk over the cable right away
  wait_for("connected", || phone.devices().iter().all(|d| d.connected)).await;
  phone
    .request::<messages::Ping>(&hex(&desktop.id()), &())
    .await
    .unwrap();
}

fn futures_block<T>(f: impl std::future::Future<Output = anyhow::Result<T>>) -> T {
  tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn pulling_the_cable_keeps_the_device_connected() {
  let (desktop, phone, _dirs) = paired().await;
  let (did, pid) = (hex(&desktop.id()), hex(&phone.id()));
  let mut dev = desktop.events();
  let cable = cable(&desktop, &phone);
  wait_for("link up", || desktop.links().is_linked(&phone.id())).await;
  // give the lower id side time to dial over the link
  tokio::time::sleep(Duration::from_millis(500)).await;
  desktop.request::<messages::Ping>(&pid, &()).await.unwrap();
  phone.request::<messages::Ping>(&did, &()).await.unwrap();

  cable.abort();
  wait_for("link down", || !desktop.links().is_linked(&phone.id())).await;
  tokio::time::sleep(Duration::from_millis(300)).await;
  assert!(desktop.devices()[0].connected);
  desktop.request::<messages::Ping>(&pid, &()).await.unwrap();
  phone.set_local_status(Some(7), false);
  next(
    &mut dev,
    |e| matches!(e, Event::DeviceConnected(s) if s.battery == Some(7)),
  )
  .await;
  assert!(
    !std::iter::from_fn(|| dev.try_recv().ok()).any(|e| matches!(e, Event::DeviceDisconnected(_))),
    "cable pull must not disconnect"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn usb_setting_round_trips() {
  let (desktop, _d) = engine("desktop").await;
  desktop.set_settings(Settings {
    usb: true,
    ..settings("desktop")
  });
  assert!(desktop.settings().usb);
  desktop.set_settings(settings("desktop"));
  assert!(!desktop.settings().usb);
}

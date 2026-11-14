//! The Bluetooth fallback: when it runs, what it dials, and that it gives way
//! to better paths. The radio is an in-memory link here.

mod common;

use std::time::Duration;

use aurora_core::{
  engine::{Engine, Settings},
  ipc::{self, IpcRegistry, IpcServer, commands},
  types::Event,
};
use aurora_transport::LinkKind;
use common::*;

fn bluetooth_on(e: &Engine, discovery: bool) {
  e.set_settings(Settings {
    bluetooth: true,
    discovery,
    ..e.settings()
  });
}

fn prefix(e: &Engine) -> [u8; 4] {
  e.id()[..4].try_into().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bluetooth_link_gives_way_to_the_network() {
  let (desktop, phone, _dirs) = paired().await;
  let mut dev = desktop.events();
  let _radio = link(LinkKind::Bluetooth, &desktop, &phone);
  wait_for("link up", || {
    desktop
      .links()
      .is_linked_by(LinkKind::Bluetooth, &phone.id())
  })
  .await;
  // the network connection exists, so the fallback is dropped again
  wait_for("link dropped", || {
    !desktop
      .links()
      .is_linked_by(LinkKind::Bluetooth, &phone.id())
  })
  .await;
  assert_eq!(desktop.devices()[0].via.as_deref(), Some("Direct"));
  assert!(
    !std::iter::from_fn(|| dev.try_recv().ok()).any(|e| matches!(e, Event::DeviceDisconnected(_))),
    "dropping the fallback must not disconnect"
  );
}

#[tokio::test(flavor = "multi_thread")]
async fn usb_is_preferred_over_bluetooth() {
  let (desktop, phone, _dirs) = paired().await;
  let _cable = cable(&desktop, &phone);
  wait_for("usb", || desktop.devices()[0].via.as_deref() == Some("USB")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_phone_dials_offline_paired_desktops_only() {
  let (desktop, phone, _dirs) = paired().await;
  let _ui = phone.ui_presence();
  bluetooth_on(&phone, false);
  assert!(
    !phone.should_dial_bt(&prefix(&desktop)),
    "connected: no need"
  );

  desktop.shutdown().await;
  wait_for("offline", || {
    let devices = phone.devices();
    !devices.is_empty() && devices.iter().all(|d| !d.connected)
  })
  .await;
  wait_for("wanted", || phone.bluetooth_wanted()).await;
  assert!(phone.should_dial_bt(&prefix(&desktop)));
  assert!(!phone.should_dial_bt(&[0xee; 4]), "a stranger, not pairing");
}

#[tokio::test(flavor = "multi_thread")]
async fn bluetooth_nearby_follows_discovery() {
  let (desktop, _d) = engine("desktop").await;
  let (phone, _p) = engine("phone").await;
  let _radio = link(LinkKind::Bluetooth, &desktop, &phone);
  wait_for("link up", || {
    phone
      .links()
      .is_linked_by(LinkKind::Bluetooth, &desktop.id())
  })
  .await;
  tokio::time::sleep(Duration::from_secs(2)).await;
  assert!(phone.scan().await.unwrap().is_empty(), "discovery off");

  // a fresh link once both sides list nearby devices
  bluetooth_on(&desktop, true);
  bluetooth_on(&phone, true);
  drop(_radio);
  let _radio = link(LinkKind::Bluetooth, &desktop, &phone);
  let mut found = Vec::new();
  for _ in 0..100 {
    found = phone.scan().await.unwrap();
    if !found.is_empty() {
      break;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
  }
  assert_eq!(found.len(), 1);
  assert!(found[0].bluetooth && !found[0].usb);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_window_counts_over_ipc() {
  let dir = tempfile::tempdir().unwrap();
  unsafe {
    std::env::set_var("AURORA_SOCKET", dir.path().join("t.sock"));
    std::env::set_var("XDG_STATE_HOME", dir.path());
  }
  let (desktop, _d) = engine("desktop").await;
  let server = IpcServer::new().unwrap().unwrap();
  let mut registry = IpcRegistry::default();
  commands::register_all(&mut registry);
  tokio::spawn(server.run(registry, desktop.clone()));
  bluetooth_on(&desktop, true);
  desktop.start_pairing().unwrap();
  assert!(!desktop.bluetooth_wanted(), "no window yet");

  let window = tokio::task::spawn_blocking(|| ipc::subscribe::<commands::UiPresent>(()).unwrap())
    .await
    .unwrap();
  wait_for("window counted", || desktop.bluetooth_wanted()).await;
  drop(window);
  wait_for("window gone", || !desktop.bluetooth_wanted()).await;
}

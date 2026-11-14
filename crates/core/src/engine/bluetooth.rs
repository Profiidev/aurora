//! When the Bluetooth fallback runs. It costs battery on both sides, so only
//! while someone has the app open, Bluetooth is on in the settings, and a
//! paired device is offline or the pair page waits with discovery on. Once
//! a device is reachable over USB or the network, its Bluetooth link goes.

use std::{
  sync::{Arc, atomic::Ordering},
  time::Duration,
};

use aurora_transport::{LinkKind, bluetooth::PREFIX_LEN};
use tokio::time::sleep;

use super::{Engine, links::Path, session::Phase};
use crate::pairing::unhex;

const CHECK_EVERY: Duration = Duration::from_secs(2);

/// One open window or foreground app, for as long as it lives
pub struct PresenceGuard(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for PresenceGuard {
  fn drop(&mut self) {
    self.0.fetch_sub(1, Ordering::AcqRel);
  }
}

impl Engine {
  /// Counts as an open app until dropped (the desktop window over IPC, the
  /// phone app while in the foreground)
  pub fn ui_presence(&self) -> PresenceGuard {
    self.ui.fetch_add(1, Ordering::AcqRel);
    PresenceGuard(self.ui.clone())
  }

  fn ui_present(&self) -> bool {
    self.ui.load(Ordering::Acquire) > 0
  }

  /// Pairing over Bluetooth: the pair page is open and discovery is on
  fn pairing_nearby(&self) -> bool {
    self.settings().discovery && matches!(self.session.lock().unwrap().phase, Phase::Waiting { .. })
  }

  /// A paired device has no connection at all
  fn any_offline(&self) -> bool {
    let live = self.live.lock().unwrap();
    (self.store.lock().unwrap().devices().iter()).any(|d| !live.contains_key(&d.id))
  }

  /// Bluetooth may be used at all: switched on and someone looking. Links
  /// end when this turns false.
  pub fn bluetooth_allowed(&self) -> bool {
    self.settings().bluetooth && self.ui_present()
  }

  /// Bluetooth should run: advertise (desktop) or scan (phone)
  pub fn bluetooth_wanted(&self) -> bool {
    self.bluetooth_allowed() && (self.any_offline() || self.pairing_nearby())
  }

  /// The phone saw a desktop advertising the start of its id: dial it when
  /// it is a paired device that is offline, or anyone while pairing
  pub fn should_dial_bt(&self, prefix: &[u8; PREFIX_LEN]) -> bool {
    if !self.bluetooth_wanted() {
      return false;
    }
    if self.pairing_nearby() {
      return true;
    }
    let live = self.live.lock().unwrap();
    (self.store.lock().unwrap().devices().iter()).any(|d| {
      unhex(&d.id).is_ok_and(|id: aurora_proto::Id| id[..PREFIX_LEN] == prefix[..])
        && !live.contains_key(&d.id)
    })
  }

  /// Devices with a Bluetooth connection and a better one
  fn bluetooth_redundant(&self) -> Vec<String> {
    (self.live.lock().unwrap().iter())
      .filter(|(_, l)| l.has(Path::Bluetooth) && (l.has(Path::Usb) || l.has(Path::Net)))
      .map(|(id, _)| id.clone())
      .collect()
  }

  pub(super) async fn bluetooth_loop(self: Arc<Self>) {
    loop {
      // a no-op on the phone, whose app drives scanning itself
      self.node.set_bluetooth(self.bluetooth_wanted());
      for id in self.bluetooth_redundant() {
        if let Ok(peer) = unhex(&id) {
          self.node.links().disconnect(LinkKind::Bluetooth, &peer);
        }
      }
      self.refresh_paths();
      sleep(CHECK_EVERY).await;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::super::tests::engine;

  #[tokio::test]
  async fn wanted_needs_a_window_and_a_reason() {
    let (e, _d) = engine("desktop", false).await;
    let mut s = e.settings();
    s.bluetooth = true;
    e.set_settings(s);
    // nothing paired, no pair page
    let ui = e.ui_presence();
    assert!(!e.bluetooth_wanted());

    // pair page open, but discovery off
    e.start_pairing().unwrap();
    assert!(!e.bluetooth_wanted());
    let mut s = e.settings();
    s.discovery = true;
    e.set_settings(s);
    assert!(e.bluetooth_wanted());
    assert!(e.should_dial_bt(&[9; 4]), "anyone while pairing");

    // nobody looking
    drop(ui);
    assert!(!e.bluetooth_wanted());
    assert!(!e.should_dial_bt(&[9; 4]));

    // switched off
    let _ui = e.ui_presence();
    let mut s = e.settings();
    s.bluetooth = false;
    e.set_settings(s);
    assert!(!e.bluetooth_wanted());
  }
}

use anyhow::Result;
use aurora_core::types::{AppSettings, DeviceStatus, Event, Nearby, PairInfo, PairState};

/// What the views need from the pairing engine. Blocking: views call it from
/// a background task. Desktop talks to `aurorad`, mobile owns an engine.
pub trait Backend: Send + Sync + 'static {
  fn devices(&self) -> Result<Vec<DeviceStatus>>;
  fn pair_state(&self) -> Result<PairState>;
  fn start_pairing(&self) -> Result<PairInfo>;
  fn cancel_pairing(&self) -> Result<()>;
  /// `aurora://pair` link or host
  fn connect(&self, target: &str) -> Result<()>;
  fn decide(&self, accept: bool) -> Result<()>;
  fn nearby(&self) -> Result<Vec<Nearby>>;
  fn unpair(&self, id: &str) -> Result<()>;
  fn settings(&self) -> Result<AppSettings>;
  fn set_discovery(&self, on: bool) -> Result<()>;
  fn set_usb(&self, on: bool) -> Result<()>;
  fn set_bluetooth(&self, on: bool) -> Result<()>;
  /// Phone only: keep the engine running when the app is closed
  fn set_background(&self, on: bool) -> Result<()>;
  /// Tells the engine a window is open, for as long as the process lives;
  /// Bluetooth only runs while someone looks
  fn hold_presence(&self) {}
  /// Live camera scan for an `aurora://pair` QR code. `None` when cancelled.
  fn scan_qr(&self) -> Result<Option<String>> {
    anyhow::bail!("QR scanning is only available in the mobile app")
  }
  fn cancel_scan(&self) {}
  /// Blocks between events and keeps going if the source drops
  fn events(&self) -> Box<dyn Iterator<Item = Event> + Send>;
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use daemon::DaemonBackend;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod daemon {
  use std::{thread::sleep, time::Duration};

  use aurora_core::ipc::{self, commands as cmd};

  use super::*;

  pub struct DaemonBackend;

  impl Backend for DaemonBackend {
    fn devices(&self) -> Result<Vec<DeviceStatus>> {
      ipc::send::<cmd::DevicesList>(())
    }
    fn pair_state(&self) -> Result<PairState> {
      ipc::send::<cmd::PairStatus>(())
    }
    fn start_pairing(&self) -> Result<PairInfo> {
      ipc::send::<cmd::PairStart>(())
    }
    fn cancel_pairing(&self) -> Result<()> {
      ipc::send::<cmd::PairCancel>(())
    }
    fn connect(&self, target: &str) -> Result<()> {
      ipc::send::<cmd::PairConnect>(target.into())
    }
    fn decide(&self, accept: bool) -> Result<()> {
      if accept {
        ipc::send::<cmd::PairAccept>(())
      } else {
        ipc::send::<cmd::PairReject>(())
      }
    }
    fn nearby(&self) -> Result<Vec<Nearby>> {
      ipc::send::<cmd::PairNearby>(())
    }
    fn unpair(&self, id: &str) -> Result<()> {
      ipc::send::<cmd::DevicesUnpair>(id.into())
    }
    fn settings(&self) -> Result<AppSettings> {
      ipc::send::<cmd::SettingsGet>(())
    }
    fn set_discovery(&self, on: bool) -> Result<()> {
      ipc::send::<cmd::SettingsSetDiscovery>(on)
    }
    fn set_usb(&self, on: bool) -> Result<()> {
      ipc::send::<cmd::SettingsSetUsb>(on)
    }
    fn set_bluetooth(&self, on: bool) -> Result<()> {
      ipc::send::<cmd::SettingsSetBluetooth>(on)
    }
    fn set_background(&self, on: bool) -> Result<()> {
      ipc::send::<cmd::SettingsSetBackground>(on)
    }
    fn hold_presence(&self) {
      std::thread::spawn(|| {
        loop {
          // the daemon counts us while this stream is open; it never sends
          if let Ok(mut held) = ipc::subscribe::<cmd::UiPresent>(()) {
            while let Some(Ok(())) = held.next() {}
          }
          sleep(Duration::from_secs(2));
        }
      });
    }

    fn events(&self) -> Box<dyn Iterator<Item = Event> + Send> {
      let mut current: Option<Box<dyn Iterator<Item = Result<Event>> + Send>> = None;
      Box::new(std::iter::from_fn(move || {
        loop {
          if let Some(events) = &mut current {
            match events.next() {
              Some(Ok(event)) => return Some(event),
              _ => current = None,
            }
          }
          match ipc::subscribe::<cmd::Events>(()) {
            // tells the view to refresh after (re)connecting
            Ok(events) => {
              current = Some(Box::new(events));
              return Some(Event::ConfigChanged);
            }
            Err(_) => sleep(Duration::from_secs(2)),
          }
        }
      }))
    }
  }
}

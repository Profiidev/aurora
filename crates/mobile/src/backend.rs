#![cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]

//! The phone has no daemon: the UI drives a pairing engine in this process.

use std::{path::Path, sync::Arc};

use anyhow::Result;
use aurora_core::{
  config,
  engine::{Engine, Settings},
  types::{AppSettings, DeviceStatus, Event, Nearby, PairInfo, PairState},
};
use aurora_ui::Backend;
use tokio::{runtime::Runtime, sync::broadcast::error::RecvError};

pub struct EngineBackend {
  rt: Runtime,
  engine: Arc<Engine>,
  dir: std::path::PathBuf,
}

impl EngineBackend {
  /// Keys, paired devices and `settings.toml` live in `dir`
  pub fn start(dir: &Path, name: String, os: &str) -> Result<Self> {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2)
      .enable_all()
      .build()?;
    let file = dir.join("settings.toml");
    // mDNS is opt-in on the phone: it needs a multicast lock that costs battery
    if !file.exists() {
      config::set_discovery(dir, false)?;
    }
    // no user config dir on the phone, only the file the app writes
    let loaded = config::read_from(&dir.join("config"), &file, std::iter::empty())?;
    let settings = Settings {
      name,
      os: os.into(),
      pairing_timeout: std::time::Duration::from_secs(60),
      discovery: loaded.config.pairing.discovery,
      port: None,
      relay: true,
      usb: USB && loaded.config.transport.usb,
      bluetooth: loaded.config.transport.bluetooth,
    };
    multicast(settings.discovery);
    let engine = rt.block_on(Engine::start(settings, dir))?;
    #[cfg(target_os = "android")]
    crate::usb::spawn(rt.handle().clone(), engine.clone());
    Ok(Self {
      rt,
      engine,
      dir: dir.to_path_buf(),
    })
  }
}

impl Backend for EngineBackend {
  fn devices(&self) -> Result<Vec<DeviceStatus>> {
    Ok(self.engine.devices())
  }
  fn pair_state(&self) -> Result<PairState> {
    Ok(self.engine.pair_state())
  }
  fn start_pairing(&self) -> Result<PairInfo> {
    let _guard = self.rt.enter();
    self.engine.start_pairing()
  }
  fn cancel_pairing(&self) -> Result<()> {
    self.engine.cancel_pairing();
    Ok(())
  }
  fn connect(&self, target: &str) -> Result<()> {
    self.rt.block_on(self.engine.connect(target))
  }
  fn decide(&self, accept: bool) -> Result<()> {
    self.engine.decide(accept)
  }
  fn nearby(&self) -> Result<Vec<Nearby>> {
    self.rt.block_on(self.engine.scan())
  }
  fn unpair(&self, id: &str) -> Result<()> {
    self.rt.block_on(self.engine.unpair(id))
  }
  fn settings(&self) -> Result<AppSettings> {
    let s = self.engine.settings();
    Ok(AppSettings {
      name: s.name,
      discovery: s.discovery,
      usb: USB.then_some(s.usb),
      bluetooth: Some(s.bluetooth),
    })
  }
  fn set_bluetooth(&self, on: bool) -> Result<()> {
    config::set_bluetooth(&self.dir, on)?;
    let _guard = self.rt.enter();
    let mut s = self.engine.settings();
    s.bluetooth = on;
    self.engine.set_settings(s);
    Ok(())
  }
  fn set_usb(&self, on: bool) -> Result<()> {
    anyhow::ensure!(USB, "this phone has no USB connection");
    config::set_usb(&self.dir, on)?;
    let _guard = self.rt.enter();
    let mut s = self.engine.settings();
    s.usb = on;
    self.engine.set_settings(s);
    Ok(())
  }
  fn set_discovery(&self, on: bool) -> Result<()> {
    config::set_discovery(&self.dir, on)?;
    multicast(on);
    let _guard = self.rt.enter();
    let mut s = self.engine.settings();
    s.discovery = on;
    self.engine.set_settings(s);
    Ok(())
  }
  #[cfg(target_os = "android")]
  fn scan_qr(&self) -> Result<Option<String>> {
    crate::scanner::scan().map_err(anyhow::Error::msg)
  }
  #[cfg(target_os = "android")]
  fn cancel_scan(&self) {
    crate::scanner::cancel();
  }
  fn events(&self) -> Box<dyn Iterator<Item = Event> + Send> {
    let mut rx = self.engine.events();
    Box::new(std::iter::from_fn(move || match rx.blocking_recv() {
      Ok(event) => Some(event),
      Err(RecvError::Lagged(_)) => Some(Event::ConfigChanged),
      Err(RecvError::Closed) => None,
    }))
  }
}

/// iOS has no USB accessory mode without Apple's MFi program
const USB: bool = cfg!(not(target_os = "ios"));

/// Android drops incoming multicast, which mDNS needs, unless the app holds a lock
fn multicast(on: bool) {
  #[cfg(target_os = "android")]
  if let Err(e) = crate::multicast::set(on) {
    log::warn!("multicast lock: {e}");
  }
  #[cfg(not(target_os = "android"))]
  let _ = on;
}

/// Model name of this phone, for the name the computer shows
#[cfg(target_os = "android")]
pub fn device_name() -> String {
  std::process::Command::new("getprop")
    .arg("ro.product.model")
    .output()
    .ok()
    .and_then(|o| String::from_utf8(o.stdout).ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| "Android".into())
}

#[cfg(test)]
mod tests {
  use aurora_ui::Backend;

  use super::EngineBackend;

  #[test]
  fn discovery_is_opt_in_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let phone = EngineBackend::start(dir.path(), "phone".into(), "Android").unwrap();
    assert!(!phone.settings().unwrap().discovery);
    phone.set_discovery(true).unwrap();
    drop(phone);

    let phone = EngineBackend::start(dir.path(), "phone".into(), "Android").unwrap();
    assert!(phone.settings().unwrap().discovery);
    let saved = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
    assert!(saved.contains("discovery = true"), "{saved}");
  }

  #[test]
  fn usb_is_on_by_default_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let phone = EngineBackend::start(dir.path(), "phone".into(), "Android").unwrap();
    assert_eq!(phone.settings().unwrap().usb, Some(true));
    phone.set_usb(false).unwrap();
    drop(phone);

    let phone = EngineBackend::start(dir.path(), "phone".into(), "Android").unwrap();
    assert_eq!(phone.settings().unwrap().usb, Some(false));
  }
}

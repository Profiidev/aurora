#![cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]

//! The phone has no daemon: the UI drives a pairing engine in this process.

use std::{path::Path, sync::Arc};

use anyhow::Result;
use aurora_core::{
  engine::{Engine, Settings},
  types::{DeviceStatus, Event, Nearby, PairInfo, PairState},
};
use aurora_ui::Backend;
use tokio::{runtime::Runtime, sync::broadcast::error::RecvError};

pub struct EngineBackend {
  rt: Runtime,
  engine: Arc<Engine>,
}

impl EngineBackend {
  /// Keys and paired devices live in `dir`
  pub fn start(dir: &Path, name: String, os: &str, emulator: bool) -> Result<Self> {
    let rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2)
      .enable_all()
      .build()?;
    let settings = Settings {
      name,
      os: os.into(),
      pairing_timeout: std::time::Duration::from_secs(60),
      discovery: true,
      port: None,
      // the emulator reaches its host at 10.0.2.2
      host_alias: emulator.then(|| "10.0.2.2".parse().unwrap()),
    };
    let engine = rt.block_on(Engine::start(settings, dir))?;
    Ok(Self { rt, engine })
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
  fn events(&self) -> Box<dyn Iterator<Item = Event> + Send> {
    let mut rx = self.engine.events();
    Box::new(std::iter::from_fn(move || match rx.blocking_recv() {
      Ok(event) => Some(event),
      Err(RecvError::Lagged(_)) => Some(Event::ConfigChanged),
      Err(RecvError::Closed) => None,
    }))
  }
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

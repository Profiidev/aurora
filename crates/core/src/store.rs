use std::{
  io::Write,
  net::SocketAddr,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::types::Device;

/// Writes `data` next to `path` and renames it over, mode 0600
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
  let tmp = path.with_extension("tmp");
  let _ = std::fs::remove_file(&tmp);
  let mut opts = std::fs::OpenOptions::new();
  opts.write(true).create_new(true);
  #[cfg(unix)]
  std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
  let mut file = opts.open(&tmp)?;
  file.write_all(data)?;
  file.sync_all()?;
  std::fs::rename(tmp, path)?;
  Ok(())
}

pub fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |d| d.as_secs())
}

#[derive(Default, Serialize, Deserialize)]
struct File {
  #[serde(default)]
  device: Vec<Device>,
}

/// Paired devices, persisted as `devices.toml` in the state dir
pub struct Store {
  path: PathBuf,
  devices: Vec<Device>,
}

impl Store {
  pub fn open(dir: &Path) -> Result<Self> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("devices.toml");
    let devices = match std::fs::read_to_string(&path) {
      Ok(text) => toml::from_str::<File>(&text)?.device,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
      Err(e) => return Err(e.into()),
    };
    Ok(Self { path, devices })
  }

  pub fn devices(&self) -> &[Device] {
    &self.devices
  }

  pub fn get(&self, id: &str) -> Option<&Device> {
    self.devices.iter().find(|d| d.id == id)
  }

  /// Adds the device, replacing an earlier pairing with the same id
  pub fn insert(&mut self, device: Device) -> Result<()> {
    self.devices.retain(|d| d.id != device.id);
    self.devices.push(device);
    self.save()
  }

  /// Remembers where a device was last reached; saves only on change
  pub fn set_hints(
    &mut self,
    id: &str,
    relay: Option<String>,
    addrs: Vec<SocketAddr>,
  ) -> Result<()> {
    let Some(d) = self.devices.iter_mut().find(|d| d.id == id) else {
      return Ok(());
    };
    if (&d.relay, &d.addrs) == (&relay, &addrs) {
      return Ok(());
    }
    (d.relay, d.addrs) = (relay, addrs);
    self.save()
  }

  pub fn remove(&mut self, id: &str) -> Result<bool> {
    let len = self.devices.len();
    self.devices.retain(|d| d.id != id);
    let removed = self.devices.len() != len;
    if removed {
      self.save()?;
    }
    Ok(removed)
  }

  fn save(&self) -> Result<()> {
    let file = File {
      device: self.devices.clone(),
    };
    write_atomic(&self.path, toml::to_string_pretty(&file)?.as_bytes())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn persists_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let device = Device {
      id: "ab".into(),
      name: "Pixel".into(),
      os: "Android".into(),
      paired_at: 1,
      relay: None,
      addrs: vec!["10.0.0.2:4000".parse().unwrap()],
    };
    let mut store = Store::open(dir.path()).unwrap();
    store.insert(device.clone()).unwrap();
    assert_eq!(Store::open(dir.path()).unwrap().devices(), [device]);
    assert!(store.remove("ab").unwrap());
    assert!(Store::open(dir.path()).unwrap().devices().is_empty());
  }
}

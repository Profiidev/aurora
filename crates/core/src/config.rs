//! TOML config, layered like corona's: built-in defaults, every `*.toml` under
//! `~/.config/aurora` (sorted, later wins), `~/.local/state/aurora/settings.toml`,
//! then `AURORA_<SECTION>__<KEY>` environment variables.

use std::{
  path::{Path, PathBuf},
  sync::mpsc,
  time::Duration,
};

use anyhow::{Context, Result};
use config::{Environment, File, FileFormat};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::engine::Settings;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
  pub device: DeviceConfig,
  pub pairing: PairingConfig,
  pub network: NetworkConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
  /// Shown to paired devices; defaults to the host name
  pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PairingConfig {
  pub timeout_secs: u64,
  /// Answer discovery broadcasts so phones list this computer as nearby
  pub discovery: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
  /// Fixed QUIC port; random when unset. Read at startup only.
  pub port: Option<u16>,
}

impl Default for PairingConfig {
  fn default() -> Self {
    Self {
      timeout_secs: 60,
      discovery: true,
    }
  }
}

impl Config {
  pub fn settings(&self) -> Settings {
    Settings {
      name: self.device.name.clone().unwrap_or_else(host_name),
      os: os_name(),
      pairing_timeout: Duration::from_secs(self.pairing.timeout_secs.max(1)),
      discovery: self.pairing.discovery,
      port: self.network.port,
      host_alias: None,
    }
  }
}

pub fn host_name() -> String {
  std::fs::read_to_string("/etc/hostname")
    .ok()
    .map(|h| h.trim().to_string())
    .filter(|h| !h.is_empty())
    .or_else(|| std::env::var("HOSTNAME").ok())
    .unwrap_or_else(|| "aurora".into())
}

pub fn os_name() -> String {
  match std::env::consts::OS {
    "linux" => "Linux".into(),
    "macos" => "macOS".into(),
    "windows" => "Windows".into(),
    other => other.into(),
  }
}

pub fn config_dir() -> PathBuf {
  dirs::config_dir().unwrap_or_default().join("aurora")
}

/// Where the daemon keeps its key, devices and settings
pub fn state_dir() -> PathBuf {
  (dirs::state_dir().or_else(dirs::data_local_dir))
    .unwrap_or_default()
    .join("aurora")
}

pub struct Loaded {
  pub config: Config,
  /// Keys no section knows, likely typos
  pub unknown: Vec<String>,
}

pub fn read() -> Result<Loaded> {
  read_from(
    &config_dir(),
    &state_dir().join("settings.toml"),
    std::env::vars(),
  )
}

pub fn read_from(
  dir: &Path,
  settings_file: &Path,
  env: impl Iterator<Item = (String, String)>,
) -> Result<Loaded> {
  let mut files = Vec::new();
  collect_toml(dir, &mut files);
  files.sort();
  files.push(settings_file.to_path_buf());

  let env = env
    .filter(|(k, _)| k.strip_prefix("AURORA_").is_some_and(|k| k.contains("__")))
    .collect();
  let mut builder = config::Config::builder();
  for file in &files {
    builder = builder.add_source(
      File::from(file.as_path())
        .format(FileFormat::Toml)
        .required(false),
    );
  }
  let merged = builder
    .add_source(
      Environment::with_prefix("AURORA")
        .prefix_separator("_")
        .separator("__")
        .source(Some(env)),
    )
    .build()
    .map_err(|e| blame(&files, e))?;

  let mut unknown = Vec::new();
  let config = serde_ignored::deserialize(merged, |path| unknown.push(path.to_string()))
    .context("invalid config")?;
  Ok(Loaded { config, unknown })
}

/// The merged build error does not name the file, so parse each on its own
fn blame(files: &[PathBuf], e: config::ConfigError) -> anyhow::Error {
  for file in files {
    let Ok(text) = std::fs::read_to_string(file) else {
      continue;
    };
    if let Err(err) = toml::from_str::<toml::Table>(&text) {
      return anyhow::anyhow!("{}: {err}", file.display());
    }
  }
  e.into()
}

fn collect_toml(dir: &Path, out: &mut Vec<PathBuf>) {
  let Ok(entries) = std::fs::read_dir(dir) else {
    return;
  };
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      collect_toml(&path, out);
    } else if path.extension().is_some_and(|e| e == "toml") {
      out.push(path);
    }
  }
}

pub fn warn_unknown(unknown: &[String]) {
  for key in unknown {
    warn!("unknown config key `{key}`");
  }
}

/// Calls `on_change` with the re-read config after edits settle. Watches the
/// directories, not the files, so editor save-by-rename and Nix symlink swaps
/// still fire. Keep the returned watcher alive.
pub fn watch(on_change: impl Fn(Result<Config>) + Send + 'static) -> Result<RecommendedWatcher> {
  let (tx, rx) = mpsc::channel();
  let mut watcher = notify::recommended_watcher(tx)?;
  for dir in [config_dir(), state_dir()] {
    std::fs::create_dir_all(&dir)?;
    watcher.watch(&dir, RecursiveMode::Recursive)?;
  }
  std::thread::spawn(move || {
    let mut last = None;
    while let Ok(event) = rx.recv() {
      if !relevant(&event) {
        continue;
      }
      // debounce: let a burst of events settle
      while rx.recv_timeout(Duration::from_millis(150)).is_ok() {}
      match read() {
        Ok(Loaded { config, unknown }) => {
          warn_unknown(&unknown);
          if last.as_ref() != Some(&config) {
            last = Some(config.clone());
            on_change(Ok(config));
          }
        }
        Err(e) => {
          error!("config reload failed: {e:#}");
          on_change(Err(e));
        }
      }
    }
  });
  Ok(watcher)
}

fn relevant(event: &notify::Result<notify::Event>) -> bool {
  let Ok(event) = event else { return true };
  !event.kind.is_access()
    && (event.paths.is_empty()
      || event
        .paths
        .iter()
        .any(|p| p.extension().is_some_and(|e| e == "toml")))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn layers_files_state_and_env() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
      dir.path().join("a.toml"),
      "[device]\nname = \"a\"\n[pairing]\ntimeout_secs = 10\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("b.toml"), "[device]\nname = \"b\"\n").unwrap();
    std::fs::write(dir.path().join("settings.state"), "").unwrap();
    let state = dir.path().join("state.toml");
    std::fs::write(&state, "[network]\nport = 4000\ntypo = 1\n").unwrap();
    let env = [
      ("AURORA_PAIRING__DISCOVERY".to_string(), "false".to_string()),
      ("AURORA_SOCKET".to_string(), "/x".to_string()),
    ];
    let Loaded { config, unknown } = read_from(dir.path(), &state, env.into_iter()).unwrap();
    assert_eq!(config.device.name.as_deref(), Some("b"));
    assert_eq!(config.pairing.timeout_secs, 10);
    assert!(!config.pairing.discovery);
    assert_eq!(config.network.port, Some(4000));
    assert_eq!(unknown, ["network.typo"]);
  }

  #[test]
  fn bad_file_is_named() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.toml"), "[device\n").unwrap();
    let err = read_from(
      dir.path(),
      &dir.path().join("none.toml"),
      std::iter::empty(),
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("bad.toml"), "{err}");
  }
}

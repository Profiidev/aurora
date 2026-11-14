//! The phone's "run in background" setting over IPC, as the Android app
//! UI talks to its engine service

mod common;

use aurora_core::{
  engine::{Engine, Settings},
  ipc::{self, IpcRegistry, IpcServer, commands},
};
use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn background_setting_round_trips_into_the_state_dir() {
  let dir = tempfile::tempdir().unwrap();
  unsafe {
    std::env::set_var("AURORA_SOCKET", dir.path().join("t.sock"));
    // the phone's files dir, used as is
    std::env::set_var("AURORA_STATE_DIR", dir.path());
  }
  assert_eq!(aurora_core::config::state_dir(), dir.path());
  let engine = Engine::start(
    Settings {
      background: Some(true),
      ..settings("phone")
    },
    dir.path(),
  )
  .await
  .unwrap();
  let server = IpcServer::new().unwrap().unwrap();
  let mut registry = IpcRegistry::default();
  commands::register_all(&mut registry);
  tokio::spawn(server.run(registry, engine.clone()));

  tokio::task::spawn_blocking(|| {
    assert_eq!(
      ipc::send::<commands::SettingsGet>(()).unwrap().background,
      Some(true)
    );
    ipc::send::<commands::SettingsSetBackground>(false).unwrap();
    assert_eq!(
      ipc::send::<commands::SettingsGet>(()).unwrap().background,
      Some(false)
    );
  })
  .await
  .unwrap();
  let saved = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();
  assert!(
    saved.contains("[mobile]") && saved.contains("background = false"),
    "{saved}"
  );
  let loaded = aurora_core::config::read_from(
    &dir.path().join("config"),
    &dir.path().join("settings.toml"),
    std::iter::empty(),
  )
  .unwrap();
  assert!(!loaded.config.mobile.background);
}

#[test]
fn background_defaults_on() {
  assert!(aurora_core::config::Config::default().mobile.background);
}

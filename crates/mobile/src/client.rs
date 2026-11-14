//! The Android app UI: an IPC client of the engine service, like the
//! desktop app is of aurorad, plus what only the UI process can do (camera,
//! permission dialogs, battery prompt).

use std::{
  sync::atomic::{AtomicBool, Ordering},
  thread::sleep,
  time::Duration,
};

use anyhow::Result;
use aurora_core::{
  ipc::{self, commands as cmd},
  types::{AppSettings, DeviceStatus, Event, Nearby, PairInfo, PairState},
};
use aurora_ui::{Backend, DaemonBackend};
use gpui_mobile::android::jni as h;
use jni::objects::JValue;

use crate::jvm;

pub struct AndroidBackend(DaemonBackend);

/// Starts the engine service and connects to it
pub fn start() -> Result<AndroidBackend, String> {
  jvm::init_from_activity()?;
  let files = crate::service::files_dir()?;
  // SAFETY: set before any thread of ours reads the environment
  unsafe { std::env::set_var("AURORA_SOCKET", crate::service::socket_path(&files)) };
  jvm::with_env(|env| {
    jvm::call_with_context(
      env,
      "dev.gpui.mobile.AuroraService",
      jni::jni_str!("start"),
      jni::jni_sig!("(Landroid/content/Context;)V"),
      &[],
    )
    .map(|_| ())
  })?;
  std::thread::spawn(prompts);
  Ok(AndroidBackend(DaemonBackend))
}

fn static_flag(class: &str, name: &'static jni::strings::JNIStr) -> bool {
  jvm::with_env(|env| {
    let cls = jvm::class(env, class)?;
    env
      .get_static_field(&cls, name, jni::jni_sig!("Z"))
      .and_then(|v| v.z())
      .map_err(|e| jvm::call_err(env, e))
  })
  .unwrap_or(false)
}

/// Tells `GpuiActivity.onDestroy` whether to stop the engine with the app
fn tell_background(on: bool) {
  let _ = jvm::with_env(|env| {
    let cls = jvm::class(env, "dev.gpui.mobile.AuroraService")?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("setBackground"),
        jni::jni_sig!("(Z)V"),
        &[JValue::Bool(on)],
      )
      .map(|_| ())
      .map_err(|e| jvm::call_err(env, e))
  });
}

/// A static method of `class` taking the activity; blocks until it returns
fn with_activity<'a, 'b>(
  class: &str,
  name: &'static jni::strings::JNIStr,
  sig: impl AsRef<jni::signature::MethodSignature<'a, 'b>>,
) {
  let _ = h::with_env(|env| {
    let activity = h::activity(env)?;
    let cls = jvm::class(env, class)?;
    env
      .call_static_method(&cls, name, sig, &[JValue::Object(&activity)])
      .map(|_| ())
      .map_err(|e| jvm::call_err(env, e))
  });
}

/// Once the engine answers: the Bluetooth permissions and the battery
/// prompt, which need an activity
fn prompts() {
  let settings = loop {
    match ipc::send::<cmd::SettingsGet>(()) {
      Ok(s) => break s,
      Err(_) => sleep(Duration::from_millis(500)),
    }
  };
  tell_background(settings.background.unwrap_or(true));
  let granted = jvm::with_env(|env| {
    jvm::call_with_context(
      env,
      "dev.gpui.mobile.AuroraBluetooth",
      jni::jni_str!("granted"),
      jni::jni_sig!("(Landroid/content/Context;)Z"),
      &[],
    )?
    .z()
    .map_err(|e| jvm::call_err(env, e))
  })
  .unwrap_or(true);
  if settings.bluetooth == Some(true) && !granted {
    with_activity(
      "dev.gpui.mobile.AuroraBluetooth",
      jni::jni_str!("ensurePermissions"),
      jni::jni_sig!("(Landroid/app/Activity;)Z"),
    );
  }
  if settings.background == Some(true) {
    with_activity(
      "dev.gpui.mobile.AuroraService",
      jni::jni_str!("askBatteryExemption"),
      jni::jni_sig!("(Landroid/app/Activity;)V"),
    );
  }
}

static PRESENCE: AtomicBool = AtomicBool::new(false);

impl Backend for AndroidBackend {
  fn devices(&self) -> Result<Vec<DeviceStatus>> {
    self.0.devices()
  }
  fn pair_state(&self) -> Result<PairState> {
    self.0.pair_state()
  }
  fn start_pairing(&self) -> Result<PairInfo> {
    self.0.start_pairing()
  }
  fn cancel_pairing(&self) -> Result<()> {
    self.0.cancel_pairing()
  }
  fn connect(&self, target: &str) -> Result<()> {
    self.0.connect(target)
  }
  fn decide(&self, accept: bool) -> Result<()> {
    self.0.decide(accept)
  }
  fn nearby(&self) -> Result<Vec<Nearby>> {
    self.0.nearby()
  }
  fn unpair(&self, id: &str) -> Result<()> {
    self.0.unpair(id)
  }
  fn settings(&self) -> Result<AppSettings> {
    let s = self.0.settings()?;
    tell_background(s.background.unwrap_or(true));
    Ok(s)
  }
  fn set_discovery(&self, on: bool) -> Result<()> {
    self.0.set_discovery(on)
  }
  fn set_usb(&self, on: bool) -> Result<()> {
    self.0.set_usb(on)
  }
  fn set_bluetooth(&self, on: bool) -> Result<()> {
    self.0.set_bluetooth(on)
  }
  fn set_background(&self, on: bool) -> Result<()> {
    self.0.set_background(on)?;
    tell_background(on);
    Ok(())
  }
  /// The engine counts the app as looked at only while it is on screen
  fn hold_presence(&self) {
    if PRESENCE.swap(true, Ordering::AcqRel) {
      return;
    }
    std::thread::spawn(|| {
      let mut held = None;
      loop {
        let on_screen = static_flag(
          "dev.gpui.mobile.AuroraBluetooth",
          jni::jni_str!("foreground"),
        );
        if on_screen && held.is_none() {
          held = ipc::subscribe::<cmd::UiPresent>(()).ok();
        } else if !on_screen {
          held = None;
        }
        sleep(Duration::from_secs(1));
      }
    });
  }
  fn scan_qr(&self) -> Result<Option<String>> {
    crate::scanner::scan().map_err(anyhow::Error::msg)
  }
  fn cancel_scan(&self) {
    crate::scanner::cancel();
  }
  fn events(&self) -> Box<dyn Iterator<Item = Event> + Send> {
    self.0.events()
  }
}

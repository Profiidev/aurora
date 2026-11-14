//! The engine process on Android: `AuroraService` (process ":engine") calls
//! `nativeStart`; the engine runs there and serves the same IPC as aurorad,
//! which the app UI talks to.

use std::{ffi::c_void, path::PathBuf, sync::OnceLock};

use aurora_core::{
  config,
  ipc::{IpcRegistry, IpcServer, commands},
};
use gpui_mobile::android::jni as h;

use crate::{backend::EngineBackend, jvm};

static ENGINE: OnceLock<EngineBackend> = OnceLock::new();

/// `<files>/aurora.sock`: app private, the same in both processes
pub fn socket_path(files: &std::path::Path) -> PathBuf {
  files.join("aurora.sock")
}

/// The app's files dir, through the Context
pub fn files_dir() -> Result<PathBuf, String> {
  jvm::with_env(|env| {
    let context = jvm::context(env)?;
    let dir = env
      .call_method(
        &context,
        jni::jni_str!("getFilesDir"),
        jni::jni_sig!("()Ljava/io/File;"),
        &[],
      )
      .and_then(|v| v.l())
      .map_err(|e| jvm::call_err(env, e))?;
    let path = env
      .call_method(
        &dir,
        jni::jni_str!("getAbsolutePath"),
        jni::jni_sig!("()Ljava/lang/String;"),
        &[],
      )
      .and_then(|v| v.l())
      .map_err(|e| jvm::call_err(env, e))?;
    Ok(PathBuf::from(h::get_string(env, &path)))
  })
}

fn start(from_boot: bool) -> Result<bool, String> {
  let files = files_dir()?;
  // SAFETY: called once, before any thread of ours reads the environment
  unsafe {
    std::env::set_var("AURORA_SOCKET", socket_path(&files));
    std::env::set_var("AURORA_STATE_DIR", &files);
  }
  if from_boot {
    let loaded = config::read_from(
      &files.join("config"),
      &files.join("settings.toml"),
      std::iter::empty(),
    )
    .map_err(|e| format!("{e:#}"))?;
    if !loaded.config.mobile.background {
      return Ok(false);
    }
  }
  let backend = EngineBackend::start(&files, crate::backend::device_name(), "Android")
    .map_err(|e| format!("{e:#}"))?;
  let _guard = backend.rt.enter();
  let Some(server) = IpcServer::new().map_err(|e| format!("{e:#}"))? else {
    return Err("another engine owns the socket".into());
  };
  let mut registry = IpcRegistry::default();
  commands::register_all(&mut registry);
  backend
    .rt
    .spawn(server.run(registry, backend.engine.clone()));
  let _ = ENGINE.set(backend);
  Ok(true)
}

/// `AuroraService.nativeStart(Context, boolean)`: false when the engine
/// should not run (started at boot with background off)
///
/// # Safety
/// Called by the JVM on the service's thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_dev_gpui_mobile_AuroraService_nativeStart(
  env: *mut c_void,
  _class: *mut c_void,
  context: *mut c_void,
  from_boot: u8,
) -> u8 {
  android_logger::init_once(
    android_logger::Config::default()
      .with_max_level(log::LevelFilter::Info)
      .with_tag("aurora"),
  );
  if ENGINE.get().is_some() {
    return 1;
  }
  // SAFETY: the JVM's contract for native methods
  if let Err(e) = unsafe { jvm::init_raw(env, context) } {
    log::error!("engine: {e}");
    return 0;
  }
  match start(from_boot != 0) {
    Ok(run) => run.into(),
    Err(e) => {
      log::error!("cannot start the engine: {e}");
      0
    }
  }
}

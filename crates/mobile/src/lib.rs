// Pulls in gpui-mobile's platform symbols (JNI / iOS FFI).
#[cfg(any(target_os = "ios", target_os = "android"))]
extern crate gpui_mobile;

mod backend;
#[cfg(target_os = "android")]
mod multicast;
pub mod platform;
#[cfg(target_os = "android")]
mod scanner;

pub use platform::is_android_emulator;

/// Called from App.swift before `gpui_ios_run_demo()`.
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_register_app() {
  let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
    .join("Library/Application Support/aurora");
  let backend: std::sync::Arc<dyn aurora_ui::Backend> =
    match backend::EngineBackend::start(&dir, "iPhone".into(), "iOS") {
      Ok(b) => std::sync::Arc::new(b),
      Err(e) => {
        log::error!("cannot start engine: {e:#}");
        return;
      }
    };
  gpui_mobile::ios::ffi::set_app_callback(Box::new(move |cx| {
    aurora_ui::open_root(cx, backend.clone(), aurora_ui::Platform::Mobile)
  }));
}

/// Entry point called by android-activity on its native thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
  use gpui_mobile::android::jni;

  android_logger::init_once(
    android_logger::Config::default()
      .with_max_level(log::LevelFilter::Info)
      .with_tag("aurora"),
  );
  jni::install_panic_hook();
  platform::configure_android_gpu();
  let _platform = jni::init_platform(&app);
  let Some(shared) = jni::shared_platform() else {
    log::error!("shared_platform() returned None");
    return;
  };
  let Some(dir) = app.internal_data_path() else {
    log::error!("no internal data path");
    return;
  };
  let backend: std::sync::Arc<dyn aurora_ui::Backend> =
    match backend::EngineBackend::start(&dir, backend::device_name(), "Android") {
      Ok(b) => std::sync::Arc::new(b),
      Err(e) => {
        log::error!("cannot start engine: {e:#}");
        return;
      }
    };
  // Blocks on the Android event loop; callback runs once the surface exists.
  gpui::Application::with_platform(shared.into_rc())
    .with_assets(aurora_ui::AppAssets)
    .run(move |cx| aurora_ui::open_root(cx, backend.clone(), aurora_ui::Platform::Mobile));
}

#[cfg(test)]
mod tests {
  #[test]
  fn not_emulator_on_host() {
    assert!(!super::is_android_emulator());
  }
}

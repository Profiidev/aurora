// Pulls in gpui-mobile's platform symbols (JNI / iOS FFI).
#[cfg(any(target_os = "ios", target_os = "android"))]
extern crate gpui_mobile;

pub mod platform;

pub use platform::is_android_emulator;

/// Called from App.swift before `gpui_ios_run_demo()`.
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_register_app() {
  gpui_mobile::ios::ffi::set_app_callback(Box::new(aurora_ui::open_root));
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
  // Blocks on the Android event loop; callback runs once the surface exists.
  gpui::Application::with_platform(shared.into_rc()).run(aurora_ui::open_root);
}

#[cfg(test)]
mod tests {
  #[test]
  fn not_emulator_on_host() {
    assert!(!super::is_android_emulator());
  }
}

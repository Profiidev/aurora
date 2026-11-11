// Pulls in gpui-mobile's platform symbols (JNI / iOS FFI).
#[cfg(any(target_os = "ios", target_os = "android"))]
extern crate gpui_mobile;

pub mod platform;


use gpui::{App, Context, IntoElement, Render, Window, WindowOptions, div, prelude::*};
pub use platform::is_android_emulator;


struct Home;

impl Render for Home {

  fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    div()
      .size_full()
      .flex()
      .items_center()
      .justify_center()
      .child("Hello from Aurora")
  }

}

/// Positron's shadcn palette (zinc + blue), dark only.
fn apply_theme(cx: &mut App) {
  use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
  ThemeRegistry::global_mut(cx)
    .load_themes_from_str(include_str!("theme.json"))
    .expect("parse theme.json");

  let dark = ThemeRegistry::global(cx).themes()["Positron Dark"].clone();

  Theme::global_mut(cx).dark_theme = dark;
  Theme::change(ThemeMode::Dark, None, cx);
}

fn open_root(cx: &mut App) {
  gpui_kit::init(cx);
  apply_theme(cx);
  let options = WindowOptions {
    // Matches StartupWMClass in packaging/app.desktop
    app_id: Some("io.profidev.aurora".into()),
    ..Default::default()
  };

  cx.open_window(options, |window, cx| {
    let content = cx.new(|_| Home);
    cx.new(|cx| gpui_kit::component::Root::new(content, window, cx))
  })
  .expect("open root window");

  cx.activate(true);
}

/// Desktop entry point (Linux, macOS, Windows), used by `src/main.rs`.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn run_desktop() {
  gpui_kit::application().run(open_root);
}

/// Called from App.swift before `gpui_ios_run_demo()`.
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_register_app() {
  gpui_mobile::ios::ffi::set_app_callback(Box::new(open_root));
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
  gpui::Application::with_platform(shared.into_rc()).run(open_root);
}

#[cfg(test)]
mod tests {
  // Placeholder so the test pipeline has something to run; replace with real tests.
  #[test]
  fn it_works() {
    assert_eq!(2 + 2, 4);
  }
}

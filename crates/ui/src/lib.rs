mod app;
mod backend;
mod desktop;
mod mobile;
mod widgets;

use std::sync::Arc;

use gpui::{App, AppContext, WindowOptions};

pub use app::Platform;
pub use backend::Backend;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use backend::DaemonBackend;

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

/// Shared entry point for desktop and mobile: opens the app window on `backend`
pub fn open_root(cx: &mut App, backend: Arc<dyn Backend>, platform: Platform) {
  gpui_kit::init(cx);
  apply_theme(cx);
  let options = WindowOptions {
    // Matches StartupWMClass in platform/packaging/app.desktop
    app_id: Some("io.profidev.aurora".into()),
    ..Default::default()
  };

  cx.open_window(options, |window, cx| {
    let content = cx.new(|cx| app::AppView::new(backend, platform, window, cx));
    cx.new(|cx| gpui_kit::component::Root::new(content, window, cx))
  })
  .expect("open root window");

  cx.activate(true);
}

/// Desktop entry point (Linux, macOS, Windows): talks to `aurorad`.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn run_desktop() {
  gpui_kit::application().run(|cx| open_root(cx, Arc::new(DaemonBackend), Platform::Desktop));
}

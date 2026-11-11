use gpui::{App, Context, IntoElement, Render, Window, WindowOptions, div, prelude::*};

pub struct Home;

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

pub fn open_root(cx: &mut App) {
  gpui_kit::init(cx);
  apply_theme(cx);
  let options = WindowOptions {
    // Matches StartupWMClass in platform/packaging/app.desktop
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

/// Desktop entry point (Linux, macOS, Windows).
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn run_desktop() {
  gpui_kit::application().run(open_root);
}

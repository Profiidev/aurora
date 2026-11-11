//! Small building blocks the desktop and mobile screens share.

use aurora_core::types::Confirm;
use gpui::App;
use gpui::{Div, ParentElement, SharedString, Styled, div, px};
use gpui_kit::component::{
  ActiveTheme,
  group_box::{GroupBox, GroupBoxVariants},
  h_flex,
  tag::Tag,
  v_flex,
};
use qrcode::{Color, QrCode};

pub fn muted(cx: &App, text: impl Into<SharedString>) -> Div {
  div()
    .text_sm()
    .text_color(cx.theme().muted_foreground)
    .child(text.into())
}

pub fn error_line(cx: &App, error: &Option<SharedString>) -> Option<Div> {
  error.as_ref().map(|e| {
    div()
      .text_sm()
      .text_color(cx.theme().danger)
      .child(e.clone())
  })
}

/// A titled panel; description first, then whatever the caller adds
pub fn card(cx: &App, title: Option<&str>, description: Option<&str>) -> GroupBox {
  let mut card = GroupBox::new().outline();
  if let Some(title) = title {
    card = card.title(title.to_string());
  }
  if let Some(description) = description {
    card = card.child(muted(cx, description.to_string()));
  }
  card
}

/// Placeholder body of a feature that is not built yet
pub fn soon(cx: &App) -> Div {
  muted(cx, "Coming soon")
}

pub fn chip(text: impl Into<SharedString>, outline: bool) -> Tag {
  let tag = Tag::secondary().child(text.into());
  if outline { tag.outline() } else { tag }
}

/// Label / value rows of the confirm screens
pub fn details(cx: &App, rows: Vec<(&'static str, String, bool)>) -> Div {
  v_flex()
    .gap_2()
    .children(rows.into_iter().map(|(label, value, mono)| {
      h_flex()
        .gap_4()
        .text_sm()
        .child(
          div()
            .w(px(120.))
            .text_color(cx.theme().muted_foreground)
            .child(label),
        )
        .child(if mono {
          div().text_xs().child(value)
        } else {
          div().child(value)
        })
    }))
}

pub fn confirm_details(cx: &App, c: &Confirm, who: &'static str) -> Div {
  details(
    cx,
    vec![
      (who, format!("{} · {}", c.peer_name, c.peer_os), false),
      ("Connected via", c.via.clone(), false),
      ("Key fingerprint", c.fingerprint.clone(), true),
    ],
  )
}

/// The QR code as dark modules on a light square
pub fn qr(link: &str, side: f32) -> Div {
  let Ok(code) = QrCode::new(link.as_bytes()) else {
    return div();
  };
  let width = code.width();
  // three modules of quiet zone all around
  let cell = px(side / (width as f32 + 6.0));
  let modules = code.to_colors();
  let rows = modules.chunks(width).map(|row| {
    h_flex().children(row.iter().map(|c| {
      div().size(cell).bg(if *c == Color::Dark {
        gpui::black()
      } else {
        gpui::white()
      })
    }))
  });
  div()
    .size(px(side))
    .rounded_xl()
    .bg(gpui::white())
    .flex()
    .items_center()
    .justify_center()
    .child(v_flex().children(rows))
}

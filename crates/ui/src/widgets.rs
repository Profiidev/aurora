//! The design's shadcn building blocks (Card, Item, Field, nav links) composed
//! from gpui elements and the theme, plus the icons the screens use.

use std::borrow::Cow;

use aurora_core::types::Confirm;
use gpui::{
  AnyElement, App, AssetSource, Div, Hsla, IntoElement, ParentElement, SharedString, Styled, div,
  prelude::FluentBuilder, px,
};
use gpui_kit::{
  assets::{Assets, IconName},
  component::{ActiveTheme, Icon, StyledExt, h_flex, tag::Tag, v_flex},
};
use qrcode::{Color, QrCode};

// every icon drawn on Android must be listed: the default set does not load there
gpui_kit::assets::icon_assets!(
  ExtraIcons,
  [
    Bell,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    CircleCheck,
    CircleX,
    Clipboard,
    Close,
    ExternalLink,
    FileUp,
    House,
    Info,
    Laptop,
    Loader,
    Lock,
    Monitor,
    MonitorSmartphone,
    Plus,
    QrCode,
    RefreshCw,
    Repeat,
    ScanQrCode,
    ScreenShare,
    Settings,
    Shuffle,
    SkipBack,
    SkipForward,
    Smartphone,
    Terminal,
    TriangleAlert
  ]
);

/// The component icons plus the extras above
pub struct AppAssets;

impl AssetSource for AppAssets {
  fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
    if let Some(bytes) = ExtraIcons.load(path)? {
      return Ok(Some(bytes));
    }
    Assets.load(path)
  }

  fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
    let mut paths = Assets.list(path)?;
    paths.extend(ExtraIcons.list(path)?);
    paths.sort();
    paths.dedup();
    Ok(paths)
  }
}

pub fn icon(name: IconName) -> Icon {
  Icon::new(name)
}

pub fn muted(cx: &App, text: impl Into<SharedString>) -> Div {
  div()
    .text_sm()
    .text_color(cx.theme().muted_foreground)
    .child(text.into())
}

/// Page title (24px semibold) with its muted subtitle
pub fn heading(cx: &App, title: impl Into<SharedString>, subtitle: Option<&str>) -> Div {
  v_flex()
    .gap_1()
    .child(div().text_2xl().font_semibold().child(title.into()))
    .children(subtitle.map(|s| muted(cx, s.to_string())))
}

// ---- Card ----------------------------------------------------------------------

/// shadcn Card: `size="sm"` tightens the padding
pub fn card(cx: &App, sm: bool) -> Div {
  v_flex()
    .when(sm, |c| c.gap_4().py_4())
    .when(!sm, |c| c.gap_6().py_6())
    .rounded_xl()
    .border_1()
    .border_color(cx.theme().border)
    .bg(cx.theme().popover)
}

pub fn card_header(
  cx: &App,
  sm: bool,
  title: impl Into<SharedString>,
  description: Option<SharedString>,
  action: Option<AnyElement>,
) -> Div {
  h_flex()
    .items_start()
    .gap_2()
    .when(sm, |c| c.px_4())
    .when(!sm, |c| c.px_6())
    .child(
      v_flex()
        .flex_1()
        .min_w_0()
        .gap_1p5()
        .child(
          div()
            .font_semibold()
            .line_height(px(16.))
            .child(title.into()),
        )
        .children(description.map(|d| muted(cx, d))),
    )
    .children(action)
}

pub fn card_content(sm: bool) -> Div {
  div().when(sm, |c| c.px_4()).when(!sm, |c| c.px_6())
}

// ---- Item ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ItemVariant {
  Outline,
  Muted,
}

/// shadcn Item: media, title and description, actions on the right
pub fn item(
  cx: &App,
  variant: ItemVariant,
  sm: bool,
  media: Option<AnyElement>,
  title: impl Into<SharedString>,
  description: Option<SharedString>,
  actions: Option<AnyElement>,
) -> Div {
  h_flex()
    .items_center()
    .rounded_md()
    .border_1()
    .text_sm()
    .when(sm, |c| c.py_3().px_4().gap_2p5())
    .when(!sm, |c| c.p_4().gap_4())
    .map(|c| match variant {
      ItemVariant::Outline => c.border_color(cx.theme().border),
      ItemVariant::Muted => c
        .border_color(gpui::transparent_black())
        .bg(cx.theme().muted.opacity(0.5)),
    })
    .children(media)
    .child(
      v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(div().font_medium().child(title.into()))
        .children(description.map(|d| muted(cx, d))),
    )
    .children(actions)
}

// ---- Field ---------------------------------------------------------------------

/// Horizontal Field: label and description, control on the right
pub fn field_row(cx: &App, label: &str, description: &str, control: impl IntoElement) -> Div {
  h_flex()
    .items_center()
    .gap_3()
    .child(
      v_flex()
        .flex_1()
        .min_w_0()
        .gap_1p5()
        .child(div().text_sm().font_medium().child(label.to_string()))
        .child(muted(cx, description.to_string())),
    )
    .child(control)
}

/// FieldSet with legend, optional description and a group of fields
pub fn field_set(
  cx: &App,
  legend: impl IntoElement,
  description: Option<&str>,
  fields: impl IntoIterator<Item = AnyElement>,
) -> Div {
  v_flex()
    .gap_6()
    .child(
      v_flex()
        .gap_1p5()
        .child(div().font_medium().child(legend))
        .children(description.map(|d| muted(cx, d.to_string()))),
    )
    .child(v_flex().gap_7().children(fields))
}

// ---- small pieces --------------------------------------------------------------

pub fn badge(text: impl Into<SharedString>, outline: bool) -> Tag {
  let tag = Tag::secondary().child(text.into());
  if outline { tag.outline() } else { tag }
}

/// Square with an icon, used as avatar on the mobile screens
pub fn avatar(cx: &App, name: IconName, side: f32, radius: f32) -> Div {
  div()
    .size(px(side))
    .flex_none()
    .rounded(px(radius))
    .bg(cx.theme().muted)
    .flex()
    .items_center()
    .justify_center()
    .child(icon(name))
}

pub fn online_color() -> Hsla {
  gpui::hsla(145. / 360., 0.6, 0.45, 1.0)
}

/// The confirm code as two filled blocks of three digits; read-only, so no input look
pub fn code_tiles(cx: &App, code: &str) -> Div {
  let group = |part: &str| {
    h_flex()
      .h(px(48.))
      .px_3()
      .gap_2()
      .items_center()
      .rounded_lg()
      .bg(cx.theme().muted)
      .text_2xl()
      .font_semibold()
      .font_family(cx.theme().mono_font_family.clone())
      .children(part.chars().map(|d| d.to_string()))
  };
  let (a, b) = code.split_at(code.len() / 2);
  h_flex().gap_3().child(group(a)).child(group(b))
}

/// Label / value grid of the confirm screens
pub fn confirm_details(cx: &App, c: &Confirm, who: &'static str, fingerprint: &'static str) -> Div {
  let rows = [
    (who, format!("{} · {}", c.peer_name, c.peer_os), false),
    ("Connected via", c.via.clone(), false),
    (fingerprint, c.fingerprint.clone(), true),
  ];
  v_flex()
    .w_full()
    .gap_1p5()
    .children(rows.into_iter().map(|(label, value, small)| {
      h_flex()
        .gap_4()
        .text_size(px(13.))
        .child(
          div()
            .w(px(110.))
            .text_color(cx.theme().muted_foreground)
            .child(label),
        )
        .child(
          div()
            .when(small, |d| {
              d.text_xs().font_family(cx.theme().mono_font_family.clone())
            })
            .child(value),
        )
    }))
}

/// The QR code as dark modules on a white rounded square
pub fn qr(link: &str, side: f32) -> Div {
  let Ok(code) = QrCode::new(link.as_bytes()) else {
    return div();
  };
  let width = code.width();
  // whole pixels per module: fractions snap and leave a wide uneven margin.
  // A fixed quiet zone, since one counted in modules gets thin on long links
  const PAD: f32 = 12.0;
  let cell = ((side - 2.0 * PAD) / width as f32).floor().max(1.0);
  let side = cell * width as f32 + 2.0 * PAD;
  let cell = px(cell);
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
    .rounded(px(12.))
    .bg(gpui::white())
    .flex()
    .items_center()
    .justify_center()
    .child(v_flex().children(rows))
}

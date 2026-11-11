//! Mobile screens, following the design boards "Mobile · Pair",
//! "Mobile · Confirm code", "Mobile · Home" and "Mobile · Settings".

use aurora_core::types::{Confirm, PairState};
use gpui::{
  Context, Div, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
  div, prelude::FluentBuilder, px,
};
use gpui_kit::{
  assets::IconName,
  component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, OtpInput},
    separator::Separator,
    spinner::Spinner,
    switch::Switch,
    v_flex,
  },
};

use crate::{
  app::{AppView, Page},
  widgets::{
    ItemVariant, avatar, badge, card, card_content, confirm_details, error_line, field_row,
    field_set, heading, icon, item, muted,
  },
};

const ACTIONS: [(&str, IconName); 6] = [
  ("Send file", IconName::FileUp),
  ("Send clipboard", IconName::Clipboard),
  ("Share screen", IconName::ScreenShare),
  ("Ring computer", IconName::Bell),
  ("Lock computer", IconName::Lock),
  ("Commands", IconName::Terminal),
];

const MODES: [(&str, &str); 4] = [
  ("Touchpad and keyboard", "Control the computer pointer"),
  ("Webcam and microphone", "Stream camera and audio"),
  ("Second display", "Extend the computer screen"),
  ("Slides remote", "Next, previous, pointer"),
];

const FEATURE_SETTINGS: [(&str, &str); 5] = [
  ("Notifications", "Show them on the computer"),
  ("Clipboard", "Share copied text and files"),
  ("Photo backup", "Upload new photos automatically"),
  ("Messages", "Read and send texts from the computer"),
  ("Pause media during calls", "On the computer"),
];

impl AppView {
  pub(crate) fn mobile(&mut self, cx: &mut Context<Self>) -> Div {
    if let PairState::Confirm(c) = self.pair.clone() {
      return self.mobile_confirm(&c, cx);
    }
    // nothing paired yet: the pair screen alone, like a first run
    if self.devices.is_empty() {
      return self.mobile_pair(cx);
    }
    let content = match self.page {
      Page::Home => self.mobile_home(cx),
      Page::Pair => self.mobile_pair(cx),
      Page::Settings => self.mobile_settings(cx),
    };
    v_flex()
      .child(
        div()
          .id("content")
          .flex_1()
          .min_h_0()
          .overflow_y_scroll()
          .child(content),
      )
      .child(self.bottom_nav(cx))
  }

  fn bottom_nav(&mut self, cx: &mut Context<Self>) -> Div {
    let tabs = [
      (Page::Home, "Home", IconName::House),
      (Page::Pair, "Devices", IconName::Laptop),
      (Page::Settings, "Settings", IconName::Settings),
    ];
    h_flex()
      .pt_2()
      .px_2()
      .pb(px(28.))
      .border_t_1()
      .border_color(cx.theme().border)
      .bg(cx.theme().background)
      .children(tabs.map(|(page, label, name)| {
        let active = self.page == page;
        v_flex()
          .id(label)
          .flex_1()
          .min_h(px(48.))
          .items_center()
          .justify_center()
          .gap_1()
          .text_size(px(11.))
          .when(!active, |t| t.text_color(cx.theme().muted_foreground))
          .child(icon(name))
          .child(label)
          .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
      }))
  }

  // ---- pair --------------------------------------------------------------------

  fn mobile_pair(&mut self, cx: &mut Context<Self>) -> Div {
    let mut nearby = v_flex().gap_2();
    for (i, n) in self.nearby.clone().into_iter().enumerate() {
      let link = n.link.clone();
      nearby = nearby.child(item(
        cx,
        ItemVariant::Outline,
        false,
        Some(icon(IconName::Monitor).into_any_element()),
        n.name.clone(),
        Some(format!("{} · {}", n.os, n.addr.ip()).into()),
        Some(
          Button::new(("nearby", i))
            .secondary()
            .small()
            .label("Pair")
            .on_click(cx.listener(move |this, _, _, cx| this.connect(link.clone(), cx)))
            .into_any_element(),
        ),
      ));
    }

    let address = if self.show_address {
      v_flex().gap_2().child(Input::new(&self.address)).child(
        Button::new("by-address")
          .outline()
          .label("Connect")
          .w_full()
          .h(px(44.))
          .on_click(cx.listener(|this, _, _, cx| {
            let host = this.address.read(cx).value().to_string();
            if !host.trim().is_empty() {
              this.connect(host, cx);
            }
          })),
      )
    } else {
      v_flex().child(
        Button::new("show-address")
          .ghost()
          .label("Connect by address or USB")
          .w_full()
          .h(px(44.))
          .on_click(cx.listener(|this, _, _, cx| {
            this.show_address = true;
            cx.notify();
          })),
      )
    };

    v_flex()
      .size_full()
      .pt(px(56.))
      .px_5()
      .pb_6()
      .gap_6()
      .child(heading(
        cx,
        "Connect a computer",
        Some("Open the app on your computer and choose Pair new device."),
      ))
      .child(
        Button::new("scan")
          .primary()
          .icon(IconName::ScanQrCode)
          .label("Scan QR code")
          .w_full()
          .h(px(48.))
          .on_click(cx.listener(|this, _, _, cx| this.scan_qr(cx))),
      )
      .children(error_line(cx, &self.error))
      .child(Separator::horizontal())
      .child(
        v_flex()
          .gap_2p5()
          .child(
            h_flex()
              .items_center()
              .gap_2()
              .child(
                div()
                  .flex_1()
                  .text_sm()
                  .font_medium()
                  .child("Computers nearby"),
              )
              .when(
                self.scanning || matches!(self.pair, PairState::Dialing),
                |h| h.child(Spinner::new()),
              ),
          )
          .child(nearby),
      )
      .child(div().flex_1())
      .child(address)
  }

  // ---- confirm -----------------------------------------------------------------

  fn mobile_confirm(&mut self, c: &Confirm, cx: &mut Context<Self>) -> Div {
    let actions = if c.local_accepted {
      h_flex()
        .justify_center()
        .gap_2()
        .child(Spinner::new())
        .child(muted(cx, "Waiting for the computer…"))
    } else {
      v_flex()
        .gap_2()
        .child(
          Button::new("pair")
            .primary()
            .label("Pair")
            .w_full()
            .h(px(48.))
            .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
        )
        .child(
          Button::new("mismatch")
            .outline()
            .label("Codes don't match")
            .w_full()
            .h(px(48.))
            .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
        )
    };
    v_flex()
      .size_full()
      .pt(px(56.))
      .px_5()
      .pb_6()
      .gap_6()
      .child(
        h_flex()
          .id("back")
          .min_h(px(44.))
          .items_center()
          .gap_1p5()
          .text_sm()
          .text_color(cx.theme().muted_foreground)
          .child(icon(IconName::ChevronLeft).small())
          .child("Back")
          .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
      )
      .child(
        v_flex()
          .items_center()
          .gap_2()
          .pt_6()
          .child(avatar(cx, IconName::Monitor, 56., 16.))
          .child(
            div()
              .text_size(px(22.))
              .font_semibold()
              .text_center()
              .child(format!("Pair with {}?", c.peer_name)),
          )
          .child(
            div()
              .max_w(px(300.))
              .text_sm()
              .text_center()
              .text_color(cx.theme().muted_foreground)
              .child("Make sure your computer shows the same code."),
          ),
      )
      .child(
        h_flex()
          .justify_center()
          .child(OtpInput::new(&self.otp).disabled(true)),
      )
      .child(
        card(cx, true).child(card_content(true).child(confirm_details(
          cx,
          c,
          "Computer",
          "Fingerprint",
        ))),
      )
      .child(div().flex_1())
      .child(actions)
  }

  // ---- home --------------------------------------------------------------------

  fn mobile_home(&mut self, cx: &mut Context<Self>) -> Div {
    let Some(d) = self.selected_device().cloned() else {
      return div();
    };
    let tiles = ACTIONS.chunks(3).map(|row| {
      h_flex()
        .gap(px(10.))
        .children(row.iter().map(|(label, name)| {
          v_flex()
            .flex_1()
            .min_h(px(84.))
            .py_3()
            .px_2()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded(px(14.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .opacity(0.5)
            .text_xs()
            .text_center()
            .child(icon(*name))
            .child(*label)
        }))
    });
    let section = |cx: &Context<Self>, title: String| {
      div()
        .text_size(px(13.))
        .text_color(cx.theme().muted_foreground)
        .child(title)
    };

    v_flex()
      .pt(px(56.))
      .px_5()
      .pb_4()
      .gap_5()
      .child(
        h_flex()
          .items_center()
          .gap_3()
          .child(avatar(cx, IconName::Monitor, 44., 12.))
          .child(
            v_flex()
              .flex_1()
              .gap_1()
              .child(div().text_xl().font_semibold().child(d.device.name.clone()))
              .child(h_flex().child(badge(
                if d.connected {
                  "Connected · LAN"
                } else {
                  "Offline"
                },
                false,
              ))),
          ),
      )
      .child(v_flex().gap(px(10.)).children(tiles))
      .child(
        v_flex()
          .gap_2()
          .child(section(cx, format!("Now playing on {}", d.device.name)))
          .child(item(
            cx,
            ItemVariant::Outline,
            false,
            Some(
              div()
                .size_10()
                .flex_none()
                .bg(cx.theme().muted)
                .into_any_element(),
            ),
            "Media control",
            Some("Coming soon".into()),
            None,
          )),
      )
      .child(
        v_flex()
          .gap_2()
          .child(section(cx, "Use this phone as".into()))
          .child(v_flex().gap_2().children(MODES.map(|(label, description)| {
            item(
              cx,
              ItemVariant::Muted,
              true,
              None,
              label,
              Some(description.into()),
              Some(
                Button::new(label)
                  .ghost()
                  .small()
                  .icon(IconName::ChevronRight)
                  .disabled(true)
                  .into_any_element(),
              ),
            )
          }))),
      )
  }

  // ---- settings ----------------------------------------------------------------

  fn mobile_settings(&mut self, cx: &mut Context<Self>) -> Div {
    let Some(d) = self.selected_device().cloned() else {
      return div();
    };
    let id = d.device.id.clone();
    let features = field_set(
      cx,
      d.device.name.clone(),
      None,
      FEATURE_SETTINGS.map(|(label, description)| {
        field_row(cx, label, description, Switch::new(label).disabled(true)).into_any_element()
      }),
    );
    v_flex()
      .pt(px(56.))
      .px_5()
      .pb_4()
      .gap_6()
      .child(div().text_2xl().font_semibold().child("Settings"))
      .child(features)
      .child(Separator::horizontal())
      .child(
        Button::new("unpair")
          .danger()
          .label(format!("Unpair {}", d.device.name))
          .w_full()
          .h(px(44.))
          .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
      )
  }
}

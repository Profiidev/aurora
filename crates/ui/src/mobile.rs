use aurora_core::types::{Confirm, PairState};
use gpui::{
  Context, Div, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
  div, px,
};
use gpui_kit::component::{
  ActiveTheme, Disableable, Sizable, StyledExt,
  button::{Button, ButtonVariants},
  h_flex,
  input::{Input, OtpInput},
  spinner::Spinner,
  switch::Switch,
  v_flex,
};

use crate::{
  app::{AppView, Page},
  widgets::{card, chip, confirm_details, error_line, muted, soon},
};

/// Same list as the desktop sidebar; none are built yet
const FEATURES: [&str; 8] = [
  "Notifications",
  "Messages",
  "Calls",
  "Files",
  "Clipboard",
  "Media",
  "Screen",
  "Commands",
];

const ACTIONS: [&str; 6] = [
  "Ring computer",
  "Send clipboard",
  "Send file",
  "Lock",
  "Share link",
  "Screen",
];

impl AppView {
  pub(crate) fn mobile(&mut self, cx: &mut Context<Self>) -> Div {
    if let PairState::Confirm(c) = self.pair.clone() {
      return self.mobile_confirm(&c, cx);
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

  /// A page body with the design's side padding and status bar room
  fn page_body(&self) -> Div {
    v_flex().pt(px(48.)).px_5().pb_4().gap_5()
  }

  fn bottom_nav(&mut self, cx: &mut Context<Self>) -> Div {
    let items = [
      (Page::Home, "Home"),
      (Page::Pair, "Devices"),
      (Page::Settings, "Settings"),
    ];
    h_flex()
      .border_t_1()
      .border_color(cx.theme().border)
      .px_2()
      .pt_2()
      .pb_6()
      .children(items.map(|(page, label)| {
        div().flex_1().child(
          {
            let b = Button::new(label).label(label);
            if self.page == page {
              b.secondary()
            } else {
              b.ghost()
            }
          }
          .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx))),
        )
      }))
  }

  // ---- home ----------------------------------------------------------------------

  fn mobile_home(&mut self, cx: &mut Context<Self>) -> Div {
    let body = self.page_body();
    let Some(d) = self.selected_device().cloned() else {
      return body
        .child(div().text_xl().font_semibold().child("No computer paired"))
        .child(muted(cx, "Pair one to see it here."))
        .child(
          h_flex().child(
            Button::new("pair-first")
              .primary()
              .label("Connect a computer")
              .on_click(cx.listener(|this, _, _, cx| this.go(Page::Pair, cx))),
          ),
        );
    };
    let tiles = h_flex().flex_wrap().gap_2().children(ACTIONS.map(|a| {
      div()
        .w(px(104.))
        .min_h(px(84.))
        .p_3()
        .rounded_xl()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .opacity(0.5)
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .child(a)
    }));
    body
      .child(
        v_flex()
          .gap_2()
          .child(
            div()
              .text_2xl()
              .font_semibold()
              .child(d.device.name.clone()),
          )
          .child(h_flex().gap_2().child(chip(
            if d.connected {
              "Connected · LAN"
            } else {
              "Offline"
            },
            false,
          ))),
      )
      .child(tiles)
      .child(
        v_flex()
          .gap_2()
          .child(muted(cx, format!("Now playing on {}", d.device.name)))
          .child(card(cx, None, None).child(soon(cx))),
      )
      .child(
        v_flex()
          .gap_2()
          .child(muted(cx, "Use this phone as"))
          .children(["Webcam", "Microphone", "Second display"].map(|m| {
            card(cx, None, None).child(h_flex().child(div().flex_1().child(m)).child(soon(cx)))
          })),
      )
  }

  // ---- devices / pair ------------------------------------------------------------

  fn mobile_pair(&mut self, cx: &mut Context<Self>) -> Div {
    let busy = matches!(self.pair, PairState::Dialing);
    let mut nearby = v_flex().gap_2();
    for (i, n) in self.nearby.iter().enumerate() {
      let link = n.link.clone();
      nearby = nearby.child(
        h_flex()
          .gap_3()
          .p_3()
          .rounded_lg()
          .border_1()
          .border_color(cx.theme().border)
          .child(
            v_flex()
              .flex_1()
              .child(div().font_medium().child(n.name.clone()))
              .child(muted(cx, format!("{} · {}", n.os, n.addr.ip()))),
          )
          .child(
            Button::new(("nearby", i))
              .secondary()
              .small()
              .label("Pair")
              .on_click(cx.listener(move |this, _, _, cx| this.connect(link.clone(), cx))),
          ),
      );
    }

    let paired = self.devices.iter().enumerate().map(|(i, d)| {
      h_flex()
        .gap_3()
        .child(
          v_flex()
            .flex_1()
            .child(div().font_medium().child(d.device.name.clone()))
            .child(muted(
              cx,
              if d.connected {
                "Connected · LAN"
              } else {
                "Offline"
              },
            )),
        )
        .child(div().id(("paired", i)))
    });

    self
      .page_body()
      .child(
        v_flex()
          .gap_1()
          .child(div().text_2xl().font_semibold().child("Connect a computer"))
          .child(muted(
            cx,
            "Open the app on your computer and choose Pair new device.",
          )),
      )
      .child(
        Button::new("scan")
          .primary()
          .label("Scan QR code")
          .on_click(cx.listener(|this, _, _, cx| this.scan_qr(cx))),
      )
      .children(error_line(cx, &self.error))
      .child(
        v_flex()
          .gap_2()
          .child(
            h_flex()
              .child(div().flex_1().font_medium().child("Computers nearby"))
              .child(if self.scanning || busy {
                Spinner::new().into_any_element()
              } else {
                Button::new("rescan")
                  .ghost()
                  .small()
                  .label("Refresh")
                  .on_click(cx.listener(|this, _, _, cx| this.scan(cx)))
                  .into_any_element()
              }),
          )
          .child(nearby),
      )
      .child(
        card(
          cx,
          Some("Connect by address"),
          Some("For computers on another network or a VPN."),
        )
        .child(Input::new(&self.address))
        .child(
          Button::new("by-address")
            .outline()
            .label("Connect")
            .on_click(cx.listener(|this, _, _, cx| {
              let host = this.address.read(cx).value().to_string();
              if !host.trim().is_empty() {
                this.connect(host, cx);
              }
            })),
        ),
      )
      .children((!self.devices.is_empty()).then(|| {
        v_flex()
          .gap_2()
          .child(muted(cx, "Paired computers"))
          .children(paired)
      }))
  }

  // ---- confirm -------------------------------------------------------------------

  fn mobile_confirm(&mut self, c: &Confirm, cx: &mut Context<Self>) -> Div {
    let actions = if c.local_accepted {
      h_flex()
        .gap_2()
        .justify_center()
        .child(Spinner::new())
        .child(muted(cx, "Waiting for the computer…"))
    } else {
      h_flex()
        .gap_2()
        .justify_center()
        .child(
          Button::new("pair")
            .primary()
            .label("Pair")
            .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
        )
        .child(
          Button::new("mismatch")
            .outline()
            .label("Codes don't match")
            .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
        )
    };
    v_flex()
      .pt(px(48.))
      .px_5()
      .pb_8()
      .gap_6()
      .child(
        h_flex().child(
          Button::new("back")
            .ghost()
            .label("← Back")
            .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
        ),
      )
      .child(
        v_flex()
          .items_center()
          .gap_2()
          .pt_6()
          .text_center()
          .child(
            div()
              .text_xl()
              .font_semibold()
              .child(format!("Pair with {}?", c.peer_name)),
          )
          .child(muted(cx, "Make sure your computer shows the same code.")),
      )
      .child(
        div()
          .flex()
          .justify_center()
          .child(OtpInput::new(&self.otp).disabled(true)),
      )
      .child(card(cx, None, None).child(confirm_details(cx, c, "Computer")))
      .child(div().flex_1())
      .child(actions)
  }

  // ---- settings ------------------------------------------------------------------

  fn mobile_settings(&mut self, cx: &mut Context<Self>) -> Div {
    let body = self
      .page_body()
      .child(div().text_2xl().font_semibold().child("Settings"));
    let Some(d) = self.selected_device().cloned() else {
      return body.child(muted(cx, "No computer paired."));
    };
    let id = d.device.id.clone();
    let mut features = card(cx, Some(&d.device.name), None);
    for f in FEATURES {
      features = features.child(
        h_flex()
          .gap_3()
          .child(
            v_flex()
              .flex_1()
              .child(div().font_medium().child(f))
              .child(muted(cx, "Coming soon")),
          )
          .child(Switch::new(f).disabled(true)),
      );
    }
    body.child(features).child(
      Button::new("unpair")
        .danger()
        .label(format!("Unpair {}", d.device.name))
        .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
    )
  }
}

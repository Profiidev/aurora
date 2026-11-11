//! Mobile screens, following the design boards "Mobile · Pair",
//! "Mobile · Confirm code", "Mobile · Home" and "Mobile · Settings".

use aurora_core::types::{Confirm, PairState};
use gpui::{
  AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement,
  StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};
use gpui_kit::{
  assets::IconName,
  component::{
    ActiveTheme, Disableable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
    separator::Separator,
    spinner::Spinner,
    switch::Switch,
    v_flex,
  },
};

use crate::{
  app::{AppView, Page},
  widgets::{
    ItemVariant, avatar, badge, card, card_content, code_tiles, confirm_details, device_id,
    field_row, field_set, heading, icon, item, muted, qr,
  },
};

const ACTIONS: [(&str, IconName); 6] = [
  ("Send file", IconName::FileUp),
  ("Send clipboard", IconName::Clipboard),
  ("Share screen", IconName::ScreenShare),
  ("Ring device", IconName::Bell),
  ("Lock device", IconName::Lock),
  ("Commands", IconName::Terminal),
];

const MODES: [(&str, &str); 4] = [
  ("Touchpad and keyboard", "Control the device's pointer"),
  ("Webcam and microphone", "Stream camera and audio"),
  ("Second display", "Extend the device's screen"),
  ("Slides remote", "Next, previous, pointer"),
];

const FEATURE_SETTINGS: [(&str, &str); 5] = [
  ("Notifications", "Show them on the device"),
  ("Clipboard", "Share copied text and files"),
  ("Photo backup", "Upload new photos automatically"),
  ("Messages", "Read and send texts from the device"),
  ("Pause media during calls", "On the device"),
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
          .child(icon(name).with_size(px(20.)))
          .child(label)
          .cursor_pointer()
          .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
      }))
  }

  // ---- pair --------------------------------------------------------------------

  fn mobile_pair(&mut self, cx: &mut Context<Self>) -> Div {
    let mut nearby = v_flex().gap_2();
    if self.nearby.is_empty() {
      nearby = nearby.child(muted(cx, "No devices found yet."));
    }
    for (i, n) in self.nearby.clone().into_iter().enumerate() {
      let link = n.link.clone();
      nearby = nearby.child(
        item(
          cx,
          ItemVariant::Outline,
          false,
          Some(
            icon(IconName::Monitor)
              .with_size(px(20.))
              .into_any_element(),
          ),
          n.name.clone(),
          Some(match n.addr {
            Some(a) => format!("{} · {}", n.os, a.ip()).into(),
            None => n.os.clone().into(),
          }),
          Some(
            Button::new(("nearby", i))
              .cursor_pointer()
              .loading(self.loading(&n.link))
              .when(self.loading(&n.link), |b| b.icon(Spinner::new()))
              .secondary()
              .label("Pair")
              .on_click(
                cx.listener(move |this, _, _, cx| this.connect(link.clone(), link.clone(), cx)),
              )
              .into_any_element(),
          ),
        )
        .rounded_xl(),
      );
    }

    let address = if self.show_address {
      v_flex().gap_2().child(Input::new(&self.address)).child(
        Button::new("by-address")
          .cursor_pointer()
          .loading(self.loading("by-address"))
          .when(self.loading("by-address"), |b| b.icon(Spinner::new()))
          .outline()
          .label("Connect")
          .w_full()
          .on_click(cx.listener(|this, _, _, cx| {
            let host = this.address.read(cx).value().to_string();
            if !host.trim().is_empty() {
              this.connect(host, "by-address", cx);
            }
          })),
      )
    } else {
      v_flex().child(
        Button::new("show-address")
          .cursor_pointer()
          .outline()
          .label("Connect by link or ID")
          .w_full()
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
        "Connect a device",
        Some("On the other device, choose Pair new device."),
      ))
      .child(
        v_flex().gap_2().child(self.show_qr(cx)).child(
          Button::new("scan")
            .cursor_pointer()
            .loading(self.loading("scan"))
            .primary()
            .icon(IconName::ScanQrCode)
            .label("Scan QR code")
            .w_full()
            .on_click(cx.listener(|this, _, _, cx| this.scan_qr(cx))),
        ),
      )
      .child(Separator::horizontal())
      .child(
        v_flex()
          .gap_2p5()
          .child(
            h_flex()
              .items_center()
              .gap_2()
              // same height with the spinner as with the button
              .min_h(px(32.))
              .child(
                div()
                  .flex_1()
                  .text_sm()
                  .font_medium()
                  .child("Devices nearby"),
              )
              .when(self.discovery(), |h| {
                h.child(
                  if self.scanning || matches!(self.pair, PairState::Dialing) {
                    // centred in the button's box, so the icon does not shift
                    div()
                      .size_8()
                      .flex()
                      .items_center()
                      .justify_center()
                      .child(Spinner::new())
                      .into_any_element()
                  } else {
                    Button::new("rescan")
                      .cursor_pointer()
                      .ghost()
                      .icon(IconName::RefreshCw)
                      .on_click(cx.listener(|this, _, _, cx| this.scan(cx)))
                      .into_any_element()
                  },
                )
              })
              .child(self.discovery_switch("discovery-pair", cx)),
          )
          .child(if self.discovery() {
            nearby
          } else {
            v_flex().child(muted(
              cx,
              "Turn this on to find devices on this network, or use a QR code.",
            ))
          }),
      )
      .child(div().flex_1())
      .child(address)
  }

  fn discovery(&self) -> bool {
    self.settings.as_ref().is_some_and(|s| s.discovery)
  }

  fn discovery_switch(&self, id: &'static str, cx: &mut Context<Self>) -> Switch {
    Switch::new(id)
      .cursor_pointer()
      .checked(self.discovery())
      .on_click(cx.listener(|this, on: &bool, _, cx| this.set_discovery(*on, cx)))
  }

  /// This phone's own pairing QR, for a device that scans instead
  fn show_qr(&self, cx: &mut Context<Self>) -> Button {
    let link = self.info.as_ref().map(|i| i.link.clone());
    Button::new("show-qr")
      .cursor_pointer()
      .outline()
      .icon(IconName::QrCode)
      .label("Show QR code")
      .w_full()
      .disabled(link.is_none())
      .on_click(cx.listener(move |_, _, window, cx| {
        let Some(link) = link.clone() else { return };
        window.open_dialog(cx, move |dialog, _, cx| {
          dialog.w(px(320.)).title("Pair this device").child(
            v_flex()
              .items_center()
              .gap_3()
              .child(qr(&link, 272.))
              .children(device_id(cx, &link))
              .child(muted(cx, "Scan this with the other device.")),
          )
        });
      }))
  }

  // ---- confirm -----------------------------------------------------------------

  fn mobile_confirm(&mut self, c: &Confirm, cx: &mut Context<Self>) -> Div {
    let actions = if c.local_accepted {
      h_flex()
        .justify_center()
        .gap_2()
        .child(Spinner::new())
        .child(muted(cx, "Now confirm the code on the other device"))
    } else {
      v_flex()
        .gap_2()
        .child(
          Button::new("pair")
            .cursor_pointer()
            .loading(self.loading("pair"))
            .when(self.loading("pair"), |b| b.icon(Spinner::new()))
            .primary()
            .label("Pair")
            .w_full()
            .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
        )
        .child(
          Button::new("mismatch")
            .cursor_pointer()
            .loading(self.loading("mismatch"))
            .when(self.loading("mismatch"), |b| b.icon(Spinner::new()))
            .outline()
            .label("Codes don't match")
            .w_full()
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
          .cursor_pointer()
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
              .text_sm()
              .text_center()
              .text_color(cx.theme().muted_foreground)
              .child("Make sure the other device shows the same code."),
          ),
      )
      .child(h_flex().justify_center().child(code_tiles(cx, &c.code)))
      .child(
        card(cx, true).child(card_content(true).child(confirm_details(
          cx,
          c,
          "Device",
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
                  .cursor_pointer()
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

  /// Switches which paired device the tabs show
  fn device_picker(&self, current: &str, cx: &mut Context<Self>) -> AnyElement {
    let devices: Vec<_> = (self.devices.iter())
      .map(|d| (d.device.id.clone(), d.device.name.clone()))
      .collect();
    let selected = self.selected.clone();
    let view = cx.entity().downgrade();
    Button::new("device-picker")
      .cursor_pointer()
      .outline()
      .label(current.to_string())
      .dropdown_caret(true)
      .dropdown_menu(move |menu, window, _| {
        // shortcut: the button spans the page minus its px_5 padding; measure it if the layout changes
        let mut menu = menu.min_w(window.bounds().size.width - px(40.));
        for (id, name) in &devices {
          let (view, id) = (view.clone(), id.clone());
          menu = menu.item(
            PopupMenuItem::new(name.clone())
              .checked(selected.as_ref() == Some(&id))
              .on_click(move |_, _, cx| {
                let _ = view.update(cx, |this, cx| {
                  this.selected = Some(id.clone());
                  cx.notify();
                });
              }),
          );
        }
        menu
      })
      .into_any_element()
  }

  fn mobile_settings(&mut self, cx: &mut Context<Self>) -> Div {
    let Some(d) = self.selected_device().cloned() else {
      return div();
    };
    let id = d.device.id.clone();
    let picker = self.device_picker(&d.device.name, cx);
    let switch = self.discovery_switch("discovery-settings", cx);
    let phone = field_set(
      cx,
      "This phone",
      None,
      [field_row(
        cx,
        "Find devices nearby",
        "Find and be found on this Wi-Fi. Uses more battery.",
        switch,
      )
      .into_any_element()],
    );
    let features = field_set(
      cx,
      picker,
      None,
      FEATURE_SETTINGS.map(|(label, description)| {
        field_row(
          cx,
          label,
          description,
          Switch::new(label).cursor_pointer().disabled(true),
        )
        .into_any_element()
      }),
    );
    v_flex()
      .pt(px(56.))
      .px_5()
      .pb_4()
      .gap_6()
      .child(div().text_2xl().font_semibold().child("Settings"))
      .child(phone)
      .child(Separator::horizontal())
      .child(features)
      .child(Separator::horizontal())
      .child(
        Button::new("unpair")
          .cursor_pointer()
          .loading(self.loading("unpair"))
          .when(self.loading("unpair"), |b| b.icon(Spinner::new()))
          .danger()
          .label(format!("Unpair {}", d.device.name))
          .w_full()
          .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
      )
  }
}

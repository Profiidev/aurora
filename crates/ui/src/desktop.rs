//! Desktop screens, following the design boards "Desktop · Pair new device",
//! "Desktop · Confirm pairing code", "Desktop · Device overview" and
//! "Desktop · Settings".

use aurora_core::types::{Confirm, PairState};
use gpui::{
  AnyElement, ClickEvent, Context, Div, InteractiveElement, IntoElement, ParentElement,
  SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_kit::{
  assets::IconName,
  component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    separator::Separator,
    spinner::Spinner,
    switch::Switch,
    v_flex,
  },
};

use crate::{
  app::{AppView, Page},
  widgets::{
    ItemVariant, badge, card, card_content, card_header, code_tiles, confirm_details, device_id,
    field_row, field_set, heading, icon, item, muted, online_color, qr,
  },
};

/// Sidebar entries under the selected device; only Overview exists so far
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

/// Per device feature switches of the settings page
const FEATURE_SETTINGS: [(&str, &str); 6] = [
  ("Notifications", "Show phone notifications here."),
  ("Clipboard", "Share copied text and files both ways."),
  ("Files", "Send and receive files."),
  ("Media control", "Control players on either device."),
  ("Screen mirroring", "View and control the phone screen."),
  ("Run commands", "Let the phone run commands you define."),
];

const SETTINGS_SECTIONS: [&str; 6] = [
  "Connection",
  "Devices",
  "Files",
  "Screen and camera",
  "Commands",
  "About",
];

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static>;

impl AppView {
  pub(crate) fn desktop(&mut self, cx: &mut Context<Self>) -> Div {
    // a code to confirm takes over the window
    if let PairState::Confirm(c) = self.pair.clone() {
      return self.desktop_confirm(&c, cx);
    }
    let (nav, main) = match self.page {
      Page::Home => (self.sidebar(cx), self.desktop_overview(cx)),
      Page::Pair => (self.sidebar(cx), self.desktop_pair(cx)),
      Page::Settings => (self.settings_nav(cx), self.desktop_settings(cx)),
    };
    h_flex().items_start().child(nav).child(
      div()
        .id("main")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .child(main),
    )
  }

  /// One sidebar row: icon, label, optional trailing element
  #[allow(clippy::too_many_arguments)]
  fn nav_row(
    &self,
    cx: &Context<Self>,
    id: impl Into<SharedString>,
    leading: Option<IconName>,
    label: impl Into<SharedString>,
    active: bool,
    dimmed: bool,
    trailing: Option<AnyElement>,
    on_click: Option<OnClick>,
  ) -> AnyElement {
    let theme = cx.theme();
    let enabled = on_click.is_some();
    let id = id.into();
    h_flex()
      .id(id.clone())
      .debug_selector(|| id.to_string())
      .items_center()
      // shadcn sidebar menu button: h-8, p-2, rounded-md
      .h_8()
      .flex_none()
      .gap_2()
      .px_2()
      .rounded_md()
      .text_sm()
      .when(active, |r| r.bg(theme.sidebar_accent).font_medium())
      .when(dimmed && !active, |r| r.text_color(theme.muted_foreground))
      .when(enabled && !active, |r| {
        r.hover(|s| s.bg(theme.sidebar_accent)).cursor_pointer()
      })
      .when(!enabled, |r| r.opacity(0.5).cursor_not_allowed())
      .children(leading.map(|i| icon(i).small()))
      .child(div().flex_1().child(label.into()))
      .children(trailing)
      .when_some(on_click, |r, f| r.on_click(f))
      .into_any_element()
  }

  fn nav_label(&self, cx: &Context<Self>, text: impl Into<SharedString>, top: f32) -> Div {
    div()
      .text_xs()
      .text_color(cx.theme().muted_foreground)
      .px_2()
      .pt(px(top))
      .pb_1()
      .child(text.into())
  }

  fn nav_shell(&self, cx: &Context<Self>) -> Div {
    v_flex()
      .w(px(240.))
      .flex_none()
      .h_full()
      .py_4()
      .px_3()
      .gap_1()
      .bg(cx.theme().sidebar)
      .border_r_1()
      .border_color(cx.theme().sidebar_border)
  }

  fn sidebar(&mut self, cx: &mut Context<Self>) -> Div {
    let mut nav = self.nav_shell(cx).child(
      h_flex()
        .items_center()
        .gap_2()
        .px_2()
        .pt_1()
        .pb_4()
        .child(icon(IconName::MonitorSmartphone))
        .child(div().text_size(px(15.)).font_semibold().child("Aurora")),
    );
    nav = nav.child(self.nav_label(cx, "Devices", 8.));
    for d in self.devices.clone() {
      let id = d.device.id.clone();
      let active = self.page == Page::Home && self.selected.as_ref() == Some(&id);
      let dot = div()
        .size_2()
        .rounded_full()
        .bg(if d.connected {
          online_color()
        } else {
          cx.theme().muted_foreground
        })
        .into_any_element();
      nav = nav.child(self.nav_row(
        cx,
        format!("dev-{id}"),
        Some(IconName::Smartphone),
        d.device.name.clone(),
        active,
        false,
        Some(dot),
        Some(Box::new(cx.listener(move |this, _, _, cx| {
          this.selected = Some(id.clone());
          this.go(Page::Home, cx);
        }))),
      ));
    }
    nav = nav.child(self.nav_row(
      cx,
      "pair-new",
      Some(IconName::Plus),
      "Pair new device",
      self.page == Page::Pair,
      true,
      None,
      Some(Box::new(
        cx.listener(|this, _, _, cx| this.go(Page::Pair, cx)),
      )),
    ));

    if self.page == Page::Home
      && let Some(d) = self.selected_device().cloned()
    {
      nav = nav.child(self.nav_label(cx, d.device.name.clone(), 16.));
      nav = nav.child(self.nav_row(
        cx,
        "f-overview",
        None,
        "Overview",
        true,
        false,
        None,
        Some(Box::new(
          cx.listener(|this, _, _, cx| this.go(Page::Home, cx)),
        )),
      ));
      for f in FEATURES {
        nav = nav.child(self.nav_row(cx, format!("f-{f}"), None, f, false, false, None, None));
      }
    }

    nav.child(div().flex_1()).child(self.nav_row(
      cx,
      "settings",
      Some(IconName::Settings),
      "Settings",
      false,
      false,
      None,
      Some(Box::new(
        cx.listener(|this, _, _, cx| this.go(Page::Settings, cx)),
      )),
    ))
  }

  // ---- pair a new device ---------------------------------------------------------

  fn desktop_pair(&mut self, cx: &mut Context<Self>) -> Div {
    let scan = {
      let body = match &self.info {
        Some(info) => v_flex()
          .items_center()
          .gap_3()
          .child(qr(&info.link, 264.0))
          .children(device_id(cx, &info.link)),
        None => v_flex()
          .items_center()
          .justify_center()
          .h(px(264.))
          .child(Spinner::new()),
      };
      card(cx, false)
        .flex_1()
        .min_w(px(320.))
        .child(card_header(
          cx,
          false,
          "Scan with your phone",
          Some("The code holds this device's key and address.".into()),
          None,
        ))
        .child(card_content(false).child(body))
    };

    let nearby = {
      let action = if self.scanning {
        // centred in the button's box, so the icon does not shift
        div()
          .size_6()
          .flex()
          .items_center()
          .justify_center()
          .child(Spinner::new().small())
          .into_any_element()
      } else {
        Button::new("rescan")
          .cursor_pointer()
          .debug_selector(|| "rescan".into())
          .ghost()
          .small()
          .icon(IconName::RefreshCw)
          .on_click(cx.listener(|this, _, _, cx| this.scan(cx)))
          .into_any_element()
      };
      let mut list = v_flex().gap_4();
      if self.nearby.is_empty() {
        list = list.child(muted(cx, "No phones found yet."));
      }
      for (i, n) in self.nearby.clone().into_iter().enumerate() {
        let link = n.link.clone();
        list = list.child(item(
          cx,
          ItemVariant::Outline,
          true,
          Some(icon(IconName::Smartphone).small().into_any_element()),
          n.name.clone(),
          Some(crate::app::nearby_detail(&n)),
          Some(
            Button::new(("nearby", i))
              .cursor_pointer()
              .loading(self.loading(&n.link))
              .when(self.loading(&n.link), |b| b.icon(Spinner::new()))
              .secondary()
              .small()
              .label("Pair")
              .on_click(
                cx.listener(move |this, _, _, cx| this.connect(link.clone(), link.clone(), cx)),
              )
              .into_any_element(),
          ),
        ));
      }
      card(cx, false)
        .child(card_header(
          cx,
          false,
          "Nearby phones",
          Some("Found on this network.".into()),
          Some(action),
        ))
        .child(card_content(false).child(list))
    };

    let address = card(cx, false)
      .child(card_header(
        cx,
        false,
        "Connect by link or ID",
        Some("Paste the other device's pairing link or device ID.".into()),
        None,
      ))
      .child(
        card_content(false).child(
          h_flex()
            .gap_2()
            .items_end()
            .child(
              v_flex()
                .flex_1()
                .gap_2()
                .child(div().text_sm().font_medium().child("Address or device ID"))
                .child(Input::new(&self.address)),
            )
            .child(
              Button::new("by-address")
                .cursor_pointer()
                .debug_selector(|| "by-address".into())
                .loading(self.loading("by-address"))
                .when(self.loading("by-address"), |b| b.icon(Spinner::new()))
                .outline()
                .label("Connect")
                .on_click(cx.listener(|this, _, _, cx| {
                  let host = this.address.read(cx).value().to_string();
                  if !host.trim().is_empty() {
                    this.connect(host, "by-address", cx);
                  }
                })),
            ),
        ),
      );

    v_flex()
      .py_10()
      .px_12()
      .gap_6()
      .child(heading(
        cx,
        "Pair a new device",
        Some("Open the app on your phone, then scan the code or pick this device from the list."),
      ))
      .child(
        h_flex()
          .items_start()
          .gap_6()
          .max_w(px(1040.))
          .child(scan)
          .child(
            v_flex()
              .flex_1()
              .min_w(px(320.))
              .gap_6()
              .child(nearby)
              .child(address),
          ),
      )
  }

  // ---- confirm -------------------------------------------------------------------

  fn desktop_confirm(&mut self, c: &Confirm, cx: &mut Context<Self>) -> Div {
    let footer = h_flex()
      .justify_end()
      .gap_2()
      .px_6()
      .py_4()
      .border_t_1()
      .border_color(cx.theme().border)
      .bg(cx.theme().muted.opacity(0.5))
      .rounded_b_xl()
      .map(|f| {
        if c.local_accepted {
          f.child(Spinner::new().small())
            .child(muted(cx, "Now confirm the code on your phone"))
        } else {
          f.child(
            Button::new("mismatch")
              .cursor_pointer()
              .debug_selector(|| "mismatch".into())
              .loading(self.loading("mismatch"))
              .when(self.loading("mismatch"), |b| b.icon(Spinner::new()))
              .outline()
              .label("Codes don't match")
              .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
          )
          .child(
            Button::new("pair")
              .cursor_pointer()
              .debug_selector(|| "pair".into())
              .loading(self.loading("pair"))
              .when(self.loading("pair"), |b| b.icon(Spinner::new()))
              .primary()
              .label("Pair")
              .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
          )
        }
      });
    let confirm = card(cx, false)
      .pb_0()
      .w(px(480.))
      .child(card_header(
        cx,
        false,
        format!("Pair with {}?", c.peer_name),
        Some("Check that your phone shows the same code. Pair only if they match.".into()),
        None,
      ))
      .child(
        card_content(false).child(
          v_flex()
            .items_center()
            .gap_5()
            .py_2()
            .child(code_tiles(cx, &c.code))
            .child(Separator::horizontal().w_full())
            .child(confirm_details(cx, c, "Device", "Key fingerprint")),
        ),
      )
      .child(footer);
    div()
      .flex()
      .items_center()
      .justify_center()
      .py_10()
      .px_6()
      .child(confirm)
  }

  // ---- device overview -----------------------------------------------------------

  fn desktop_overview(&mut self, cx: &mut Context<Self>) -> Div {
    let page = v_flex().py_8().px_10().gap_6();
    let Some(d) = self.selected_device().cloned() else {
      return page.child(heading(
        cx,
        if self.offline {
          "aurorad is not running"
        } else {
          "No device paired"
        },
        Some(if self.offline {
          "Start the daemon, then this window updates by itself."
        } else {
          "Choose Pair new device to connect a phone."
        }),
      ));
    };

    let mut badges = h_flex()
      .flex_wrap()
      .gap_1p5()
      .child(badge(crate::app::connection_label(&d), false));
    if let Some(b) = d.battery {
      badges = badges.child(badge(
        format!(
          "Battery {b}%{}",
          if d.charging { " · Charging" } else { "" }
        ),
        true,
      ));
    }
    let header = h_flex()
      .flex_wrap()
      .items_center()
      .gap_4()
      .child(
        v_flex()
          .flex_1()
          .gap_1p5()
          .child(
            div()
              .text_2xl()
              .font_semibold()
              .child(d.device.name.clone()),
          )
          .child(badges),
      )
      .child(
        h_flex()
          .gap_2()
          .child(
            Button::new("ring")
              .cursor_pointer()
              .outline()
              .label("Ring phone")
              .disabled(true),
          )
          .child(
            Button::new("clip")
              .cursor_pointer()
              .outline()
              .label("Send clipboard")
              .disabled(true),
          )
          .child(
            Button::new("file")
              .cursor_pointer()
              .primary()
              .label("Send file")
              .disabled(true),
          ),
      );

    let soon = |cx: &Context<Self>| card_content(true).child(muted(cx, "Coming soon"));
    let notifications = card(cx, true)
      .flex_1()
      .child(card_header(
        cx,
        true,
        "Notifications",
        None,
        Some(
          Button::new("all")
            .cursor_pointer()
            .ghost()
            .xsmall()
            .label("View all")
            .disabled(true)
            .into_any_element(),
        ),
      ))
      .child(soon(cx));
    let transfers = card(cx, true)
      .flex_1()
      .child(card_header(
        cx,
        true,
        "Transfers",
        Some("Saved to ~/Downloads/Aurora".into()),
        None,
      ))
      .child(soon(cx));
    let media = card(cx, true)
      .flex_1()
      .child(card_header(
        cx,
        true,
        "Media",
        Some("Playing on phone".into()),
        None,
      ))
      .child(soon(cx));
    let live = card(cx, true)
      .flex_1()
      .child(card_header(
        cx,
        true,
        "Live sessions",
        Some("Use the phone's hardware here".into()),
        None,
      ))
      .child(
        card_content(true).child(
          h_flex()
            .flex_wrap()
            .gap_2()
            .child(
              Button::new("webcam")
                .cursor_pointer()
                .outline()
                .small()
                .label("Webcam")
                .disabled(true),
            )
            .child(
              Button::new("mic")
                .cursor_pointer()
                .outline()
                .small()
                .label("Microphone")
                .disabled(true),
            )
            .child(
              Button::new("display")
                .cursor_pointer()
                .outline()
                .small()
                .label("Second display")
                .disabled(true),
            ),
        ),
      );

    let screen = v_flex()
      .w(px(300.))
      .flex_none()
      .p_4()
      .gap_3()
      .rounded(px(14.))
      .border_1()
      .border_color(cx.theme().border)
      .bg(cx.theme().popover)
      .child(
        h_flex()
          .items_center()
          .gap_2()
          .child(div().flex_1().text_sm().font_medium().child("Phone screen"))
          .child(badge("Not available", false))
          .child(
            Button::new("popout")
              .cursor_pointer()
              .ghost()
              .small()
              .icon(IconName::ExternalLink)
              .disabled(true),
          ),
      )
      .child(
        div()
          .mx_auto()
          .w(px(220.))
          .h(px(476.))
          .rounded(px(22.))
          .border(px(6.))
          .border_color(cx.theme().muted)
          .bg(gpui::black())
          .flex()
          .items_center()
          .justify_center()
          .child(muted(cx, "Coming soon")),
      )
      .child(
        h_flex()
          .justify_center()
          .gap_1()
          .children(["Back", "Home", "Recents"].map(|l| {
            Button::new(l)
              .cursor_pointer()
              .ghost()
              .small()
              .label(l)
              .disabled(true)
          })),
      );

    page.child(header).child(
      h_flex()
        .items_start()
        .gap_6()
        .child(
          v_flex()
            .flex_1()
            .min_w(px(380.))
            .gap_4()
            .child(
              h_flex()
                .items_stretch()
                .gap_4()
                .child(notifications)
                .child(transfers),
            )
            .child(h_flex().items_stretch().gap_4().child(media).child(live)),
        )
        .child(screen),
    )
  }

  // ---- settings ------------------------------------------------------------------

  fn settings_nav(&mut self, cx: &mut Context<Self>) -> Div {
    let back = h_flex()
      .id("settings-back")
      .items_center()
      .gap_2()
      .p_2()
      .mb_3()
      .text_sm()
      .text_color(cx.theme().muted_foreground)
      .cursor_pointer()
      .child(icon(IconName::ChevronLeft).small())
      .child("Back")
      .on_click(cx.listener(|this, _, _, cx| {
        let page = if this.devices.is_empty() {
          Page::Pair
        } else {
          Page::Home
        };
        this.go(page, cx);
      }));
    let mut nav = self.nav_shell(cx).child(back);
    for (i, s) in SETTINGS_SECTIONS.iter().enumerate() {
      // only Connection exists so far
      nav = nav.child(self.nav_row(cx, format!("sec-{s}"), None, *s, i == 0, i != 0, None, None));
    }
    nav
  }

  fn desktop_settings(&mut self, cx: &mut Context<Self>) -> Div {
    let discovery = self.settings.as_ref().is_some_and(|s| s.discovery);
    let connection = field_set(
      cx,
      "Connection",
      None,
      [
        field_row(
          cx,
          "Discover on local network",
          "Find and be found by devices on this network.",
          Switch::new("discovery")
            .cursor_pointer()
            .checked(discovery)
            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_discovery(*on, cx))),
        )
        .into_any_element(),
        field_row(
          cx,
          "Connect away from home",
          "Use a relay when a direct connection fails.",
          Switch::new("relay-on").cursor_pointer().disabled(true),
        )
        .into_any_element(),
        v_flex()
          .gap_3()
          .child(div().text_sm().font_medium().child("Relay server"))
          .child(Input::new(&self.relay).disabled(true))
          .child(muted(cx, "Leave empty to use the default relay."))
          .into_any_element(),
        field_row(
          cx,
          "USB connection",
          "Connect over a cable when one is plugged in.",
          Switch::new("usb")
            .cursor_pointer()
            .checked(self.usb().unwrap_or(false))
            .disabled(self.usb().is_none())
            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_usb(*on, cx))),
        )
        .into_any_element(),
      ],
    );

    let mut page = v_flex()
      .py_8()
      .px_10()
      .gap_8()
      .max_w(px(760.))
      .child(div().text_2xl().font_semibold().child("Settings"))
      .child(connection);

    if let Some(d) = self.selected_device().cloned() {
      let features = field_set(
        cx,
        format!("Features for {}", d.device.name),
        Some("Each feature can be turned off per device."),
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
      let id = d.device.id.clone();
      page = page
        .child(Separator::horizontal())
        .child(features)
        .child(Separator::horizontal())
        .child(
          h_flex()
            .flex_wrap()
            .items_center()
            .gap_4()
            .child(
              v_flex()
                .flex_1()
                .gap_0p5()
                .child(
                  div()
                    .text_sm()
                    .font_medium()
                    .child(format!("Unpair {}", d.device.name)),
                )
                .child(
                  div()
                    .text_size(px(13.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Removes its key. You'll need to pair again to reconnect."),
                ),
            )
            .child(
              Button::new("unpair")
                .cursor_pointer()
                .debug_selector(|| "unpair".into())
                .loading(self.loading("unpair"))
                .when(self.loading("unpair"), |b| b.icon(Spinner::new()))
                .danger()
                .label("Unpair device")
                .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
            ),
        );
    }
    page
  }
}

#[cfg(test)]
mod tests {
  use aurora_core::types::PairState;
  use gpui::{AppContext, TestAppContext};

  use crate::{
    app::{Page, Platform},
    test_support::*,
  };

  #[gpui::test]
  fn every_page_renders(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![device("a", "home")]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    assert!(shown(vcx, "f-overview"));
    for page in [Page::Pair, Page::Settings, Page::Home] {
      vcx.update_entity(&app, |a, cx| a.go(page, cx));
      draw(vcx);
    }
    fake.set(|s| s.nearby = vec![nearby("phone")]);
    vcx.update_entity(&app, |a, cx| {
      a.go(Page::Pair, cx);
      a.scan(cx);
    });
    draw(vcx);
    for accepted in [false, true] {
      fake.set(|s| s.pair = PairState::Confirm(confirm(accepted)));
      vcx.update_entity(&app, |a, cx| a.refresh(cx));
      draw(vcx);
      assert_eq!(shown(vcx, "mismatch"), !accepted);
    }
  }

  #[gpui::test]
  fn sidebar_rows_navigate(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![device("a", "home")]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    click(vcx, "pair-new");
    vcx.read_entity(&app, |a, _| assert!(a.page == Page::Pair));
    assert!(fake.calls().contains(&"start_pairing".to_string()));
    click(vcx, "dev-a");
    vcx.read_entity(&app, |a, _| assert!(a.page == Page::Home));
    click(vcx, "f-overview");
    vcx.read_entity(&app, |a, _| assert!(a.page == Page::Home));
    click(vcx, "settings");
    vcx.read_entity(&app, |a, _| assert!(a.page == Page::Settings));
  }

  #[gpui::test]
  fn connect_by_link_dials_what_was_typed(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    let input = vcx.read_entity(&app, |a, _| a.address.clone());
    vcx.update(|window, cx| {
      input.update(cx, |s, cx| s.set_value("aurora://pair?id=ab", window, cx))
    });
    draw(vcx);
    click(vcx, "by-address");
    assert!(
      fake
        .calls()
        .contains(&"connect aurora://pair?id=ab".to_string())
    );
  }
}

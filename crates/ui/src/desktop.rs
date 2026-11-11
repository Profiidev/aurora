use std::time::Instant;

use aurora_core::types::{Confirm, PairState};
use gpui::{
  Context, Div, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
  div, px,
};
use gpui_kit::component::{
  ActiveTheme, Disableable, Sizable, StyledExt,
  button::{Button, ButtonVariants},
  group_box::GroupBox,
  h_flex,
  input::{Input, OtpInput},
  progress::Progress,
  separator::Separator,
  sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem},
  spinner::Spinner,
  switch::Switch,
  v_flex,
};

use crate::{
  app::{AppView, Page, SettingsTab},
  widgets::{card, chip, confirm_details, error_line, muted, qr, soon},
};

/// What the device overview lists; none of these are built yet
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

impl AppView {
  pub(crate) fn desktop(&mut self, cx: &mut Context<Self>) -> Div {
    // a code to confirm replaces the whole window content, like the design
    if let PairState::Confirm(c) = self.pair.clone() {
      return self.desktop_confirm(&c, cx);
    }
    let main = match self.page {
      Page::Home => self.desktop_overview(cx),
      Page::Pair => self.desktop_pair(cx),
      Page::Settings => self.desktop_settings(cx),
    };
    h_flex().child(self.sidebar(cx)).child(
      div()
        .id("main")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .child(main),
    )
  }

  fn sidebar(&mut self, cx: &mut Context<Self>) -> Sidebar<SidebarGroup<SidebarMenu>> {
    let online = gpui::hsla(0.4, 0.6, 0.5, 1.0);
    let mut devices = SidebarMenu::new();
    for d in &self.devices {
      let id = d.device.id.clone();
      let active = self.page == Page::Home && self.selected.as_ref() == Some(&id);
      let connected = d.connected;
      let muted = cx.theme().muted_foreground;
      devices = devices.child(
        SidebarMenuItem::new(d.device.name.clone())
          .active(active)
          .suffix(move |_, _| {
            div()
              .size_2()
              .rounded_full()
              .bg(if connected { online } else { muted })
          })
          .on_click(cx.listener(move |this, _, _, cx| {
            this.selected = Some(id.clone());
            this.go(Page::Home, cx);
          })),
      );
    }
    devices = devices.child(
      SidebarMenuItem::new("Pair new device")
        .active(self.page == Page::Pair)
        .on_click(cx.listener(|this, _, _, cx| this.go(Page::Pair, cx))),
    );

    let mut sidebar = Sidebar::new("sidebar")
      .header(SidebarHeader::new().child(div().font_semibold().child("Aurora")))
      .child(SidebarGroup::new("Devices").child(devices));
    if let Some(device) = self.selected_device() {
      let features = std::iter::once(
        SidebarMenuItem::new("Overview")
          .active(self.page == Page::Home)
          .on_click(cx.listener(|this, _, _, cx| this.go(Page::Home, cx))),
      )
      .chain(
        FEATURES
          .iter()
          .map(|f| SidebarMenuItem::new(*f).disable(true)),
      );
      sidebar = sidebar.child(
        SidebarGroup::new(device.device.name.clone()).child(SidebarMenu::new().children(features)),
      );
    }
    let settings = Button::new("settings").label("Settings");
    let settings = if self.page == Page::Settings {
      settings.secondary()
    } else {
      settings.ghost()
    };
    sidebar.footer(
      SidebarFooter::new()
        .child(settings.on_click(cx.listener(|this, _, _, cx| this.go(Page::Settings, cx)))),
    )
  }

  fn heading(&self, cx: &Context<Self>, title: &str, subtitle: Option<&str>) -> Div {
    v_flex()
      .gap_1()
      .child(div().text_2xl().font_semibold().child(title.to_string()))
      .children(subtitle.map(|s| muted(cx, s.to_string())))
  }

  // ---- device overview -------------------------------------------------------

  fn desktop_overview(&mut self, cx: &mut Context<Self>) -> Div {
    let page = v_flex().p_8().gap_6();
    let Some(d) = self.selected_device().cloned() else {
      return page
        .child(self.heading(
          cx,
          if self.offline {
            "aurorad is not running"
          } else {
            "No device paired yet"
          },
          Some(if self.offline {
            "Start the daemon, then this window updates by itself."
          } else {
            "Choose Pair new device to connect your phone."
          }),
        ))
        .child(
          h_flex().child(
            Button::new("pair-first")
              .primary()
              .label("Pair new device")
              .disabled(self.offline)
              .on_click(cx.listener(|this, _, _, cx| this.go(Page::Pair, cx))),
          ),
        );
    };

    let mut tags = h_flex().flex_wrap().gap_2().child(chip(
      if d.connected {
        "Connected · Local network"
      } else {
        "Offline"
      },
      false,
    ));
    if let Some(b) = d.battery {
      tags = tags.child(chip(
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
          .gap_2()
          .child(
            div()
              .text_2xl()
              .font_semibold()
              .child(d.device.name.clone()),
          )
          .child(tags),
      )
      .child(
        h_flex()
          .gap_2()
          .child(
            Button::new("ring")
              .outline()
              .label("Ring phone")
              .disabled(true),
          )
          .child(
            Button::new("clip")
              .outline()
              .label("Send clipboard")
              .disabled(true),
          )
          .child(
            Button::new("file")
              .primary()
              .label("Send file")
              .disabled(true),
          ),
      );

    let cards = h_flex().flex_wrap().items_start().gap_4().children(
      [
        ("Notifications", None),
        ("Transfers", Some("Saved to ~/Downloads/Aurora")),
        ("Media", Some("Playing on phone")),
        ("Live sessions", Some("Use the phone's hardware here")),
      ]
      .map(|(title, description)| {
        card(cx, Some(title), description)
          .w(px(280.))
          .flex_1()
          .min_w(px(240.))
          .child(soon(cx))
      }),
    );

    let screen = card(cx, None, None)
      .w(px(320.))
      .child(
        h_flex()
          .gap_2()
          .items_center()
          .child(div().flex_1().font_medium().child("Phone screen"))
          .child(chip("Not available yet", false)),
      )
      .child(
        div()
          .w(px(220.))
          .h(px(476.))
          .mx_auto()
          .rounded_3xl()
          .border_4()
          .border_color(cx.theme().border)
          .bg(cx.theme().background),
      )
      .child(h_flex().justify_center().gap_1().children(
        ["Back", "Home", "Recents"].map(|l| Button::new(l).ghost().small().label(l).disabled(true)),
      ));

    page.child(header).child(
      h_flex()
        .flex_wrap()
        .items_start()
        .gap_6()
        .child(div().flex_1().min_w(px(380.)).child(cards))
        .child(screen),
    )
  }

  // ---- pair new device ----------------------------------------------------------

  fn desktop_pair(&mut self, cx: &mut Context<Self>) -> Div {
    let qr_card = {
      let mut c = card(
        cx,
        Some("Scan with your phone"),
        Some("The code holds this computer's key and address. It refreshes every 60 seconds."),
      )
      .w(px(380.));
      match &self.info {
        Some((info, end)) => {
          let left = end.saturating_duration_since(Instant::now()).as_secs_f32();
          let pct = (left / info.expires_in.max(1) as f32 * 100.0).clamp(0.0, 100.0);
          c = c.child(
            v_flex()
              .items_center()
              .gap_4()
              .child(qr(&info.link, 200.0))
              .child(muted(
                cx,
                format!("Or connect by address: {}", {
                  info
                    .addrs
                    .iter()
                    .map(|a| a.ip().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
                }),
              ))
              .child(Progress::new("countdown").value(pct).w(px(200.))),
          );
        }
        None => {
          c = c.child(
            h_flex()
              .gap_2()
              .child(Spinner::new())
              .child(muted(cx, "Starting…")),
          )
        }
      }
      c
    };

    let nearby = {
      let mut c = card(cx, Some("Nearby phones"), Some("Found on this network."));
      c = c.child(h_flex().justify_end().child(if self.scanning {
        Spinner::new().into_any_element()
      } else {
        Button::new("rescan")
          .ghost()
          .small()
          .label("Refresh")
          .on_click(cx.listener(|this, _, _, cx| this.scan(cx)))
          .into_any_element()
      }));
      if self.nearby.is_empty() {
        c = c.child(muted(cx, "No phones found yet."));
      }
      for (i, n) in self.nearby.iter().enumerate() {
        let link = n.link.clone();
        c = c.child(
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
      c
    };

    let address = card(
      cx,
      Some("Connect by address"),
      Some("For phones on another network, a VPN or a USB cable."),
    )
    .child(
      h_flex()
        .gap_2()
        .items_end()
        .child(div().flex_1().child(Input::new(&self.address)))
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
    );

    v_flex()
      .p_10()
      .gap_6()
      .child(self.heading(
        cx,
        "Pair a new device",
        Some("Open the app on your phone, then scan the code or pick this computer from the list."),
      ))
      .children(error_line(cx, &self.error))
      .child(
        h_flex()
          .flex_wrap()
          .items_start()
          .gap_6()
          .child(qr_card)
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

  // ---- confirm ------------------------------------------------------------------

  fn desktop_confirm(&mut self, c: &Confirm, cx: &mut Context<Self>) -> Div {
    let footer = if c.local_accepted {
      h_flex()
        .gap_2()
        .justify_end()
        .child(Spinner::new())
        .child(muted(cx, "Waiting for the phone…"))
    } else {
      h_flex()
        .gap_2()
        .justify_end()
        .child(
          Button::new("mismatch")
            .outline()
            .label("Codes don't match")
            .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
        )
        .child(
          Button::new("pair")
            .primary()
            .label("Pair")
            .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
        )
    };
    div().flex().items_center().justify_center().p_10().child(
      card(
        cx,
        Some(&format!("Pair with {}?", c.peer_name)),
        Some("Check that your phone shows the same code. Pair only if they match."),
      )
      .w(px(480.))
      .child(
        v_flex()
          .items_center()
          .gap_5()
          .child(OtpInput::new(&self.otp).disabled(true))
          .child(Separator::horizontal())
          .child(div().w_full().child(confirm_details(cx, c, "Device"))),
      )
      .child(footer),
    )
  }

  // ---- settings -----------------------------------------------------------------

  fn desktop_settings(&mut self, cx: &mut Context<Self>) -> Div {
    let tabs = [
      (SettingsTab::Connection, "Connection"),
      (SettingsTab::Features, "Features"),
      (SettingsTab::Device, "Device"),
    ];
    let nav = v_flex()
      .w(px(200.))
      .gap_1()
      .children(tabs.map(|(tab, label)| {
        tab_button(label, self.settings_tab == tab).on_click(cx.listener(move |this, _, _, cx| {
          this.settings_tab = tab;
          cx.notify();
        }))
      }));

    let body = match self.settings_tab {
      SettingsTab::Connection => self.settings_connection(cx),
      SettingsTab::Features => self.settings_features(cx),
      SettingsTab::Device => self.settings_device(cx),
    };
    v_flex()
      .p_10()
      .gap_6()
      .child(div().text_2xl().font_semibold().child("Settings"))
      .child(
        h_flex()
          .items_start()
          .gap_8()
          .child(nav)
          .child(body.flex_1().max_w(px(640.))),
      )
  }

  fn row(
    &self,
    cx: &Context<Self>,
    title: &str,
    description: &str,
    control: impl IntoElement,
  ) -> Div {
    h_flex()
      .gap_4()
      .items_center()
      .child(
        v_flex()
          .flex_1()
          .child(div().font_medium().child(title.to_string()))
          .child(muted(cx, description.to_string())),
      )
      .child(control)
  }

  fn settings_connection(&mut self, cx: &mut Context<Self>) -> GroupBox {
    let discovery = self.settings.as_ref().is_some_and(|s| s.discovery);
    card(cx, Some("Connection"), None)
      .child(
        self.row(
          cx,
          "Discover on local network",
          "Announce this computer to phones on the same network.",
          Switch::new("discovery")
            .checked(discovery)
            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_discovery(*on, cx))),
        ),
      )
      .child(Separator::horizontal())
      .child(self.row(
        cx,
        "Connect away from home",
        "Use a relay when a direct connection fails.",
        Switch::new("relay").disabled(true),
      ))
      .child(Separator::horizontal())
      .child(self.row(
        cx,
        "USB connection",
        "Connect over a cable when one is plugged in.",
        Switch::new("usb").disabled(true),
      ))
  }

  fn settings_features(&mut self, cx: &mut Context<Self>) -> GroupBox {
    let name = self
      .selected_device()
      .map_or("this device".to_string(), |d| d.device.name.clone());
    let mut c = card(
      cx,
      Some(&format!("Features for {name}")),
      Some("Each feature can be turned off per device."),
    );
    for (i, f) in FEATURES.iter().enumerate() {
      if i > 0 {
        c = c.child(Separator::horizontal());
      }
      c = c.child(self.row(cx, f, "Coming soon", Switch::new(*f).disabled(true)));
    }
    c
  }

  fn settings_device(&mut self, cx: &mut Context<Self>) -> GroupBox {
    let Some(d) = self.selected_device().cloned() else {
      return card(cx, Some("Device"), None).child(muted(cx, "No device paired."));
    };
    let id = d.device.id.clone();
    card(cx, Some("Device"), None).child(
      self.row(
        cx,
        &format!("Unpair {}", d.device.name),
        "Removes its key. You'll need to pair again to reconnect.",
        Button::new("unpair")
          .danger()
          .label("Unpair device")
          .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
      ),
    )
  }
}

fn tab_button(label: &'static str, active: bool) -> Button {
  let b = Button::new(label).label(label);
  if active { b.secondary() } else { b.ghost() }
}

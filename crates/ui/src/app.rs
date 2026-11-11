use std::sync::Arc;

use aurora_core::types::{Confirm, DeviceStatus, Event, Nearby, PairInfo, PairState};
use gpui::{
  AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Window,
  div, prelude::*, px,
};
use gpui_kit::component::{
  ActiveTheme, Sizable, StyledExt,
  button::{Button, ButtonVariants},
  h_flex,
  input::{Input, InputState},
  spinner::Spinner,
  v_flex,
};
use qrcode::{Color, QrCode};

use crate::Backend;

#[derive(Clone, Copy, PartialEq)]
pub enum Platform {
  Desktop,
  Mobile,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
  Home,
  Pair,
}

pub struct AppView {
  backend: Arc<dyn Backend>,
  platform: Platform,
  screen: Screen,
  devices: Vec<DeviceStatus>,
  selected: Option<String>,
  pair: PairState,
  info: Option<PairInfo>,
  nearby: Vec<Nearby>,
  scanning: bool,
  /// Shown on the pair screen after a failed or expired session
  error: Option<SharedString>,
  /// The daemon cannot be reached
  offline: bool,
  address: Entity<InputState>,
}

impl AppView {
  pub fn new(
    backend: Arc<dyn Backend>,
    platform: Platform,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let address = cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.10"));

    // events arrive on a blocking iterator; hand them to the UI thread
    let (tx, rx) = flume::unbounded();
    let source = backend.clone();
    std::thread::spawn(move || {
      for event in source.events() {
        if tx.send(event).is_err() {
          break;
        }
      }
    });
    cx.spawn(async move |this, cx| {
      while let Ok(event) = rx.recv_async().await {
        if this
          .update(cx, |this, cx| this.on_event(event, cx))
          .is_err()
        {
          break;
        }
      }
    })
    .detach();

    let mut this = Self {
      backend,
      platform,
      screen: Screen::Home,
      devices: Vec::new(),
      selected: None,
      pair: PairState::Idle,
      info: None,
      nearby: Vec::new(),
      scanning: false,
      error: None,
      offline: false,
      address,
    };
    this.refresh(cx);
    if platform == Platform::Mobile {
      this.open_pair(cx);
    }
    this
  }

  /// Runs `work` off the UI thread, then `done` on it
  fn bg<T: Send + 'static>(
    &self,
    cx: &mut Context<Self>,
    work: impl FnOnce(&dyn Backend) -> T + Send + 'static,
    done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
  ) {
    let backend = self.backend.clone();
    cx.spawn(async move |this, cx| {
      let res = cx
        .background_executor()
        .spawn(async move { work(&*backend) })
        .await;
      this.update(cx, |this, cx| done(this, res, cx)).ok();
    })
    .detach();
  }

  fn refresh(&mut self, cx: &mut Context<Self>) {
    self.bg(
      cx,
      |b| (b.devices(), b.pair_state()),
      |this, (devices, state), cx| {
        this.offline = devices.is_err();
        if let (Ok(devices), Ok(state)) = (devices, state) {
          this.devices = devices;
          this.pair = state;
        }
        if this.selected.is_none() {
          this.selected = this.devices.first().map(|d| d.device.id.clone());
        }
        cx.notify();
      },
    );
  }

  fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
    match &event {
      Event::Paired(device) => {
        self.selected = Some(device.id.clone());
        self.screen = Screen::Home;
        self.error = None;
      }
      Event::PairingExpired => self.error = Some("Pairing timed out".into()),
      Event::PairingFailed(why) => self.error = Some(format!("Pairing failed: {why}").into()),
      Event::Unpaired(id) if self.selected.as_ref() == Some(id) => self.selected = None,
      _ => {}
    }
    self.refresh(cx);
    cx.notify();
  }

  fn open_pair(&mut self, cx: &mut Context<Self>) {
    self.screen = Screen::Pair;
    self.error = None;
    self.info = None;
    // waiting for a dialer also makes this device reachable for the other side
    self.bg(
      cx,
      |b| b.start_pairing(),
      |this, res, cx| {
        match res {
          Ok(info) => this.info = Some(info),
          Err(e) => this.error = Some(format!("{e:#}").into()),
        }
        cx.notify();
      },
    );
    if self.platform == Platform::Mobile {
      self.scan(cx);
    }
    cx.notify();
  }

  fn close_pair(&mut self, cx: &mut Context<Self>) {
    self.screen = Screen::Home;
    self.bg(cx, |b| b.cancel_pairing(), |_, _, _| {});
    cx.notify();
  }

  fn scan(&mut self, cx: &mut Context<Self>) {
    self.scanning = true;
    self.bg(
      cx,
      |b| b.nearby(),
      |this, res, cx| {
        this.scanning = false;
        this.nearby = res.unwrap_or_default();
        cx.notify();
      },
    );
  }

  fn connect(&mut self, target: String, cx: &mut Context<Self>) {
    self.error = None;
    self.bg(
      cx,
      move |b| {
        // an open waiting session would refuse to dial out
        let _ = b.cancel_pairing();
        b.connect(&target)
      },
      |this, res, cx| {
        if let Err(e) = res {
          this.error = Some(format!("{e:#}").into());
        }
        this.refresh(cx);
        cx.notify();
      },
    );
  }

  fn decide(&mut self, accept: bool, cx: &mut Context<Self>) {
    self.bg(cx, move |b| b.decide(accept), |_, _, _| {});
    self.refresh(cx);
  }

  fn unpair(&mut self, id: String, cx: &mut Context<Self>) {
    self.bg(cx, move |b| b.unpair(&id), |_, _, _| {});
  }

  // ---- shared pieces -------------------------------------------------------

  fn muted(&self, cx: &Context<Self>, text: impl Into<SharedString>) -> gpui::Div {
    div()
      .text_sm()
      .text_color(cx.theme().muted_foreground)
      .child(text.into())
  }

  fn error_line(&self, cx: &Context<Self>) -> Option<gpui::Div> {
    self.error.as_ref().map(|e| {
      div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(e.clone())
    })
  }

  /// Six digits, both devices must show the same
  fn confirm_view(&self, c: &Confirm, cx: &mut Context<Self>) -> impl IntoElement {
    let (head, tail) = c.code.split_at(3);
    v_flex()
      .gap_4()
      .child(
        div()
          .text_xl()
          .font_semibold()
          .child(format!("Pair with {}?", c.peer_name)),
      )
      .child(self.muted(
        cx,
        "Check that the other device shows the same code. Pair only if they match.",
      ))
      .child(
        div()
          .text_size(px(44.))
          .font_semibold()
          .child(format!("{head} {tail}")),
      )
      .child(self.muted(cx, format!("{} · {}", c.peer_name, c.peer_os)))
      .child(self.muted(cx, format!("Connected via {}", c.via)))
      .child(if c.local_accepted {
        h_flex()
          .gap_2()
          .child(Spinner::new())
          .child(self.muted(cx, "Waiting for the other device…"))
      } else {
        h_flex()
          .gap_2()
          .child(
            Button::new("pair")
              .primary()
              .label("Pair")
              .on_click(cx.listener(|this, _, _, cx| this.decide(true, cx))),
          )
          .child(
            Button::new("mismatch")
              .label("Codes don't match")
              .on_click(cx.listener(|this, _, _, cx| this.decide(false, cx))),
          )
      })
  }

  fn qr(&self, link: &str) -> gpui::Div {
    let Ok(code) = QrCode::new(link.as_bytes()) else {
      return div();
    };
    let width = code.width();
    let cell = px(4.);
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
    // a white border keeps the code scannable on the dark theme
    div()
      .p_3()
      .bg(gpui::white())
      .rounded_lg()
      .child(v_flex().children(rows))
  }

  fn nearby_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let items = self.nearby.iter().enumerate().map(|(i, n)| {
      let link = n.link.clone();
      h_flex()
        .gap_3()
        .p_3()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_lg()
        .child(
          v_flex()
            .flex_1()
            .child(div().font_medium().child(n.name.clone()))
            .child(self.muted(cx, format!("{} · {}", n.os, n.addr.ip()))),
        )
        .child(
          Button::new(("nearby", i))
            .small()
            .label("Pair")
            .on_click(cx.listener(move |this, _, _, cx| this.connect(link.clone(), cx))),
        )
    });
    v_flex().gap_2().children(items)
  }

  // ---- desktop ---------------------------------------------------------------

  fn desktop_home(&mut self, cx: &mut Context<Self>) -> gpui::Div {
    let rows = self.devices.iter().map(|d| {
      let id = d.device.id.clone();
      let active = self.selected.as_ref() == Some(&id);
      Button::new(SharedString::from(id.clone()))
        .label(format!(
          "{}{}",
          d.device.name,
          if d.connected { " ●" } else { "" }
        ))
        .when(active, |b| b.primary())
        .when(!active, |b| b.ghost())
        .on_click(cx.listener(move |this, _, _, cx| {
          this.selected = Some(id.clone());
          cx.notify();
        }))
    });
    let sidebar = v_flex()
      .w(px(240.))
      .h_full()
      .p_4()
      .gap_2()
      .border_r_1()
      .border_color(cx.theme().border)
      .child(div().font_semibold().child("Devices"))
      .children(rows)
      .child(div().flex_1())
      .child(
        Button::new("pair-new")
          .primary()
          .label("Pair new device")
          .on_click(cx.listener(|this, _, _, cx| this.open_pair(cx))),
      );

    let selected = self
      .devices
      .iter()
      .find(|d| Some(&d.device.id) == self.selected.as_ref())
      .cloned();
    let main = match selected {
      Some(d) => self.device_card(&d, cx),
      None => v_flex()
        .gap_2()
        .child(div().text_xl().font_semibold().child(if self.offline {
          "aurorad is not running"
        } else {
          "No device paired yet"
        }))
        .child(self.muted(
          cx,
          if self.offline {
            "Start the daemon (aurorad), then this window updates by itself."
          } else {
            "Choose Pair new device to connect your phone."
          },
        )),
    };
    h_flex()
      .size_full()
      .child(sidebar)
      .child(main.p_6().flex_1())
  }

  fn device_card(&self, d: &DeviceStatus, cx: &mut Context<Self>) -> gpui::Div {
    let id = d.device.id.clone();
    let battery = match d.battery {
      Some(b) => format!(
        "Battery {b}%{}",
        if d.charging { " · Charging" } else { "" }
      ),
      None => "Battery unknown".into(),
    };
    v_flex()
      .gap_2()
      .child(div().text_xl().font_semibold().child(d.device.name.clone()))
      .child(self.muted(
        cx,
        if d.connected {
          "Connected · Local network"
        } else {
          "Offline"
        },
      ))
      .child(self.muted(cx, d.device.os.clone()))
      .when(d.connected, |v| v.child(self.muted(cx, battery)))
      .child(
        h_flex().pt_4().child(
          Button::new("unpair")
            .danger()
            .label("Unpair")
            .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
        ),
      )
  }

  fn desktop_pair(&mut self, cx: &mut Context<Self>) -> gpui::Div {
    let body = match self.pair.clone() {
      PairState::Confirm(c) => self.confirm_view(&c, cx).into_any_element(),
      PairState::Waiting(info) => v_flex()
        .gap_3()
        .child(div().text_xl().font_semibold().child("Pair new device"))
        .child(self.muted(cx, "Open Aurora on your phone and scan this code, or choose this computer under Computers nearby."))
        .child(self.qr(&info.link))
        .child(self.muted(
          cx,
          format!(
            "Or connect by address: {}",
            info.addrs.iter().map(|a| a.ip().to_string()).collect::<Vec<_>>().join(", ")
          ),
        ))
        .child(h_flex().gap_2().child(Spinner::new()).child(self.muted(cx, format!("Waiting for your phone… ({} s)", info.expires_in))))
        .into_any_element(),
      PairState::Dialing => h_flex().gap_2().child(Spinner::new()).child(self.muted(cx, "Connecting…")).into_any_element(),
      PairState::Idle => v_flex()
        .gap_3()
        .children(self.error_line(cx))
        .child(Button::new("retry").primary().label("Try again").on_click(cx.listener(|this, _, _, cx| this.open_pair(cx))))
        .into_any_element(),
    };
    v_flex()
      .size_full()
      .p_6()
      .gap_4()
      .child(
        Button::new("back")
          .ghost()
          .label("← Back")
          .on_click(cx.listener(|this, _, _, cx| this.close_pair(cx))),
      )
      .child(body)
  }

  // ---- mobile ----------------------------------------------------------------

  fn mobile_pair(&mut self, cx: &mut Context<Self>) -> gpui::Div {
    let page = v_flex().size_full().pt(px(48.)).px_5().pb_6().gap_5();
    if let PairState::Confirm(c) = self.pair.clone() {
      return page.child(self.confirm_view(&c, cx));
    }
    let busy = matches!(self.pair, PairState::Dialing);
    page
      .child(
        v_flex()
          .gap_1()
          .child(div().text_xl().font_semibold().child("Connect a computer"))
          .child(self.muted(
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
      .child(
        v_flex()
          .gap_2()
          .child(
            h_flex()
              .child(div().font_medium().flex_1().child("Computers nearby"))
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
          .child(self.nearby_list(cx)),
      )
      .child(
        v_flex()
          .gap_2()
          .child(div().font_medium().child("Connect by address"))
          .child(Input::new(&self.address))
          .child(
            Button::new("by-address")
              .label("Connect")
              .on_click(cx.listener(|this, _, _, cx| {
                let host = this.address.read(cx).value().to_string();
                if !host.trim().is_empty() {
                  this.connect(host, cx);
                }
              })),
          ),
      )
      .children(self.error_line(cx))
      .child(self.paired_list(cx))
  }

  fn paired_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
    let rows = self.devices.iter().enumerate().map(|(i, d)| {
      let id = d.device.id.clone();
      h_flex()
        .gap_3()
        .child(
          v_flex()
            .flex_1()
            .child(div().font_medium().child(d.device.name.clone()))
            .child(self.muted(
              cx,
              if d.connected {
                "Connected · Local network"
              } else {
                "Offline"
              },
            )),
        )
        .child(
          Button::new(("unpair", i))
            .small()
            .danger()
            .label("Unpair")
            .on_click(cx.listener(move |this, _, _, cx| this.unpair(id.clone(), cx))),
        )
    });
    v_flex()
      .gap_2()
      .when(!self.devices.is_empty(), |v| {
        v.child(div().font_medium().child("Paired computers"))
      })
      .children(rows)
  }

  /// Opens the live camera view; a scanned `aurora://pair` code starts pairing
  fn scan_qr(&mut self, cx: &mut Context<Self>) {
    self.error = None;
    self.bg(
      cx,
      |b| {
        // an open waiting session would refuse to dial out
        let _ = b.cancel_pairing();
        b.scan_qr()
      },
      |this, res, cx| {
        match res {
          Ok(Some(link)) => this.connect(link, cx),
          Ok(None) => this.open_pair(cx),
          Err(e) => this.error = Some(format!("{e:#}").into()),
        }
        cx.notify();
      },
    );
  }
}

impl Render for AppView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let page = match (self.platform, self.screen) {
      (Platform::Desktop, Screen::Home) => self.desktop_home(cx),
      (Platform::Desktop, Screen::Pair) => self.desktop_pair(cx),
      (Platform::Mobile, _) => self.mobile_pair(cx),
    };
    page
      .size_full()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
  }
}

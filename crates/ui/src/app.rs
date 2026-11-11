use std::{
  sync::Arc,
  time::{Duration, Instant},
};

use aurora_core::types::{AppSettings, DeviceStatus, Event, Nearby, PairInfo, PairState};
use gpui::{AppContext, Context, Entity, IntoElement, Render, SharedString, Styled, Window};
use gpui_kit::component::{
  ActiveTheme,
  input::{InputState, OtpState},
};

use crate::Backend;

#[derive(Clone, Copy, PartialEq)]
pub enum Platform {
  Desktop,
  Mobile,
}

/// Desktop: sidebar entries; mobile: bottom navigation
#[derive(Clone, Copy, PartialEq)]
pub enum Page {
  /// Device overview (desktop) / home (mobile)
  Home,
  /// Pair new device (desktop) / devices (mobile)
  Pair,
  Settings,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SettingsTab {
  Connection,
  Features,
  Device,
}

pub struct AppView {
  pub(crate) backend: Arc<dyn Backend>,
  pub(crate) platform: Platform,
  pub(crate) page: Page,
  pub(crate) settings_tab: SettingsTab,
  pub(crate) devices: Vec<DeviceStatus>,
  pub(crate) selected: Option<String>,
  pub(crate) pair: PairState,
  /// The waiting session shown as QR code, and when it ends
  pub(crate) info: Option<(PairInfo, Instant)>,
  pub(crate) nearby: Vec<Nearby>,
  pub(crate) scanning: bool,
  /// Shown on the pair page after a failed or expired session
  pub(crate) error: Option<SharedString>,
  /// The daemon cannot be reached
  pub(crate) offline: bool,
  pub(crate) settings: Option<AppSettings>,
  pub(crate) address: Entity<InputState>,
  pub(crate) otp: Entity<OtpState>,
  tick: u64,
}

impl AppView {
  pub fn new(
    backend: Arc<dyn Backend>,
    platform: Platform,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let address = cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.42"));
    let otp = cx.new(|cx| OtpState::new(6, window, cx));

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

    // drives the countdown and keeps the nearby list fresh
    cx.spawn(async move |this, cx| {
      loop {
        cx.background_executor().timer(Duration::from_secs(1)).await;
        if this.update(cx, |this, cx| this.on_tick(cx)).is_err() {
          break;
        }
      }
    })
    .detach();

    let mut this = Self {
      backend,
      platform,
      page: Page::Home,
      settings_tab: SettingsTab::Connection,
      devices: Vec::new(),
      selected: None,
      pair: PairState::Idle,
      info: None,
      nearby: Vec::new(),
      scanning: false,
      error: None,
      offline: false,
      settings: None,
      address,
      otp,
      tick: 0,
    };
    this.refresh(cx);
    this
  }

  /// Runs `work` off the UI thread, then `done` on it
  pub(crate) fn bg<T: Send + 'static>(
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

  pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
    self.bg(
      cx,
      |b| (b.devices(), b.pair_state(), b.settings()),
      |this, (devices, state, settings), cx| {
        this.offline = devices.is_err();
        if let (Ok(devices), Ok(state)) = (devices, state) {
          this.devices = devices;
          this.pair = state;
        }
        if let Ok(settings) = settings {
          this.settings = Some(settings);
        }
        let known = this
          .devices
          .iter()
          .any(|d| Some(&d.device.id) == this.selected.as_ref());
        if !known {
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
        self.page = Page::Home;
        self.error = None;
        self.info = None;
      }
      Event::PairingExpired => self.rearm(cx),
      Event::PairingFailed(why) => {
        self.error = Some(format!("Pairing failed: {why}").into());
        self.rearm(cx);
      }
      Event::PairingCancelled => self.rearm(cx),
      Event::Unpaired(id) if self.selected.as_ref() == Some(id) => self.selected = None,
      _ => {}
    }
    self.refresh(cx);
    cx.notify();
  }

  fn on_tick(&mut self, cx: &mut Context<Self>) {
    self.tick += 1;
    if self.on_pair_page() {
      if self.tick.is_multiple_of(5) && !self.scanning {
        self.scan(cx);
      }
      cx.notify();
    }
  }

  pub(crate) fn on_pair_page(&self) -> bool {
    self.page == Page::Pair
  }

  // ---- navigation ------------------------------------------------------------

  pub(crate) fn go(&mut self, page: Page, cx: &mut Context<Self>) {
    let was_pairing = self.on_pair_page();
    self.page = page;
    if page == Page::Pair && !was_pairing {
      self.error = None;
      self.open_pair(cx);
    } else if page != Page::Pair && was_pairing {
      self.info = None;
      if !matches!(self.pair, PairState::Confirm(_)) {
        self.bg(cx, |b| b.cancel_pairing(), |_, _, _| {});
      }
    }
    cx.notify();
  }

  /// Waits for a dialer, which also makes this device reachable by the other side
  pub(crate) fn open_pair(&mut self, cx: &mut Context<Self>) {
    self.bg(
      cx,
      |b| b.start_pairing(),
      |this, res, cx| {
        // an error means a session is already running, e.g. while a code is confirmed
        if let Ok(info) = res {
          let end = Instant::now() + Duration::from_secs(info.expires_in);
          this.info = Some((info, end));
        }
        cx.notify();
      },
    );
    self.scan(cx);
  }

  /// A finished session on the pair page starts the next one
  fn rearm(&mut self, cx: &mut Context<Self>) {
    self.info = None;
    if self.on_pair_page() {
      self.open_pair(cx);
    }
  }

  pub(crate) fn scan(&mut self, cx: &mut Context<Self>) {
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

  // ---- actions -----------------------------------------------------------------

  pub(crate) fn connect(&mut self, target: String, cx: &mut Context<Self>) {
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
          this.rearm(cx);
        }
        this.refresh(cx);
        cx.notify();
      },
    );
  }

  /// Opens the live camera view; a scanned `aurora://pair` code starts pairing
  pub(crate) fn scan_qr(&mut self, cx: &mut Context<Self>) {
    self.error = None;
    self.bg(
      cx,
      |b| {
        let _ = b.cancel_pairing();
        b.scan_qr()
      },
      |this, res, cx| {
        match res {
          Ok(Some(link)) => this.connect(link, cx),
          Ok(None) => this.rearm(cx),
          Err(e) => {
            this.error = Some(format!("{e:#}").into());
            this.rearm(cx);
          }
        }
        cx.notify();
      },
    );
  }

  pub(crate) fn decide(&mut self, accept: bool, cx: &mut Context<Self>) {
    self.bg(cx, move |b| b.decide(accept), |_, _, _| {});
    self.refresh(cx);
  }

  pub(crate) fn unpair(&mut self, id: String, cx: &mut Context<Self>) {
    self.bg(cx, move |b| b.unpair(&id), |_, _, _| {});
  }

  pub(crate) fn set_discovery(&mut self, on: bool, cx: &mut Context<Self>) {
    if let Some(s) = &mut self.settings {
      s.discovery = on;
    }
    self.bg(
      cx,
      move |b| b.set_discovery(on),
      |this, _, cx| this.refresh(cx),
    );
    cx.notify();
  }

  pub(crate) fn selected_device(&self) -> Option<&DeviceStatus> {
    self
      .devices
      .iter()
      .find(|d| Some(&d.device.id) == self.selected.as_ref())
  }
}

impl Render for AppView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    // the confirm code is shown in a read-only OTP field
    if let PairState::Confirm(c) = &self.pair
      && self.otp.read(cx).value().as_ref() != c.code
    {
      let code = c.code.clone();
      self.otp.update(cx, |s, cx| s.set_value(code, window, cx));
    }
    let page = match self.platform {
      Platform::Desktop => self.desktop(cx),
      Platform::Mobile => self.mobile(cx),
    };
    page
      .size_full()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
  }
}

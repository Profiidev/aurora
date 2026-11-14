use std::{sync::Arc, time::Duration};

use aurora_core::types::{AppSettings, DeviceStatus, Event, Nearby, PairInfo, PairState};
use gpui::{AppContext, Context, Entity, IntoElement, Render, SharedString, Styled, Window};
use gpui_kit::component::{ActiveTheme, WindowExt, input::InputState, notification::Notification};

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

pub struct AppView {
  pub(crate) backend: Arc<dyn Backend>,
  pub(crate) platform: Platform,
  pub(crate) page: Page,
  pub(crate) devices: Vec<DeviceStatus>,
  pub(crate) selected: Option<String>,
  pub(crate) pair: PairState,
  /// The waiting session shown as QR code, and when it ends
  pub(crate) info: Option<PairInfo>,
  pub(crate) nearby: Vec<Nearby>,
  pub(crate) scanning: bool,
  /// Shown on the pair page after a failed or expired session
  pub(crate) error: Option<SharedString>,
  /// Key of the button whose action is still running; it shows a spinner
  pub(crate) busy: Option<SharedString>,
  /// The daemon cannot be reached
  pub(crate) offline: bool,
  pub(crate) settings: Option<AppSettings>,
  pub(crate) address: Entity<InputState>,
  /// Relay server field of the settings, not wired up yet
  pub(crate) relay: Entity<InputState>,
  /// Mobile: the address field behind "Connect by address or USB"
  pub(crate) show_address: bool,
  /// The first device list decides where the app opens
  loaded: bool,
  tick: u64,
}

impl AppView {
  pub fn new(
    backend: Arc<dyn Backend>,
    platform: Platform,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let address =
      cx.new(|cx| InputState::new(window, cx).placeholder("aurora:// link or device ID"));
    let relay = cx.new(|cx| InputState::new(window, cx).placeholder("https://relay.example.com"));

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

    // keeps the nearby list fresh
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
      devices: Vec::new(),
      selected: None,
      pair: PairState::Idle,
      info: None,
      nearby: Vec::new(),
      scanning: false,
      error: None,
      busy: None,
      offline: false,
      settings: None,
      address,
      relay,
      show_address: false,
      loaded: false,
      tick: 0,
    };
    this.refresh(cx);
    // debug builds: open a page directly, for screenshots without input devices
    #[cfg(debug_assertions)]
    if let Ok(page) = std::env::var("AURORA_PAGE") {
      this.loaded = true;
      match page.as_str() {
        "pair" => this.go(Page::Pair, cx),
        "settings" => this.go(Page::Settings, cx),
        _ => {}
      }
    }
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
          // the daemon lost our waiting session (it restarted): the QR shown is dead.
          // Not while a button runs: our own dial closed the session on purpose
          if this.on_pair_page()
            && this.info.is_some()
            && this.busy.is_none()
            && matches!(state, PairState::Idle)
          {
            this.rearm(cx);
          }
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
        // nothing paired: open on pairing, like a first run
        let first = !this.loaded && !this.offline;
        this.loaded |= first;
        // mobile shows only the pair screen then, so its page must match
        let mobile = this.platform == Platform::Mobile;
        if this.devices.is_empty() && !this.offline && (first || mobile || this.page == Page::Home)
        {
          this.go(Page::Pair, cx);
        }
        cx.notify();
      },
    );
  }

  pub(crate) fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
    if matches!(
      event,
      Event::CodeReady(_)
        | Event::Paired(_)
        | Event::PairingFailed(_)
        | Event::PairingExpired
        | Event::Unpaired(_)
    ) {
      self.busy = None;
    }
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
      // a busy dial cancelled our waiting session itself; rearming would race it
      Event::PairingCancelled if self.busy.is_none() => self.rearm(cx),
      Event::Unpaired(id) if self.selected.as_ref() == Some(id) => self.selected = None,
      _ => {}
    }
    self.refresh(cx);
    cx.notify();
  }

  fn on_tick(&mut self, cx: &mut Context<Self>) {
    self.tick += 1;
    if self.on_pair_page() && self.tick.is_multiple_of(5) && !self.scanning {
      self.scan(cx);
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
      self.busy = None;
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
      // a session that is already waiting (another window, a restart) is shown as is
      |b| match b.start_pairing() {
        Ok(info) => Some(info),
        Err(_) => match b.pair_state() {
          Ok(PairState::Waiting(info)) => Some(info),
          _ => None,
        },
      },
      |this, info, cx| {
        if info.is_some() {
          this.info = info;
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

  /// Buttons without an icon also need `.icon(Spinner)`: gpui-kit draws the
  /// loading spinner in place of the icon
  pub(crate) fn loading(&self, key: &str) -> bool {
    self.busy.as_deref() == Some(key)
  }

  /// `key` names the button that shows a spinner until the dial ends
  pub(crate) fn connect(
    &mut self,
    target: String,
    key: impl Into<SharedString>,
    cx: &mut Context<Self>,
  ) {
    self.error = None;
    self.busy = Some(key.into());
    cx.notify();
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
          this.busy = None;
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
          Ok(Some(link)) => this.connect(link, "scan", cx),
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
    self.busy = Some(if accept { "pair" } else { "mismatch" }.into());
    cx.notify();
    self.bg(
      cx,
      move |b| b.decide(accept),
      |this, res, cx| {
        if let Err(e) = res {
          this.error = Some(format!("{e:#}").into());
          this.busy = None;
        }
        // shows the waiting state once our answer is in
        this.refresh(cx);
      },
    );
  }

  pub(crate) fn unpair(&mut self, id: String, cx: &mut Context<Self>) {
    self.busy = Some("unpair".into());
    cx.notify();
    self.bg(
      cx,
      move |b| b.unpair(&id),
      |this, res, cx| {
        if let Err(e) = res {
          this.error = Some(format!("{e:#}").into());
          this.busy = None;
          cx.notify();
        }
      },
    );
  }

  /// Whether USB is on; `None` where the platform has no USB connection
  pub(crate) fn usb(&self) -> Option<bool> {
    self.settings.as_ref().and_then(|s| s.usb)
  }

  pub(crate) fn set_usb(&mut self, on: bool, cx: &mut Context<Self>) {
    if let Some(s) = &mut self.settings {
      s.usb = Some(on);
    }
    self.bg(cx, move |b| b.set_usb(on), |this, _, cx| this.refresh(cx));
    cx.notify();
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
    // async results have no window, so errors queue here and become toasts
    if let Some(e) = self.error.take() {
      cx.defer_in(window, move |_, window, cx| {
        window.push_notification(Notification::error(e), cx)
      });
    }
    // a device dialed in: the code screen must not sit under the QR dialog
    if matches!(self.pair, PairState::Confirm(_)) {
      cx.defer_in(window, |_, window, cx| {
        if window.has_active_dialog(cx) {
          window.close_all_dialogs(cx);
        }
      });
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

/// The second line of a nearby device: its OS and how it was found
pub(crate) fn nearby_detail(n: &Nearby) -> SharedString {
  match (n.usb, n.addr) {
    (true, _) => format!("{} · USB", n.os).into(),
    (false, Some(a)) => format!("{} · {}", n.os, a.ip()).into(),
    (false, None) => n.os.clone().into(),
  }
}

#[cfg(test)]
mod tests {
  use gpui::TestAppContext;

  use super::*;
  use crate::test_support::*;

  fn count(fake: &FakeBackend, call: &str) -> usize {
    fake.calls().iter().filter(|c| *c == call).count()
  }

  #[gpui::test]
  fn first_run_opens_the_pair_page(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.read_entity(&app, |a, _| {
      assert!(a.page == Page::Pair);
      assert_eq!(a.info, Some(info()));
    });
    assert_eq!(count(&fake, "start_pairing"), 1);
  }

  #[gpui::test]
  fn a_paired_device_opens_its_overview(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![device("a", "home")]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.read_entity(&app, |a, _| {
      assert!(a.page == Page::Home);
      assert_eq!(a.selected.as_deref(), Some("a"));
    });
    assert_eq!(count(&fake, "start_pairing"), 0);
  }

  #[gpui::test]
  fn the_phone_returns_to_pairing_once_nothing_is_paired(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![device("a", "home")]);
    let (app, vcx) = open_app(cx, Platform::Mobile, fake.clone());
    vcx.update_entity(&app, |a, cx| a.go(Page::Settings, cx));
    fake.set(|s| s.devices.clear());
    vcx.update_entity(&app, |a, cx| a.refresh(cx));
    draw(vcx);
    vcx.read_entity(&app, |a, _| assert!(a.page == Page::Pair));
    assert_eq!(count(&fake, "start_pairing"), 1, "pairing session opened");
  }

  #[gpui::test]
  fn a_lost_session_gets_a_fresh_qr(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    // the daemon restarted: our QR is dead
    fake.set(|s| s.pair = PairState::Idle);
    vcx.update_entity(&app, |a, cx| a.refresh(cx));
    draw(vcx);
    assert_eq!(count(&fake, "start_pairing"), 2);
  }

  #[gpui::test]
  fn leaving_the_pair_page_ends_the_session_except_while_confirming(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.update_entity(&app, |a, cx| {
      a.busy = Some("x".into());
      a.go(Page::Settings, cx);
      assert!(a.busy.is_none());
    });
    draw(vcx);
    assert_eq!(count(&fake, "cancel_pairing"), 1);

    vcx.update_entity(&app, |a, cx| a.go(Page::Pair, cx));
    fake.set(|s| s.pair = PairState::Confirm(confirm(false)));
    vcx.update_entity(&app, |a, cx| a.refresh(cx));
    draw(vcx);
    vcx.update_entity(&app, |a, cx| a.go(Page::Settings, cx));
    draw(vcx);
    assert_eq!(count(&fake, "cancel_pairing"), 1, "kept the code on screen");
  }

  #[gpui::test]
  fn a_failed_pairing_ends_the_spinner_and_rearms(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    fake.set(|s| s.pair = PairState::Idle);
    vcx.update_entity(&app, |a, cx| {
      a.busy = Some("mismatch".into());
      a.on_event(Event::PairingFailed("rejected".into()), cx);
      assert!(a.busy.is_none());
      assert_eq!(a.error.as_deref(), Some("Pairing failed: rejected"));
    });
    draw(vcx);
    vcx.read_entity(&app, |a, _| assert!(a.error.is_none(), "shown as a toast"));
    assert_eq!(count(&fake, "start_pairing"), 2);
  }

  #[gpui::test]
  fn a_cancel_from_our_own_dial_does_not_rearm(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    fake.set(|s| s.pair = PairState::Idle);
    vcx.update_entity(&app, |a, cx| {
      a.busy = Some("by-address".into());
      a.on_event(Event::PairingCancelled, cx);
    });
    draw(vcx);
    assert_eq!(count(&fake, "start_pairing"), 1);
  }

  #[gpui::test]
  fn connect_spins_until_the_code_or_an_error(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.update_entity(&app, |a, cx| a.connect("abc".into(), "by-address", cx));
    vcx.run_until_parked();
    vcx.read_entity(&app, |a, _| assert!(a.loading("by-address")));
    assert!(fake.calls().contains(&"connect abc".to_string()));
    vcx.update_entity(&app, |a, cx| {
      a.on_event(Event::CodeReady(confirm(false)), cx)
    });
    vcx.read_entity(&app, |a, _| assert!(a.busy.is_none()));

    fake.set(|s| {
      s.pair = PairState::Idle;
      s.connect_error = Some("did not answer".into());
    });
    vcx.update_entity(&app, |a, cx| a.connect("abc".into(), "by-address", cx));
    vcx.run_until_parked();
    // the error became a toast with the frame that ran meanwhile
    vcx.read_entity(&app, |a, _| assert!(a.busy.is_none()));
  }

  #[gpui::test]
  fn decide_and_unpair_spin_until_the_engine_answers(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![device("a", "home")]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.update_entity(&app, |a, cx| a.decide(true, cx));
    vcx.run_until_parked();
    vcx.read_entity(&app, |a, _| assert!(a.loading("pair")));
    vcx.update_entity(&app, |a, cx| {
      a.on_event(Event::Paired(device("a", "home").device), cx)
    });
    vcx.read_entity(&app, |a, _| assert!(a.busy.is_none()));

    vcx.update_entity(&app, |a, cx| a.unpair("a".into(), cx));
    vcx.run_until_parked();
    assert!(fake.calls().contains(&"unpair a".to_string()));
    vcx.update_entity(&app, |a, cx| {
      assert!(a.loading("unpair"));
      a.on_event(Event::Unpaired("a".into()), cx);
      assert!(a.busy.is_none());
    });
  }

  #[gpui::test]
  fn usb_changes_reach_the_backend(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.run_until_parked();
    vcx.update_entity(&app, |a, cx| a.set_usb(false, cx));
    vcx.run_until_parked();
    assert!(fake.calls().contains(&"set_usb false".to_string()));
    vcx.read_entity(&app, |a, _| assert_eq!(a.usb(), Some(false)));
  }

  #[test]
  fn nearby_detail_names_usb_or_address() {
    let mut n = crate::test_support::nearby("x");
    assert_eq!(super::nearby_detail(&n).as_ref(), "Linux · 10.0.0.3");
    n.usb = true;
    assert_eq!(super::nearby_detail(&n).as_ref(), "Linux · USB");
    (n.usb, n.addr) = (false, None);
    assert_eq!(super::nearby_detail(&n).as_ref(), "Linux");
  }

  #[gpui::test]
  fn discovery_changes_reach_the_backend(cx: &mut TestAppContext) {
    let fake = FakeBackend::new(vec![]);
    let (app, vcx) = open_app(cx, Platform::Desktop, fake.clone());
    vcx.update_entity(&app, |a, cx| a.set_discovery(true, cx));
    vcx.run_until_parked();
    assert!(fake.calls().contains(&"set_discovery true".to_string()));
    vcx.read_entity(&app, |a, _| {
      assert!(a.settings.as_ref().unwrap().discovery);
    });
  }
}

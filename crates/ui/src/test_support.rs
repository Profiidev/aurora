//! A scripted [`Backend`] and a headless app window for the view tests.

use std::{
  cell::RefCell,
  rc::Rc,
  sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use aurora_core::{
  pairing::Target,
  types::{AppSettings, Confirm, Device, DeviceStatus, Event, Nearby, PairInfo, PairState},
};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use gpui_kit::component::Root;

use crate::{Backend, Platform, app::AppView};

pub struct State {
  pub devices: Vec<DeviceStatus>,
  pub pair: PairState,
  pub nearby: Vec<Nearby>,
  pub settings: AppSettings,
  /// Every backend call, in order
  pub calls: Vec<String>,
  /// The next `connect` fails with this
  pub connect_error: Option<String>,
}

/// In-memory backend; tests inject events through `AppView::on_event`
pub struct FakeBackend(pub Mutex<State>);

impl FakeBackend {
  pub fn new(devices: Vec<DeviceStatus>) -> Arc<Self> {
    Arc::new(Self(Mutex::new(State {
      devices,
      pair: PairState::Idle,
      nearby: Vec::new(),
      settings: AppSettings {
        name: "test".into(),
        discovery: false,
        usb: Some(true),
        bluetooth: Some(true),
      },
      calls: Vec::new(),
      connect_error: None,
    })))
  }

  pub fn calls(&self) -> Vec<String> {
    self.0.lock().unwrap().calls.clone()
  }

  pub fn set(&self, edit: impl FnOnce(&mut State)) {
    edit(&mut self.0.lock().unwrap());
  }

  fn log(&self, call: impl Into<String>) -> std::sync::MutexGuard<'_, State> {
    let mut s = self.0.lock().unwrap();
    s.calls.push(call.into());
    s
  }
}

pub fn info() -> PairInfo {
  PairInfo {
    link: Target {
      id: [7; 32],
      relay: None,
      addrs: vec![],
    }
    .link(),
    name: "test".into(),
    addrs: vec![],
  }
}

pub fn device(id: &str, name: &str) -> DeviceStatus {
  DeviceStatus {
    device: Device {
      id: id.into(),
      name: name.into(),
      os: "Linux".into(),
      paired_at: 1,
      relay: None,
      addrs: vec![],
    },
    connected: true,
    battery: None,
    charging: false,
    via: Some("USB".into()),
  }
}

pub fn confirm(local_accepted: bool) -> Confirm {
  Confirm {
    code: "123456".into(),
    peer_name: "home".into(),
    peer_os: "Linux".into(),
    via: "Direct · 10.0.0.2".into(),
    fingerprint: "088a ee1e 84c3 db97".into(),
    local_accepted,
  }
}

pub fn nearby(name: &str) -> Nearby {
  Nearby {
    name: name.into(),
    os: "Linux".into(),
    addr: Some("10.0.0.3:4000".parse().unwrap()),
    link: info().link,
    usb: false,
    bluetooth: false,
  }
}

impl Backend for FakeBackend {
  fn devices(&self) -> Result<Vec<DeviceStatus>> {
    Ok(self.0.lock().unwrap().devices.clone())
  }
  fn pair_state(&self) -> Result<PairState> {
    Ok(self.0.lock().unwrap().pair.clone())
  }
  fn start_pairing(&self) -> Result<PairInfo> {
    let mut s = self.log("start_pairing");
    if s.pair != PairState::Idle {
      bail!("pairing in progress");
    }
    s.pair = PairState::Waiting(info());
    Ok(info())
  }
  fn cancel_pairing(&self) -> Result<()> {
    self.log("cancel_pairing").pair = PairState::Idle;
    Ok(())
  }
  fn connect(&self, target: &str) -> Result<()> {
    let mut s = self.log(format!("connect {target}"));
    if let Some(e) = s.connect_error.take() {
      bail!(e);
    }
    s.pair = PairState::Dialing;
    Ok(())
  }
  fn decide(&self, accept: bool) -> Result<()> {
    drop(self.log(format!("decide {accept}")));
    Ok(())
  }
  fn nearby(&self) -> Result<Vec<Nearby>> {
    Ok(self.0.lock().unwrap().nearby.clone())
  }
  fn unpair(&self, id: &str) -> Result<()> {
    self
      .log(format!("unpair {id}"))
      .devices
      .retain(|d| d.device.id != id);
    Ok(())
  }
  fn settings(&self) -> Result<AppSettings> {
    Ok(self.0.lock().unwrap().settings.clone())
  }
  fn set_discovery(&self, on: bool) -> Result<()> {
    self.log(format!("set_discovery {on}")).settings.discovery = on;
    Ok(())
  }
  fn set_usb(&self, on: bool) -> Result<()> {
    self.log(format!("set_usb {on}")).settings.usb = Some(on);
    Ok(())
  }
  fn set_bluetooth(&self, on: bool) -> Result<()> {
    self.log(format!("set_bluetooth {on}")).settings.bluetooth = Some(on);
    Ok(())
  }
  fn events(&self) -> Box<dyn Iterator<Item = Event> + Send> {
    Box::new(std::iter::empty())
  }
}

/// The app in a headless window, inside the kit's Root like `open_root`
pub fn open_app(
  cx: &mut TestAppContext,
  platform: Platform,
  backend: Arc<FakeBackend>,
) -> (Entity<AppView>, &mut VisualTestContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    crate::apply_theme(cx);
  });
  let slot = Rc::new(RefCell::new(None));
  let view = slot.clone();
  let (_, vcx) = cx.add_window_view(move |window, cx| {
    let content = cx.new(|cx| AppView::new(backend, platform, window, cx));
    *view.borrow_mut() = Some(content.clone());
    Root::new(content, window, cx)
  });
  vcx.run_until_parked();
  let app = slot.borrow_mut().take().unwrap();
  draw(vcx);
  (app, vcx)
}

/// Settles pending work and completes a frame
pub fn draw(vcx: &mut VisualTestContext) {
  vcx.run_until_parked();
  vcx.update(|window, cx| {
    window.refresh();
    window.draw(cx).clear(cx);
  });
}

/// Clicks the middle of the element tagged `debug_selector(id)`, then settles
pub fn click(vcx: &mut VisualTestContext, id: &'static str) {
  let bounds = vcx
    .debug_bounds(id)
    .unwrap_or_else(|| panic!("{id} not on screen"));
  vcx.simulate_click(bounds.center(), Modifiers::none());
  draw(vcx);
}

/// Whether the element tagged `id` was drawn in the last frame
pub fn shown(vcx: &mut VisualTestContext, id: &'static str) -> bool {
  vcx.debug_bounds(id).is_some()
}

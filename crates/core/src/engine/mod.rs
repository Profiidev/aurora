//! The pairing and session engine, shared by `aurorad` and the mobile app.
//!
//! - [`session`]: the single pairing session and its state machine
//! - [`pair_flow`]: the pairing protocol on the dialing and the waiting side
//! - [`links`]: connections to paired devices, kept alive and reconnected
//! - [`nearby`]: unpaired devices found by mDNS, cable or Bluetooth
//! - [`bluetooth`]: when the Bluetooth fallback runs
//!
//! What paired devices say to each other once connected lives in
//! [`crate::peer`].

mod bluetooth;
mod links;
mod nearby;
mod pair_flow;
mod session;

use std::{
  collections::{HashMap, HashSet},
  path::Path,
  sync::{Arc, Mutex, RwLock, atomic::AtomicUsize},
  time::Duration,
};

use anyhow::Result;
use aurora_proto::Id;
use aurora_transport::{Links, Node, load_or_create_key};
use tokio::sync::broadcast;
use tracing::{info, warn};

pub use self::bluetooth::PresenceGuard;
use self::{links::Live, session::Session};
use crate::{
  pairing::hex,
  peer::{PeerRegistry, messages},
  store::Store,
  types::{DeviceStatus, Event, Nearby, PairState},
};

#[derive(Debug, Clone)]
pub struct Settings {
  pub name: String,
  pub os: String,
  pub pairing_timeout: Duration,
  /// mDNS on the local network: be found, find others, tell them our name
  pub discovery: bool,
  /// Fixed QUIC port, read once when the engine starts
  pub port: Option<u16>,
  /// n0's DNS lookup and relays, read once when the engine starts
  pub relay: bool,
  /// Connect to phones over USB (desktop: probe devices; phone: open the
  /// accessory)
  pub usb: bool,
  /// Bluetooth LE fallback (desktop: advertise; phone: scan and dial)
  pub bluetooth: bool,
}

pub struct Engine {
  node: Node,
  settings: RwLock<Settings>,
  store: Mutex<Store>,
  session: Mutex<Session>,
  live: Mutex<HashMap<String, Live>>,
  /// Devices being dialed, so the reconnect loop does not dial twice
  trying: Mutex<HashSet<String>>,
  nearby: Mutex<HashMap<Id, Nearby>>,
  local: Mutex<(Option<u8>, bool)>,
  events: broadcast::Sender<Event>,
  peers: PeerRegistry,
  /// Open app windows (desktop) or a foreground app (phone)
  ui: Arc<AtomicUsize>,
}

impl Engine {
  /// Opens the store and key in `dir`, binds the node and starts the accept,
  /// reconnect and discovery loops
  pub async fn start(settings: Settings, dir: &Path) -> Result<Arc<Self>> {
    Self::start_with(settings, dir, messages::register_all).await
  }

  /// [`Engine::start`] with a custom set of peer messages
  pub async fn start_with(
    settings: Settings,
    dir: &Path,
    register: impl FnOnce(&mut PeerRegistry),
  ) -> Result<Arc<Self>> {
    let key = load_or_create_key(&dir.join("key"))?;
    let node = Node::bind(key, settings.port, settings.relay).await?;
    if let Err(e) = node.set_mdns(settings.discovery) {
      warn!("{e:#}");
    }
    node.set_usb(settings.usb);
    let mut peers = PeerRegistry::default();
    register(&mut peers);
    let engine = Arc::new(Self {
      node,
      settings: RwLock::new(settings),
      store: Mutex::new(Store::open(dir)?),
      session: Mutex::new(Session::default()),
      live: Default::default(),
      trying: Default::default(),
      nearby: Default::default(),
      local: Default::default(),
      events: broadcast::channel(64).0,
      peers,
      ui: Arc::default(),
    });
    info!(
      "endpoint {} on port {}",
      hex(&engine.node.id()),
      engine.node.port()
    );
    tokio::spawn(engine.clone().accept_loop());
    tokio::spawn(engine.clone().reconnect_loop());
    tokio::spawn(engine.clone().link_loop());
    tokio::spawn(engine.clone().nearby_loop());
    tokio::spawn(engine.clone().bluetooth_loop());
    Ok(engine)
  }

  pub fn events(&self) -> broadcast::Receiver<Event> {
    self.events.subscribe()
  }

  pub(crate) fn emit(&self, event: Event) {
    let _ = self.events.send(event);
  }

  pub fn set_settings(&self, settings: Settings) {
    let old = self.settings();
    if settings.discovery != old.discovery {
      if let Err(e) = self.node.set_mdns(settings.discovery) {
        warn!("{e:#}");
      }
      self.nearby.lock().unwrap().clear();
    }
    if settings.usb != old.usb {
      self.node.set_usb(settings.usb);
    }
    *self.settings.write().unwrap() = settings;
    self.emit(Event::ConfigChanged);
  }

  pub fn settings(&self) -> Settings {
    self.settings.read().unwrap().clone()
  }

  /// Battery of this device, sent to peers in `Hello` and as
  /// [`messages::BatteryStatus`] when it changes
  pub fn set_local_status(self: &Arc<Self>, battery: Option<u8>, charging: bool) {
    *self.local.lock().unwrap() = (battery, charging);
    let ids: Vec<String> = self.live.lock().unwrap().keys().cloned().collect();
    for id in ids {
      let engine = self.clone();
      tokio::spawn(async move {
        let status = messages::Battery { battery, charging };
        engine.notify::<messages::BatteryStatus>(&id, &status).await
      });
    }
  }

  pub fn devices(&self) -> Vec<DeviceStatus> {
    let live = self.live.lock().unwrap();
    (self.store.lock().unwrap().devices().iter())
      .map(|d| match live.get(&d.id) {
        Some(l) => DeviceStatus {
          device: d.clone(),
          ..l.snapshot()
        },
        None => DeviceStatus {
          device: d.clone(),
          connected: false,
          battery: None,
          charging: false,
          via: None,
        },
      })
      .collect()
  }

  pub fn pair_state(&self) -> PairState {
    self.session.lock().unwrap().state()
  }

  pub(crate) fn peers(&self) -> &PeerRegistry {
    &self.peers
  }

  /// Where the phone app attaches its USB accessory
  pub fn links(&self) -> &Links {
    self.node.links()
  }

  /// Ends the session, tells peers we are going and closes the endpoints so
  /// iroh does not abort on drop
  pub async fn shutdown(&self) {
    self.cancel_pairing();
    let conns: Vec<_> = (self.live.lock().unwrap().drain())
      .flat_map(|(_, l)| l.conns())
      .collect();
    for conn in conns {
      conn.close(0u32.into(), b"shutdown");
    }
    self.node.close().await;
  }

  pub fn id(&self) -> Id {
    self.node.id()
  }

  pub fn port(&self) -> u16 {
    self.node.port()
  }
}

#[cfg(test)]
pub(crate) mod tests {
  use super::*;

  pub(crate) fn settings(name: &str, discovery: bool) -> Settings {
    Settings {
      name: name.into(),
      os: "Linux".into(),
      pairing_timeout: Duration::from_secs(60),
      discovery,
      port: None,
      relay: false,
      usb: false,
      bluetooth: false,
    }
  }

  pub(crate) async fn engine(name: &str, discovery: bool) -> (Arc<Engine>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (
      Engine::start(settings(name, discovery), dir.path())
        .await
        .unwrap(),
      dir,
    )
  }
}

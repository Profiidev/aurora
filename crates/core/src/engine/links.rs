//! Connections to paired devices. A device can be reached over USB, the
//! network and Bluetooth at once; each connection is kept and messages take
//! the best one, so pulling the cable loses nothing. Bluetooth is a fallback
//! and is dropped once a better path exists (see `engine::bluetooth`).

use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow, bail};
use aurora_proto::{Id, Msg, read_msg, write_msg};
use aurora_transport::{Connection, Hints, LinkEvent, LinkKind, SendStream, Via, remote_id, via};
use tokio::{
  sync::broadcast::error::RecvError,
  time::{sleep, timeout},
};
use tracing::debug;

use super::{Engine, pair_flow::Hello};
use crate::{
  pairing::{hex, unhex},
  peer::{self, PeerCtx, messages},
  types::{Device, DeviceStatus, Event},
};

const RECONNECT_EVERY: Duration = Duration::from_secs(10);
/// Long enough for a lookup through n0's DNS and a relay
pub(super) const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// How a connection reaches the device, best first
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Path {
  Usb,
  Net,
  Bluetooth,
}

impl Path {
  pub(super) fn of(conn: &Connection) -> Self {
    match via(conn) {
      Via::Link(LinkKind::Usb) => Path::Usb,
      Via::Link(LinkKind::Bluetooth) => Path::Bluetooth,
      Via::Direct(_) | Via::Relay => Path::Net,
    }
  }

  fn link(self) -> Option<LinkKind> {
    match self {
      Path::Usb => Some(LinkKind::Usb),
      Path::Bluetooth => Some(LinkKind::Bluetooth),
      Path::Net => None,
    }
  }
}

/// The connections to one paired device, one per [`Path`]
pub(super) struct Live {
  /// Indexed by `Path as usize`, so iteration goes best first
  conns: [Option<Connection>; 3],
  pub(super) status: DeviceStatus,
}

impl Live {
  pub(super) fn conns(&self) -> impl Iterator<Item = Connection> + use<> {
    self.conns.clone().into_iter().flatten()
  }

  pub(super) fn has(&self, path: Path) -> bool {
    self.conns[path as usize].is_some()
  }

  /// The status with the path messages take right now
  pub(super) fn snapshot(&self) -> DeviceStatus {
    DeviceStatus {
      via: self.conns().next().map(|c| via(&c).label().into()),
      ..self.status.clone()
    }
  }

  fn slot(&mut self, path: Path) -> &mut Option<Connection> {
    &mut self.conns[path as usize]
  }
}

/// The side with the lower endpoint id opened `conn`
fn dialed_by_lower(conn: &Connection, me: &Id) -> bool {
  let we_dialed = conn.side().is_client();
  we_dialed == (*me < remote_id(conn))
}

impl Engine {
  /// The connection to send to `device` on: the link one while it is up
  pub(crate) fn live_conn(&self, device: &str) -> Option<Connection> {
    self.live.lock().unwrap().get(device)?.conns().next()
  }

  /// Updates what `device` last reported about itself
  pub(crate) fn update_status(&self, device: &str, edit: impl FnOnce(&mut DeviceStatus)) {
    let status = {
      let mut live = self.live.lock().unwrap();
      let Some(l) = live.get_mut(device) else {
        return;
      };
      edit(&mut l.status);
      l.snapshot()
    };
    self.emit(Event::DeviceConnected(status));
  }

  /// Forgets `device` after it unpaired us
  pub(crate) fn forget(&self, device: &str) {
    let _ = self.store.lock().unwrap().remove(device);
    let live = self.live.lock().unwrap().remove(device);
    for conn in live.iter().flat_map(Live::conns) {
      conn.close(0u32.into(), b"unpaired");
    }
    self.emit(Event::Unpaired(device.into()));
  }

  pub async fn unpair(&self, id: &str) -> Result<()> {
    if !self.store.lock().unwrap().remove(id)? {
      bail!("unknown device");
    }
    let live = self.live.lock().unwrap().remove(id);
    if let Some(conn) = live.as_ref().and_then(|l| l.conns().next()) {
      let _ = peer::notify_on::<messages::Unpair>(&conn, &()).await;
    }
    for conn in live.iter().flat_map(Live::conns) {
      linger_close(&conn).await;
    }
    self.emit(Event::Unpaired(id.into()));
    Ok(())
  }

  // ---- incoming ------------------------------------------------------------

  pub(super) async fn accept_loop(self: Arc<Self>) {
    while let Some(conn) = self.node.accept().await {
      let conn = match conn {
        Ok(conn) => conn,
        Err(e) => {
          debug!("incoming connection failed: {e:#}");
          continue;
        }
      };
      let engine = self.clone();
      tokio::spawn(async move {
        if let Err(e) = engine.handle_incoming(conn).await {
          debug!("incoming connection ended: {e:#}");
        }
      });
    }
  }

  /// Routes a new connection by its first message
  async fn handle_incoming(self: &Arc<Self>, conn: Connection) -> Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    match read_msg(&mut recv).await? {
      Msg::PairHello {
        name,
        os,
        nonce,
        relay,
        addrs,
      } => {
        let hello = Hello {
          name,
          os,
          nonce,
          hints: Hints { relay, addrs },
        };
        self.accept_pair(&conn, send, recv, hello).await
      }
      Msg::Who => {
        let s = self.settings();
        // devices that find us by mDNS, or at the other end of our cable,
        // learn our name; Bluetooth range is no more consent than the LAN
        if s.discovery || Path::of(&conn) == Path::Usb {
          write_msg(
            &mut send,
            &Msg::Info {
              name: s.name,
              os: s.os,
            },
          )
          .await?;
        }
        linger(&conn, send).await;
        Ok(())
      }
      Msg::Hello {
        name,
        os,
        battery,
        charging,
      } => {
        let id = hex(&remote_id(&conn));
        if self.store.lock().unwrap().get(&id).is_none() {
          write_msg(&mut send, &Msg::Unpair).await?;
          send.finish()?;
          linger_close(&conn).await;
          return Ok(());
        }
        write_msg(&mut send, &self.hello()).await?;
        send.finish()?;
        self.run_live(id, conn, name, os, battery, charging).await;
        Ok(())
      }
      _ => bail!("unexpected first message"),
    }
  }

  // ---- outgoing ------------------------------------------------------------

  /// Dials paired devices that have no network connection
  pub(super) async fn reconnect_loop(self: Arc<Self>) {
    sleep(Duration::from_secs(1)).await;
    loop {
      let due: Vec<Device> = (self.store.lock().unwrap().devices().iter())
        .filter(|d| (self.live.lock().unwrap().get(&d.id)).is_none_or(|l| !l.has(Path::Net)))
        .cloned()
        .collect();
      for device in due {
        self.spawn_dial(device, false);
      }
      // links that came up before the pairing did, or whose dial failed
      for device in self.link_dials_due() {
        self.spawn_dial(device, true);
      }
      sleep(RECONNECT_EVERY).await;
    }
  }

  /// Dials a paired device over a link as soon as it comes up. Only the side
  /// with the lower id dials, so the two never race each other.
  pub(super) async fn link_loop(self: Arc<Self>) {
    let mut events = self.node.links().events();
    loop {
      let peer = match events.recv().await {
        Ok(LinkEvent::Up { peer, .. }) => peer,
        Ok(LinkEvent::Down { .. }) | Err(RecvError::Lagged(_)) => continue,
        Err(RecvError::Closed) => break,
      };
      if self.node.id() > peer {
        continue;
      }
      let device = self.store.lock().unwrap().get(&hex(&peer)).cloned();
      if let Some(device) = device {
        self.spawn_dial(device, true);
      }
    }
  }

  /// A device just paired over a cable gets its link connection right away
  pub(super) fn paired_over(self: &Arc<Self>, res: &Result<Option<Device>>) {
    if let Ok(Some(device)) = res
      && self.link_dials_due().iter().any(|d| d.id == device.id)
    {
      self.spawn_dial(device.clone(), true);
    }
  }

  /// Paired devices at the other end of a link without a link connection,
  /// where we are the side that dials
  fn link_dials_due(&self) -> Vec<Device> {
    let links = self.node.links();
    (self.store.lock().unwrap().devices().iter())
      .filter(|d| unhex(&d.id).is_ok_and(|p| self.node.id() < p && links.is_linked(&p)))
      .filter(|d| {
        (self.live.lock().unwrap().get(&d.id))
          .is_none_or(|l| !l.has(Path::Usb) && !l.has(Path::Bluetooth))
      })
      .cloned()
      .collect()
  }

  fn spawn_dial(self: &Arc<Self>, device: Device, link: bool) {
    let key = format!("{}{}", if link { "link:" } else { "" }, device.id);
    if !self.trying.lock().unwrap().insert(key.clone()) {
      return;
    }
    let engine = self.clone();
    tokio::spawn(async move {
      if let Err(e) = engine.dial_known(&device, link).await {
        debug!("cannot reach {}: {e:#}", device.name);
      }
      engine.trying.lock().unwrap().remove(&key);
    });
  }

  async fn dial_known(self: &Arc<Self>, device: &Device, link: bool) -> Result<()> {
    let id: Id = unhex(&device.id)?;
    let dial = async {
      if link {
        self.node.connect_link(id).await
      } else {
        let hints = Hints {
          relay: device.relay.clone(),
          addrs: device.addrs.clone(),
        };
        self.node.connect(id, &hints).await
      }
    };
    let conn = timeout(DIAL_TIMEOUT, dial)
      .await
      .map_err(|_| anyhow!("timed out"))??;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(&mut send, &self.hello()).await?;
    send.finish()?;
    match read_msg(&mut recv).await? {
      Msg::Hello {
        name,
        os,
        battery,
        charging,
      } => {
        self
          .run_live(device.id.clone(), conn, name, os, battery, charging)
          .await;
        Ok(())
      }
      Msg::Unpair => {
        // the other side forgot us while we were away
        self.store.lock().unwrap().remove(&device.id)?;
        self.emit(Event::Unpaired(device.id.clone()));
        Ok(())
      }
      _ => bail!("unexpected reply"),
    }
  }

  pub(super) fn hello(&self) -> Msg {
    let s = self.settings();
    let (battery, charging) = *self.local.lock().unwrap();
    Msg::Hello {
      name: s.name,
      os: s.os,
      battery,
      charging,
    }
  }

  // ---- a live connection ----------------------------------------------------

  /// Serves a connection to a paired device until it drops
  async fn run_live(
    self: &Arc<Self>,
    id: String,
    conn: Connection,
    name: String,
    os: String,
    battery: Option<u8>,
    charging: bool,
  ) {
    let Some(mut device) = self.store.lock().unwrap().get(&id).cloned() else {
      return;
    };
    let path = Path::of(&conn);
    // remember the direct address that works now, for the next reconnect
    if let Via::Direct(a) = via(&conn) {
      let mut store = self.store.lock().unwrap();
      let _ = store.set_hints(&id, device.relay.clone(), vec![a]);
    }
    (device.name, device.os) = (name, os);
    let status = DeviceStatus {
      device,
      connected: true,
      battery,
      charging,
      via: None,
    };
    let kept = {
      let mut live = self.live.lock().unwrap();
      let entry = live.entry(id.clone()).or_insert_with(|| Live {
        conns: Default::default(),
        status: status.clone(),
      });
      let me = self.node.id();
      let slot = entry.slot(path);
      // both sides dialed: each keeps the one the lower id dialed, so they
      // agree on which one survives
      let replaces = match slot {
        Some(old) if !dialed_by_lower(&conn, &me) || dialed_by_lower(old, &me) => None,
        old => Some(old.replace(conn.clone())),
      };
      match replaces {
        None => {
          close_later(conn.clone());
          None
        }
        Some(old) => {
          if let Some(old) = old {
            close_later(old);
          }
          entry.status = status;
          Some(entry.snapshot())
        }
      }
    };
    if let Some(status) = kept {
      self.emit(Event::DeviceConnected(status));
    }

    let peer = remote_id(&conn);
    let links = self.node.links().clone();
    // QUIC only notices a pulled cable at its idle timeout
    let unplugged = async {
      match path.link() {
        Some(kind) => links.gone_by(kind, &peer).await,
        None => std::future::pending().await,
      }
    };
    tokio::pin!(unplugged);
    let ctx = PeerCtx {
      engine: self.clone(),
      device: id.clone(),
      conn: conn.clone(),
    };
    loop {
      tokio::select! {
        _ = conn.closed() => break,
        () = &mut unplugged => break,
        stream = conn.accept_uni() => {
          let Ok(recv) = stream else { break };
          tokio::spawn(peer::dispatch_uni(ctx.clone(), recv));
        }
        stream = conn.accept_bi() => {
          let Ok((send, recv)) = stream else { break };
          tokio::spawn(peer::dispatch_bi(ctx.clone(), send, recv));
        }
      }
    }

    // `Some(status)`: still reachable the other way, which the UI should show
    let left = {
      let mut live = self.live.lock().unwrap();
      let ours = |l: &Live| {
        let slot = &l.conns[path as usize];
        slot.as_ref().map(Connection::stable_id) == Some(conn.stable_id())
      };
      match live.get_mut(&id) {
        Some(l) if ours(l) => {
          *l.slot(path) = None;
          if l.conns().next().is_none() {
            live.remove(&id);
            Some(None)
          } else {
            Some(Some(l.snapshot()))
          }
        }
        _ => None,
      }
    };
    match left {
      Some(None) => self.emit(Event::DeviceDisconnected(id)),
      Some(Some(status)) => self.emit(Event::DeviceConnected(status)),
      None => {}
    }
    conn.close(0u32.into(), b"bye");
  }
}

/// Closes a duplicate connection once streams the peer already sent on it
/// are served; it keeps serving until then
fn close_later(conn: Connection) {
  tokio::spawn(async move {
    sleep(Duration::from_secs(2)).await;
    conn.close(0u32.into(), b"duplicate");
  });
}

/// Gives the peer time to read our last message before the connection goes
pub(super) async fn linger(conn: &Connection, mut send: SendStream) {
  let _ = send.finish();
  linger_close(conn).await;
}

pub(super) async fn linger_close(conn: &Connection) {
  let _ = timeout(Duration::from_secs(2), conn.closed()).await;
  conn.close(0u32.into(), b"done");
}

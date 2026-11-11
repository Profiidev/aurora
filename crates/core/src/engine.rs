//! The pairing and session engine, shared by `aurorad` and the mobile app.
//!
//! It owns the iroh node and the device store, runs the pairing state machine
//! (one session at a time) and keeps connections to paired devices alive.

use std::{
  collections::{HashMap, HashSet},
  net::SocketAddr,
  path::Path,
  sync::{Arc, Mutex, RwLock},
  time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use aurora_proto::{Id, Msg, read_msg, write_msg};
use aurora_transport::{
  Connection, Hints, Node, RecvStream, Seen, SendStream, Via, load_or_create_key, random_bytes,
  remote_id, via,
};
use tokio::{
  sync::{broadcast, oneshot, watch},
  time::{sleep, timeout},
};
use tracing::{debug, info, warn};

use crate::{
  pairing::{Target, derive_code, hex},
  store::{Store, now},
  types::{Confirm, Device, DeviceStatus, Event, Nearby, PairInfo, PairState},
};

const RECONNECT_EVERY: Duration = Duration::from_secs(10);
/// Long enough for a lookup through n0's DNS and a relay
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const WHO_TIMEOUT: Duration = Duration::from_secs(3);

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
}

enum Phase {
  Idle,
  /// The pair page is open: dialers are accepted
  Waiting {
    info: PairInfo,
  },
  /// Dialing out, or a dialer is connected and being validated
  Dialing,
  Confirm {
    view: Confirm,
    decide: Option<oneshot::Sender<bool>>,
  },
}

struct Session {
  sid: u64,
  phase: Phase,
  cancel: watch::Sender<bool>,
}

struct Live {
  conn: Connection,
  status: DeviceStatus,
}

struct Peer {
  id: Id,
  name: String,
  os: String,
  hints: Hints,
  /// How the pairing connection reaches it, for the confirm screen
  via: Via,
}

pub struct Engine {
  node: Node,
  settings: RwLock<Settings>,
  store: Mutex<Store>,
  session: Mutex<Session>,
  live: Mutex<HashMap<String, Live>>,
  trying: Mutex<HashSet<String>>,
  nearby: Mutex<HashMap<Id, Nearby>>,
  local: Mutex<(Option<u8>, bool)>,
  events: broadcast::Sender<Event>,
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
  let _ = cancel.wait_for(|c| *c).await;
}

impl Engine {
  /// Opens the store and key in `dir`, binds the node and starts the accept,
  /// reconnect and discovery loops
  pub async fn start(settings: Settings, dir: &Path) -> Result<Arc<Self>> {
    let key = load_or_create_key(&dir.join("key"))?;
    let node = Node::bind(key, settings.port, settings.relay).await?;
    if let Err(e) = node.set_mdns(settings.discovery) {
      warn!("{e:#}");
    }
    let (events, _) = broadcast::channel(64);
    let engine = Arc::new(Self {
      node,
      settings: RwLock::new(settings),
      store: Mutex::new(Store::open(dir)?),
      session: Mutex::new(Session {
        sid: 0,
        phase: Phase::Idle,
        cancel: watch::channel(false).0,
      }),
      live: Default::default(),
      trying: Default::default(),
      nearby: Default::default(),
      local: Default::default(),
      events,
    });
    info!(
      "endpoint {} on port {}",
      hex(&engine.node.id()),
      engine.node.port()
    );
    tokio::spawn(engine.clone().accept_loop());
    tokio::spawn(engine.clone().reconnect_loop());
    tokio::spawn(engine.clone().nearby_loop());
    Ok(engine)
  }

  pub fn events(&self) -> broadcast::Receiver<Event> {
    self.events.subscribe()
  }

  fn emit(&self, event: Event) {
    let _ = self.events.send(event);
  }

  pub fn set_settings(&self, settings: Settings) {
    if settings.discovery != self.settings().discovery {
      if let Err(e) = self.node.set_mdns(settings.discovery) {
        warn!("{e:#}");
      }
      self.nearby.lock().unwrap().clear();
    }
    *self.settings.write().unwrap() = settings;
    self.emit(Event::ConfigChanged);
  }

  pub fn settings(&self) -> Settings {
    self.settings.read().unwrap().clone()
  }

  /// Battery of this device, sent to peers in `Hello`
  pub fn set_local_status(self: &Arc<Self>, battery: Option<u8>, charging: bool) {
    *self.local.lock().unwrap() = (battery, charging);
    let conns: Vec<_> = (self.live.lock().unwrap().values())
      .map(|l| l.conn.clone())
      .collect();
    for conn in conns {
      let hello = self.hello();
      tokio::spawn(async move {
        let (mut send, _recv) = conn.open_bi().await?;
        write_msg(&mut send, &hello).await?;
        send.finish()?;
        anyhow::Ok(())
      });
    }
  }

  fn hello(&self) -> Msg {
    let s = self.settings();
    let (battery, charging) = *self.local.lock().unwrap();
    Msg::Hello {
      name: s.name,
      os: s.os,
      battery,
      charging,
    }
  }

  // ---- queries -----------------------------------------------------------

  pub fn devices(&self) -> Vec<DeviceStatus> {
    let live = self.live.lock().unwrap();
    (self.store.lock().unwrap().devices().iter())
      .map(|d| match live.get(&d.id) {
        Some(l) => DeviceStatus {
          device: d.clone(),
          ..l.status.clone()
        },
        None => DeviceStatus {
          device: d.clone(),
          connected: false,
          battery: None,
          charging: false,
        },
      })
      .collect()
  }

  pub fn pair_state(&self) -> PairState {
    let s = self.session.lock().unwrap();
    match &s.phase {
      Phase::Idle => PairState::Idle,
      Phase::Waiting { info } => PairState::Waiting(info.clone()),
      Phase::Dialing => PairState::Dialing,
      Phase::Confirm { view, .. } => PairState::Confirm(view.clone()),
    }
  }

  // ---- session bookkeeping ----------------------------------------------

  /// Claims the single pairing session
  fn begin(self: &Arc<Self>, phase: Phase) -> Result<(u64, watch::Receiver<bool>)> {
    let mut s = self.session.lock().unwrap();
    if !matches!(s.phase, Phase::Idle) {
      bail!("pairing in progress");
    }
    s.sid += 1;
    s.cancel = watch::channel(false).0;
    // waiting lasts as long as the pair page is open
    let waiting = matches!(phase, Phase::Waiting { .. });
    s.phase = phase;
    let (sid, rx) = (s.sid, s.cancel.subscribe());
    drop(s);
    if !waiting {
      self.arm(sid);
    }
    Ok((sid, rx))
  }

  /// Ends session `sid` once `pairing_timeout` passes
  fn arm(self: &Arc<Self>, sid: u64) {
    let timeout_for = self.settings().pairing_timeout;
    let engine = self.clone();
    tokio::spawn(async move {
      sleep(timeout_for).await;
      engine.expire(sid);
    });
  }

  fn expire(&self, sid: u64) {
    if self.reset(sid) {
      self.emit(Event::PairingExpired);
    }
  }

  /// Back to idle and stop the session's tasks; `false` when `sid` is no
  /// longer the running session
  fn reset(&self, sid: u64) -> bool {
    let mut s = self.session.lock().unwrap();
    if s.sid != sid || matches!(s.phase, Phase::Idle) {
      return false;
    }
    s.phase = Phase::Idle;
    let _ = s.cancel.send(true);
    true
  }

  /// Ends the session and reports how it went, unless it was cancelled or
  /// expired meanwhile
  fn conclude(&self, sid: u64, res: Result<Option<Device>>) {
    if !self.reset(sid) {
      return;
    }
    self.emit(match res {
      Ok(Some(device)) => Event::Paired(device),
      Ok(None) => Event::PairingFailed("rejected".into()),
      Err(e) => Event::PairingFailed(format!("{e:#}")),
    });
  }

  pub fn cancel_pairing(&self) {
    let sid = self.session.lock().unwrap().sid;
    if self.reset(sid) {
      self.emit(Event::PairingCancelled);
    }
  }

  // ---- pairing: waiting side --------------------------------------------

  /// Shows up for dialers and waits (the QR / "Pair new device" side)
  pub fn start_pairing(self: &Arc<Self>) -> Result<PairInfo> {
    let settings = self.settings();
    let hints = self.node.hints();
    // IPv4 only keeps the QR code small
    let addrs: Vec<_> = (hints.addrs.into_iter())
      .filter(SocketAddr::is_ipv4)
      .collect();
    let info = PairInfo {
      link: Target {
        id: self.node.id(),
        relay: hints.relay,
        addrs: addrs.clone(),
      }
      .link(),
      name: settings.name,
      addrs,
    };
    self.begin(Phase::Waiting { info: info.clone() })?;
    self.emit(Event::PairingStarted(info.clone()));
    Ok(info)
  }

  /// A dialer reached us: take over the waiting session
  fn claim(self: &Arc<Self>, peer: &Id) -> Result<(u64, watch::Receiver<bool>)> {
    if self.store.lock().unwrap().get(&hex(peer)).is_some() {
      bail!("already paired");
    }
    let mut s = self.session.lock().unwrap();
    match &s.phase {
      Phase::Waiting { .. } => {}
      Phase::Idle => bail!("pair page is closed"),
      _ => bail!("another pairing is in progress"),
    }
    s.phase = Phase::Dialing;
    let (sid, rx) = (s.sid, s.cancel.subscribe());
    drop(s);
    self.arm(sid);
    Ok((sid, rx))
  }

  async fn accept_pair(
    self: &Arc<Self>,
    conn: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    hello: (String, String, [u8; 16], Hints),
  ) -> Result<()> {
    let (name, os, dialer_nonce, hints) = hello;
    let (sid, cancel) = match self.claim(&remote_id(conn)) {
      Ok(c) => c,
      Err(e) => {
        // the dialer is untrusted: it gets a fixed reason, the detail stays in our log
        info!("refused pairing from {name}: {e:#}");
        conn.close(1u32.into(), b"not accepting pairing");
        return Ok(());
      }
    };
    let res = async {
      let nonce: [u8; 16] = random_bytes();
      let (s, mine) = (self.settings(), self.node.hints());
      write_msg(
        &mut send,
        &Msg::PairNonce {
          name: s.name,
          os: s.os,
          nonce,
          relay: mine.relay,
          addrs: mine.addrs,
        },
      )
      .await?;
      let id = remote_id(conn);
      let code = derive_code(&self.node.id(), &id, &dialer_nonce, &nonce);
      let peer = Peer {
        id,
        name,
        os,
        hints,
        via: via(conn),
      };
      self
        .confirm(sid, cancel, &mut send, &mut recv, peer, code)
        .await
    }
    .await;
    self.conclude(sid, res);
    linger(conn, send).await;
    Ok(())
  }

  // ---- pairing: dialing side --------------------------------------------

  /// Pairs with a device given as an `aurora://pair` link or a hex device ID
  pub async fn connect(self: &Arc<Self>, target: &str) -> Result<()> {
    let target = Target::parse(target)?;
    if target.id == self.node.id() {
      bail!("that is this device");
    }
    if let Some(d) = self.store.lock().unwrap().get(&hex(&target.id)) {
      bail!("already paired with {}", d.name);
    }
    let (sid, cancel) = self.begin(Phase::Dialing)?;
    let engine = self.clone();
    tokio::spawn(async move {
      let res = engine.dial_pair(sid, cancel, target).await;
      engine.conclude(sid, res);
    });
    Ok(())
  }

  async fn dial_pair(
    self: &Arc<Self>,
    sid: u64,
    mut cancel: watch::Receiver<bool>,
    target: Target,
  ) -> Result<Option<Device>> {
    let hints = Hints {
      relay: target.relay,
      addrs: target.addrs,
    };
    let conn = tokio::select! {
      conn = timeout(DIAL_TIMEOUT, self.node.connect(target.id, &hints)) =>
        conn.map_err(|_| anyhow!("device did not answer"))??,
      () = cancelled(&mut cancel) => bail!("cancelled"),
    };
    let (mut send, mut recv) = conn.open_bi().await?;
    let nonce: [u8; 16] = random_bytes();
    let (s, mine) = (self.settings(), self.node.hints());
    write_msg(
      &mut send,
      &Msg::PairHello {
        name: s.name,
        os: s.os,
        nonce,
        relay: mine.relay,
        addrs: mine.addrs,
      },
    )
    .await?;
    let Msg::PairNonce {
      name,
      os,
      nonce: acceptor_nonce,
      relay,
      addrs: reported,
    } = read_msg(&mut recv)
      .await
      .context("device refused pairing")?
    else {
      bail!("unexpected reply");
    };
    let mut addrs = hints.addrs;
    for a in reported {
      if !addrs.contains(&a) {
        addrs.push(a);
      }
    }
    let code = derive_code(&self.node.id(), &target.id, &nonce, &acceptor_nonce);
    let peer = Peer {
      id: target.id,
      name,
      os,
      hints: Hints {
        relay: relay.or(hints.relay),
        addrs,
      },
      via: via(&conn),
    };
    let res = self
      .confirm(sid, cancel, &mut send, &mut recv, peer, code)
      .await;
    // report the outcome now; the peer still gets time to read our decision
    tokio::spawn(async move { linger(&conn, send).await });
    res
  }

  // ---- pairing: code confirmation (both sides) ---------------------------

  /// Shows the code and waits for both users; stores the device on success.
  /// `None` when either side declined.
  async fn confirm(
    &self,
    sid: u64,
    mut cancel: watch::Receiver<bool>,
    send: &mut SendStream,
    recv: &mut RecvStream,
    peer: Peer,
    code: String,
  ) -> Result<Option<Device>> {
    let (decide, mut local_rx) = oneshot::channel::<bool>();
    let mut view = Confirm {
      code,
      peer_name: peer.name.clone(),
      peer_os: peer.os.clone(),
      via: match peer.via {
        Via::Direct(a) => format!("Direct · {}", a.ip()),
        Via::Relay => "Relay".into(),
      },
      fingerprint: hex(&peer.id[..8])
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c))
        .collect::<Vec<_>>()
        .join(" "),
      local_accepted: false,
    };
    {
      let mut s = self.session.lock().unwrap();
      if s.sid != sid || matches!(s.phase, Phase::Idle) {
        bail!("cancelled");
      }
      s.phase = Phase::Confirm {
        view: view.clone(),
        decide: Some(decide),
      };
    }
    self.emit(Event::CodeReady(view.clone()));

    let (mut local, mut remote) = (None::<bool>, None::<bool>);
    let remote_fut = read_msg(recv);
    tokio::pin!(remote_fut);
    loop {
      tokio::select! {
        d = &mut local_rx, if local.is_none() => {
          let accept = d.unwrap_or(false);
          local = Some(accept);
          view.local_accepted = accept;
          write_msg(send, &Msg::PairDecision { accept }).await?;
        }
        m = &mut remote_fut, if remote.is_none() => match m? {
          Msg::PairDecision { accept } => remote = Some(accept),
          _ => bail!("unexpected message while confirming"),
        },
        () = cancelled(&mut cancel) => bail!("cancelled"),
      }
      match (local, remote) {
        (Some(false), _) | (_, Some(false)) => return Ok(None),
        (Some(true), Some(true)) => break,
        _ => {}
      }
    }

    let device = Device {
      id: hex(&peer.id),
      name: peer.name,
      os: peer.os,
      paired_at: now(),
      relay: peer.hints.relay,
      addrs: peer.hints.addrs,
    };
    self.store.lock().unwrap().insert(device.clone())?;
    Ok(Some(device))
  }

  /// The local user's answer to the shown code
  pub fn decide(&self, accept: bool) -> Result<()> {
    let mut s = self.session.lock().unwrap();
    match &mut s.phase {
      Phase::Confirm { view, decide } => {
        let tx = decide.take().ok_or_else(|| anyhow!("already answered"))?;
        // set here, not in `confirm`, so a `pair_state` right after this call sees it
        view.local_accepted = accept;
        tx.send(accept).map_err(|_| anyhow!("pairing ended"))
      }
      _ => bail!("no code to confirm"),
    }
  }

  // ---- devices -----------------------------------------------------------

  pub async fn unpair(&self, id: &str) -> Result<()> {
    if !self.store.lock().unwrap().remove(id)? {
      bail!("unknown device");
    }
    let live = self.live.lock().unwrap().remove(id);
    if let Some(live) = live {
      if let Ok((mut send, _)) = live.conn.open_bi().await {
        let _ = write_msg(&mut send, &Msg::Unpair).await;
        let _ = send.finish();
      }
      linger_close(&live.conn).await;
    }
    self.emit(Event::Unpaired(id.into()));
    Ok(())
  }

  // ---- connections to paired devices -------------------------------------

  async fn accept_loop(self: Arc<Self>) {
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
        let hints = Hints { relay, addrs };
        self
          .accept_pair(&conn, send, recv, (name, os, nonce, hints))
          .await
      }
      Msg::Who => {
        let s = self.settings();
        // only devices that may find us by mDNS learn our name
        if s.discovery {
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

  async fn reconnect_loop(self: Arc<Self>) {
    sleep(Duration::from_secs(1)).await;
    loop {
      let due: Vec<Device> = (self.store.lock().unwrap().devices().iter())
        .filter(|d| !self.live.lock().unwrap().contains_key(&d.id))
        .cloned()
        .collect();
      for device in due {
        if !self.trying.lock().unwrap().insert(device.id.clone()) {
          continue;
        }
        let engine = self.clone();
        tokio::spawn(async move {
          if let Err(e) = engine.dial_known(&device).await {
            debug!("cannot reach {}: {e:#}", device.name);
          }
          engine.trying.lock().unwrap().remove(&device.id);
        });
      }
      sleep(RECONNECT_EVERY).await;
    }
  }

  async fn dial_known(self: &Arc<Self>, device: &Device) -> Result<()> {
    let id: Id = crate::pairing::unhex(&device.id)?;
    let hints = Hints {
      relay: device.relay.clone(),
      addrs: device.addrs.clone(),
    };
    let conn = timeout(DIAL_TIMEOUT, self.node.connect(id, &hints))
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

  /// Holds a connection to a paired device until it drops
  async fn run_live(
    &self,
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
    };
    {
      let mut live = self.live.lock().unwrap();
      if live.contains_key(&id) {
        return; // both sides dialed; keep the first
      }
      live.insert(
        id.clone(),
        Live {
          conn: conn.clone(),
          status: status.clone(),
        },
      );
    }
    self.emit(Event::DeviceConnected(status));

    loop {
      tokio::select! {
        _ = conn.closed() => break,
        stream = conn.accept_bi() => {
          let Ok((_, mut recv)) = stream else { break };
          match read_msg(&mut recv).await {
            Ok(Msg::Unpair) => {
              let _ = self.store.lock().unwrap().remove(&id);
              self.emit(Event::Unpaired(id.clone()));
              break;
            }
            Ok(Msg::Hello { battery, charging, .. }) => {
              let status = {
                let mut live = self.live.lock().unwrap();
                let Some(l) = live.get_mut(&id) else { break };
                (l.status.battery, l.status.charging) = (battery, charging);
                l.status.clone()
              };
              self.emit(Event::DeviceConnected(status));
            }
            _ => {}
          }
        }
      }
    }

    let mine = {
      let mut live = self.live.lock().unwrap();
      match live.get(&id) {
        Some(l) if l.conn.stable_id() == conn.stable_id() => live.remove(&id).is_some(),
        _ => false,
      }
    };
    if mine {
      self.emit(Event::DeviceDisconnected(id));
    }
    conn.close(0u32.into(), b"bye");
  }

  // ---- nearby devices ------------------------------------------------------

  /// Keeps the nearby list from mDNS, asking each new device for its name
  async fn nearby_loop(self: Arc<Self>) {
    let mut seen = self.node.seen();
    loop {
      match seen.recv().await {
        Ok(Seen::Found { id, addrs }) => {
          if self.nearby.lock().unwrap().contains_key(&id) {
            continue;
          }
          let engine = self.clone();
          tokio::spawn(async move {
            match timeout(WHO_TIMEOUT, engine.who(id, addrs.clone())).await {
              Ok(Ok((name, os))) if engine.settings().discovery => {
                let addr = addrs.iter().find(|a| a.is_ipv4()).copied();
                let link = Target {
                  id,
                  relay: None,
                  addrs,
                }
                .link();
                let nearby = Nearby {
                  name,
                  os,
                  addr,
                  link,
                };
                engine.nearby.lock().unwrap().insert(id, nearby);
              }
              Ok(Err(e)) => debug!("no name from {}: {e:#}", hex(&id)),
              _ => {}
            }
          });
        }
        Ok(Seen::Gone(id)) => {
          self.nearby.lock().unwrap().remove(&id);
        }
        Err(broadcast::error::RecvError::Lagged(_)) => {}
        Err(broadcast::error::RecvError::Closed) => break,
      }
    }
  }

  /// Name and OS of a device mDNS found
  async fn who(&self, id: Id, addrs: Vec<SocketAddr>) -> Result<(String, String)> {
    let conn = self.node.connect(id, &Hints { relay: None, addrs }).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(&mut send, &Msg::Who).await?;
    send.finish()?;
    let reply = read_msg(&mut recv).await;
    conn.close(0u32.into(), b"thanks");
    match reply? {
      Msg::Info { name, os } => Ok((name, os)),
      _ => bail!("unexpected reply"),
    }
  }

  /// Devices mDNS found on the local network, minus paired ones
  pub async fn scan(&self) -> Result<Vec<Nearby>> {
    let store = self.store.lock().unwrap();
    Ok(
      (self.nearby.lock().unwrap().iter())
        .filter(|(id, _)| store.get(&hex(*id)).is_none())
        .map(|(_, n)| n.clone())
        .collect(),
    )
  }

  /// Ends the session, tells peers we are going and closes the endpoint so iroh
  /// does not abort on drop
  pub async fn shutdown(&self) {
    self.cancel_pairing();
    let conns: Vec<_> = (self.live.lock().unwrap().drain())
      .map(|(_, l)| l.conn)
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

/// Gives the peer time to read our last message before the connection goes
async fn linger(conn: &Connection, mut send: SendStream) {
  let _ = send.finish();
  linger_close(conn).await;
}

async fn linger_close(conn: &Connection) {
  let _ = timeout(Duration::from_secs(2), conn.closed()).await;
  conn.close(0u32.into(), b"done");
}

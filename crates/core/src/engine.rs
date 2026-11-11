//! The pairing and session engine, shared by `aurorad` and the mobile app.
//!
//! It owns the iroh node and the device store, runs the pairing state machine
//! (one session at a time) and keeps connections to paired devices alive.

use std::{
  collections::{HashMap, HashSet},
  net::{IpAddr, Ipv4Addr, SocketAddr},
  path::Path,
  sync::{Arc, Mutex, RwLock},
  time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use aurora_proto::{Beacon, DISCOVERY_PORT, DISCOVERY_QUERY, Id, Msg, Token, read_msg, write_msg};
use aurora_transport::{
  Connection, Node, RecvStream, SendStream, load_or_create_key, random_bytes, remote_id,
};
use tokio::{
  net::UdpSocket,
  sync::{broadcast, oneshot, watch},
  time::{Instant, sleep, timeout},
};
use tracing::{debug, info, warn};

use crate::{
  pairing::{Target, derive_code, hex},
  store::{Store, now},
  types::{Confirm, Device, DeviceStatus, Event, Nearby, PairInfo, PairState},
};

const RECONNECT_EVERY: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Settings {
  pub name: String,
  pub os: String,
  pub pairing_timeout: Duration,
  /// Answer discovery broadcasts
  pub discovery: bool,
  /// Fixed QUIC port, read once when the engine starts
  pub port: Option<u16>,
  /// Extra host to dial every known port on (the Android emulator reaches its
  /// host at 10.0.2.2)
  pub host_alias: Option<IpAddr>,
}

enum Phase {
  Idle,
  Waiting {
    token: Token,
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
  deadline: Instant,
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
  addrs: Vec<SocketAddr>,
}

pub struct Engine {
  node: Node,
  settings: RwLock<Settings>,
  store: Mutex<Store>,
  session: Mutex<Session>,
  live: Mutex<HashMap<String, Live>>,
  trying: Mutex<HashSet<String>>,
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
    let node = Node::bind(key, settings.port).await?;
    let (events, _) = broadcast::channel(64);
    let engine = Arc::new(Self {
      node,
      settings: RwLock::new(settings),
      store: Mutex::new(Store::open(dir)?),
      session: Mutex::new(Session {
        sid: 0,
        phase: Phase::Idle,
        deadline: Instant::now(),
        cancel: watch::channel(false).0,
      }),
      live: Default::default(),
      trying: Default::default(),
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
    tokio::spawn(engine.clone().discovery_responder());
    Ok(engine)
  }

  pub fn events(&self) -> broadcast::Receiver<Event> {
    self.events.subscribe()
  }

  fn emit(&self, event: Event) {
    let _ = self.events.send(event);
  }

  pub fn set_settings(&self, settings: Settings) {
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
      Phase::Waiting { info, .. } => PairState::Waiting(PairInfo {
        expires_in: s
          .deadline
          .saturating_duration_since(Instant::now())
          .as_secs(),
        ..info.clone()
      }),
      Phase::Dialing => PairState::Dialing,
      Phase::Confirm { view, .. } => PairState::Confirm(view.clone()),
    }
  }

  // ---- session bookkeeping ----------------------------------------------

  /// Claims the single pairing session
  fn begin(self: &Arc<Self>, phase: Phase) -> Result<(u64, watch::Receiver<bool>)> {
    let timeout_for = self.settings().pairing_timeout;
    let mut s = self.session.lock().unwrap();
    if !matches!(s.phase, Phase::Idle) {
      bail!("pairing in progress");
    }
    s.sid += 1;
    s.cancel = watch::channel(false).0;
    s.phase = phase;
    s.deadline = Instant::now() + timeout_for;
    let (sid, rx) = (s.sid, s.cancel.subscribe());
    drop(s);

    let engine = self.clone();
    tokio::spawn(async move {
      sleep(timeout_for).await;
      engine.expire(sid);
    });
    Ok((sid, rx))
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
    let token: Token = random_bytes();
    let settings = self.settings();
    // IPv4 only keeps the QR code small
    let addrs: Vec<_> = (self.node.addrs().into_iter())
      .filter(SocketAddr::is_ipv4)
      .collect();
    let info = PairInfo {
      link: Target {
        id: self.node.id(),
        addrs: addrs.clone(),
        token: Some(token),
      }
      .link(),
      name: settings.name,
      addrs,
      expires_in: settings.pairing_timeout.as_secs(),
    };
    self.begin(Phase::Waiting {
      token,
      info: info.clone(),
    })?;
    self.emit(Event::PairingStarted(info.clone()));
    Ok(info)
  }

  /// A dialer reached us: take over the waiting session. The token (QR) is
  /// single use because the phase leaves `Waiting` here.
  fn claim(&self, token: Option<Token>) -> Result<(u64, watch::Receiver<bool>)> {
    let mut s = self.session.lock().unwrap();
    match &s.phase {
      Phase::Waiting { token: want, .. } => {
        if token.is_some_and(|t| t != *want) {
          bail!("bad pairing token");
        }
      }
      _ => bail!("not waiting for a device"),
    }
    s.phase = Phase::Dialing;
    Ok((s.sid, s.cancel.subscribe()))
  }

  async fn accept_pair(
    self: &Arc<Self>,
    conn: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    hello: (String, String, [u8; 16], Option<Token>, Vec<SocketAddr>),
  ) -> Result<()> {
    let (name, os, dialer_nonce, token, addrs) = hello;
    let (sid, cancel) = self.claim(token)?;
    let res = async {
      let nonce: [u8; 16] = random_bytes();
      let s = self.settings();
      write_msg(
        &mut send,
        &Msg::PairNonce {
          name: s.name,
          os: s.os,
          nonce,
          addrs: self.node.addrs(),
        },
      )
      .await?;
      let id = remote_id(conn);
      let code = derive_code(&self.node.id(), &id, &dialer_nonce, &nonce);
      let peer = Peer {
        id,
        name,
        os,
        addrs,
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

  /// Pairs with a device given as an `aurora://pair` link or as a host
  /// (`192.168.1.10`, `host:port`) that answers discovery
  pub async fn connect(self: &Arc<Self>, target: &str) -> Result<()> {
    let target = match Target::parse(target) {
      Ok(t) => t,
      Err(_) => {
        let (_, beacon, addr) = self.probe(target).await?;
        Target {
          id: beacon.id,
          addrs: vec![addr],
          token: None,
        }
      }
    };
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
    let addrs = self.dial_addrs(&target.addrs);
    let conn = tokio::select! {
      conn = timeout(DIAL_TIMEOUT, self.node.connect(target.id, &addrs)) =>
        conn.map_err(|_| anyhow!("device did not answer"))??,
      () = cancelled(&mut cancel) => bail!("cancelled"),
    };
    let (mut send, mut recv) = conn.open_bi().await?;
    let nonce: [u8; 16] = random_bytes();
    let s = self.settings();
    write_msg(
      &mut send,
      &Msg::PairHello {
        name: s.name,
        os: s.os,
        nonce,
        token: target.token,
        addrs: self.node.addrs(),
      },
    )
    .await?;
    let Msg::PairNonce {
      name,
      os,
      nonce: acceptor_nonce,
      addrs: reported,
    } = read_msg(&mut recv)
      .await
      .context("device refused pairing")?
    else {
      bail!("unexpected reply");
    };
    let mut addrs = target.addrs;
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
      addrs,
    };
    let res = self
      .confirm(sid, cancel, &mut send, &mut recv, peer, code)
      .await;
    linger(&conn, send).await;
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
      via: match peer.addrs.first() {
        Some(a) => format!("Local network · {}", a.ip()),
        None => "Local network".into(),
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
          if let Phase::Confirm { view: v, .. } = &mut self.session.lock().unwrap().phase {
            v.local_accepted = accept;
          }
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
      addrs: peer.addrs,
    };
    self.store.lock().unwrap().insert(device.clone())?;
    Ok(Some(device))
  }

  /// The local user's answer to the shown code
  pub fn decide(&self, accept: bool) -> Result<()> {
    let mut s = self.session.lock().unwrap();
    match &mut s.phase {
      Phase::Confirm { decide, .. } => decide
        .take()
        .ok_or_else(|| anyhow!("already answered"))?
        .send(accept)
        .map_err(|_| anyhow!("pairing ended")),
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
        token,
        addrs,
      } => {
        self
          .accept_pair(&conn, send, recv, (name, os, nonce, token, addrs))
          .await
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
        .filter(|d| !d.addrs.is_empty())
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
    let addrs = self.dial_addrs(&device.addrs);
    let conn = timeout(DIAL_TIMEOUT, self.node.connect(id, &addrs))
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

  /// `addrs` plus the host alias on every port they use
  fn dial_addrs(&self, addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut out = addrs.to_vec();
    if let Some(alias) = self.settings().host_alias {
      for port in addrs.iter().map(SocketAddr::port) {
        let a = SocketAddr::new(alias, port);
        if !out.contains(&a) {
          out.push(a);
        }
      }
    }
    out
  }

  // ---- discovery -----------------------------------------------------------

  async fn discovery_responder(self: Arc<Self>) {
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await {
      Ok(s) => s,
      Err(e) => {
        warn!("discovery responder disabled: {e}");
        return;
      }
    };
    let mut buf = [0u8; 64];
    loop {
      let Ok((len, from)) = socket.recv_from(&mut buf).await else {
        continue;
      };
      let s = self.settings();
      if !s.discovery || &buf[..len] != DISCOVERY_QUERY {
        continue;
      }
      let beacon = Beacon {
        id: self.node.id(),
        name: s.name,
        os: s.os,
        port: self.node.port(),
      };
      if let Ok(bytes) = postcard_bytes(&beacon) {
        let _ = socket.send_to(&bytes, from).await;
      }
    }
  }

  /// Devices on the local network that answer the discovery broadcast
  pub async fn scan(&self) -> Result<Vec<Nearby>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.set_broadcast(true)?;
    socket
      .send_to(DISCOVERY_QUERY, (Ipv4Addr::BROADCAST, DISCOVERY_PORT))
      .await?;
    // behind a NAT (the Android emulator) broadcasts stay inside; ask the host directly
    if let Some(alias) = self.settings().host_alias {
      let _ = socket
        .send_to(DISCOVERY_QUERY, (alias, DISCOVERY_PORT))
        .await;
    }
    let mut found = Vec::new();
    let end = Instant::now() + Duration::from_millis(1200);
    let mut buf = [0u8; 512];
    while let Ok(Ok((len, from))) = tokio::time::timeout_at(end, socket.recv_from(&mut buf)).await {
      let Ok(beacon) = postcard_from(&buf[..len]) else {
        continue;
      };
      if beacon.id == self.node.id() || found.iter().any(|n: &Nearby| n.addr.ip() == from.ip()) {
        continue;
      }
      let addr = SocketAddr::new(from.ip(), beacon.port);
      found.push(Nearby {
        name: beacon.name,
        os: beacon.os,
        addr,
        link: Target {
          id: beacon.id,
          addrs: vec![addr],
          token: None,
        }
        .link(),
      });
    }
    Ok(found)
  }

  /// Asks one host for its beacon; `host` is `name`, `ip` or `name:port` of
  /// the discovery port
  async fn probe(&self, host: &str) -> Result<(SocketAddr, Beacon, SocketAddr)> {
    let host = host.trim();
    let query = if host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err() {
      host.to_string()
    } else {
      format!("{host}:{DISCOVERY_PORT}")
    };
    let to = tokio::net::lookup_host(&query)
      .await
      .with_context(|| format!("cannot resolve {host}"))?
      .next()
      .ok_or_else(|| anyhow!("cannot resolve {host}"))?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.send_to(DISCOVERY_QUERY, to).await?;
    let mut buf = [0u8; 512];
    let (len, from) = timeout(Duration::from_secs(2), socket.recv_from(&mut buf))
      .await
      .map_err(|_| anyhow!("{host} did not answer; is Aurora running there?"))??;
    let beacon = postcard_from(&buf[..len])?;
    let addr = SocketAddr::new(from.ip(), beacon.port);
    Ok((to, beacon, addr))
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

fn postcard_bytes(beacon: &Beacon) -> Result<Vec<u8>> {
  aurora_proto::encode_beacon(beacon)
}

fn postcard_from(bytes: &[u8]) -> Result<Beacon> {
  aurora_proto::decode_beacon(bytes)
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

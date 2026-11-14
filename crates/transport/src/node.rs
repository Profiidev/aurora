//! The iroh endpoint every transport runs under.

use std::{
  net::{Ipv4Addr, SocketAddr},
  path::Path,
  sync::Arc,
  time::Duration,
};

use anyhow::{Result, anyhow};
use aurora_proto::{ALPN, Id};
use iroh::{
  Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr,
  endpoint::{
    Connection as IrohConn, QuicTransportConfig, RecvStream as IrohRecv, SendStream as IrohSend,
    presets,
  },
};
use tokio::sync::broadcast;

use crate::{
  link::{LinkEvent, LinkKind, Links},
  mdns::Mdns,
};

pub type Connection = IrohConn;
pub type SendStream = IrohSend;
pub type RecvStream = IrohRecv;

/// Reads the endpoint key from `path`, generating it (mode 0600) on first use
pub fn load_or_create_key(path: &Path) -> Result<SecretKey> {
  if let Ok(bytes) = std::fs::read(path) {
    let bytes: [u8; 32] = bytes
      .try_into()
      .map_err(|_| anyhow!("{} is not a 32 byte key", path.display()))?;
    return Ok(SecretKey::from_bytes(&bytes));
  }
  let key = SecretKey::generate();
  if let Some(dir) = path.parent() {
    std::fs::create_dir_all(dir)?;
  }
  write_private(path, &key.to_bytes())?;
  Ok(key)
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
  use std::io::Write;
  let mut opts = std::fs::OpenOptions::new();
  opts.write(true).create(true).truncate(true);
  #[cfg(unix)]
  std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
  opts.open(path)?.write_all(data)?;
  Ok(())
}

/// Where a device was last seen; iroh's lookups fill in the rest
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hints {
  pub relay: Option<String>,
  pub addrs: Vec<SocketAddr>,
}

/// How a connection currently reaches the peer
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Via {
  Direct(SocketAddr),
  Relay,
  Link(LinkKind),
}

impl Via {
  /// Without the address: "USB", "Direct", "Relay"
  pub fn label(&self) -> &'static str {
    match self {
      Via::Direct(_) => "Direct",
      Via::Relay => "Relay",
      Via::Link(LinkKind::Usb) => "USB",
      Via::Link(LinkKind::Bluetooth) => "Bluetooth",
    }
  }
}

/// With the address, for the pairing screen: "Direct · 192.168.1.5"
impl std::fmt::Display for Via {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Via::Direct(a) => write!(f, "Direct · {}", a.ip()),
      Via::Relay => f.write_str("Relay"),
      Via::Link(LinkKind::Usb) => f.write_str("USB"),
      Via::Link(LinkKind::Bluetooth) => f.write_str("Bluetooth"),
    }
  }
}

/// A device found nearby: by mDNS on the local network, or at the other end
/// of a link
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
  Found {
    id: Id,
    addrs: Vec<SocketAddr>,
    via: Via,
  },
  Gone(Id),
}

/// Two iroh endpoints under one key. The network one: n0's DNS lookup and
/// relays when `relay` is set, mDNS on the local network while discovery is
/// on. The link one: only point-to-point links. iroh 1.4 never opens a link
/// path on a connection once it knows the peer over IP, so a device keeps one
/// connection per endpoint and the caller prefers the link one.
#[derive(Clone)]
pub struct Node {
  ep: Endpoint,
  link_ep: Endpoint,
  mdns: Arc<Mdns>,
  links: Links,
  seen: broadcast::Sender<Seen>,
  #[cfg(feature = "nusb")]
  usb: crate::usb::UsbHost,
  #[cfg(feature = "bluer")]
  bluetooth: crate::bluetooth::BtHost,
}

impl Node {
  pub async fn bind(key: SecretKey, port: Option<u16>, relay: bool) -> Result<Self> {
    let builder = if relay {
      Endpoint::builder(presets::N0)
    } else {
      Endpoint::builder(presets::Minimal)
    };
    let seen = broadcast::channel(64).0;
    let mdns = Arc::new(Mdns::new(key.public(), seen.clone()));
    let links = Links::new(*key.public().as_bytes());
    let ep = builder
      .transport_config(network_transport()?)
      .secret_key(key.clone())
      .alpns(vec![ALPN.to_vec()])
      .bind_addr((Ipv4Addr::UNSPECIFIED, port.unwrap_or(0)))?
      .bind()
      .await?;
    ep.address_lookup()?.add(mdns.clone());
    let link_ep = Endpoint::builder(presets::Minimal)
      .secret_key(key)
      .alpns(vec![ALPN.to_vec()])
      .clear_ip_transports()
      .clear_relay_transports()
      .add_custom_transport(Arc::new(links.clone()))
      .bind()
      .await?;
    link_ep.address_lookup()?.add(links.clone());
    tokio::spawn(forward_links(links.events(), seen.clone()));
    Ok(Self {
      #[cfg(feature = "nusb")]
      usb: crate::usb::UsbHost::new(links.clone()),
      #[cfg(feature = "bluer")]
      bluetooth: crate::bluetooth::BtHost::new(links.clone()),
      ep,
      link_ep,
      mdns,
      links,
      seen,
    })
  }

  pub fn id(&self) -> Id {
    *self.ep.id().as_bytes()
  }

  /// Port the endpoint listens on (IPv4)
  pub fn port(&self) -> u16 {
    self
      .ep
      .bound_sockets()
      .iter()
      .find(|a| a.is_ipv4())
      .map_or(0, SocketAddr::port)
  }

  /// This endpoint's relay and direct addresses
  pub fn hints(&self) -> Hints {
    let mut hints = Hints::default();
    for a in &self.ep.addr().addrs {
      match a {
        TransportAddr::Ip(a) => hints.addrs.push(*a),
        TransportAddr::Relay(url) => hints.relay = Some(url.to_string()),
        _ => {}
      }
    }
    hints
  }

  /// Advertise and look up devices on the local network
  pub fn set_mdns(&self, on: bool) -> Result<()> {
    self.mdns.set_enabled(on)
  }

  /// Look for phones on USB (desktop); without the `nusb` feature a no-op,
  /// the app attaches its accessory through [`Node::links`] instead
  pub fn set_usb(&self, on: bool) {
    #[cfg(feature = "nusb")]
    self.usb.set_enabled(on);
    #[cfg(not(feature = "nusb"))]
    let _ = on;
  }

  /// Advertise and accept Bluetooth channels (desktop); without the `bluer`
  /// feature a no-op, phones dial and attach through [`Node::links`]
  pub fn set_bluetooth(&self, on: bool) {
    #[cfg(feature = "bluer")]
    self.bluetooth.set_active(on);
    #[cfg(not(feature = "bluer"))]
    let _ = on;
  }

  pub fn links(&self) -> &Links {
    &self.links
  }

  /// Devices found or lost by mDNS or links
  pub fn seen(&self) -> broadcast::Receiver<Seen> {
    self.seen.subscribe()
  }

  /// Dials `id`; `hints` may be empty, the lookups find the device then
  pub async fn connect(&self, id: Id, hints: &Hints) -> Result<Connection> {
    let id = EndpointId::from_bytes(&id)?;
    let relay = (hints.relay.iter())
      .filter_map(|r| r.parse::<RelayUrl>().ok())
      .map(TransportAddr::Relay);
    let ips = hints.addrs.iter().map(|a| TransportAddr::Ip(*a));
    let addr = EndpointAddr::from_parts(id, relay.chain(ips));
    Ok(self.ep.connect(addr, ALPN).await?)
  }

  /// Dials `id` over a running link; the connection ends with the link
  pub async fn connect_link(&self, id: Id) -> Result<Connection> {
    if !self.links.is_linked(&id) {
      anyhow::bail!("no link to that device");
    }
    let id = EndpointId::from_bytes(&id)?;
    Ok(self.link_ep.connect(id, ALPN).await?)
  }

  /// Over a link when there is one, else over the network
  pub async fn dial(&self, id: Id, hints: &Hints) -> Result<Connection> {
    if self.links.is_linked(&id) {
      self.connect_link(id).await
    } else {
      self.connect(id, hints).await
    }
  }

  /// Next incoming connection on either endpoint, `None` once the node is
  /// closed
  pub async fn accept(&self) -> Option<Result<Connection>> {
    let incoming = tokio::select! {
      i = self.ep.accept() => i?,
      i = self.link_ep.accept() => i?,
    };
    Some(async { Ok(incoming.accept()?.await?) }.await)
  }

  pub async fn close(&self) {
    self.mdns.set_enabled(false).ok();
    self.set_usb(false);
    self.set_bluetooth(false);
    tokio::join!(self.ep.close(), self.link_ep.close());
  }
}

/// A network connection that hears nothing for this long is dead: iroh
/// sends a heartbeat every 5 s, so two in a row went missing. Its default
/// (30 s) kept a phone that lost Wi-Fi "connected" while Bluetooth waited.
const NETWORK_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

fn network_transport() -> Result<QuicTransportConfig> {
  Ok(
    QuicTransportConfig::builder()
      .max_idle_timeout(Some(NETWORK_IDLE_TIMEOUT.try_into()?))
      .build(),
  )
}

async fn forward_links(mut events: broadcast::Receiver<LinkEvent>, seen: broadcast::Sender<Seen>) {
  loop {
    let event = match events.recv().await {
      Ok(e) => e,
      Err(broadcast::error::RecvError::Lagged(_)) => continue,
      Err(broadcast::error::RecvError::Closed) => break,
    };
    let _ = seen.send(match event {
      LinkEvent::Up { kind, peer } => Seen::Found {
        id: peer,
        addrs: Vec::new(),
        via: Via::Link(kind),
      },
      LinkEvent::Down { peer, .. } => Seen::Gone(peer),
    });
  }
}

/// Endpoint id of the peer on `conn`
pub fn remote_id(conn: &Connection) -> Id {
  *conn.remote_id().as_bytes()
}

/// The path the connection uses right now
pub fn via(conn: &Connection) -> Via {
  let paths = conn.paths();
  match paths
    .iter()
    .find(|p| p.is_selected())
    .map(|p| p.remote_addr().clone())
  {
    Some(TransportAddr::Ip(a)) => Via::Direct(a),
    Some(TransportAddr::Custom(c)) => {
      LinkKind::from_transport_id(c.id()).map_or(Via::Relay, Via::Link)
    }
    _ => Via::Relay,
  }
}

/// Random bytes for nonces and tokens
pub fn random_bytes<const N: usize>() -> [u8; N] {
  let mut out = [0u8; N];
  let key = SecretKey::generate().to_bytes();
  out.copy_from_slice(&key[..N]);
  out
}

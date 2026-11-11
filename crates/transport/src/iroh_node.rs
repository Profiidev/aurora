use std::{
  net::{Ipv4Addr, SocketAddr},
  path::Path,
  sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow};
use aurora_proto::{ALPN, Id};
use iroh::{
  Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr,
  address_lookup::{AddressLookup, EndpointData, Error as LookupError, Item},
  endpoint::{Connection as IrohConn, RecvStream as IrohRecv, SendStream as IrohSend, presets},
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::{StreamExt, boxed::BoxStream};
use tokio::sync::broadcast;

pub type Connection = IrohConn;
pub type SendStream = IrohSend;
pub type RecvStream = IrohRecv;

/// mDNS service name, so only Aurora endpoints show up
const SERVICE: &str = "aurora";

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
}

/// A device seen on the local network by mDNS
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
  Found { id: Id, addrs: Vec<SocketAddr> },
  Gone(Id),
}

/// mDNS that can be switched on and off while the endpoint runs; iroh has no
/// way to remove an address lookup again
#[derive(Debug)]
struct Mdns {
  id: EndpointId,
  inner: Mutex<Option<MdnsAddressLookup>>,
  /// What we published last, handed to a freshly enabled service
  data: Mutex<Option<EndpointData>>,
  seen: broadcast::Sender<Seen>,
}

impl Mdns {
  fn set_enabled(&self, on: bool) -> Result<()> {
    let mut inner = self.inner.lock().unwrap();
    if on == inner.is_some() {
      return Ok(());
    }
    if !on {
      *inner = None;
      return Ok(());
    }
    let mdns = MdnsAddressLookup::builder()
      .service_name(SERVICE)
      .build(self.id)
      .map_err(|e| anyhow!("mDNS: {e}"))?;
    if let Some(data) = &*self.data.lock().unwrap() {
      mdns.publish(data);
    }
    let (me, tx, events) = (self.id, self.seen.clone(), mdns.clone());
    tokio::spawn(async move {
      // ends when the service is dropped
      let mut stream = events.subscribe().await;
      while let Some(event) = stream.next().await {
        let seen = match event {
          DiscoveryEvent::Discovered { endpoint_info, .. } if endpoint_info.endpoint_id != me => {
            Seen::Found {
              id: *endpoint_info.endpoint_id.as_bytes(),
              addrs: endpoint_info.ip_addrs().copied().collect(),
            }
          }
          DiscoveryEvent::Expired { endpoint_id } => Seen::Gone(*endpoint_id.as_bytes()),
          _ => continue,
        };
        let _ = tx.send(seen);
      }
    });
    *inner = Some(mdns);
    Ok(())
  }
}

impl AddressLookup for Mdns {
  fn publish(&self, data: &EndpointData) {
    *self.data.lock().unwrap() = Some(data.clone());
    if let Some(mdns) = &*self.inner.lock().unwrap() {
      mdns.publish(data);
    }
  }

  fn resolve(&self, id: EndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
    self.inner.lock().unwrap().as_ref()?.resolve(id)
  }
}

/// An iroh endpoint: n0's DNS lookup and relays when `relay` is set, plus mDNS
/// on the local network while discovery is on
#[derive(Clone)]
pub struct Node {
  ep: Endpoint,
  mdns: Arc<Mdns>,
}

impl Node {
  pub async fn bind(key: SecretKey, port: Option<u16>, relay: bool) -> Result<Self> {
    let builder = if relay {
      Endpoint::builder(presets::N0)
    } else {
      Endpoint::builder(presets::Minimal)
    };
    let mdns = Arc::new(Mdns {
      id: key.public(),
      inner: Mutex::new(None),
      data: Mutex::new(None),
      seen: broadcast::channel(64).0,
    });
    let ep = builder
      .secret_key(key)
      .alpns(vec![ALPN.to_vec()])
      .bind_addr((Ipv4Addr::UNSPECIFIED, port.unwrap_or(0)))?
      .bind()
      .await?;
    ep.address_lookup()?.add(mdns.clone());
    Ok(Self { ep, mdns })
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

  /// Devices mDNS finds or loses
  pub fn seen(&self) -> broadcast::Receiver<Seen> {
    self.mdns.seen.subscribe()
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

  /// Next incoming connection, `None` once the node is closed
  pub async fn accept(&self) -> Option<Result<Connection>> {
    let incoming = self.ep.accept().await?;
    Some(async { Ok(incoming.accept()?.await?) }.await)
  }

  pub async fn close(&self) {
    self.mdns.set_enabled(false).ok();
    self.ep.close().await;
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

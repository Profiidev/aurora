use std::{
  net::{Ipv4Addr, SocketAddr},
  path::Path,
};

use anyhow::{Result, anyhow};
use aurora_proto::{ALPN, Id};
use iroh::{
  Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
  endpoint::{Connection as IrohConn, RecvStream as IrohRecv, SendStream as IrohSend, presets},
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

/// An iroh endpoint on the local network: direct addresses only, no relay
#[derive(Clone)]
pub struct Node {
  ep: Endpoint,
}

impl Node {
  pub async fn bind(key: SecretKey, port: Option<u16>) -> Result<Self> {
    let ep = Endpoint::builder(presets::Minimal)
      .secret_key(key)
      .alpns(vec![ALPN.to_vec()])
      .bind_addr((Ipv4Addr::UNSPECIFIED, port.unwrap_or(0)))?
      .bind()
      .await?;
    Ok(Self { ep })
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

  /// Direct addresses a peer can dial
  pub fn addrs(&self) -> Vec<SocketAddr> {
    (self.ep.addr().addrs.iter())
      .filter_map(|a| match a {
        TransportAddr::Ip(a) => Some(*a),
        _ => None,
      })
      .collect()
  }

  pub async fn connect(&self, id: Id, addrs: &[SocketAddr]) -> Result<Connection> {
    let id = EndpointId::from_bytes(&id)?;
    let addr = EndpointAddr::from_parts(id, addrs.iter().map(|a| TransportAddr::Ip(*a)));
    Ok(self.ep.connect(addr, ALPN).await?)
  }

  /// Next incoming connection, `None` once the node is closed
  pub async fn accept(&self) -> Option<Result<Connection>> {
    let incoming = self.ep.accept().await?;
    Some(async { Ok(incoming.accept()?.await?) }.await)
  }

  pub async fn close(&self) {
    self.ep.close().await;
  }
}

/// Endpoint id of the peer on `conn`
pub fn remote_id(conn: &Connection) -> Id {
  *conn.remote_id().as_bytes()
}

/// Random bytes for nonces and tokens
pub fn random_bytes<const N: usize>() -> [u8; N] {
  let mut out = [0u8; N];
  let key = SecretKey::generate().to_bytes();
  out.copy_from_slice(&key[..N]);
  out
}

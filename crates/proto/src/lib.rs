//! Postcard wire messages exchanged between aurorad and the mobile app.

use std::net::SocketAddr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
pub const ALPN: &[u8] = b"aurora/1";
/// UDP port answering [`DISCOVERY_QUERY`] with a postcard [`Beacon`]
pub const DISCOVERY_PORT: u16 = 47421;
pub const DISCOVERY_QUERY: &[u8] = b"aurora?";

const MAX_FRAME: u32 = 64 * 1024;

pub type Id = [u8; 32];
pub type Token = [u8; 16];

/// Discovery reply: who answers and which port its QUIC endpoint listens on
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beacon {
  pub id: Id,
  pub name: String,
  pub os: String,
  pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Msg {
  /// First message of a pairing stream, sent by the dialer
  PairHello {
    name: String,
    os: String,
    nonce: [u8; 16],
    token: Option<Token>,
    /// Where the dialer can be reached, so the acceptor can reconnect later
    addrs: Vec<SocketAddr>,
  },
  /// Acceptor's answer to [`Msg::PairHello`]
  PairNonce {
    name: String,
    os: String,
    nonce: [u8; 16],
    addrs: Vec<SocketAddr>,
  },
  PairDecision {
    accept: bool,
  },
  /// First message of a session stream between paired devices, both ways
  Hello {
    name: String,
    os: String,
    battery: Option<u8>,
    charging: bool,
  },
  /// Sender removed the pairing
  Unpair,
}

pub fn encode_beacon(beacon: &Beacon) -> Result<Vec<u8>> {
  Ok(postcard::to_stdvec(beacon)?)
}

pub fn decode_beacon(bytes: &[u8]) -> Result<Beacon> {
  Ok(postcard::from_bytes(bytes)?)
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
  let buf = postcard::to_stdvec(msg)?;
  w.write_all(&(buf.len() as u32).to_be_bytes()).await?;
  w.write_all(&buf).await?;
  Ok(())
}

pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
  let mut len = [0u8; 4];
  r.read_exact(&mut len).await?;
  let len = u32::from_be_bytes(len);
  if len > MAX_FRAME {
    bail!("frame too large: {len}");
  }
  let mut buf = vec![0u8; len as usize];
  r.read_exact(&mut buf).await?;
  Ok(postcard::from_bytes(&buf)?)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn frames_round_trip() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let msg = Msg::PairHello {
      name: "x".into(),
      os: "Android".into(),
      nonce: [7; 16],
      token: Some([1; 16]),
      addrs: vec![],
    };
    write_msg(&mut a, &msg).await.unwrap();
    assert_eq!(read_msg(&mut b).await.unwrap(), msg);
  }
}

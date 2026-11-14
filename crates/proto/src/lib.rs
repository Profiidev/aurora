//! Postcard wire messages exchanged between aurorad and the mobile app.

use std::net::SocketAddr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 3;
pub const ALPN: &[u8] = b"aurora/3";

const MAX_FRAME: u32 = 64 * 1024;

pub type Id = [u8; 32];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Handshakes: pairing, the session `Hello` and the nearby `Who`. Everything
/// a session carries afterwards is an [`Envelope`].
pub enum Msg {
  /// First message of a pairing stream, sent by the dialer
  PairHello {
    name: String,
    os: String,
    nonce: [u8; 16],
    /// Where the dialer can be reached, so the acceptor can reconnect later
    relay: Option<String>,
    addrs: Vec<SocketAddr>,
  },
  /// Acceptor's answer to [`Msg::PairHello`]
  PairNonce {
    name: String,
    os: String,
    nonce: [u8; 16],
    relay: Option<String>,
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
  /// Answer to a [`Msg::Hello`] from a device we no longer know
  Unpair,
  /// Asks a device found on the local network for its name
  Who,
  /// Answer to [`Msg::Who`]
  Info {
    name: String,
    os: String,
  },
}

/// First frame of every stream in a session between paired devices: which
/// registered message it carries and its postcard body
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
  /// `area:verb`, like the IPC commands
  pub kind: String,
  pub body: Vec<u8>,
}

/// Answer to a request [`Envelope`]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Reply {
  Ok(Vec<u8>),
  Err(String),
  /// The receiver has no handler for this kind (an older or newer build)
  Unsupported,
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
  Ok(postcard::to_stdvec(value)?)
}

pub fn decode<T: DeserializeOwned>(buf: &[u8]) -> Result<T> {
  Ok(postcard::from_bytes(buf)?)
}

/// One length-prefixed postcard frame
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
  let buf = encode(value)?;
  w.write_all(&(buf.len() as u32).to_be_bytes()).await?;
  w.write_all(&buf).await?;
  Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
  let mut len = [0u8; 4];
  r.read_exact(&mut len).await?;
  let len = u32::from_be_bytes(len);
  if len > MAX_FRAME {
    bail!("frame too large: {len}");
  }
  let mut buf = vec![0u8; len as usize];
  r.read_exact(&mut buf).await?;
  decode(&buf)
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
  write_frame(w, msg).await
}

pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
  read_frame(r).await
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
      relay: None,
      addrs: vec![],
    };
    for msg in [
      msg,
      Msg::Who,
      Msg::Info {
        name: "home".into(),
        os: "Linux".into(),
      },
    ] {
      write_msg(&mut a, &msg).await.unwrap();
      assert_eq!(read_msg(&mut b).await.unwrap(), msg);
    }
  }

  #[tokio::test]
  async fn envelopes_and_replies_round_trip() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let env = Envelope {
      kind: "status:battery".into(),
      body: encode(&(Some(80u8), true)).unwrap(),
    };
    write_frame(&mut a, &env).await.unwrap();
    let got: Envelope = read_frame(&mut b).await.unwrap();
    assert_eq!(got, env);
    assert_eq!(
      decode::<(Option<u8>, bool)>(&got.body).unwrap(),
      (Some(80), true)
    );
    for reply in [
      Reply::Ok(vec![1]),
      Reply::Err("no".into()),
      Reply::Unsupported,
    ] {
      write_frame(&mut a, &reply).await.unwrap();
      assert_eq!(read_frame::<_, Reply>(&mut b).await.unwrap(), reply);
    }
  }

  #[tokio::test]
  async fn rejects_oversize_frames() {
    let (mut a, mut b) = tokio::io::duplex(16);
    a.write_all(&(MAX_FRAME + 1).to_be_bytes()).await.unwrap();
    assert!(read_frame::<_, Msg>(&mut b).await.is_err());
  }
}

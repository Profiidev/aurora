//! Byte-pipe framing for links: a hello naming the endpoint, then QUIC
//! datagrams prefixed with their `u16` length.

use std::io;

use aurora_proto::Id;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAGIC: &[u8; 4] = b"AURL";
const VERSION: u8 = 1;

pub async fn write_hello<W: AsyncWrite + Unpin>(w: &mut W, me: &Id) -> io::Result<()> {
  let mut buf = [0u8; 37];
  buf[..4].copy_from_slice(MAGIC);
  buf[4] = VERSION;
  buf[5..].copy_from_slice(me);
  w.write_all(&buf).await?;
  w.flush().await
}

/// The endpoint id the other side claims; QUIC's TLS proves it later
pub async fn read_hello<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Id> {
  let mut buf = [0u8; 37];
  r.read_exact(&mut buf).await?;
  if &buf[..4] != MAGIC {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      "not an aurora link",
    ));
  }
  if buf[4] != VERSION {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      format!("link version {} unsupported", buf[4]),
    ));
  }
  Ok(buf[5..].try_into().unwrap())
}

/// Writes one datagram; the caller flushes once its queue is empty
pub async fn write_datagram<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> io::Result<()> {
  let len = u16::try_from(data.len())
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "datagram over 64 KiB"))?;
  w.write_all(&len.to_be_bytes()).await?;
  w.write_all(data).await
}

/// Next datagram, `None` on a clean end of the pipe
pub async fn read_datagram<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
  let mut len = [0u8; 2];
  match r.read_exact(&mut len).await {
    Ok(_) => {}
    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
    Err(e) => return Err(e),
  }
  let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
  r.read_exact(&mut buf).await?;
  Ok(Some(buf))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn hello_and_datagrams_round_trip() {
    let (mut a, mut b) = tokio::io::duplex(1 << 16);
    write_hello(&mut a, &[9; 32]).await.unwrap();
    assert_eq!(read_hello(&mut b).await.unwrap(), [9; 32]);
    for d in [&b""[..], b"x", &[7u8; 1500]] {
      write_datagram(&mut a, d).await.unwrap();
      assert_eq!(read_datagram(&mut b).await.unwrap().unwrap(), d);
    }
    drop(a);
    assert!(read_datagram(&mut b).await.unwrap().is_none());
  }

  #[tokio::test]
  async fn rejects_bad_hello_and_oversize() {
    let (mut a, mut b) = tokio::io::duplex(1 << 17);
    a.write_all(&[0u8; 37]).await.unwrap();
    assert!(read_hello(&mut b).await.is_err());
    assert!(write_datagram(&mut a, &vec![0; 70_000]).await.is_err());
  }
}

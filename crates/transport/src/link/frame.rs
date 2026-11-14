//! Byte-pipe framing for links. Every frame starts with a marker, so a reader
//! that joins mid-stream (stale bytes from an earlier session in a USB
//! buffer) skips ahead to the next frame instead of failing:
//!
//! `"AU"`, kind (1 byte), payload length (u16 big-endian), payload
//!
//! A hello names the sender's endpoint id, and whether it wants a hello back;
//! a datagram carries one QUIC packet.

use std::{collections::VecDeque, io};

use aurora_proto::Id;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::debug;

const MARKER: [u8; 2] = *b"AU";
const VERSION: u8 = 2;
const HELLO: u8 = 1;
const DATAGRAM: u8 = 2;
/// version, wants-reply flag, endpoint id
const HELLO_LEN: usize = 2 + 32;

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
  /// The id the sender claims (QUIC's TLS proves it later); `reply`: answer
  /// with a hello of our own
  Hello {
    id: Id,
    reply: bool,
  },
  Datagram(Vec<u8>),
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
  let len = u16::try_from(payload.len())
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame over 64 KiB"))?;
  let mut head = [0u8; 5];
  head[..2].copy_from_slice(&MARKER);
  head[2] = kind;
  head[3..].copy_from_slice(&len.to_be_bytes());
  w.write_all(&head).await?;
  w.write_all(payload).await
}

pub async fn write_hello<W: AsyncWrite + Unpin>(w: &mut W, me: &Id, reply: bool) -> io::Result<()> {
  let mut payload = [0u8; HELLO_LEN];
  payload[0] = VERSION;
  payload[1] = reply.into();
  payload[2..].copy_from_slice(me);
  write_frame(w, HELLO, &payload).await?;
  w.flush().await
}

/// Writes one datagram; the caller flushes once its queue is empty
pub async fn write_datagram<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> io::Result<()> {
  write_frame(w, DATAGRAM, data).await
}

/// Reads frames, skipping bytes that do not form one
pub struct FrameReader<R> {
  inner: R,
  /// Bytes read ahead that turned out not to start a frame
  back: VecDeque<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
  pub fn new(inner: R) -> Self {
    Self {
      inner,
      back: VecDeque::new(),
    }
  }

  async fn byte(&mut self) -> io::Result<Option<u8>> {
    if let Some(b) = self.back.pop_front() {
      return Ok(Some(b));
    }
    match self.inner.read_u8().await {
      Ok(b) => Ok(Some(b)),
      Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
      Err(e) => Err(e),
    }
  }

  async fn exact(&mut self, buf: &mut [u8]) -> io::Result<bool> {
    for b in buf.iter_mut() {
      let Some(next) = self.byte().await? else {
        return Ok(false);
      };
      *b = next;
    }
    Ok(true)
  }

  /// Next frame, `None` once the pipe ends
  pub async fn next(&mut self) -> io::Result<Option<Frame>> {
    let mut skipped = 0usize;
    let mut prev = None;
    loop {
      let Some(b) = self.byte().await? else {
        return Ok(None);
      };
      if (prev, b) != (Some(MARKER[0]), MARKER[1]) {
        skipped += usize::from(prev.is_some());
        prev = Some(b);
        continue;
      }
      prev = None;
      let mut head = [0u8; 3];
      if !self.exact(&mut head).await? {
        return Ok(None);
      }
      let (kind, len) = (head[0], u16::from_be_bytes([head[1], head[2]]) as usize);
      let valid = match kind {
        HELLO => len == HELLO_LEN,
        DATAGRAM => true,
        _ => false,
      };
      if !valid {
        // a marker inside other bytes: scan on right after it
        skipped += 2;
        for b in head.into_iter().rev() {
          self.back.push_front(b);
        }
        continue;
      }
      let mut payload = vec![0u8; len];
      if !self.exact(&mut payload).await? {
        return Ok(None);
      }
      if skipped > 0 {
        debug!("link skipped {skipped} stray bytes");
      }
      return match kind {
        HELLO if payload[0] != VERSION => Err(io::Error::new(
          io::ErrorKind::InvalidData,
          format!("link version {} unsupported", payload[0]),
        )),
        HELLO => Ok(Some(Frame::Hello {
          id: payload[2..].try_into().unwrap(),
          reply: payload[1] != 0,
        })),
        _ => Ok(Some(Frame::Datagram(payload))),
      };
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn frames_round_trip() {
    let (mut a, mut b) = tokio::io::duplex(1 << 17);
    write_hello(&mut a, &[9; 32], true).await.unwrap();
    assert_eq!(
      FrameReader::new(&mut b).next().await.unwrap(),
      Some(Frame::Hello {
        id: [9; 32],
        reply: true
      })
    );
    for d in [&b""[..], b"x", b"AUAU", &[7u8; 1500]] {
      write_datagram(&mut a, d).await.unwrap();
      assert_eq!(
        FrameReader::new(&mut b).next().await.unwrap(),
        Some(Frame::Datagram(d.to_vec()))
      );
    }
    drop(a);
    assert_eq!(FrameReader::new(&mut b).next().await.unwrap(), None);
  }

  #[tokio::test]
  async fn skips_stray_bytes_before_a_frame() {
    let (mut a, mut b) = tokio::io::duplex(1 << 16);
    // tail of an old session, a fake marker with a bad kind, then a hello
    a.write_all(b"\x00\x05garbageAAU\x09\x00\x01")
      .await
      .unwrap();
    write_hello(&mut a, &[3; 32], false).await.unwrap();
    assert_eq!(
      FrameReader::new(&mut b).next().await.unwrap(),
      Some(Frame::Hello {
        id: [3; 32],
        reply: false
      })
    );
  }

  #[tokio::test]
  async fn a_bad_header_does_not_eat_the_next_frame() {
    let (mut a, b) = tokio::io::duplex(1 << 16);
    // its length bytes would swallow the hello's marker without pushback
    a.write_all(b"AU\xff\x00").await.unwrap();
    write_hello(&mut a, &[4; 32], false).await.unwrap();
    write_datagram(&mut a, b"x").await.unwrap();
    let mut r = FrameReader::new(b);
    assert!(matches!(r.next().await.unwrap(), Some(Frame::Hello { .. })));
    assert_eq!(
      r.next().await.unwrap(),
      Some(Frame::Datagram(b"x".to_vec()))
    );
  }

  #[tokio::test]
  async fn rejects_oversize_and_other_versions() {
    let (mut a, mut b) = tokio::io::duplex(1 << 17);
    assert!(write_datagram(&mut a, &vec![0; 70_000]).await.is_err());
    let mut hello = [0u8; HELLO_LEN];
    hello[0] = 9;
    write_frame(&mut a, HELLO, &hello).await.unwrap();
    assert!(FrameReader::new(&mut b).next().await.is_err());
  }
}

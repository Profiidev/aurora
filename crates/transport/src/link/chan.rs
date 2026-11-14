//! A link over byte channels, for pipes that are not Rust I/O objects: the
//! accessory fd read by blocking threads, a Bluetooth socket owned by Java
//! or Swift. Whatever pumps the bytes holds the [`PipeEnds`]; dropping either
//! side ends the other.

use std::{
  io,
  pin::Pin,
  task::{Context, Poll, ready},
};

use tokio::{
  io::{AsyncRead, AsyncWrite, ReadBuf},
  sync::mpsc,
};

/// The platform side of a [`pipe`]
pub struct PipeEnds {
  /// Bytes from the other device; dropping it is end of file for the link
  pub incoming: mpsc::Sender<Vec<u8>>,
  /// Bytes for the other device; `None` once the link is gone
  pub outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
}

/// A byte pipe for `Links::attach`: the platform side, and the reader and
/// writer for the link
pub fn pipe() -> (PipeEnds, ChanRead, ChanWrite) {
  let (in_tx, in_rx) = mpsc::channel(64);
  let (out_tx, out_rx) = mpsc::unbounded_channel();
  (
    PipeEnds {
      incoming: in_tx,
      outgoing: out_rx,
    },
    ChanRead {
      rx: in_rx,
      buf: Vec::new(),
      pos: 0,
    },
    ChanWrite(out_tx),
  )
}

/// Bytes from the platform side
pub struct ChanRead {
  rx: mpsc::Receiver<Vec<u8>>,
  buf: Vec<u8>,
  pos: usize,
}

impl AsyncRead for ChanRead {
  fn poll_read(
    mut self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    out: &mut ReadBuf<'_>,
  ) -> Poll<io::Result<()>> {
    if self.pos == self.buf.len() {
      match ready!(self.rx.poll_recv(cx)) {
        Some(data) => (self.buf, self.pos) = (data, 0),
        // the platform side is gone: end of file
        None => return Poll::Ready(Ok(())),
      }
    }
    let n = out.remaining().min(self.buf.len() - self.pos);
    out.put_slice(&self.buf[self.pos..self.pos + n]);
    self.pos += n;
    Poll::Ready(Ok(()))
  }
}

/// Bytes for the platform side; QUIC's congestion control bounds the queue
pub struct ChanWrite(mpsc::UnboundedSender<Vec<u8>>);

impl AsyncWrite for ChanWrite {
  fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
    Poll::Ready(
      (self.0.send(buf.to_vec()))
        .map(|()| buf.len())
        .map_err(|_| io::ErrorKind::BrokenPipe.into()),
    )
  }

  fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }

  fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
    Poll::Ready(Ok(()))
  }
}

#[cfg(test)]
mod tests {
  use tokio::io::{AsyncReadExt, AsyncWriteExt};

  use super::*;

  #[tokio::test]
  async fn bytes_flow_and_drops_end_both_ways() {
    let (mut ends, mut r, mut w) = pipe();
    ends.incoming.send(b"hello".to_vec()).await.unwrap();
    let mut buf = [0u8; 3];
    r.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hel");
    w.write_all(b"out").await.unwrap();
    assert_eq!(ends.outgoing.recv().await.unwrap(), b"out");

    drop(ends.incoming);
    let mut rest = Vec::new();
    r.read_to_end(&mut rest).await.unwrap();
    assert_eq!(rest, b"lo", "buffered bytes, then end of file");

    drop(ends.outgoing);
    assert!(w.write_all(b"x").await.is_err());
  }
}

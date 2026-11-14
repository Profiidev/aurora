//! Phone side: the accessory file descriptor from Android's `UsbManager`.

use std::{
  fs::File,
  io::{self, Read, Write},
  os::fd::OwnedFd,
  pin::Pin,
  task::{Context, Poll, ready},
};

use tokio::{
  io::{AsyncRead, AsyncWrite, ReadBuf},
  sync::mpsc,
};
use tracing::info;

use crate::link::{LinkKind, Links};

/// Bigger than any transfer the host sends; smaller reads fail on AOA
const READ_BUF: usize = 64 * 1024;
const MAX_EMPTY_READS: u32 = 8;

/// Runs USB links over the accessory `fd` until the cable goes. Blocking
/// threads do the I/O (the accessory fd does not support polling) and talk
/// to the link through channels, so either side going away ends the other.
pub async fn attach_fd(links: &Links, fd: OwnedFd) -> io::Result<()> {
  let mut read_file = File::from(fd);
  let mut write_file = read_file.try_clone()?;
  let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);
  let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
  std::thread::Builder::new()
    .name("usb-read".into())
    .spawn(move || {
      let mut buf = vec![0u8; READ_BUF];
      let mut empty = 0;
      loop {
        match read_file.read(&mut buf) {
          // a zero-length packet ends a transfer, not the link; the host sends
          // at most one per flush, so a run of them is end of file
          Ok(0) => {
            empty += 1;
            if empty > MAX_EMPTY_READS {
              break;
            }
          }
          Ok(n) => {
            empty = 0;
            // fails once the link side is gone
            if in_tx.blocking_send(buf[..n].to_vec()).is_err() {
              break;
            }
          }
          Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
          Err(_) => break,
        }
      }
    })?;
  std::thread::Builder::new()
    .name("usb-write".into())
    .spawn(move || {
      while let Some(data) = out_rx.blocking_recv() {
        if write_file.write_all(&data).is_err() {
          break;
        }
      }
    })?;

  let mut reader = ChanRead {
    rx: in_rx,
    buf: Vec::new(),
    pos: 0,
  };
  let mut writer = ChanWrite(out_tx);
  loop {
    match links
      .attach_waiting(LinkKind::Usb, &mut reader, &mut writer)
      .await
    {
      // the cable went
      Ok(()) => return Ok(()),
      Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
      // another computer, or the same one restarted: start over on this fd
      Err(e) => info!("USB link restarts: {e}"),
    }
  }
}

/// Bytes from the read thread
struct ChanRead {
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
        // the read thread ended: end of file
        None => return Poll::Ready(Ok(())),
      }
    }
    let n = out.remaining().min(self.buf.len() - self.pos);
    out.put_slice(&self.buf[self.pos..self.pos + n]);
    self.pos += n;
    Poll::Ready(Ok(()))
  }
}

/// Bytes for the write thread; QUIC's congestion control bounds the queue
struct ChanWrite(mpsc::UnboundedSender<Vec<u8>>);

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

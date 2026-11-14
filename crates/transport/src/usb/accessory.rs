//! Phone side: the accessory file descriptor from Android's `UsbManager`.

use std::{
  fs::File,
  io::{self, Read, Write},
  os::fd::OwnedFd,
};

use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  runtime::Handle,
};

use crate::link::{LinkKind, Links};

/// Bigger than any transfer the host sends; smaller reads fail on AOA
const READ_BUF: usize = 64 * 1024;
const MAX_EMPTY_READS: u32 = 8;

/// Runs a USB link over the accessory `fd` until the cable goes. Blocking
/// threads do the I/O: the accessory fd does not support polling.
pub async fn attach_fd(links: &Links, fd: OwnedFd) -> io::Result<()> {
  let mut read_file = File::from(fd);
  let mut write_file = read_file.try_clone()?;
  // one pipe per direction, so the end of either reaches the other side
  let (link_r, mut to_us) = tokio::io::simplex(READ_BUF);
  let (mut from_us, link_w) = tokio::io::simplex(READ_BUF);
  let rt = Handle::current();
  let rt2 = rt.clone();
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
            if rt.block_on(to_us.write_all(&buf[..n])).is_err() {
              break;
            }
          }
          Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
          Err(_) => break,
        }
      }
      // ends the link's reader
      let _ = rt.block_on(to_us.shutdown());
    })?;
  std::thread::Builder::new()
    .name("usb-write".into())
    .spawn(move || {
      let mut buf = vec![0u8; READ_BUF];
      loop {
        match rt2.block_on(from_us.read(&mut buf)) {
          Ok(0) | Err(_) => break,
          Ok(n) => {
            if write_file.write_all(&buf[..n]).is_err() {
              break;
            }
          }
        }
      }
    })?;
  links.attach_waiting(LinkKind::Usb, link_r, link_w).await
  // the read thread stays blocked on the fd until the cable goes; the write
  // thread ends once `link_w` is dropped
}

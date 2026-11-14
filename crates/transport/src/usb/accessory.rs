//! Phone side: the accessory file descriptor from Android's `UsbManager`.

use std::{
  fs::File,
  io::{self, Read, Write},
  os::{fd::OwnedFd, unix::thread::JoinHandleExt},
  sync::{
    Arc, Once,
    atomic::{AtomicBool, Ordering},
  },
  thread::JoinHandle,
  time::Duration,
};

use tracing::info;

use crate::link::{
  LinkKind, Links,
  chan::{self, PipeEnds},
};

/// Bigger than any transfer the host sends; smaller reads fail on AOA
const READ_BUF: usize = 64 * 1024;
const MAX_EMPTY_READS: u32 = 8;

/// Runs USB links over the accessory `fd` until the cable goes. Blocking
/// threads do the I/O (the accessory fd does not support polling) and talk
/// to the link through channels, so either side going away ends the other.
///
/// Some kernels never wake a read blocked on the accessory once the cable
/// is pulled, which keeps `/dev/usb_accessory` open so the phone cannot
/// enter accessory mode again. Drop this future when Android reports the
/// accessory detached: that interrupts the read and closes the fd.
pub async fn attach_fd(links: &Links, fd: OwnedFd) -> io::Result<()> {
  let mut read_file = File::from(fd);
  let mut write_file = read_file.try_clone()?;
  let (ends, mut reader, mut writer) = chan::pipe();
  let PipeEnds {
    incoming: in_tx,
    outgoing: mut out_rx,
  } = ends;
  let stop = Arc::new(AtomicBool::new(false));
  install_wakeup();
  let read_stop = stop.clone();
  let reader_thread = std::thread::Builder::new()
    .name("usb-read".into())
    .spawn(move || {
      let mut buf = vec![0u8; READ_BUF];
      let mut empty = 0;
      loop {
        if read_stop.load(Ordering::Acquire) {
          break;
        }
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
  let _interrupt = Interrupt {
    stop,
    thread: Some(reader_thread),
  };
  std::thread::Builder::new()
    .name("usb-write".into())
    .spawn(move || {
      while let Some(data) = out_rx.blocking_recv() {
        if write_file.write_all(&data).is_err() {
          break;
        }
      }
    })?;

  loop {
    match links
      .attach_waiting(LinkKind::Usb, &mut reader, &mut writer)
      .await
    {
      // another computer, or the same one restarted: start over on this fd
      Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
        info!("USB link restarts: {e}");
      }
      // the cable went
      Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
      res => return res,
    }
  }
}

/// Stops the read thread when the link ends, by a flag and a signal that
/// breaks its blocking read with `EINTR`
struct Interrupt {
  stop: Arc<AtomicBool>,
  thread: Option<JoinHandle<()>>,
}

impl Drop for Interrupt {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::Release);
    let Some(thread) = self.thread.take() else {
      return;
    };
    // the signal can land just before the thread enters `read`; repeat it
    std::thread::spawn(move || {
      for _ in 0..50 {
        if thread.is_finished() {
          return;
        }
        // SAFETY: the thread is alive (not finished, not joined), so its
        // pthread_t is valid; the handler does nothing
        unsafe { libc::pthread_kill(thread.as_pthread_t() as _, WAKEUP) };
        std::thread::sleep(Duration::from_millis(100));
      }
    });
  }
}

/// Rarely used, and nothing else in the app handles it
const WAKEUP: libc::c_int = libc::SIGURG;

/// A no-op handler without `SA_RESTART`, so a blocked `read` returns `EINTR`
fn install_wakeup() {
  static ONCE: Once = Once::new();
  extern "C" fn wake(_: libc::c_int) {}
  ONCE.call_once(|| {
    // SAFETY: plain sigaction with an empty async-signal-safe handler
    unsafe {
      let mut action: libc::sigaction = std::mem::zeroed();
      action.sa_sigaction = wake as *const () as libc::sighandler_t;
      libc::sigemptyset(&mut action.sa_mask);
      libc::sigaction(WAKEUP, &action, std::ptr::null_mut());
    }
  });
}

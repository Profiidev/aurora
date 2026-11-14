//! Bluetooth fallback on iOS. `AuroraBluetooth.swift` owns CoreBluetooth
//! (scanning, the L2CAP channel's streams) and calls these C functions; the
//! link runs here, over a channel pipe per L2CAP channel.
//!
//! Platform-neutral on purpose, so host tests cover it.

use std::{
  collections::HashMap,
  sync::{Arc, Mutex, OnceLock},
};

use aurora_core::engine::{Engine, PresenceGuard};
use aurora_transport::{
  LinkKind,
  bluetooth::decode_advert,
  link::chan::{self, PipeEnds},
};
use tokio::{runtime::Handle, sync::mpsc::error::TryRecvError};

struct State {
  rt: Handle,
  engine: Arc<Engine>,
  /// Held while the app is active
  presence: Mutex<Option<PresenceGuard>>,
  channels: Mutex<HashMap<u64, Channel>>,
}

struct Channel {
  ends: PipeEnds,
  /// Taken from the link, not yet copied out to Swift
  pending: Vec<u8>,
}

static STATE: OnceLock<State> = OnceLock::new();

/// Called once the engine runs
pub fn init(rt: Handle, engine: Arc<Engine>) {
  let _ = STATE.set(State {
    rt,
    engine,
    presence: Mutex::new(None),
    channels: Mutex::default(),
  });
}

fn state() -> Option<&'static State> {
  STATE.get()
}

/// The app became active (`true`) or resigned active (`false`)
#[unsafe(no_mangle)]
pub extern "C" fn aurora_bt_set_foreground(on: bool) {
  let Some(s) = state() else { return };
  let mut presence = s.presence.lock().unwrap();
  match (on, presence.is_some()) {
    (true, false) => *presence = Some(s.engine.ui_presence()),
    (false, true) => *presence = None,
    _ => {}
  }
}

/// Scan now
#[unsafe(no_mangle)]
pub extern "C" fn aurora_bt_wanted() -> bool {
  state().is_some_and(|s| s.engine.bluetooth_wanted())
}

/// An advert's manufacturer data (without the company id): dial it? The
/// PSM to open goes to `psm`.
///
/// # Safety
/// `data` points to `len` readable bytes, `psm` to a writable `u16`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aurora_bt_should_dial(data: *const u8, len: usize, psm: *mut u16) -> bool {
  let Some(s) = state() else { return false };
  if data.is_null() || psm.is_null() {
    return false;
  }
  // SAFETY: the caller's contract
  let data = unsafe { std::slice::from_raw_parts(data, len) };
  match decode_advert(data) {
    Some((p, prefix)) if s.engine.should_dial_bt(&prefix) => {
      // SAFETY: the caller's contract
      unsafe { *psm = p };
      true
    }
    _ => false,
  }
}

/// An L2CAP channel opened; Swift names it by `handle`
#[unsafe(no_mangle)]
pub extern "C" fn aurora_bt_opened(handle: u64) {
  let Some(s) = state() else { return };
  let (ends, reader, writer) = chan::pipe();
  s.channels.lock().unwrap().insert(
    handle,
    Channel {
      ends,
      pending: Vec::new(),
    },
  );
  let links = s.engine.links().clone();
  s.rt.spawn(async move {
    if let Err(e) = links.attach(LinkKind::Bluetooth, reader, writer).await {
      log::info!("Bluetooth link ended: {e}");
    }
  });
}

/// Bytes the channel received
///
/// # Safety
/// `data` points to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aurora_bt_received(handle: u64, data: *const u8, len: usize) {
  let Some(s) = state() else { return };
  if data.is_null() {
    return;
  }
  // SAFETY: the caller's contract
  let data = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
  if let Some(c) = s.channels.lock().unwrap().get(&handle) {
    // a full queue drops the bytes; the link framing resyncs, QUIC resends
    let _ = c.ends.incoming.try_send(data);
  }
}

/// Copies bytes to send into `buf`: how many, 0 for none yet, -1 once the
/// link is over and Swift should close the channel
///
/// # Safety
/// `buf` points to `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aurora_bt_take_outgoing(handle: u64, buf: *mut u8, cap: usize) -> isize {
  let Some(s) = state() else { return -1 };
  let mut channels = s.channels.lock().unwrap();
  let Some(c) = channels.get_mut(&handle) else {
    return -1;
  };
  while c.pending.len() < cap {
    match c.ends.outgoing.try_recv() {
      Ok(data) => c.pending.extend_from_slice(&data),
      Err(TryRecvError::Empty) => break,
      Err(TryRecvError::Disconnected) if c.pending.is_empty() => {
        channels.remove(&handle);
        return -1;
      }
      Err(TryRecvError::Disconnected) => break,
    }
  }
  let n = c.pending.len().min(cap);
  if n > 0 && !buf.is_null() {
    // SAFETY: the caller's contract, `n <= cap`
    unsafe { std::ptr::copy_nonoverlapping(c.pending.as_ptr(), buf, n) };
    c.pending.drain(..n);
  }
  n as isize
}

/// The channel closed on the Swift side: end the link
#[unsafe(no_mangle)]
pub extern "C" fn aurora_bt_closed(handle: u64) {
  if let Some(s) = state() {
    s.channels.lock().unwrap().remove(&handle);
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use super::*;

  #[test]
  fn ffi_moves_bytes_and_ends_links() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let settings = aurora_core::engine::Settings {
      name: "phone".into(),
      os: "iOS".into(),
      pairing_timeout: Duration::from_secs(60),
      discovery: false,
      port: None,
      relay: false,
      usb: false,
      bluetooth: true,
    };
    let engine = rt.block_on(Engine::start(settings, dir.path())).unwrap();
    init(rt.handle().clone(), engine.clone());

    // nobody looking and nothing to reach
    assert!(!aurora_bt_wanted());
    aurora_bt_set_foreground(true);
    let data = aurora_transport::bluetooth::encode_advert(0x81, &[7; 32]);
    let mut psm = 0u16;
    assert!(!unsafe { aurora_bt_should_dial(data.as_ptr(), data.len(), &mut psm) });

    // a channel: the link's hello comes out, a close ends it
    aurora_bt_opened(1);
    let mut buf = [0u8; 256];
    let mut n = 0;
    for _ in 0..50 {
      n = unsafe { aurora_bt_take_outgoing(1, buf.as_mut_ptr(), buf.len()) };
      if n > 0 {
        break;
      }
      std::thread::sleep(Duration::from_millis(20));
    }
    assert!(n > 0, "the link hello");
    assert_eq!(&buf[..2], b"AU");
    aurora_bt_closed(1);
    assert_eq!(
      unsafe { aurora_bt_take_outgoing(1, buf.as_mut_ptr(), buf.len()) },
      -1
    );
    aurora_bt_set_foreground(false);
    rt.block_on(engine.shutdown());
  }
}

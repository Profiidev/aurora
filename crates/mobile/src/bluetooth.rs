//! Bluetooth fallback through `dev.gpui.mobile.AuroraBluetooth`: while the
//! app is in the foreground and the engine wants Bluetooth, scan for the
//! desktop's advert, dial it over an L2CAP channel and run a link on it.

use std::{
  collections::HashSet,
  sync::{Arc, Mutex},
  thread::sleep,
  time::Duration,
};

use aurora_core::engine::Engine;
use aurora_transport::{
  LinkKind,
  bluetooth::decode_advert,
  link::chan::{self, PipeEnds},
};
use gpui_mobile::android::jni as h;

use crate::jvm;
use jni::objects::{JByteArray, JObject, JValue};
use tokio::runtime::Handle;

const CLASS: &str = "dev.gpui.mobile.AuroraBluetooth";
const TICK: Duration = Duration::from_secs(1);

fn call_err(env: &mut jni::Env, e: jni::errors::Error) -> String {
  env.exception_clear();
  e.to_string()
}

/// A static `(Landroid/content/Context;)Z` method
fn context_flag(name: &'static jni::strings::JNIStr) -> bool {
  jvm::with_env(|env| {
    let activity = jvm::context(env)?;
    let cls = jvm::class(env, CLASS)?;
    env
      .call_static_method(
        &cls,
        name,
        jni::jni_sig!("(Landroid/content/Context;)Z"),
        &[JValue::Object(&activity)],
      )
      .and_then(|v| v.z())
      .map_err(|e| call_err(env, e))
  })
  .unwrap_or(false)
}

fn scan(on: bool) {
  let _ = jvm::with_env(|env| {
    let activity = jvm::context(env)?;
    let cls = jvm::class(env, CLASS)?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("scan"),
        jni::jni_sig!("(Landroid/content/Context;Z)V"),
        &[JValue::Object(&activity), JValue::Bool(on)],
      )
      .map(|_| ())
      .map_err(|e| call_err(env, e))
  });
}

/// Next advert: device address and manufacturer data
fn poll() -> Option<(String, Vec<u8>)> {
  let line = jvm::with_env(|env| {
    let cls = jvm::class(env, CLASS)?;
    let obj = env
      .call_static_method(
        &cls,
        jni::jni_str!("poll"),
        jni::jni_sig!("()Ljava/lang/String;"),
        &[],
      )
      .and_then(|v| v.l())
      .map_err(|e| call_err(env, e))?;
    Ok((!obj.is_null()).then(|| h::get_string(env, &obj)))
  })
  .ok()??;
  let (addr, hex) = line.split_once('|')?;
  let data = (0..hex.len())
    .step_by(2)
    .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
    .collect::<Option<Vec<u8>>>()?;
  Some((addr.to_string(), data))
}

/// Blocks while connecting; the channel's handle
fn connect(addr: &str, psm: u16) -> Option<i32> {
  jvm::with_env(|env| {
    let activity = jvm::context(env)?;
    let cls = jvm::class(env, CLASS)?;
    let addr = env.new_string(addr).map_err(|e| call_err(env, e))?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("connect"),
        jni::jni_sig!("(Landroid/content/Context;Ljava/lang/String;I)I"),
        &[
          JValue::Object(&activity),
          JValue::Object(&addr),
          JValue::Int(psm.into()),
        ],
      )
      .and_then(|v| v.i())
      .map_err(|e| call_err(env, e))
  })
  .ok()
  .filter(|h| *h >= 0)
}

/// Blocks for the next bytes; `None` once the channel is closed
fn read(handle: i32) -> Option<Vec<u8>> {
  jvm::with_env(|env| {
    let cls = jvm::class(env, CLASS)?;
    let obj = env
      .call_static_method(
        &cls,
        jni::jni_str!("read"),
        jni::jni_sig!("(I)[B"),
        &[JValue::Int(handle)],
      )
      .and_then(|v| v.l())
      .map_err(|e| call_err(env, e))?;
    if obj.is_null() {
      return Ok(None);
    }
    let array = JByteArray::cast_local(env, obj).map_err(|e| call_err(env, e))?;
    env
      .convert_byte_array(&array)
      .map(Some)
      .map_err(|e| call_err(env, e))
  })
  .ok()
  .flatten()
}

fn write(handle: i32, data: &[u8]) -> bool {
  jvm::with_env(|env| {
    let cls = jvm::class(env, CLASS)?;
    let array = env
      .byte_array_from_slice(data)
      .map_err(|e| call_err(env, e))?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("write"),
        jni::jni_sig!("(I[B)Z"),
        &[JValue::Int(handle), JValue::Object(&JObject::from(array))],
      )
      .and_then(|v| v.z())
      .map_err(|e| call_err(env, e))
  })
  .unwrap_or(false)
}

fn close(handle: i32) {
  let _ = jvm::with_env(|env| {
    let cls = jvm::class(env, CLASS)?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("close"),
        jni::jni_sig!("(I)V"),
        &[JValue::Int(handle)],
      )
      .map(|_| ())
      .map_err(|e| call_err(env, e))
  });
}

/// Watches foreground and engine state and runs Bluetooth links
pub fn spawn(rt: Handle, engine: Arc<Engine>) {
  let res = std::thread::Builder::new()
    .name("bluetooth".into())
    .spawn(move || run(rt, engine));
  if let Err(e) = res {
    log::error!("cannot start the Bluetooth thread: {e}");
  }
}

fn run(rt: Handle, engine: Arc<Engine>) {
  let mut scanning = false;
  // addresses being dialed or linked, and their channel handles
  let open: Arc<Mutex<HashSet<String>>> = Arc::default();
  let handles: Arc<Mutex<Vec<i32>>> = Arc::default();
  loop {
    sleep(TICK);
    // the app UI holds `ui:present` while on screen; it also asks for the
    // permissions, this process has no activity to show a dialog
    let allowed = engine.bluetooth_allowed();
    let scan_now = allowed
      && engine.bluetooth_wanted()
      && context_flag(jni::jni_str!("available"))
      && context_flag(jni::jni_str!("granted"));
    if scan_now != scanning {
      scan(scan_now);
      scanning = scan_now;
    }
    if !allowed {
      // app off screen or switched off: Bluetooth stops entirely
      for handle in handles.lock().unwrap().drain(..) {
        close(handle);
      }
    }
    while let Some((addr, data)) = poll() {
      let Some((psm, prefix)) = decode_advert(&data) else {
        continue;
      };
      if !scanning || !engine.should_dial_bt(&prefix) || !open.lock().unwrap().insert(addr.clone())
      {
        continue;
      }
      let (engine, rt, open, handles) = (engine.clone(), rt.clone(), open.clone(), handles.clone());
      std::thread::spawn(move || {
        if let Some(handle) = connect(&addr, psm) {
          log::info!("Bluetooth channel to {addr}");
          handles.lock().unwrap().push(handle);
          run_link(&rt, &engine, handle);
          close(handle);
          handles.lock().unwrap().retain(|h| *h != handle);
        }
        open.lock().unwrap().remove(&addr);
      });
    }
  }
}

/// Pumps the channel through a link until either ends
fn run_link(rt: &Handle, engine: &Engine, handle: i32) {
  let (ends, reader, writer) = chan::pipe();
  let PipeEnds {
    incoming,
    mut outgoing,
  } = ends;
  std::thread::spawn(move || {
    while let Some(data) = read(handle) {
      if incoming.blocking_send(data).is_err() {
        break;
      }
    }
  });
  std::thread::spawn(move || {
    while let Some(data) = outgoing.blocking_recv() {
      if !write(handle, &data) {
        break;
      }
    }
  });
  let links = engine.links().clone();
  if let Err(e) = rt.block_on(links.attach(LinkKind::Bluetooth, reader, writer)) {
    log::info!("Bluetooth link ended: {e}");
  }
}

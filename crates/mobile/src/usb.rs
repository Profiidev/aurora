//! USB accessory link through `dev.gpui.mobile.AuroraUsb`: the computer
//! switches the phone to accessory mode, Android hands us its file descriptor.

use std::{
  os::fd::{FromRawFd, OwnedFd},
  sync::Arc,
  thread::sleep,
  time::Duration,
};

use aurora_core::engine::Engine;
use gpui_mobile::android::jni as h;
use jni::objects::JValue;
use tokio::runtime::Handle;

const CLASS: &str = "dev.gpui.mobile.AuroraUsb";
const POLL: Duration = Duration::from_secs(1);

/// Watches for the accessory while USB is on and runs a link over it
pub fn spawn(rt: Handle, engine: Arc<Engine>) {
  let res = std::thread::Builder::new()
    .name("usb-accessory".into())
    .spawn(move || {
      loop {
        sleep(POLL);
        if !engine.settings().usb {
          continue;
        }
        let fd = match open() {
          Ok(Some(fd)) => fd,
          Ok(None) => continue,
          Err(e) => {
            log::warn!("USB accessory: {e}");
            continue;
          }
        };
        log::info!("USB accessory opened");
        let engine = engine.clone();
        let res = rt.block_on(async move {
          tokio::select! {
            r = aurora_transport::usb::attach_fd(engine.links(), fd) => r,
            // switched off in settings
            () = async { while engine.settings().usb { tokio::time::sleep(POLL).await } } => Ok(()),
          }
        });
        if let Err(e) = res {
          log::info!("USB link ended: {e}");
        }
      }
    });
  if let Err(e) = res {
    log::error!("cannot start the USB thread: {e}");
  }
}

/// The accessory's fd, `None` while there is none or permission is pending
fn open() -> Result<Option<OwnedFd>, String> {
  let fd = h::with_env(|env| {
    let activity = h::activity(env)?;
    let cls = h::find_app_class(env, CLASS)?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("open"),
        jni::jni_sig!("(Landroid/app/Activity;)I"),
        &[JValue::Object(&activity)],
      )
      .and_then(|v| v.i())
      .map_err(|e| {
        env.exception_clear();
        e.to_string()
      })
  })?;
  // SAFETY: `detachFd` hands us sole ownership of a valid descriptor
  Ok((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) }))
}

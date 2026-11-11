//! Live QR scanning through `dev.gpui.mobile.AuroraQrActivity` (CameraX + ML Kit).

use std::{thread::sleep, time::Duration};

use gpui_mobile::{
  android::jni::{self as h},
  packages::permission_handler::{Permission, request_permission},
};
use jni::objects::JValue;

const CLASS: &str = "dev.gpui.mobile.AuroraQrActivity";

/// Blocks while the camera view is up: the scanned text, `None` when cancelled
pub fn scan() -> Result<Option<String>, String> {
  if !request_permission(Permission::Camera)?.is_granted() {
    return Err("Camera permission denied".into());
  }
  h::with_env(|env| {
    let activity = h::activity(env)?;
    let cls = h::find_app_class(env, CLASS)?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("start"),
        jni::jni_sig!("(Landroid/app/Activity;)V"),
        &[JValue::Object(&activity)],
      )
      .map(|_| ())
      .map_err(|e| {
        env.exception_clear();
        e.to_string()
      })
  })?;

  loop {
    sleep(Duration::from_millis(150));
    let polled = h::with_env(|env| {
      let cls = h::find_app_class(env, CLASS)?;
      let obj = env
        .call_static_method(
          &cls,
          jni::jni_str!("poll"),
          jni::jni_sig!("()Ljava/lang/String;"),
          &[],
        )
        .and_then(|v| v.l())
        .map_err(|e| {
          env.exception_clear();
          e.to_string()
        })?;
      Ok((!obj.is_null()).then(|| h::get_string(env, &obj)))
    })?;
    if let Some(text) = polled {
      return Ok((!text.is_empty()).then_some(text));
    }
  }
}

pub fn cancel() {
  let _ = h::with_env(|env| {
    let cls = h::find_app_class(env, CLASS)?;
    env
      .call_static_method(&cls, jni::jni_str!("cancel"), jni::jni_sig!("()V"), &[])
      .map(|_| ())
      .map_err(|e| {
        env.exception_clear();
        e.to_string()
      })
  });
}

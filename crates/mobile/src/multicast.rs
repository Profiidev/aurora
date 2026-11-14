//! Wi-Fi multicast lock through `dev.gpui.mobile.AuroraMulticast`.

use jni::objects::JValue;

use crate::jvm;

/// Holds or releases the Wi-Fi multicast lock (`AuroraMulticast`)
pub fn set(on: bool) -> Result<(), String> {
  jvm::with_env(|env| {
    jvm::call_with_context(
      env,
      "dev.gpui.mobile.AuroraMulticast",
      jni::jni_str!("set"),
      jni::jni_sig!("(Landroid/content/Context;Z)V"),
      &[JValue::Bool(on)],
    )
    .map(|_| ())
  })
}

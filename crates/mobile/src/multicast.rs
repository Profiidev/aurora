//! Wi-Fi multicast lock through `dev.gpui.mobile.AuroraMulticast`.

use gpui_mobile::android::jni as h;
use jni::objects::JValue;

/// Holds or releases the Wi-Fi multicast lock (`AuroraMulticast`)
pub fn set(on: bool) -> Result<(), String> {
  h::with_env(|env| {
    let activity = h::activity(env)?;
    let cls = h::find_app_class(env, "dev.gpui.mobile.AuroraMulticast")?;
    env
      .call_static_method(
        &cls,
        jni::jni_str!("set"),
        jni::jni_sig!("(Landroid/app/Activity;Z)V"),
        &[JValue::Object(&activity), JValue::Bool(on.into())],
      )
      .map(|_| ())
      .map_err(|e| {
        env.exception_clear();
        e.to_string()
      })
  })
}

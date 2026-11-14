//! JNI access that works without an activity. gpui-mobile's helpers all go
//! through the activity, but the engine runs in a service process that has
//! none; this keeps the JavaVM and the application Context instead.

use std::{ffi::c_void, sync::OnceLock};

use jni::{
  Env, EnvUnowned, JavaVM, Outcome,
  objects::{JClass, JObject, JValue},
  refs::Global,
};

static VM: OnceLock<JavaVM> = OnceLock::new();
static CONTEXT: OnceLock<Global<JObject<'static>>> = OnceLock::new();

pub fn call_err(env: &mut Env, e: jni::errors::Error) -> String {
  env.exception_clear();
  e.to_string()
}

/// Keeps the VM and `context.getApplicationContext()`; later calls are no-ops
fn store(env: &mut Env, context: &JObject) -> Result<(), String> {
  if CONTEXT.get().is_some() {
    return Ok(());
  }
  let app = env
    .call_method(
      context,
      jni::jni_str!("getApplicationContext"),
      jni::jni_sig!("()Landroid/content/Context;"),
      &[],
    )
    .and_then(|v| v.l())
    .map_err(|e| call_err(env, e))?;
  let global = env.new_global_ref(app).map_err(|e| call_err(env, e))?;
  let vm = env.get_java_vm().map_err(|e| call_err(env, e))?;
  let _ = VM.set(vm);
  let _ = CONTEXT.set(global);
  Ok(())
}

/// From a native method: its `JNIEnv` and a `Context` argument
///
/// # Safety
/// `env` is the calling thread's `JNIEnv`, `context` a live local reference.
pub unsafe fn init_raw(env: *mut c_void, context: *mut c_void) -> Result<(), String> {
  // SAFETY: the caller's contract
  let mut unowned = unsafe { EnvUnowned::from_raw(env.cast()) };
  let outcome = unowned
    .with_env(|env| -> Result<Result<(), String>, jni::errors::Error> {
      // SAFETY: the caller's contract
      let context = unsafe { JObject::from_raw(env, context.cast()) };
      Ok(store(env, &context))
    })
    .into_outcome();
  match outcome {
    Outcome::Ok(res) => res,
    Outcome::Err(e) => Err(e.to_string()),
    Outcome::Panic(_) => Err("panicked".into()),
  }
}

/// From the UI process, through gpui's activity
pub fn init_from_activity() -> Result<(), String> {
  use gpui_mobile::android::jni as h;
  h::with_env(|env| {
    let activity = h::activity(env)?;
    store(env, &activity)
  })
}

/// Runs `f` with this thread attached to the VM
pub fn with_env<T>(f: impl FnOnce(&mut Env) -> Result<T, String>) -> Result<T, String> {
  let vm = VM.get().ok_or("JavaVM not set")?;
  vm.attach_current_thread(|env| -> Result<Result<T, String>, jni::errors::Error> { Ok(f(env)) })
    .map_err(|e| e.to_string())?
}

/// The application Context, as a local reference
pub fn context<'l>(env: &mut Env<'l>) -> Result<JObject<'l>, String> {
  let global = CONTEXT.get().ok_or("Context not set")?;
  env
    .new_local_ref(global.as_obj())
    .map_err(|e| call_err(env, e))
}

/// An app class, through the Context's class loader (threads attached from
/// Rust only see system classes with `FindClass`)
pub fn class<'l>(env: &mut Env<'l>, name: &str) -> Result<JClass<'l>, String> {
  let context = context(env)?;
  let loader = env
    .call_method(
      &context,
      jni::jni_str!("getClassLoader"),
      jni::jni_sig!("()Ljava/lang/ClassLoader;"),
      &[],
    )
    .and_then(|v| v.l())
    .map_err(|e| call_err(env, e))?;
  let jname = env.new_string(name).map_err(|e| call_err(env, e))?;
  let class = env
    .call_method(
      &loader,
      jni::jni_str!("loadClass"),
      jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
      &[JValue::Object(&jname)],
    )
    .and_then(|v| v.l())
    .map_err(|e| call_err(env, e))?;
  // SAFETY: loadClass returns a java.lang.Class
  Ok(unsafe { JClass::from_raw(env, class.into_raw()) })
}

/// A static method of `class` taking the Context first
pub fn call_with_context<'l, 'a, 'b>(
  env: &mut Env<'l>,
  class_name: &str,
  method: &'static jni::strings::JNIStr,
  sig: impl AsRef<jni::signature::MethodSignature<'a, 'b>>,
  args: &[JValue],
) -> Result<jni::JValueOwned<'l>, String> {
  let cls = class(env, class_name)?;
  let context = context(env)?;
  let mut all = vec![JValue::Object(&context)];
  all.extend_from_slice(args);
  env
    .call_static_method(&cls, method, sig, &all)
    .map_err(|e| call_err(env, e))
}

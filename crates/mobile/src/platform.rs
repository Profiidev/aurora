/// Returns `true` when running on an Android emulator (`ro.kernel.qemu` or `ranchu`/`goldfish` hardware).
pub fn is_android_emulator() -> bool {
  #[cfg(target_os = "android")]
  {
    unsafe extern "C" {
      fn __system_property_get(
        name: *const std::ffi::c_char,
        value: *mut std::ffi::c_char,
      ) -> std::ffi::c_int;
    }
    let mut buf = [0u8; 92];
    let len = unsafe {
      __system_property_get(
        c"ro.kernel.qemu".as_ptr(),
        buf.as_mut_ptr().cast::<std::ffi::c_char>(),
      )
    };
    if len > 0 && &buf[..len as usize] == b"1" {
      return true;
    }
    let len = unsafe {
      __system_property_get(
        c"ro.hardware".as_ptr(),
        buf.as_mut_ptr().cast::<std::ffi::c_char>(),
      )
    };
    if len > 0 {
      let hw = std::str::from_utf8(&buf[..len as usize]).unwrap_or("");
      if hw.contains("ranchu") || hw.contains("goldfish") {
        return true;
      }
    }
    false
  }
  #[cfg(not(target_os = "android"))]
  {
    false
  }
}


/// Selects the OpenGL ES backend when running on an Android emulator to avoid ranchu Vulkan driver bugs.
#[cfg(target_os = "android")]
pub fn configure_android_gpu() {
  use gpui_mobile::android::host::{GpuBackend, set_gpu_backend};
  if is_android_emulator() {
    log::info!(
      "Android emulator detected; selecting OpenGL ES backend to avoid ranchu Vulkan bugs"
    );
    set_gpu_backend(GpuBackend::Gl);
  }
}


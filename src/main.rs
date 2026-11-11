fn main() {
  // Mobile hosts enter through the library (App.swift / android_main).
  #[cfg(not(any(target_os = "ios", target_os = "android")))]
  aurora::run_desktop();
}

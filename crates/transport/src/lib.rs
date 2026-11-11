//! One `Transport` per link type. Backends are cargo features: `iroh`
//! (network), `bluer` (bluetooth, linux), `nusb` (usb).

/// A link to a paired device.
pub trait Transport {
  fn name(&self) -> &'static str;
}

//! One `Transport` per link type. Backends are cargo features: `iroh`
//! (network), `bluer` (bluetooth, linux), `nusb` (usb).

/// A link to a paired device.
pub trait Transport {
  fn name(&self) -> &'static str;
}

#[cfg(feature = "iroh")]
mod iroh_node;
#[cfg(feature = "iroh")]
pub use iroh_node::{
  Connection, Hints, Node, RecvStream, Seen, SendStream, Via, load_or_create_key, random_bytes,
  remote_id, via,
};

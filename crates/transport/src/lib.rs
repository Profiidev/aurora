//! Everything runs under one iroh endpoint ([`Node`]): QUIC over the network
//! (direct, mDNS, relays) and over point-to-point [`link`]s such as USB. A
//! connection moves between them without the caller noticing.

#[cfg(feature = "iroh")]
pub mod link;
#[cfg(feature = "iroh")]
mod mdns;
#[cfg(feature = "iroh")]
mod node;
#[cfg(feature = "iroh")]
pub mod usb;

#[cfg(feature = "iroh")]
pub use link::{LinkEvent, LinkKind, Links};
#[cfg(feature = "iroh")]
pub use node::{
  Connection, Hints, Node, RecvStream, Seen, SendStream, Via, load_or_create_key, random_bytes,
  remote_id, via,
};

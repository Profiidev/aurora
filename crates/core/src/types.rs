use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// A paired device. `id` is the hex endpoint id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
  pub id: String,
  pub name: String,
  pub os: String,
  pub paired_at: u64,
  /// Last known relay and direct addresses, hints for iroh's lookups
  #[serde(default)]
  pub relay: Option<String>,
  #[serde(default)]
  pub addrs: Vec<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceStatus {
  pub device: Device,
  pub connected: bool,
  pub battery: Option<u8>,
  pub charging: bool,
  /// How messages reach it now ("USB", "Direct · 192.168.1.5", "Relay")
  #[serde(default)]
  pub via: Option<String>,
}

/// What a waiting session shows so the other side can find it
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairInfo {
  /// `aurora://pair?...` link, rendered as the QR code
  pub link: String,
  pub name: String,
  pub addrs: Vec<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Confirm {
  pub code: String,
  pub peer_name: String,
  pub peer_os: String,
  pub via: String,
  /// First bytes of the peer's endpoint id, grouped for reading aloud
  pub fingerprint: String,
  pub local_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PairState {
  Idle,
  Waiting(PairInfo),
  Dialing,
  Confirm(Confirm),
}

/// An unpaired device found by mDNS or at the other end of a USB cable
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Nearby {
  pub name: String,
  pub os: String,
  pub addr: Option<SocketAddr>,
  /// Pass to `pair:connect`
  pub link: String,
  /// Found over a USB cable
  #[serde(default)]
  pub usb: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
  PairingStarted(PairInfo),
  CodeReady(Confirm),
  PairingExpired,
  PairingCancelled,
  PairingFailed(String),
  Paired(Device),
  DeviceConnected(DeviceStatus),
  DeviceDisconnected(String),
  Unpaired(String),
  ConfigChanged,
}

/// What the settings screens read and change
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
  pub name: String,
  pub discovery: bool,
  /// USB connections; `None` where the platform has none (iOS)
  #[serde(default)]
  pub usb: Option<bool>,
}

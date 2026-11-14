//! USB through Android Open Accessory: the desktop is the USB host
//! ([`UsbHost`], `nusb` feature), the phone the accessory ([`attach_fd`]).

#[cfg(unix)]
mod accessory;
#[cfg(feature = "nusb")]
mod host;

#[cfg(unix)]
pub use accessory::attach_fd;
#[cfg(feature = "nusb")]
pub use host::ANDROID_VENDORS;
#[cfg(feature = "nusb")]
pub(crate) use host::UsbHost;

/// Strings the host sends when switching a phone to accessory mode; the
/// phone's `accessory_filter.xml` matches manufacturer and model
pub const MANUFACTURER: &str = "Aurora";
pub const MODEL: &str = "Aurora";

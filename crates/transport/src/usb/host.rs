//! Desktop side: finds phones, switches them to accessory mode and runs a
//! link over the accessory's bulk endpoints.

use std::{
  collections::HashSet,
  io,
  pin::Pin,
  sync::{Arc, Mutex},
  task::{Context, Poll},
  time::Duration,
};

use anyhow::{Result, bail};
use n0_future::StreamExt;
use nusb::{
  DeviceId, DeviceInfo,
  descriptors::TransferType,
  hotplug::HotplugEvent,
  io::EndpointWrite,
  transfer::{Bulk, ControlIn, ControlOut, ControlType, Direction, In, Out, Recipient},
};
use tokio::{io::AsyncWrite, task::JoinSet, time::sleep};
use tracing::{debug, info, warn};

use super::{MANUFACTURER, MODEL};
use crate::link::{LinkKind, Links};

/// USB vendor ids of Android phone makers; only these get the accessory
/// handshake. Keep in sync with `platform/packaging/70-aurora.rules`.
pub const ANDROID_VENDORS: &[u16] = &[
  0x18d1, // Google
  0x04e8, // Samsung
  0x2717, // Xiaomi
  0x2a70, // OnePlus
  0x22b8, // Motorola
  0x12d1, // Huawei
  0x0bb4, // HTC
  0x1004, // LG
  0x0fce, // Sony
  0x2ae5, // Fairphone
  0x22d9, // OPPO
  0x2d95, // vivo
  0x19d2, // ZTE
  0x17ef, // Lenovo
  0x0b05, // Asus
  0x1949, // Amazon
  0x05c6, // Qualcomm reference
  0x0e8d, // MediaTek reference
];

const GOOGLE: u16 = 0x18d1;
/// Accessory, accessory + adb and the audio variants
const AOA_PRODUCTS: std::ops::RangeInclusive<u16> = 0x2d00..=0x2d05;

const GET_PROTOCOL: u8 = 51;
const SEND_STRING: u8 = 52;
const START: u8 = 53;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
const IO_BUF: usize = 16 * 1024;
/// Between link attempts while the phone app has not opened the accessory
const RETRY: Duration = Duration::from_secs(2);

/// Starts and stops the USB watcher
#[derive(Clone)]
pub(crate) struct UsbHost {
  links: Links,
  task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl UsbHost {
  pub(crate) fn new(links: Links) -> Self {
    Self {
      links,
      task: Arc::default(),
    }
  }

  pub(crate) fn set_enabled(&self, on: bool) {
    let mut task = self.task.lock().unwrap();
    match (on, task.is_some()) {
      (true, false) => *task = Some(tokio::spawn(watch(self.links.clone()))),
      (false, true) => task.take().unwrap().abort(),
      _ => {}
    }
  }
}

/// Handles every device present now and plugged in later. Aborting it ends
/// all links it runs.
async fn watch(links: Links) {
  let mut watcher = match nusb::watch_devices() {
    Ok(w) => w,
    Err(e) => {
      warn!("USB disabled: {e}");
      return;
    }
  };
  let mut tasks = JoinSet::new();
  // probed once per plug: a phone that refused the switch is left alone
  let mut handled = HashSet::<DeviceId>::new();
  let present = nusb::list_devices().await.map(Iterator::collect::<Vec<_>>);
  for info in present.unwrap_or_default() {
    if handled.insert(info.id()) {
      tasks.spawn(handle(links.clone(), info));
    }
  }
  loop {
    tokio::select! {
      event = watcher.next() => match event {
        Some(HotplugEvent::Connected(info)) => {
          if handled.insert(info.id()) {
            tasks.spawn(handle(links.clone(), info));
          }
        }
        Some(HotplugEvent::Disconnected(id)) => {
          handled.remove(&id);
        }
        None => break,
      },
      Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
    }
  }
}

#[derive(Debug, PartialEq)]
enum Class {
  /// Already in accessory mode: run a link
  Accessory,
  /// A phone maker's device: try the accessory handshake
  Android,
  Other,
}

fn classify(vendor: u16, product: u16) -> Class {
  if vendor == GOOGLE && AOA_PRODUCTS.contains(&product) {
    Class::Accessory
  } else if ANDROID_VENDORS.contains(&vendor) {
    Class::Android
  } else {
    Class::Other
  }
}

async fn handle(links: Links, info: DeviceInfo) {
  let res = match classify(info.vendor_id(), info.product_id()) {
    Class::Accessory => run_accessory(&links, &info).await,
    Class::Android => switch_to_accessory(&info).await,
    Class::Other => return,
  };
  if let Err(e) = res {
    debug!(
      "USB {:04x}:{:04x}: {e:#}",
      info.vendor_id(),
      info.product_id()
    );
  }
}

/// Sends the AOA handshake; the phone drops off the bus and comes back as an
/// accessory, which the watcher picks up as a new device
async fn switch_to_accessory(info: &DeviceInfo) -> Result<()> {
  let dev = info.open().await?;
  let version = dev
    .control_in(
      ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: GET_PROTOCOL,
        value: 0,
        index: 0,
        length: 2,
      },
      CONTROL_TIMEOUT,
    )
    .await?;
  let version = u16::from_le_bytes([
    version.first().copied().unwrap_or(0),
    version.get(1).copied().unwrap_or(0),
  ]);
  if version < 1 {
    bail!("no accessory support");
  }
  let strings = [
    MANUFACTURER,
    MODEL,
    "Aurora desktop link",
    env!("CARGO_PKG_VERSION"),
    "https://github.com/Profiidev/aurora",
    "",
  ];
  for (index, s) in strings.iter().enumerate() {
    let mut data = s.as_bytes().to_vec();
    data.push(0);
    dev
      .control_out(
        ControlOut {
          control_type: ControlType::Vendor,
          recipient: Recipient::Device,
          request: SEND_STRING,
          value: 0,
          index: index as u16,
          data: &data,
        },
        CONTROL_TIMEOUT,
      )
      .await?;
  }
  dev
    .control_out(
      ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: START,
        value: 0,
        index: 0,
        data: &[],
      },
      CONTROL_TIMEOUT,
    )
    .await?;
  info!(
    "switched USB device {:04x}:{:04x} to accessory mode",
    info.vendor_id(),
    info.product_id()
  );
  Ok(())
}

/// Runs links until the accessory is unplugged (the watcher then aborts or
/// open fails)
async fn run_accessory(links: &Links, info: &DeviceInfo) -> Result<()> {
  loop {
    let dev = info.open().await?;
    let intf = dev.detach_and_claim_interface(0).await?;
    let (mut ep_in, mut ep_out) = (None, None);
    for ep in intf.descriptor().iter().flat_map(|d| d.endpoints()) {
      if ep.transfer_type() != TransferType::Bulk {
        continue;
      }
      match ep.direction() {
        Direction::In => ep_in = ep_in.or(Some(ep.address())),
        Direction::Out => ep_out = ep_out.or(Some(ep.address())),
      }
    }
    let (Some(ep_in), Some(ep_out)) = (ep_in, ep_out) else {
      bail!("accessory has no bulk endpoints");
    };
    let reader = intf.endpoint::<Bulk, In>(ep_in)?.reader(IO_BUF);
    let writer = DelimitedWrite::new(intf.endpoint::<Bulk, Out>(ep_out)?.writer(IO_BUF));
    if let Err(e) = links.attach(LinkKind::Usb, reader, writer).await {
      debug!("USB link: {e}");
    }
    sleep(RETRY).await;
  }
}

/// Ends each flush with a short or zero-length packet, so the phone's read
/// returns right away instead of waiting for a full buffer
struct DelimitedWrite {
  inner: EndpointWrite<Bulk>,
  /// The terminator for the current flush is submitted
  ended: bool,
}

impl DelimitedWrite {
  fn new(inner: EndpointWrite<Bulk>) -> Self {
    Self {
      inner,
      ended: false,
    }
  }
}

impl AsyncWrite for DelimitedWrite {
  fn poll_write(
    mut self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    buf: &[u8],
  ) -> Poll<io::Result<usize>> {
    self.ended = false;
    Pin::new(&mut self.inner).poll_write(cx, buf)
  }

  fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    if !self.ended {
      self.inner.submit_end();
      self.ended = true;
    }
    Pin::new(&mut self.inner).poll_flush(cx)
  }

  fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Pin::new(&mut self.inner).poll_shutdown(cx)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classifies_devices() {
    assert_eq!(classify(0x18d1, 0x2d00), Class::Accessory);
    assert_eq!(classify(0x18d1, 0x2d01), Class::Accessory);
    assert_eq!(classify(0x18d1, 0x4ee7), Class::Android);
    assert_eq!(classify(0x04e8, 0x6860), Class::Android);
    assert_eq!(classify(0x046d, 0xc52b), Class::Other);
  }

  #[test]
  fn udev_rule_lists_every_vendor() {
    let rules = include_str!("../../../../platform/packaging/70-aurora.rules");
    for v in ANDROID_VENDORS {
      assert!(
        rules.contains(&format!("ATTR{{idVendor}}==\"{v:04x}\"")),
        "70-aurora.rules misses {v:04x}"
      );
    }
  }
}

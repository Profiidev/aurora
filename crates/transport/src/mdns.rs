//! mDNS that can be switched on and off while the endpoint runs; iroh has no
//! way to remove an address lookup again

use std::sync::Mutex;

use anyhow::{Result, anyhow};
use iroh::{
  EndpointId,
  address_lookup::{AddressLookup, EndpointData, Error as LookupError, Item},
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::{StreamExt, boxed::BoxStream};
use tokio::sync::broadcast;

use crate::node::{Seen, Via};

/// mDNS service name, so only Aurora endpoints show up
const SERVICE: &str = "aurora";

#[derive(Debug)]
pub(crate) struct Mdns {
  id: EndpointId,
  inner: Mutex<Option<MdnsAddressLookup>>,
  /// What we published last, handed to a freshly enabled service
  data: Mutex<Option<EndpointData>>,
  seen: broadcast::Sender<Seen>,
}

impl Mdns {
  pub(crate) fn new(id: EndpointId, seen: broadcast::Sender<Seen>) -> Self {
    Self {
      id,
      inner: Mutex::new(None),
      data: Mutex::new(None),
      seen,
    }
  }

  pub(crate) fn set_enabled(&self, on: bool) -> Result<()> {
    let mut inner = self.inner.lock().unwrap();
    if on == inner.is_some() {
      return Ok(());
    }
    if !on {
      *inner = None;
      return Ok(());
    }
    let mdns = MdnsAddressLookup::builder()
      .service_name(SERVICE)
      .build(self.id)
      .map_err(|e| anyhow!("mDNS: {e}"))?;
    if let Some(data) = &*self.data.lock().unwrap() {
      mdns.publish(data);
    }
    let (me, tx, events) = (self.id, self.seen.clone(), mdns.clone());
    tokio::spawn(async move {
      // ends when the service is dropped
      let mut stream = events.subscribe().await;
      while let Some(event) = stream.next().await {
        let seen = match event {
          DiscoveryEvent::Discovered { endpoint_info, .. } if endpoint_info.endpoint_id != me => {
            let addrs: Vec<_> = endpoint_info.ip_addrs().copied().collect();
            Seen::Found {
              id: *endpoint_info.endpoint_id.as_bytes(),
              via: addrs.first().map_or(Via::Relay, |a| Via::Direct(*a)),
              addrs,
            }
          }
          DiscoveryEvent::Expired { endpoint_id } => Seen::Gone(*endpoint_id.as_bytes()),
          _ => continue,
        };
        let _ = tx.send(seen);
      }
    });
    *inner = Some(mdns);
    Ok(())
  }
}

impl AddressLookup for Mdns {
  fn publish(&self, data: &EndpointData) {
    *self.data.lock().unwrap() = Some(data.clone());
    if let Some(mdns) = &*self.inner.lock().unwrap() {
      mdns.publish(data);
    }
  }

  fn resolve(&self, id: EndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
    self.inner.lock().unwrap().as_ref()?.resolve(id)
  }
}

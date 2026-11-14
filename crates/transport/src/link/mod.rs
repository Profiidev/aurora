//! Point-to-point links (USB, later Bluetooth) as one more iroh path.
//!
//! A link is any byte pipe to one peer. [`Links::attach`] runs it: both sides
//! name their endpoint, then QUIC datagrams flow over it framed by
//! [`frame`]. To iroh the link is a custom transport whose address is
//! `(kind, peer id)`, so QUIC and TLS run end to end over the cable and a
//! connection moves between Wi-Fi, relay and links on its own.

pub mod frame;

use std::{
  collections::HashMap,
  io,
  num::NonZeroUsize,
  sync::{Arc, Mutex},
  task::{Context, Poll},
  time::Duration,
};

use aurora_proto::Id;
use bytes::Bytes;
use iroh::{
  EndpointId, TransportAddr,
  address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error as LookupError, Item},
  endpoint::transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
};
use iroh_base::CustomAddr;
use n0_future::boxed::BoxStream;
use tokio::{
  io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
  sync::{broadcast, mpsc},
  time::timeout,
};
use tracing::{debug, info};

/// Datagrams queued per link before new ones are dropped, like a full socket
const QUEUE: usize = 256;
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Bigger than any USB bulk transfer the accessory side gets (AOA needs ≥ 16 KiB reads)
const READ_BUF: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
  Usb,
  Bluetooth,
}

impl LinkKind {
  const ALL: [LinkKind; 2] = [LinkKind::Usb, LinkKind::Bluetooth];

  /// iroh custom transport id, from the private range
  pub const fn transport_id(self) -> u64 {
    match self {
      LinkKind::Usb => 0xa0a0_0001,
      LinkKind::Bluetooth => 0xa0a0_0002,
    }
  }

  pub fn from_transport_id(id: u64) -> Option<Self> {
    Self::ALL.into_iter().find(|k| k.transport_id() == id)
  }

  pub(crate) fn addr(self, peer: &Id) -> CustomAddr {
    CustomAddr::from_parts(self.transport_id(), peer)
  }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinkEvent {
  Up { kind: LinkKind, peer: Id },
  Down { kind: LinkKind, peer: Id },
}

#[derive(Debug)]
struct Inner {
  me: Id,
  /// Outgoing queue of each running link
  peers: Mutex<HashMap<(LinkKind, Id), mpsc::Sender<Bytes>>>,
  inbound: mpsc::Sender<(CustomAddr, Bytes)>,
  /// Taken by the endpoint iroh binds
  inbound_rx: Mutex<Option<mpsc::Receiver<(CustomAddr, Bytes)>>>,
  /// Our own address per kind with a link up; changes make iroh probe new paths
  local: n0_watcher::Watchable<Vec<CustomAddr>>,
  events: broadcast::Sender<LinkEvent>,
}

/// The links of one endpoint. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Links(Arc<Inner>);

impl Links {
  pub fn new(me: Id) -> Self {
    let (inbound, rx) = mpsc::channel(QUEUE * 4);
    Self(Arc::new(Inner {
      me,
      peers: Mutex::default(),
      inbound,
      inbound_rx: Mutex::new(Some(rx)),
      local: n0_watcher::Watchable::new(Vec::new()),
      events: broadcast::channel(16).0,
    }))
  }

  pub fn events(&self) -> broadcast::Receiver<LinkEvent> {
    self.0.events.subscribe()
  }

  /// A link of any kind to `peer` is up
  pub fn is_linked(&self, peer: &Id) -> bool {
    self.0.peers.lock().unwrap().keys().any(|(_, p)| p == peer)
  }

  /// Resolves once no link to `peer` is left. Close connections over it
  /// then: QUIC itself only notices at its idle timeout.
  pub async fn gone(&self, peer: &Id) {
    let mut events = self.events();
    while self.is_linked(peer) {
      if let Err(broadcast::error::RecvError::Closed) = events.recv().await {
        return;
      }
    }
  }

  /// Runs a link over a byte pipe until it closes. Both ends call this.
  pub async fn attach<R, W>(&self, kind: LinkKind, reader: R, mut writer: W) -> io::Result<()>
  where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
  {
    let mut reader = BufReader::with_capacity(READ_BUF, reader);
    let peer = timeout(HELLO_TIMEOUT, async {
      frame::write_hello(&mut writer, &self.0.me).await?;
      frame::read_hello(&mut reader).await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no link hello"))??;
    if peer == self.0.me {
      return Err(io::Error::other("link loops back to this endpoint"));
    }
    let (tx, mut rx) = mpsc::channel::<Bytes>(QUEUE);
    // unregisters even when the task running this future is aborted
    let _up = Registered::new(self.clone(), kind, peer, tx);

    let from = kind.addr(&peer);
    let inbound = self.0.inbound.clone();
    let read = async {
      while let Some(data) = frame::read_datagram(&mut reader).await? {
        // a full queue drops the packet, QUIC resends
        let _ = inbound.try_send((from.clone(), data.into()));
      }
      io::Result::Ok(())
    };
    let write = async {
      while let Some(data) = rx.recv().await {
        frame::write_datagram(&mut writer, &data).await?;
        while let Ok(data) = rx.try_recv() {
          frame::write_datagram(&mut writer, &data).await?;
        }
        writer.flush().await?;
      }
      io::Result::Ok(())
    };
    let res = tokio::select! {
      r = read => r,
      r = write => r,
    };
    // tells pipe-backed writers (the accessory threads) that the link is over
    let _ = writer.shutdown().await;
    res
  }

  fn update_local(&self) {
    let peers = self.0.peers.lock().unwrap();
    let addrs = (LinkKind::ALL.into_iter())
      .filter(|k| peers.keys().any(|(pk, _)| pk == k))
      .map(|k| k.addr(&self.0.me))
      .collect();
    let _ = self.0.local.set(addrs);
  }

  fn sender_for(&self, addr: &CustomAddr) -> Option<mpsc::Sender<Bytes>> {
    let kind = LinkKind::from_transport_id(addr.id())?;
    let peer: Id = addr.data().try_into().ok()?;
    self.0.peers.lock().unwrap().get(&(kind, peer)).cloned()
  }
}

impl CustomTransport for Links {
  fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
    let rx = (self.0.inbound_rx.lock().unwrap().take())
      .ok_or_else(|| io::Error::other("links are bound to an endpoint already"))?;
    Ok(Box::new(LinkEndpoint {
      links: self.clone(),
      rx,
    }))
  }
}

/// A running link in the peer table, for as long as it lives
struct Registered {
  links: Links,
  kind: LinkKind,
  peer: Id,
  tx: mpsc::Sender<Bytes>,
}

impl Registered {
  fn new(links: Links, kind: LinkKind, peer: Id, tx: mpsc::Sender<Bytes>) -> Self {
    links
      .0
      .peers
      .lock()
      .unwrap()
      .insert((kind, peer), tx.clone());
    links.update_local();
    info!("{kind:?} link up");
    let _ = links.0.events.send(LinkEvent::Up { kind, peer });
    Self {
      links,
      kind,
      peer,
      tx,
    }
  }
}

impl Drop for Registered {
  fn drop(&mut self) {
    let key = (self.kind, self.peer);
    {
      let mut peers = self.links.0.peers.lock().unwrap();
      // a newer link to the same peer may have replaced this one
      if !peers.get(&key).is_some_and(|t| t.same_channel(&self.tx)) {
        return;
      }
      peers.remove(&key);
    }
    self.links.update_local();
    info!("{:?} link down", self.kind);
    let _ = self.links.0.events.send(LinkEvent::Down {
      kind: self.kind,
      peer: self.peer,
    });
  }
}

#[derive(Debug)]
struct LinkEndpoint {
  links: Links,
  rx: mpsc::Receiver<(CustomAddr, Bytes)>,
}

impl CustomEndpoint for LinkEndpoint {
  fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
    self.links.0.local.watch()
  }

  fn create_sender(&self) -> Arc<dyn CustomSender> {
    Arc::new(self.links.clone())
  }

  fn poll_recv(
    &mut self,
    cx: &mut Context,
    bufs: &mut [io::IoSliceMut<'_>],
    metas: &mut [noq_udp::RecvMeta],
    recv_infos: &mut [RecvInfo],
  ) -> Poll<io::Result<usize>> {
    let mut filled = 0;
    while filled < bufs.len() {
      let (from, data) = match self.rx.poll_recv(cx) {
        Poll::Ready(Some(p)) => p,
        // `Inner` holds a sender, so the channel never closes while we exist
        Poll::Ready(None) | Poll::Pending if filled > 0 => break,
        Poll::Ready(None) => return Poll::Ready(Err(io::Error::other("links closed"))),
        Poll::Pending => return Poll::Pending,
      };
      if bufs[filled].len() < data.len() {
        debug!("dropped {} byte datagram, buffer too small", data.len());
        continue;
      }
      bufs[filled][..data.len()].copy_from_slice(&data);
      metas[filled].len = data.len();
      metas[filled].stride = data.len();
      let local = LinkKind::from_transport_id(from.id()).map(|k| k.addr(&self.links.0.me));
      recv_infos[filled] = RecvInfo::new(from, local);
      filled += 1;
    }
    Poll::Ready(Ok(filled))
  }

  fn max_transmit_segments(&self) -> NonZeroUsize {
    NonZeroUsize::MIN
  }
}

impl CustomSender for Links {
  fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
    LinkKind::from_transport_id(addr.id()).is_some()
  }

  fn poll_send(
    &self,
    _cx: &mut Context,
    dst: &CustomAddr,
    _src: Option<&CustomAddr>,
    transmit: &Transmit<'_>,
  ) -> Poll<io::Result<()>> {
    // no link to `dst` (yet): drop like an unreachable UDP peer
    if let Some(tx) = self.sender_for(dst) {
      let size = transmit
        .segment_size
        .unwrap_or(transmit.contents.len())
        .max(1);
      for chunk in transmit.contents.chunks(size) {
        let _ = tx.try_send(Bytes::copy_from_slice(chunk));
      }
    }
    Poll::Ready(Ok(()))
  }
}

impl AddressLookup for Links {
  fn publish(&self, _data: &EndpointData) {}

  fn resolve(&self, id: EndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
    let peer = *id.as_bytes();
    let addrs: Vec<_> = (self.0.peers.lock().unwrap().keys())
      .filter(|(_, p)| *p == peer)
      .map(|(k, p)| TransportAddr::Custom(k.addr(p)))
      .collect();
    if addrs.is_empty() {
      return None;
    }
    let item = Item::new(
      EndpointInfo {
        endpoint_id: id,
        data: EndpointData::from_iter(addrs),
      },
      "aurora links",
      None,
    );
    Some(Box::pin(n0_future::stream::once(Ok(item))))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn transport_ids_round_trip() {
    for kind in LinkKind::ALL {
      assert_eq!(LinkKind::from_transport_id(kind.transport_id()), Some(kind));
    }
    assert_eq!(LinkKind::from_transport_id(0x20), None);
  }
}

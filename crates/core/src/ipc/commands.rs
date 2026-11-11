//! The commands and event stream `aurorad` serves. Each type is shared by the
//! daemon (`handle`) and its clients (`ipc::send`).

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast::error::RecvError, mpsc};

use super::{IpcCommand, IpcRegistry, IpcStream};
use crate::{
  engine::Engine,
  types::{DeviceStatus, Event, Nearby, PairInfo, PairState},
};

macro_rules! command {
  ($name:ident, $cmd:literal, $payload:ty => $resp:ty, |$p:ident, $e:ident| $body:expr) => {
    pub struct $name;

    impl IpcCommand for $name {
      const COMMAND: &'static str = $cmd;
      type Payload = $payload;
      type Response = $resp;

      async fn handle($p: $payload, $e: Arc<Engine>) -> Result<$resp> {
        $body
      }
    }
  };
}

command!(PairStart, "pair:start", () => PairInfo, |_p, e| e.start_pairing());
command!(PairCancel, "pair:cancel", () => (), |_p, e| {
  e.cancel_pairing();
  Ok(())
});
command!(PairConnect, "pair:connect", String => (), |p, e| e.connect(&p).await);
command!(PairAccept, "pair:accept", () => (), |_p, e| e.decide(true));
command!(PairReject, "pair:reject", () => (), |_p, e| e.decide(false));
command!(PairStatus, "pair:status", () => PairState, |_p, e| Ok(e.pair_state()));
command!(PairNearby, "pair:nearby", () => Vec<Nearby>, |_p, e| e.scan().await);
command!(DevicesList, "devices:list", () => Vec<DeviceStatus>, |_p, e| Ok(e.devices()));
command!(DevicesUnpair, "devices:unpair", String => (), |p, e| e.unpair(&p).await);

/// Every [`Event`] of the engine until the client closes
pub struct Events;

impl IpcStream for Events {
  const COMMAND: &'static str = "events:subscribe";
  type Payload = ();
  type Event = Event;

  async fn subscribe(_: (), engine: Arc<Engine>) -> Result<mpsc::Receiver<Event>> {
    let mut events = engine.events();
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
      loop {
        match events.recv().await {
          Ok(event) => {
            if tx.send(event).await.is_err() {
              break;
            }
          }
          Err(RecvError::Lagged(_)) => {}
          Err(RecvError::Closed) => break,
        }
      }
    });
    Ok(rx)
  }
}

pub fn register_all(registry: &mut IpcRegistry) {
  registry
    .register::<PairStart>()
    .register::<PairCancel>()
    .register::<PairConnect>()
    .register::<PairAccept>()
    .register::<PairReject>()
    .register::<PairStatus>()
    .register::<PairNearby>()
    .register::<DevicesList>()
    .register::<DevicesUnpair>()
    .register_stream::<Events>();
}

//! Unpaired devices found nearby: by mDNS while discovery is on, or at the
//! other end of a cable (always: plugging it in is consent enough).

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Result, bail};
use aurora_proto::{Id, Msg, read_msg, write_msg};
use aurora_transport::{Hints, Seen, Via};
use tokio::{sync::broadcast, time::timeout};
use tracing::debug;

use super::Engine;
use crate::{
  pairing::{Target, hex},
  types::Nearby,
};

const WHO_TIMEOUT: Duration = Duration::from_secs(3);

impl Engine {
  /// Keeps the nearby list, asking each new device for its name
  pub(super) async fn nearby_loop(self: Arc<Self>) {
    let mut seen = self.node.seen();
    loop {
      match seen.recv().await {
        Ok(Seen::Found { id, addrs, via }) => {
          let usb = matches!(via, Via::Link(_));
          if self.nearby.lock().unwrap().contains_key(&id) && !usb {
            continue;
          }
          let engine = self.clone();
          tokio::spawn(async move {
            match timeout(WHO_TIMEOUT, engine.who(id, addrs.clone())).await {
              Ok(Ok((name, os))) if usb || engine.settings().discovery => {
                let nearby = Nearby {
                  name,
                  os,
                  addr: addrs.iter().find(|a| a.is_ipv4()).copied(),
                  link: Target {
                    id,
                    relay: None,
                    addrs,
                  }
                  .link(),
                  usb,
                };
                engine.nearby.lock().unwrap().insert(id, nearby);
              }
              Ok(Err(e)) => debug!("no name from {}: {e:#}", hex(&id)),
              _ => {}
            }
          });
        }
        Ok(Seen::Gone(id)) => {
          self.nearby.lock().unwrap().remove(&id);
        }
        Err(broadcast::error::RecvError::Lagged(_)) => {}
        Err(broadcast::error::RecvError::Closed) => break,
      }
    }
  }

  /// Name and OS of a device found nearby
  pub(super) async fn who(&self, id: Id, addrs: Vec<SocketAddr>) -> Result<(String, String)> {
    let conn = self.node.dial(id, &Hints { relay: None, addrs }).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(&mut send, &Msg::Who).await?;
    send.finish()?;
    let reply = read_msg(&mut recv).await;
    conn.close(0u32.into(), b"thanks");
    match reply? {
      Msg::Info { name, os } => Ok((name, os)),
      _ => bail!("unexpected reply"),
    }
  }

  /// Devices found nearby, minus paired ones
  pub async fn scan(&self) -> Result<Vec<Nearby>> {
    let store = self.store.lock().unwrap();
    Ok(
      (self.nearby.lock().unwrap().iter())
        .filter(|(id, _)| store.get(&hex(*id)).is_none())
        .map(|(_, n)| n.clone())
        .collect(),
    )
  }
}

#[cfg(test)]
mod tests {
  use super::super::tests::engine;

  #[tokio::test]
  async fn who_answers_only_while_discovery_is_on() {
    let (asker, _a) = engine("asker", false).await;
    let (quiet, _q) = engine("quiet", false).await;
    let addrs = quiet.node.hints().addrs;
    assert!(asker.who(quiet.id(), addrs.clone()).await.is_err());

    let mut s = quiet.settings();
    s.discovery = true;
    quiet.set_settings(s);
    let (name, os) = asker.who(quiet.id(), addrs).await.unwrap();
    assert_eq!((name.as_str(), os.as_str()), ("quiet", "Linux"));
  }
}

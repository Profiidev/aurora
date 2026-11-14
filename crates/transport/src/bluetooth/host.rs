//! Desktop side: advertise and accept L2CAP channels through BlueZ.

use std::{
  collections::{BTreeMap, BTreeSet},
  sync::{Arc, Mutex},
  time::Duration,
};

use anyhow::{Result, bail};
use bluer::{
  Uuid,
  adv::Advertisement,
  l2cap::{SocketAddr, StreamListener},
};
use tokio::{task::JoinSet, time::sleep};
use tracing::{debug, info, warn};

use super::{COMPANY, PSM, SERVICE, encode_advert};
use crate::link::{LinkKind, Links};

/// Between tries while BlueZ or the adapter is not there
const RETRY: Duration = Duration::from_secs(5);

/// Starts and stops advertising and listening
#[derive(Clone)]
pub(crate) struct BtHost {
  links: Links,
  task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl BtHost {
  pub(crate) fn new(links: Links) -> Self {
    Self {
      links,
      task: Arc::default(),
    }
  }

  /// Aborting the task drops the advertisement, the listener and every link
  /// it runs
  pub(crate) fn set_active(&self, on: bool) {
    let mut task = self.task.lock().unwrap();
    match (on, task.is_some()) {
      (true, false) => *task = Some(tokio::spawn(run(self.links.clone()))),
      (false, true) => task.take().unwrap().abort(),
      _ => {}
    }
  }
}

async fn run(links: Links) {
  let mut warned = false;
  loop {
    if let Err(e) = serve(&links).await {
      // once per activation: an adapter that is off stays off for a while
      if !warned {
        warn!("Bluetooth: {e:#}");
        warned = true;
      }
    }
    sleep(RETRY).await;
  }
}

async fn serve(links: &Links) -> Result<()> {
  let session = bluer::Session::new().await?;
  let adapter = session.default_adapter().await?;
  // the user's choice; we never switch the radio on
  if !adapter.is_powered().await? {
    bail!("adapter {} is off", adapter.name());
  }
  let addr = SocketAddr::new(adapter.address().await?, adapter.address_type().await?, PSM);
  let listener = StreamListener::bind(addr).await?;
  let _advertising = adapter
    .advertise(Advertisement {
      service_uuids: BTreeSet::from([Uuid::from_u128(SERVICE)]),
      manufacturer_data: BTreeMap::from([(COMPANY, encode_advert(PSM, &links.id()))]),
      discoverable: Some(true),
      ..Default::default()
    })
    .await?;
  info!("Bluetooth: advertising on {}", adapter.name());
  let mut tasks = JoinSet::new();
  loop {
    tokio::select! {
      accepted = listener.accept() => {
        let (stream, from) = accepted?;
        debug!("Bluetooth channel from {}", from.addr);
        let links = links.clone();
        tasks.spawn(async move {
          let (r, w) = stream.into_split();
          if let Err(e) = links.attach(LinkKind::Bluetooth, r, w).await {
            debug!("Bluetooth link: {e}");
          }
        });
      }
      Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
    }
  }
}

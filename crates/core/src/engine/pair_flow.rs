//! The pairing protocol. The waiting side shows a QR code or link and accepts
//! one dialer; both sides swap nonces, show the derived code and store the
//! device once both users accepted.

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
use aurora_proto::{Id, Msg, read_msg, write_msg};
use aurora_transport::{
  Connection, Hints, RecvStream, SendStream, Via, random_bytes, remote_id, via,
};
use tokio::{sync::watch, time::timeout};
use tracing::info;

use super::{
  Engine,
  links::linger,
  session::{Phase, cancelled},
};
use crate::{
  pairing::{Target, derive_code, hex},
  store::now,
  types::{Confirm, Device, Event, PairInfo},
};

/// The other side of a pairing, as it described itself
struct Peer {
  id: Id,
  name: String,
  os: String,
  hints: Hints,
  /// How the pairing connection reaches it, for the confirm screen
  via: Via,
}

/// What a dialer sent in its `PairHello`
pub(super) struct Hello {
  pub(super) name: String,
  pub(super) os: String,
  pub(super) nonce: [u8; 16],
  pub(super) hints: Hints,
}

impl Engine {
  // ---- waiting side -------------------------------------------------------

  /// Shows up for dialers and waits (the QR / "Pair new device" side)
  pub fn start_pairing(self: &Arc<Self>) -> Result<PairInfo> {
    let settings = self.settings();
    let hints = self.node.hints();
    // IPv4 only keeps the QR code small
    let addrs: Vec<_> = (hints.addrs.into_iter())
      .filter(SocketAddr::is_ipv4)
      .collect();
    let info = PairInfo {
      link: Target {
        id: self.node.id(),
        relay: hints.relay,
        addrs: addrs.clone(),
      }
      .link(),
      name: settings.name,
      addrs,
    };
    self.begin(Phase::Waiting { info: info.clone() })?;
    self.emit(Event::PairingStarted(info.clone()));
    Ok(info)
  }

  /// A dialer sent `PairHello` on its first stream
  pub(super) async fn accept_pair(
    self: &Arc<Self>,
    conn: &Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    hello: Hello,
  ) -> Result<()> {
    let (sid, cancel) = match self.claim(&remote_id(conn)) {
      Ok(c) => c,
      Err(e) => {
        // the dialer is untrusted: it gets a fixed reason, the detail stays in our log
        info!("refused pairing from {}: {e:#}", hello.name);
        conn.close(1u32.into(), b"not accepting pairing");
        return Ok(());
      }
    };
    let res = async {
      let nonce: [u8; 16] = random_bytes();
      let (s, mine) = (self.settings(), self.node.hints());
      write_msg(
        &mut send,
        &Msg::PairNonce {
          name: s.name,
          os: s.os,
          nonce,
          relay: mine.relay,
          addrs: mine.addrs,
        },
      )
      .await?;
      let id = remote_id(conn);
      let code = derive_code(&self.node.id(), &id, &hello.nonce, &nonce);
      let peer = Peer {
        id,
        name: hello.name,
        os: hello.os,
        hints: hello.hints,
        via: via(conn),
      };
      self
        .confirm(sid, cancel, &mut send, &mut recv, peer, code)
        .await
    }
    .await;
    self.paired_over(&res);
    self.conclude(sid, res);
    linger(conn, send).await;
    Ok(())
  }

  // ---- dialing side -------------------------------------------------------

  /// Pairs with a device given as an `aurora://pair` link or a hex device ID
  pub async fn connect(self: &Arc<Self>, target: &str) -> Result<()> {
    let target = Target::parse(target)?;
    if target.id == self.node.id() {
      bail!("that is this device");
    }
    if let Some(d) = self.store.lock().unwrap().get(&hex(&target.id)) {
      bail!("already paired with {}", d.name);
    }
    let (sid, cancel) = self.begin(Phase::Dialing)?;
    let engine = self.clone();
    tokio::spawn(async move {
      let res = engine.dial_pair(sid, cancel, target).await;
      engine.conclude(sid, res);
    });
    Ok(())
  }

  async fn dial_pair(
    self: &Arc<Self>,
    sid: u64,
    mut cancel: watch::Receiver<bool>,
    target: Target,
  ) -> Result<Option<Device>> {
    let hints = Hints {
      relay: target.relay,
      addrs: target.addrs,
    };
    let conn = tokio::select! {
      conn = timeout(super::links::DIAL_TIMEOUT, self.node.dial(target.id, &hints)) =>
        conn.map_err(|_| anyhow!("device did not answer"))??,
      () = cancelled(&mut cancel) => bail!("cancelled"),
    };
    let (mut send, mut recv) = conn.open_bi().await?;
    let nonce: [u8; 16] = random_bytes();
    let (s, mine) = (self.settings(), self.node.hints());
    write_msg(
      &mut send,
      &Msg::PairHello {
        name: s.name,
        os: s.os,
        nonce,
        relay: mine.relay,
        addrs: mine.addrs,
      },
    )
    .await?;
    let Msg::PairNonce {
      name,
      os,
      nonce: acceptor_nonce,
      relay,
      addrs: reported,
    } = read_msg(&mut recv)
      .await
      .context("device refused pairing")?
    else {
      bail!("unexpected reply");
    };
    let mut addrs = hints.addrs;
    for a in reported {
      if !addrs.contains(&a) {
        addrs.push(a);
      }
    }
    let code = derive_code(&self.node.id(), &target.id, &nonce, &acceptor_nonce);
    let peer = Peer {
      id: target.id,
      name,
      os,
      hints: Hints {
        relay: relay.or(hints.relay),
        addrs,
      },
      via: via(&conn),
    };
    let res = self
      .confirm(sid, cancel, &mut send, &mut recv, peer, code)
      .await;
    self.paired_over(&res);
    // report the outcome now; the peer still gets time to read our decision
    tokio::spawn(async move { linger(&conn, send).await });
    res
  }

  // ---- code confirmation (both sides) -------------------------------------

  /// Shows the code and waits for both users; stores the device on success.
  /// `None` when either side declined.
  async fn confirm(
    &self,
    sid: u64,
    mut cancel: watch::Receiver<bool>,
    send: &mut SendStream,
    recv: &mut RecvStream,
    peer: Peer,
    code: String,
  ) -> Result<Option<Device>> {
    let mut view = Confirm {
      code,
      peer_name: peer.name.clone(),
      peer_os: peer.os.clone(),
      via: peer.via.to_string(),
      fingerprint: hex(&peer.id[..8])
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c))
        .collect::<Vec<_>>()
        .join(" "),
      local_accepted: false,
    };
    let mut local_rx = self.show_code(sid, view.clone())?;
    self.emit(Event::CodeReady(view.clone()));

    let (mut local, mut remote) = (None::<bool>, None::<bool>);
    let remote_fut = read_msg(recv);
    tokio::pin!(remote_fut);
    loop {
      tokio::select! {
        d = &mut local_rx, if local.is_none() => {
          let accept = d.unwrap_or(false);
          local = Some(accept);
          view.local_accepted = accept;
          write_msg(send, &Msg::PairDecision { accept }).await?;
        }
        m = &mut remote_fut, if remote.is_none() => match m? {
          Msg::PairDecision { accept } => remote = Some(accept),
          _ => bail!("unexpected message while confirming"),
        },
        () = cancelled(&mut cancel) => bail!("cancelled"),
      }
      match (local, remote) {
        (Some(false), _) | (_, Some(false)) => return Ok(None),
        (Some(true), Some(true)) => break,
        _ => {}
      }
    }

    let device = Device {
      id: hex(&peer.id),
      name: peer.name,
      os: peer.os,
      paired_at: now(),
      relay: peer.hints.relay,
      addrs: peer.hints.addrs,
    };
    self.store.lock().unwrap().insert(device.clone())?;
    Ok(Some(device))
  }
}

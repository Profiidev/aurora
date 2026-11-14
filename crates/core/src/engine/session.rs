//! The single pairing session: who owns it, its phase, its timeout and how
//! it ends. The protocol driving it is in [`super::pair_flow`].

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use aurora_proto::Id;
use tokio::{
  sync::{oneshot, watch},
  time::sleep,
};

use super::Engine;
use crate::{
  pairing::hex,
  types::{Confirm, Device, Event, PairInfo, PairState},
};

pub(super) enum Phase {
  Idle,
  /// The pair page is open: dialers are accepted
  Waiting {
    info: PairInfo,
  },
  /// Dialing out, or a dialer is connected and being validated
  Dialing,
  Confirm {
    view: Confirm,
    decide: Option<oneshot::Sender<bool>>,
  },
}

pub(super) struct Session {
  pub(super) sid: u64,
  pub(super) phase: Phase,
  cancel: watch::Sender<bool>,
}

impl Default for Session {
  fn default() -> Self {
    Self {
      sid: 0,
      phase: Phase::Idle,
      cancel: watch::channel(false).0,
    }
  }
}

impl Session {
  pub(super) fn state(&self) -> PairState {
    match &self.phase {
      Phase::Idle => PairState::Idle,
      Phase::Waiting { info } => PairState::Waiting(info.clone()),
      Phase::Dialing => PairState::Dialing,
      Phase::Confirm { view, .. } => PairState::Confirm(view.clone()),
    }
  }
}

/// Resolves once the session it belongs to ends
pub(super) async fn cancelled(cancel: &mut watch::Receiver<bool>) {
  let _ = cancel.wait_for(|c| *c).await;
}

impl Engine {
  /// Claims the single pairing session
  pub(super) fn begin(self: &Arc<Self>, phase: Phase) -> Result<(u64, watch::Receiver<bool>)> {
    let mut s = self.session.lock().unwrap();
    if !matches!(s.phase, Phase::Idle) {
      bail!("pairing in progress");
    }
    s.sid += 1;
    s.cancel = watch::channel(false).0;
    // waiting lasts as long as the pair page is open
    let waiting = matches!(phase, Phase::Waiting { .. });
    s.phase = phase;
    let (sid, rx) = (s.sid, s.cancel.subscribe());
    drop(s);
    if !waiting {
      self.arm(sid);
    }
    Ok((sid, rx))
  }

  /// A dialer reached us: take over the waiting session
  pub(super) fn claim(self: &Arc<Self>, peer: &Id) -> Result<(u64, watch::Receiver<bool>)> {
    if self.store.lock().unwrap().get(&hex(peer)).is_some() {
      bail!("already paired");
    }
    let mut s = self.session.lock().unwrap();
    match &s.phase {
      Phase::Waiting { .. } => {}
      Phase::Idle => bail!("pair page is closed"),
      _ => bail!("another pairing is in progress"),
    }
    s.phase = Phase::Dialing;
    let (sid, rx) = (s.sid, s.cancel.subscribe());
    drop(s);
    self.arm(sid);
    Ok((sid, rx))
  }

  /// Moves session `sid` to the code confirmation; fails if it ended meanwhile
  pub(super) fn show_code(&self, sid: u64, view: Confirm) -> Result<oneshot::Receiver<bool>> {
    let (decide, rx) = oneshot::channel();
    let mut s = self.session.lock().unwrap();
    if s.sid != sid || matches!(s.phase, Phase::Idle) {
      bail!("cancelled");
    }
    s.phase = Phase::Confirm {
      view,
      decide: Some(decide),
    };
    Ok(rx)
  }

  /// Ends session `sid` once `pairing_timeout` passes
  fn arm(self: &Arc<Self>, sid: u64) {
    let timeout_for = self.settings().pairing_timeout;
    let engine = self.clone();
    tokio::spawn(async move {
      sleep(timeout_for).await;
      if engine.reset(sid) {
        engine.emit(Event::PairingExpired);
      }
    });
  }

  /// Back to idle and stop the session's tasks; `false` when `sid` is no
  /// longer the running session
  fn reset(&self, sid: u64) -> bool {
    let mut s = self.session.lock().unwrap();
    if s.sid != sid || matches!(s.phase, Phase::Idle) {
      return false;
    }
    s.phase = Phase::Idle;
    let _ = s.cancel.send(true);
    true
  }

  /// Ends the session and reports how it went, unless it was cancelled or
  /// expired meanwhile
  pub(super) fn conclude(&self, sid: u64, res: Result<Option<Device>>) {
    if !self.reset(sid) {
      return;
    }
    self.emit(match res {
      Ok(Some(device)) => Event::Paired(device),
      Ok(None) => Event::PairingFailed("rejected".into()),
      Err(e) => Event::PairingFailed(format!("{e:#}")),
    });
  }

  pub fn cancel_pairing(&self) {
    let sid = self.session.lock().unwrap().sid;
    if self.reset(sid) {
      self.emit(Event::PairingCancelled);
    }
  }

  /// The local user's answer to the shown code
  pub fn decide(&self, accept: bool) -> Result<()> {
    let mut s = self.session.lock().unwrap();
    match &mut s.phase {
      Phase::Confirm { view, decide } => {
        let tx = decide.take().ok_or_else(|| anyhow!("already answered"))?;
        // set here, not in `confirm`, so a `pair_state` right after this call sees it
        view.local_accepted = accept;
        tx.send(accept).map_err(|_| anyhow!("pairing ended"))
      }
      _ => bail!("no code to confirm"),
    }
  }
}

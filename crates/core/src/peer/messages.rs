//! The messages paired devices exchange. Add one by defining a type for its
//! trait and registering it in [`register_all`].

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{PeerCtx, PeerMessage, PeerRegistry, PeerRequest};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Battery {
  pub battery: Option<u8>,
  pub charging: bool,
}

/// The sender's battery changed
pub struct BatteryStatus;

impl PeerMessage for BatteryStatus {
  const KIND: &'static str = "status:battery";
  type Body = Battery;

  async fn handle(body: Battery, ctx: PeerCtx) -> Result<()> {
    ctx.engine.update_status(&ctx.device, |s| {
      (s.battery, s.charging) = (body.battery, body.charging);
    });
    Ok(())
  }
}

/// The sender removed the pairing
pub struct Unpair;

impl PeerMessage for Unpair {
  const KIND: &'static str = "device:unpair";
  type Body = ();

  async fn handle((): (), ctx: PeerCtx) -> Result<()> {
    ctx.engine.forget(&ctx.device);
    Ok(())
  }
}

/// Liveness check; the round trip time is the caller's to measure
pub struct Ping;

impl PeerRequest for Ping {
  const KIND: &'static str = "ping";
  type Req = ();
  type Resp = ();

  async fn handle((): (), _ctx: PeerCtx) -> Result<()> {
    Ok(())
  }
}

pub fn register_all(registry: &mut PeerRegistry) {
  registry
    .register::<BatteryStatus>()
    .register::<Unpair>()
    .register_request::<Ping>();
}

//! Messages between paired devices, registered like the IPC commands.
//!
//! Each message is a type implementing one of three traits, shared by the
//! sender and the receiver so both agree on the wire types:
//!
//! - [`PeerMessage`]: one-way, sent with [`Engine::notify`]
//! - [`PeerRequest`]: answered, sent with [`Engine::request`]
//! - [`PeerStream`]: a long-lived typed channel, opened with
//!   [`Engine::open_stream`]
//!
//! Register the receiving side in [`messages::register_all`]. Every message
//! travels on its own QUIC stream, whose first frame is an
//! [`Envelope`](aurora_proto::Envelope) naming the `KIND`. A device that does
//! not know a kind ignores a message and answers a request or stream with
//! [`Reply::Unsupported`], so builds of different ages still get along.

mod framed;
pub mod messages;

use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use anyhow::{Result, anyhow, bail};
use aurora_proto::{Envelope, Reply, decode, encode, read_frame, write_frame};
use aurora_transport::{Connection, RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};
use tokio::time::timeout;
use tracing::debug;

pub use self::framed::Framed;
use crate::{engine::Engine, ipc::command::BoxFuture};

/// Largest envelope a receiver reads
const MAX_ENVELOPE: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// What a handler knows about where a message came from
#[derive(Clone)]
pub struct PeerCtx {
  pub engine: Arc<Engine>,
  /// Hex id of the paired device that sent it
  pub device: String,
  /// The connection it arrived on
  pub conn: Connection,
}

/// One-way: fire and forget, one QUIC uni stream per message
pub trait PeerMessage: 'static {
  const KIND: &'static str;
  type Body: Serialize + DeserializeOwned + Send + Sync + 'static;

  fn handle(body: Self::Body, ctx: PeerCtx) -> impl Future<Output = Result<()>> + Send;
}

/// Request and response on one bi stream
pub trait PeerRequest: 'static {
  const KIND: &'static str;
  type Req: Serialize + DeserializeOwned + Send + Sync + 'static;
  type Resp: Serialize + DeserializeOwned + Send + 'static;

  fn handle(req: Self::Req, ctx: PeerCtx) -> impl Future<Output = Result<Self::Resp>> + Send;
}

/// A long-lived bi stream: the opener sends a `Header`, then both sides send
/// typed frames, `Up` from the opener and `Down` from the handler
pub trait PeerStream: 'static {
  const KIND: &'static str;
  type Header: Serialize + DeserializeOwned + Send + Sync + 'static;
  type Up: Serialize + DeserializeOwned + Send + 'static;
  type Down: Serialize + DeserializeOwned + Send + 'static;

  fn handle(
    header: Self::Header,
    ctx: PeerCtx,
    channel: Framed<Self::Down, Self::Up>,
  ) -> impl Future<Output = Result<()>> + Send;
}

type NotifyFn = Arc<dyn Fn(Vec<u8>, PeerCtx) -> BoxFuture<Result<()>> + Send + Sync>;
type RequestFn = Arc<dyn Fn(Vec<u8>, PeerCtx) -> BoxFuture<Result<Vec<u8>>> + Send + Sync>;
type StreamFn =
  Arc<dyn Fn(Vec<u8>, PeerCtx, SendStream, RecvStream) -> BoxFuture<Result<()>> + Send + Sync>;

#[derive(Clone)]
enum Handler {
  Notify(NotifyFn),
  Request(RequestFn),
  Stream(StreamFn),
}

/// The messages this device answers, by kind
#[derive(Default)]
pub struct PeerRegistry {
  handlers: HashMap<&'static str, Handler>,
}

impl PeerRegistry {
  pub fn register<M: PeerMessage>(&mut self) -> &mut Self {
    let f: NotifyFn =
      Arc::new(|body, ctx| Box::pin(async move { M::handle(decode(&body)?, ctx).await }));
    self.insert(M::KIND, Handler::Notify(f))
  }

  pub fn register_request<R: PeerRequest>(&mut self) -> &mut Self {
    let f: RequestFn =
      Arc::new(|body, ctx| Box::pin(async move { encode(&R::handle(decode(&body)?, ctx).await?) }));
    self.insert(R::KIND, Handler::Request(f))
  }

  pub fn register_stream<S: PeerStream>(&mut self) -> &mut Self {
    let f: StreamFn = Arc::new(|body, ctx, send, recv| {
      Box::pin(async move { S::handle(decode(&body)?, ctx, Framed::new(send, recv)).await })
    });
    self.insert(S::KIND, Handler::Stream(f))
  }

  fn insert(&mut self, kind: &'static str, handler: Handler) -> &mut Self {
    assert!(
      self.handlers.insert(kind, handler).is_none(),
      "peer message {kind} registered twice"
    );
    self
  }

  fn get(&self, kind: &str) -> Option<Handler> {
    self.handlers.get(kind).cloned()
  }
}

async fn read_envelope(recv: &mut RecvStream) -> Result<Envelope> {
  let env: Envelope = read_frame(recv).await?;
  if env.body.len() > MAX_ENVELOPE {
    bail!("envelope too large");
  }
  Ok(env)
}

/// Serves an incoming uni stream: a one-way message
pub(crate) async fn dispatch_uni(ctx: PeerCtx, mut recv: RecvStream) {
  let res = async {
    let env = read_envelope(&mut recv).await?;
    match ctx.engine.peers().get(&env.kind) {
      Some(Handler::Notify(f)) => f(env.body, ctx.clone()).await,
      _ => {
        debug!("ignored unknown message {}", env.kind);
        Ok(())
      }
    }
  }
  .await;
  if let Err(e) = res {
    debug!("message from {}: {e:#}", ctx.device);
  }
}

/// Serves an incoming bi stream: a request or a stream
pub(crate) async fn dispatch_bi(ctx: PeerCtx, mut send: SendStream, mut recv: RecvStream) {
  let res = async {
    let env = read_envelope(&mut recv).await?;
    match ctx.engine.peers().get(&env.kind) {
      Some(Handler::Request(f)) => {
        let reply = match f(env.body, ctx.clone()).await {
          Ok(body) => Reply::Ok(body),
          Err(e) => Reply::Err(format!("{e:#}")),
        };
        write_frame(&mut send, &reply).await?;
        send.finish()?;
        Ok(())
      }
      Some(Handler::Stream(f)) => {
        write_frame(&mut send, &Reply::Ok(Vec::new())).await?;
        f(env.body, ctx.clone(), send, recv).await
      }
      _ => {
        debug!("unsupported request {}", env.kind);
        write_frame(&mut send, &Reply::Unsupported).await?;
        send.finish()?;
        Ok(())
      }
    }
  }
  .await;
  if let Err(e) = res {
    debug!("stream from {}: {e:#}", ctx.device);
  }
}

fn envelope<T: Serialize>(kind: &str, body: &T) -> Result<Envelope> {
  Ok(Envelope {
    kind: kind.into(),
    body: encode(body)?,
  })
}

fn check(kind: &str, reply: Reply) -> Result<Vec<u8>> {
  match reply {
    Reply::Ok(body) => Ok(body),
    Reply::Err(e) => Err(anyhow!("{kind}: {e}")),
    Reply::Unsupported => Err(anyhow!("{kind} is not supported by the device")),
  }
}

/// Sends `M` on `conn`, for when the device is no longer in the live table
pub(crate) async fn notify_on<M: PeerMessage>(conn: &Connection, body: &M::Body) -> Result<()> {
  let mut send = conn.open_uni().await?;
  write_frame(&mut send, &envelope(M::KIND, body)?).await?;
  send.finish()?;
  Ok(())
}

impl Engine {
  fn conn_for(&self, device: &str) -> Result<Connection> {
    self
      .live_conn(device)
      .ok_or_else(|| anyhow!("device is not connected"))
  }

  /// Sends a one-way message to a connected device
  pub async fn notify<M: PeerMessage>(&self, device: &str, body: &M::Body) -> Result<()> {
    notify_on::<M>(&self.conn_for(device)?, body).await
  }

  /// Asks a connected device and waits for its answer
  pub async fn request<R: PeerRequest>(&self, device: &str, req: &R::Req) -> Result<R::Resp> {
    let conn = self.conn_for(device)?;
    timeout(REQUEST_TIMEOUT, async {
      let (mut send, mut recv) = conn.open_bi().await?;
      write_frame(&mut send, &envelope(R::KIND, req)?).await?;
      send.finish()?;
      let reply: Reply = read_frame(&mut recv).await?;
      decode(&check(R::KIND, reply)?)
    })
    .await
    .map_err(|_| anyhow!("{} timed out", R::KIND))?
  }

  /// Opens a typed channel to a connected device's `S` handler
  pub async fn open_stream<S: PeerStream>(
    &self,
    device: &str,
    header: &S::Header,
  ) -> Result<Framed<S::Up, S::Down>> {
    let conn = self.conn_for(device)?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &envelope(S::KIND, header)?).await?;
    let reply = timeout(REQUEST_TIMEOUT, read_frame::<_, Reply>(&mut recv))
      .await
      .map_err(|_| anyhow!("{} timed out", S::KIND))??;
    check(S::KIND, reply)?;
    Ok(Framed::new(send, recv))
  }
}

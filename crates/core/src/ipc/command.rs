use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::engine::Engine;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One request line: `{"command": "area:verb", "data": <json>}`
#[derive(Serialize, Deserialize)]
pub struct IpcPayload {
  pub command: String,
  #[serde(default)]
  pub data: Value,
}

/// A one-shot request. The type is shared by the server (`handle`) and the
/// client (`send`), so both sides agree on the wire types.
pub trait IpcCommand: 'static {
  const COMMAND: &'static str;
  type Payload: Serialize + DeserializeOwned + Send + 'static;
  type Response: Serialize + DeserializeOwned + Send + 'static;

  fn handle(
    payload: Self::Payload,
    engine: Arc<Engine>,
  ) -> impl Future<Output = Result<Self::Response>> + Send;
}

/// A long-lived request: the server answers with `Ok(null)` and then pushes one
/// JSON event per line until either side closes
pub trait IpcStream: 'static {
  const COMMAND: &'static str;
  type Payload: Serialize + DeserializeOwned + Send + 'static;
  type Event: Serialize + DeserializeOwned + Send + 'static;

  fn subscribe(
    payload: Self::Payload,
    engine: Arc<Engine>,
  ) -> impl Future<Output = Result<mpsc::Receiver<Self::Event>>> + Send;
}

pub type Handler = Arc<dyn Fn(Value, Arc<Engine>) -> BoxFuture<Result<Value>> + Send + Sync>;
pub type StreamHandler =
  Arc<dyn Fn(Value, Arc<Engine>) -> BoxFuture<Result<mpsc::Receiver<Value>>> + Send + Sync>;

pub fn erase<C: IpcCommand>() -> Handler {
  Arc::new(|data, engine| {
    Box::pin(async move {
      let payload = serde_json::from_value(data)?;
      Ok(serde_json::to_value(C::handle(payload, engine).await?)?)
    })
  })
}

pub fn erase_stream<S: IpcStream>() -> StreamHandler {
  Arc::new(|data, engine| {
    Box::pin(async move {
      let mut events = S::subscribe(serde_json::from_value(data)?, engine).await?;
      let (tx, rx) = mpsc::channel(32);
      tokio::spawn(async move {
        loop {
          let event = tokio::select! {
            event = events.recv() => event,
            // the client went away; quiet streams would never notice by sending
            () = tx.closed() => None,
          };
          let Some(event) = event else { break };
          let Ok(value) = serde_json::to_value(event) else {
            continue;
          };
          if tx.send(value).await.is_err() {
            break;
          }
        }
      });
      Ok(rx)
    })
  })
}

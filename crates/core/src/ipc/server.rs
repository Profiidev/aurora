use std::{
  collections::HashMap,
  fs::Permissions,
  os::unix::fs::{FileTypeExt, PermissionsExt},
  sync::Arc,
  time::Duration,
};

use anyhow::{Result, anyhow};
use interprocess::local_socket::{
  GenericFilePath, ListenerOptions, ToFsName,
  tokio::{Listener, Stream, prelude::*},
};
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  time::timeout,
};
use tracing::{debug, error};

use super::{
  command::{Handler, IpcCommand, IpcPayload, IpcStream, StreamHandler, erase, erase_stream},
  util,
};
use crate::engine::Engine;

/// Largest request line a client may send
const MAX_REQUEST: u64 = 1 << 20;
/// How long a client may take to send its request line
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct IpcRegistry {
  commands: HashMap<&'static str, Handler>,
  streams: HashMap<&'static str, StreamHandler>,
}

impl IpcRegistry {
  pub fn register<C: IpcCommand>(&mut self) -> &mut Self {
    self.commands.insert(C::COMMAND, erase::<C>());
    self
  }

  pub fn register_stream<S: IpcStream>(&mut self) -> &mut Self {
    self.streams.insert(S::COMMAND, erase_stream::<S>());
    self
  }
}

pub struct IpcServer {
  listener: Listener,
}

impl IpcServer {
  /// `None` when another instance already answers on the socket
  pub fn new() -> Result<Option<Self>> {
    let path = util::socket_path();
    util::private_dir(&path)?;
    if path.exists() {
      if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return Ok(None);
      }
      if !std::fs::symlink_metadata(&path)?.file_type().is_socket() {
        return Err(anyhow!("{} exists and is not a socket", path.display()));
      }
      std::fs::remove_file(&path)?;
    }
    let listener = ListenerOptions::new()
      .name(path.clone().to_fs_name::<GenericFilePath>()?)
      .create_tokio()?;
    std::fs::set_permissions(&path, Permissions::from_mode(0o600))?;
    Ok(Some(Self { listener }))
  }

  pub async fn run(self, registry: IpcRegistry, engine: Arc<Engine>) {
    let registry = Arc::new(registry);
    loop {
      let stream = match self.listener.accept().await {
        Ok(stream) => stream,
        Err(e) => {
          error!("Error accepting connection: {e}");
          continue;
        }
      };
      // a slow client or answer must not hold up the next connection
      let (registry, engine) = (registry.clone(), engine.clone());
      tokio::spawn(async move {
        if let Err(e) = serve_one(stream, &registry, engine).await {
          debug!("connection ended: {e:#}");
        }
      });
    }
  }
}

async fn serve_one(stream: Stream, registry: &IpcRegistry, engine: Arc<Engine>) -> Result<()> {
  let (reader, mut writer) = tokio::io::split(stream);
  let mut reader = BufReader::new(reader.take(MAX_REQUEST));
  let mut line = String::new();
  // an empty connection is the liveness probe from a second instance
  if timeout(REQUEST_TIMEOUT, reader.read_line(&mut line)).await?? == 0 {
    return Ok(());
  }
  let payload: IpcPayload = serde_json::from_str(&line)?;

  if let Some(handler) = registry.streams.get(payload.command.as_str()) {
    let mut events = match handler(payload.data, engine).await {
      Ok(events) => events,
      Err(e) => return reply(&mut writer, &Err(format!("{e:#}"))).await,
    };
    reply(&mut writer, &Ok(serde_json::Value::Null)).await?;
    let mut sink = [0u8; 64];
    loop {
      tokio::select! {
        event = events.recv() => {
          let Some(event) = event else { return Ok(()) };
          let mut buf = serde_json::to_vec(&event)?;
          buf.push(b'\n');
          writer.write_all(&buf).await?;
        }
        // the client never writes again; EOF means it went away
        read = reader.read(&mut sink) => if read? == 0 { return Ok(()) },
      }
    }
  }

  let res = match registry.commands.get(payload.command.as_str()) {
    Some(handler) => handler(payload.data, engine).await,
    None => Err(anyhow!("Unknown command: {}", payload.command)),
  }
  .map_err(|e| format!("{e:#}"));
  reply(&mut writer, &res).await
}

async fn reply<W: AsyncWriteExt + Unpin>(
  writer: &mut W,
  res: &Result<serde_json::Value, String>,
) -> Result<()> {
  let mut buf = serde_json::to_vec(res)?;
  buf.push(b'\n');
  Ok(writer.write_all(&buf).await?)
}

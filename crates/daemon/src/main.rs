use anyhow::Result;
use aurora_core::{
  config,
  engine::Engine,
  ipc::{self, IpcRegistry, IpcServer, commands},
};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
  tracing_subscriber::fmt()
    .with_env_filter(
      // noq_proto warns "failed closing path" each time a peer's last path idles out
      tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,noq_proto=error".into()),
    )
    .init();

  // binding the socket first is the single-instance guard
  let Some(server) = IpcServer::new()? else {
    error!("aurorad is already running");
    std::process::exit(1);
  };
  let socket = ipc::util::socket_path();
  let socket_id = socket_inode(&socket);

  let loaded = match config::read() {
    Ok(loaded) => loaded,
    Err(e) => {
      error!("{e:#}; using defaults");
      config::Loaded {
        config: Default::default(),
        unknown: Vec::new(),
      }
    }
  };
  config::warn_unknown(&loaded.unknown);

  let engine = Engine::start(loaded.config.settings(), &config::state_dir()).await?;
  let _watcher = config::watch({
    let engine = engine.clone();
    move |config| {
      if let Ok(config) = config {
        // ponytail: the QUIC port is read once at startup; restart to change it
        engine.set_settings(config.settings());
      }
    }
  })
  .inspect_err(|e| error!("config reload disabled: {e:#}"));

  let mut registry = IpcRegistry::default();
  commands::register_all(&mut registry);
  info!("aurorad started");
  tokio::select! {
    () = server.run(registry, engine.clone()) => {}
    () = shutdown_signal() => info!("shutting down"),
  }
  engine.shutdown().await;
  // the listener does not unlink its socket on drop; leave one a newer
  // instance bound meanwhile
  if socket_id.is_some() && socket_inode(&socket) == socket_id {
    let _ = std::fs::remove_file(&socket);
  }
  Ok(())
}

fn socket_inode(path: &std::path::Path) -> Option<u64> {
  use std::os::unix::fs::MetadataExt;
  std::fs::metadata(path).ok().map(|m| m.ino())
}

/// Resolves on SIGINT (Ctrl+C) or SIGTERM (systemd stop)
async fn shutdown_signal() {
  use tokio::signal::unix::{SignalKind, signal};
  let (Ok(mut term), Ok(mut int)) = (
    signal(SignalKind::terminate()),
    signal(SignalKind::interrupt()),
  ) else {
    // no handlers: stay up until killed
    return std::future::pending().await;
  };
  tokio::select! {
    _ = term.recv() => {}
    _ = int.recv() => {}
  }
}

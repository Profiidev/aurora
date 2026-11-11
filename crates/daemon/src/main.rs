use anyhow::Result;
use aurora_core::{
  config,
  engine::Engine,
  ipc::{IpcRegistry, IpcServer, commands},
};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
  tracing_subscriber::fmt()
    .with_env_filter(
      tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
    )
    .init();

  // binding the socket first is the single-instance guard
  let Some(server) = IpcServer::new()? else {
    error!("aurorad is already running");
    std::process::exit(1);
  };

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
    () = server.run(registry, engine) => {}
    _ = tokio::signal::ctrl_c() => {}
  }
  Ok(())
}

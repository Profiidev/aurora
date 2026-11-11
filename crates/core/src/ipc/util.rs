use std::{
  fs::{self, Permissions},
  os::unix::fs::{MetadataExt, PermissionsExt},
  path::{Path, PathBuf},
};

use anyhow::{Result, bail};

/// `$AURORA_SOCKET`, else `$XDG_RUNTIME_DIR/aurora.sock`, else a private
/// directory under `/tmp`
pub fn socket_path() -> PathBuf {
  if let Some(path) = std::env::var_os("AURORA_SOCKET") {
    return path.into();
  }
  match std::env::var_os("XDG_RUNTIME_DIR") {
    Some(dir) => PathBuf::from(dir).join("aurora.sock"),
    None => PathBuf::from(format!(
      "/tmp/aurora-{}",
      rustix::process::getuid().as_raw()
    ))
    .join("aurora.sock"),
  }
}

/// Creates the socket's `/tmp` fallback directory as 0700 and refuses one that
/// others could reach; any other parent must already exist
pub fn private_dir(socket: &Path) -> Result<()> {
  let Some(dir) = socket.parent() else {
    return Ok(());
  };
  let fallback = dir.parent() == Some(Path::new("/tmp"))
    && dir
      .file_name()
      .is_some_and(|n| n.to_string_lossy().starts_with("aurora-"));
  if !fallback {
    return Ok(());
  }
  if !dir.exists() {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))?;
  }
  let meta = fs::metadata(dir)?;
  if !meta.is_dir() || meta.uid() != rustix::process::getuid().as_raw() || meta.mode() & 0o077 != 0
  {
    bail!("{} is not a private directory", dir.display());
  }
  Ok(())
}

//! Desktop self update via cargo-packager-updater, ported from proton:
//! `latest.json` on the GitHub release (written by
//! `.github/workflows/desktop-release.yml`), bundles signed with aurora's own key.
//! Also starts `aurorad` when nothing runs it yet: deb installs through the
//! systemd user unit the package ships, AppImages as a child of the GUI.

use std::{
  path::{Path, PathBuf},
  process::{Child, Command},
  sync::Mutex,
  time::Duration,
};

use aurora_core::{
  config,
  ipc::{self, commands as cmd},
};
use cargo_packager_updater::{Config, Update, UpdaterBuilder, semver::Version, url::Url};
use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};
use gpui_kit::component::{WindowExt, notification::Notification};

pub const ENDPOINT: &str =
  "https://github.com/Profiidev/aurora/releases/latest/download/latest.json";
/// Aurora's own minisign updater key
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU3MDNEMjJBQTI0MUU4NUQKUldSZDZFR2lLdElEVjdwd3pVdVBTcE83REIrN3FJb1hPdSswRURJdWpNZ2xzMkNYUEF1WitZMFEK";

/// Binaries and dpkg file list of the `aurora` deb
const DEB_GUI: &str = "/usr/bin/aurora-gui";
const DPKG_LIST: &str = "/var/lib/dpkg/info/aurora.list";
/// `platform/packaging/aurorad.service`, installed by the deb
const UNIT: &str = "aurorad.service";

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
  Idle,
  Checking,
  UpToDate,
  Available {
    version: SharedString,
  },
  Downloading {
    version: SharedString,
    percent: Option<u8>,
  },
  Installing {
    version: SharedString,
  },
  Error(SharedString),
}

pub struct Updater {
  pub state: UpdateState,
  /// `[update] auto` of the config
  pub auto: bool,
  endpoint: String,
  pending: Option<Update>,
}

struct GlobalUpdater(Entity<Updater>);
impl Global for GlobalUpdater {}

/// The updater, `None` before `init` (tests, mobile)
pub fn updater(cx: &App) -> Option<Entity<Updater>> {
  cx.try_global::<GlobalUpdater>().map(|g| g.0.clone())
}

/// `aurorad` the GUI started itself (AppImage), stopped when the GUI quits
static DAEMON: Mutex<Option<Child>> = Mutex::new(None);

pub fn init(cx: &mut App) {
  let auto = config::read().is_ok_and(|l| l.config.update.auto);
  let updater = cx.new(|_| Updater {
    state: UpdateState::Idle,
    auto,
    endpoint: ENDPOINT.into(),
    pending: None,
  });
  cx.set_global(GlobalUpdater(updater));
  cx.on_app_quit(|_| async { stop_own_daemon() }).detach();
  std::thread::spawn(ensure_daemon);
  check_on_start(cx);
}

/// How this build updates itself
#[derive(Debug, Clone, Copy, PartialEq)]
enum Install {
  /// Replace the AppImage
  AppImage,
  /// Download the signed .deb and install it with `pkexec dpkg -i`
  Deb,
  /// Managed elsewhere (nix, cargo run), refuse to self update
  Unmanaged,
}

/// True when `exe` is the GUI the aurora deb installed, `dpkg_list` is its dpkg file list
fn deb_owns(exe: &Path, dpkg_list: &str) -> bool {
  exe == Path::new(DEB_GUI) && dpkg_list.lines().any(|l| l == DEB_GUI)
}

fn install_kind() -> Install {
  if std::env::var_os("APPIMAGE").is_some() {
    return Install::AppImage;
  }
  let exe = std::env::current_exe().unwrap_or_default();
  let list = std::fs::read_to_string(DPKG_LIST).unwrap_or_default();
  if deb_owns(&exe, &list) {
    Install::Deb
  } else {
    Install::Unmanaged
  }
}

/// Starts `aurorad` when it does not answer (blocking). Unmanaged installs
/// start it themselves (nix modules, `just daemon`).
fn ensure_daemon() {
  if ipc::send::<cmd::DevicesList>(()).is_ok() {
    return;
  }
  match install_kind() {
    // enabled so it also starts at the next login
    Install::Deb => {
      let _ = Command::new("systemctl")
        .args(["--user", "enable", "--now", UNIT])
        .status();
    }
    Install::AppImage => {
      let Some(dir) = std::env::var_os("APPDIR") else {
        return;
      };
      match Command::new(Path::new(&dir).join("usr/bin/aurorad")).spawn() {
        Ok(child) => *DAEMON.lock().unwrap() = Some(child),
        Err(e) => eprintln!("failed to start aurorad: {e}"),
      }
    }
    Install::Unmanaged => {}
  }
}

/// The AppImage is unmounted once the GUI exits, its daemon cannot outlive it
fn stop_own_daemon() {
  if let Some(mut child) = DAEMON.lock().unwrap().take() {
    // SIGTERM: aurorad shuts its engine down and removes its socket
    let _ = Command::new("kill").arg(child.id().to_string()).status();
    let _ = child.wait();
  }
}

fn builder(endpoint: &str) -> anyhow::Result<cargo_packager_updater::Updater> {
  let config = Config {
    endpoints: vec![Url::parse(endpoint)?],
    pubkey: PUBKEY.into(),
    ..Default::default()
  };
  let mut builder = UpdaterBuilder::new(Version::parse(env!("CARGO_PKG_VERSION"))?, config)
    .timeout(Duration::from_secs(30));
  // `linux-x86_64-deb` in latest.json (desktop-release.yml)
  if install_kind() == Install::Deb
    && let Some(target) = cargo_packager_updater::target()
  {
    builder = builder.target(format!("{target}-deb"));
  }
  Ok(builder.build()?)
}

/// The newer release at `endpoint`, `None` when this build is up to date (blocking)
fn fetch(endpoint: &str) -> anyhow::Result<Option<Update>> {
  Ok(builder(endpoint)?.check()?)
}

impl Updater {
  fn set(&mut self, state: UpdateState, cx: &mut Context<Self>) {
    if self.state != state {
      self.state = state;
      cx.notify();
    }
  }

  pub fn set_auto(&mut self, on: bool, cx: &mut Context<Self>) {
    self.auto = on;
    cx.notify();
    if let Err(e) = config::set_auto_update(&config::state_dir(), on) {
      toast(Notification::error(format!("Failed to save: {e:#}")), cx);
    }
  }

  /// Looks for a newer release, `Some(version)` when there is one
  pub fn check(&mut self, cx: &mut Context<Self>) -> Task<anyhow::Result<Option<SharedString>>> {
    self.set(UpdateState::Checking, cx);
    let endpoint = self.endpoint.clone();
    let job = cx
      .background_executor()
      .spawn(async move { fetch(&endpoint) });
    cx.spawn(async move |this, cx| {
      let res = job.await;
      this.update(cx, |this, cx| {
        let state = match &res {
          Ok(Some(u)) => UpdateState::Available {
            version: u.version.clone().into(),
          },
          Ok(None) => UpdateState::UpToDate,
          Err(e) => UpdateState::Error(format!("{e:#}").into()),
        };
        this.set(state, cx);
        res.map(|u| {
          let version = u.as_ref().map(|u| SharedString::from(u.version.clone()));
          this.pending = u;
          version
        })
      })?
    })
  }
}

/// Downloads, verifies and installs the pending update, then restarts
pub fn install(cx: &mut App) {
  let Some(updater) = updater(cx) else { return };
  let Some(update) = updater.read(cx).pending.clone() else {
    return;
  };
  let version: SharedString = update.version.clone().into();
  updater.update(cx, |u, cx| {
    u.set(
      UpdateState::Downloading {
        version: version.clone(),
        percent: Some(0),
      },
      cx,
    )
  });

  let (tx, rx) = flume::unbounded::<Progress>();
  let job = cx
    .background_executor()
    .spawn(async move { download_and_install(&update, tx) });
  cx.spawn(async move |cx| {
    while let Ok(p) = rx.recv_async().await {
      let state = match p {
        Progress::Percent(percent) => UpdateState::Downloading {
          version: version.clone(),
          percent,
        },
        Progress::Installing => UpdateState::Installing {
          version: version.clone(),
        },
      };
      updater.update(cx, |u, cx| u.set(state, cx));
    }
    let res = job.await;
    cx.update(|cx| match res {
      Ok(relaunch) => restart(relaunch, cx),
      Err(e) => {
        eprintln!("update failed: {e:#}");
        toast(Notification::error(format!("Update failed: {e}")), cx);
        updater.update(cx, |u, cx| {
          u.set(
            UpdateState::Available {
              version: version.clone(),
            },
            cx,
          )
        });
      }
    });
  })
  .detach();
}

enum Progress {
  Percent(Option<u8>),
  Installing,
}

/// Blocking download + signature check + install, returns the binary to relaunch
fn download_and_install(update: &Update, tx: flume::Sender<Progress>) -> anyhow::Result<PathBuf> {
  let kind = install_kind();
  let relaunch = match kind {
    Install::Deb => PathBuf::from(DEB_GUI),
    Install::AppImage => std::env::var_os("APPIMAGE")
      .map(PathBuf::from)
      .unwrap_or_default(),
    Install::Unmanaged => {
      anyhow::bail!("this install is not self updating, use your package manager")
    }
  };

  let done = std::cell::Cell::new(0usize);
  let last = std::cell::Cell::new(None::<u8>);
  let bytes = update.download_extended(
    |n, total| {
      done.set(done.get() + n);
      let percent = total.map(|t| (done.get() as u64 * 100 / t.max(1)).min(100) as u8);
      if percent != last.get() {
        last.set(percent);
        let _ = tx.send(Progress::Percent(percent));
      }
    },
    || {
      let _ = tx.send(Progress::Installing);
    },
  )?;
  if kind == Install::Deb {
    install_deb(&bytes)?;
    // only restarts a running unit; an AppImage daemon stops with the GUI
    let _ = Command::new("systemctl")
      .args(["--user", "try-restart", UNIT])
      .status();
  } else {
    update.install(bytes)?;
  }
  Ok(relaunch)
}

/// `bytes` are signature checked by `download_extended`
// shortcut: untested, it needs root (pkexec dpkg -i); test with a fake `pkexec` early on PATH
fn install_deb(bytes: &[u8]) -> anyhow::Result<()> {
  let path = std::env::temp_dir().join(format!("aurora-update-{}.deb", std::process::id()));
  std::fs::write(&path, bytes)?;
  let status = Command::new("pkexec")
    .args(["dpkg", "-i"])
    .arg(&path)
    .status();
  let _ = std::fs::remove_file(&path);
  let status =
    status.map_err(|e| anyhow::anyhow!("pkexec is needed to install the update: {e}"))?;
  anyhow::ensure!(status.success(), "dpkg failed: {status}");
  Ok(())
}

/// Starts the updated GUI and quits; quitting stops an AppImage's own daemon
/// and the new GUI starts the new one
fn restart(binary: PathBuf, cx: &mut App) {
  match Command::new(&binary).spawn() {
    Ok(_) => cx.quit(),
    Err(e) => {
      eprintln!("failed to relaunch {}: {e}", binary.display());
      toast(
        Notification::warning("Update installed, please restart Aurora"),
        cx,
      );
    }
  }
}

/// Startup check: installs right away with auto update on, otherwise offers it in a toast
fn check_on_start(cx: &mut App) {
  let Some(updater) = updater(cx) else { return };
  let task = updater.update(cx, |u, cx| u.check(cx));
  cx.spawn(async move |cx| match task.await {
    Ok(Some(version)) => cx.update(|cx| {
      if updater.read(cx).auto {
        install(cx);
      } else {
        toast(
          Notification::info(format!(
            "Aurora {version} is available, install it under Settings"
          )),
          cx,
        );
      }
    }),
    Ok(None) => {}
    Err(e) => eprintln!("update check failed: {e:#}"),
  })
  .detach();
}

/// Shows `note` in the first window
pub fn toast(note: Notification, cx: &mut App) {
  // deferred: from a click handler the window is mid-update and a nested update fails
  cx.defer(move |cx| {
    if let Some(window) = cx.windows().into_iter().next() {
      let _ = window.update(cx, |_, window, cx| window.push_notification(note, cx));
    }
  });
}

#[cfg(test)]
mod tests {
  use std::{
    io::{Read, Write},
    net::TcpListener,
  };

  use super::*;

  /// Serves `body` to every request, returns the `latest.json` URL
  fn serve(body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/latest.json", listener.local_addr().unwrap());
    std::thread::spawn(move || {
      for mut stream in listener.incoming().flatten() {
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = write!(
          stream,
          "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
          body.len()
        );
      }
    });
    url
  }

  /// The shape desktop-release.yml writes
  fn latest_json(version: &str) -> String {
    let platform = serde_json::json!({
      "url": "http://127.0.0.1:9/Aurora.AppImage",
      "signature": "c2ln",
      "format": "appimage",
    });
    serde_json::json!({
      "version": version,
      "notes": "",
      "pub_date": "2026-11-14T12:00:00Z",
      "platforms": {
        "linux-x86_64": platform,
        "linux-x86_64-deb": platform,
        "linux-aarch64": platform,
      }
    })
    .to_string()
  }

  #[test]
  fn newer_release_is_an_update_same_or_older_is_not() {
    let update = fetch(&serve(latest_json("v99.0.0"))).unwrap().unwrap();
    assert_eq!(update.version, "99.0.0");
    for version in [env!("CARGO_PKG_VERSION"), "0.0.1"] {
      assert!(
        fetch(&serve(latest_json(version))).unwrap().is_none(),
        "{version}"
      );
    }
    assert!(fetch(&serve("<html>".into())).is_err());
    assert!(fetch("not a url").is_err());
  }

  #[test]
  fn pubkey_is_a_minisign_key() {
    use base64::Engine;
    let key = base64::engine::general_purpose::STANDARD
      .decode(PUBKEY)
      .unwrap();
    assert!(
      String::from_utf8(key)
        .unwrap()
        .starts_with("untrusted comment: minisign public key")
    );
  }

  #[test]
  fn deb_detection_needs_the_binary_and_its_dpkg_entry() {
    let list = "/.\n/usr/bin\n/usr/bin/aurora-gui\n/usr/bin/aurorad\n";
    assert!(deb_owns(Path::new("/usr/bin/aurora-gui"), list));
    assert!(!deb_owns(Path::new("/usr/bin/aurora-gui"), ""));
    assert!(!deb_owns(Path::new("/nix/store/x/bin/aurora-gui"), list));
  }
}

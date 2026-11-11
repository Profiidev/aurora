use std::io::{self, BufRead, Write};

use anyhow::Result;
use aurora_core::{
  config,
  ipc::{self, commands as cmd},
  types::{Confirm, DeviceStatus, Event, PairInfo, PairState},
};
use clap::{Parser, Subcommand};
use qrcode::{QrCode, render::unicode};

#[derive(Parser)]
#[command(name = "aurora", version, about = "Aurora command line client")]
pub struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(Subcommand)]
enum Commands {
  /// Pair a phone
  #[command(subcommand)]
  Pair(Pair),
  /// Paired devices
  #[command(subcommand)]
  Devices(Devices),
  /// Configuration files
  #[command(subcommand)]
  Config(Config),
}

#[derive(Subcommand)]
enum Pair {
  /// Show a QR code and wait for a phone
  Start,
  /// Pair with an aurora:// link or a device ID
  Connect {
    target: String,
  },
  /// Find devices on the local network
  Nearby,
  /// Confirm the shown code
  Accept,
  /// Decline the shown code
  Reject,
  Cancel,
  Status,
}

#[derive(Subcommand)]
enum Devices {
  List,
  Unpair { id: String },
}

#[derive(Subcommand)]
enum Config {
  /// Check the config files for errors and unknown keys
  Validate,
  /// Print the effective config
  Show,
}

pub fn run() {
  if let Err(e) = Cli::parse().execute() {
    eprintln!("error: {e:#}");
    std::process::exit(1);
  }
}

impl Cli {
  fn execute(self) -> Result<()> {
    match self.command {
      Commands::Pair(p) => p.run(),
      Commands::Devices(d) => d.run(),
      Commands::Config(c) => c.run(),
    }
  }
}

impl Pair {
  fn run(self) -> Result<()> {
    match self {
      Pair::Start => {
        // subscribe first so no event is missed
        let events = ipc::subscribe::<cmd::Events>(())?;
        show_info(&ipc::send::<cmd::PairStart>(())?);
        follow(events)
      }
      Pair::Connect { target } => {
        let events = ipc::subscribe::<cmd::Events>(())?;
        ipc::send::<cmd::PairConnect>(target)?;
        follow(events)
      }
      Pair::Nearby => {
        for n in ipc::send::<cmd::PairNearby>(())? {
          let addr = n.addr.map(|a| a.ip().to_string()).unwrap_or_default();
          println!("{}  {}  {addr}\n  {}", n.name, n.os, n.link);
        }
        Ok(())
      }
      Pair::Accept => Ok(ipc::send::<cmd::PairAccept>(())?),
      Pair::Reject => Ok(ipc::send::<cmd::PairReject>(())?),
      Pair::Cancel => Ok(ipc::send::<cmd::PairCancel>(())?),
      Pair::Status => {
        match ipc::send::<cmd::PairStatus>(())? {
          PairState::Idle => println!("idle"),
          PairState::Waiting(info) => show_info(&info),
          PairState::Dialing => println!("connecting"),
          PairState::Confirm(c) => println!("code {} with {} ({})", c.code, c.peer_name, c.peer_os),
        }
        Ok(())
      }
    }
  }
}

fn show_info(info: &PairInfo) {
  if let Ok(qr) = QrCode::new(info.link.as_bytes()) {
    println!(
      "{}",
      qr.render::<unicode::Dense1x2>().quiet_zone(true).build()
    );
  }
  println!("Scan this on your phone, or enter one of these addresses:");
  for addr in &info.addrs {
    println!("  {}", addr.ip());
  }
  println!("{}\nwaiting...", info.link);
}

/// Prints events of the running session; asks about the code
fn follow(events: impl Iterator<Item = Result<Event>>) -> Result<()> {
  for event in events {
    match event? {
      Event::CodeReady(Confirm {
        code,
        peer_name,
        peer_os,
        via,
        ..
      }) => {
        println!("\n{peer_name} ({peer_os}) via {via}\nCode: {code}");
        print!("Does the phone show the same code? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("y") {
          ipc::send::<cmd::PairAccept>(())?;
          println!("waiting for the phone...");
        } else {
          ipc::send::<cmd::PairReject>(())?;
        }
      }
      Event::Paired(d) => {
        println!("Paired with {} ({})", d.name, d.os);
        return Ok(());
      }
      Event::PairingExpired => anyhow::bail!("pairing timed out"),
      Event::PairingCancelled => anyhow::bail!("pairing cancelled"),
      Event::PairingFailed(why) => anyhow::bail!("pairing failed: {why}"),
      _ => {}
    }
  }
  anyhow::bail!("aurorad went away")
}

impl Devices {
  fn run(self) -> Result<()> {
    match self {
      Devices::List => {
        for DeviceStatus {
          device,
          connected,
          battery,
          charging,
        } in ipc::send::<cmd::DevicesList>(())?
        {
          let state = if connected { "connected" } else { "offline" };
          let battery = battery.map_or(String::new(), |b| {
            format!("  battery {b}%{}", if charging { " charging" } else { "" })
          });
          println!(
            "{}  {}  {}  {state}{battery}",
            device.id, device.name, device.os
          );
        }
        Ok(())
      }
      Devices::Unpair { id } => Ok(ipc::send::<cmd::DevicesUnpair>(id)?),
    }
  }
}

impl Config {
  fn run(self) -> Result<()> {
    let loaded = config::read()?;
    for key in &loaded.unknown {
      eprintln!("warning: unknown config key `{key}`");
    }
    match self {
      Config::Validate => println!("ok"),
      Config::Show => print!("{}", toml::to_string_pretty(&loaded.config)?),
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use clap::CommandFactory;

  use super::*;

  #[test]
  fn commands_parse() {
    Cli::command().debug_assert();
    let cli = Cli::try_parse_from(["aurora", "pair", "connect", "aurora://pair?id=ab"]).unwrap();
    assert!(
      matches!(cli.command, Commands::Pair(Pair::Connect { target }) if target == "aurora://pair?id=ab")
    );
    assert!(Cli::try_parse_from(["aurora", "pair", "connect"]).is_err());
  }
}

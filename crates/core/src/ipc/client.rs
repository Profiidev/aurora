use std::io::{BufRead, BufReader, Write};

use anyhow::{Result, anyhow, bail};
use interprocess::local_socket::{GenericFilePath, Stream, ToFsName, prelude::*};
use serde_json::Value;

use super::{
  command::{IpcCommand, IpcPayload, IpcStream},
  util,
};

fn connect(command: &str, data: Value) -> Result<BufReader<Stream>> {
  let name = util::socket_path().to_fs_name::<GenericFilePath>()?;
  let mut stream = Stream::connect(name)
    .map_err(|e| anyhow!("cannot reach aurorad ({e}); is the daemon running?"))?;
  let mut line = serde_json::to_vec(&IpcPayload {
    command: command.into(),
    data,
  })?;
  line.push(b'\n');
  stream.write_all(&line)?;
  Ok(BufReader::new(stream))
}

/// Reads the `{"Ok": ..}` / `{"Err": ".."}` reply line
fn reply(reader: &mut BufReader<Stream>) -> Result<Value> {
  let mut line = String::new();
  if reader.read_line(&mut line)? == 0 {
    bail!("aurorad closed the connection without a reply");
  }
  match serde_json::from_str::<Result<Value, String>>(&line)? {
    Ok(value) => Ok(value),
    Err(e) => bail!(e),
  }
}

pub fn send_raw(command: &str, data: Value) -> Result<Value> {
  reply(&mut connect(command, data)?)
}

/// Blocking request to the running daemon
pub fn send<C: IpcCommand>(payload: C::Payload) -> Result<C::Response> {
  Ok(serde_json::from_value(send_raw(
    C::COMMAND,
    serde_json::to_value(payload)?,
  )?)?)
}

/// Blocking iterator over the events of a stream, ends when the daemon goes away
pub fn subscribe<S: IpcStream>(
  payload: S::Payload,
) -> Result<impl Iterator<Item = Result<S::Event>>> {
  let mut reader = connect(S::COMMAND, serde_json::to_value(payload)?)?;
  reply(&mut reader)?;
  Ok(reader.lines().map(|line| Ok(serde_json::from_str(&line?)?)))
}

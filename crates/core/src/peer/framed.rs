use std::marker::PhantomData;

use anyhow::Result;
use aurora_proto::{read_frame, write_frame};
use aurora_transport::{RecvStream, SendStream};
use serde::{Serialize, de::DeserializeOwned};

/// A typed channel over one QUIC bi stream: sends `Tx`, receives `Rx`, one
/// postcard frame each
pub struct Framed<Tx, Rx> {
  send: SendStream,
  recv: RecvStream,
  _types: PhantomData<fn(Tx) -> Rx>,
}

impl<Tx: Serialize, Rx: DeserializeOwned> Framed<Tx, Rx> {
  pub(super) fn new(send: SendStream, recv: RecvStream) -> Self {
    Self {
      send,
      recv,
      _types: PhantomData,
    }
  }

  pub async fn send(&mut self, item: &Tx) -> Result<()> {
    write_frame(&mut self.send, item).await
  }

  /// The next item, `None` once the other side finished
  pub async fn recv(&mut self) -> Result<Option<Rx>> {
    match read_frame(&mut self.recv).await {
      Ok(item) => Ok(Some(item)),
      Err(e) if is_end(&e) => Ok(None),
      Err(e) => Err(e),
    }
  }

  /// Ends our direction; the other side's `recv` returns `None`
  pub fn finish(&mut self) -> Result<()> {
    self.send.finish()?;
    Ok(())
  }

  pub fn into_inner(self) -> (SendStream, RecvStream) {
    (self.send, self.recv)
  }
}

fn is_end(e: &anyhow::Error) -> bool {
  e.downcast_ref::<std::io::Error>()
    .is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)
}

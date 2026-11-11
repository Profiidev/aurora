//! Pure pairing helpers: the confirm code and the QR / deep link.

use std::net::SocketAddr;

use anyhow::{Context, Result, bail};
use aurora_proto::{Id, Token};
use url::Url;

pub fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex<const N: usize>(s: &str) -> Result<[u8; N]> {
  if s.len() != N * 2 || !s.is_ascii() {
    bail!("expected {} hex digits", N * 2);
  }
  let mut out = [0u8; N];
  for (i, byte) in out.iter_mut().enumerate() {
    *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)?;
  }
  Ok(out)
}

/// Six digits both devices compute from the two (TLS authenticated) endpoint
/// ids and the two nonces, so a man in the middle would show a different code
pub fn derive_code(a: &Id, b: &Id, dialer_nonce: &[u8; 16], acceptor_nonce: &[u8; 16]) -> String {
  let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
  let mut hasher = blake3::Hasher::new_derive_key("aurora pairing code v1");
  hasher.update(lo);
  hasher.update(hi);
  hasher.update(dialer_nonce);
  hasher.update(acceptor_nonce);
  let hash = hasher.finalize();
  let n = u32::from_be_bytes(hash.as_bytes()[..4].try_into().unwrap());
  format!("{:06}", n % 1_000_000)
}

/// Where and how to reach a device for pairing
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
  pub id: Id,
  pub addrs: Vec<SocketAddr>,
  pub token: Option<Token>,
}

impl Target {
  pub fn link(&self) -> String {
    let mut url = Url::parse("aurora://pair").unwrap();
    {
      let mut q = url.query_pairs_mut();
      q.append_pair("id", &hex(&self.id));
      for addr in &self.addrs {
        q.append_pair("addr", &addr.to_string());
      }
      if let Some(token) = &self.token {
        q.append_pair("token", &hex(token));
      }
    }
    url.into()
  }

  pub fn parse(link: &str) -> Result<Self> {
    let url = Url::parse(link.trim()).context("not an aurora:// link")?;
    if url.scheme() != "aurora" || url.host_str() != Some("pair") {
      bail!("not an aurora://pair link");
    }
    let (mut id, mut addrs, mut token) = (None, Vec::new(), None);
    for (key, value) in url.query_pairs() {
      match &*key {
        "id" => id = Some(unhex(&value)?),
        "addr" => addrs.push(value.parse()?),
        "token" => token = Some(unhex(&value)?),
        _ => {}
      }
    }
    Ok(Self {
      id: id.context("link has no id")?,
      addrs,
      token,
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn code_is_symmetric_and_nonce_bound() {
    let (a, b) = ([1u8; 32], [2u8; 32]);
    let (n1, n2) = ([3u8; 16], [4u8; 16]);
    let code = derive_code(&a, &b, &n1, &n2);
    assert_eq!(code.len(), 6);
    assert_eq!(code, derive_code(&b, &a, &n1, &n2));
    assert_ne!(code, derive_code(&a, &b, &n2, &n1));
    assert_ne!(code, derive_code(&a, &[9u8; 32], &n1, &n2));
  }

  #[test]
  fn link_round_trips() {
    let target = Target {
      id: [7; 32],
      addrs: vec![
        "192.168.1.10:4000".parse().unwrap(),
        "[::1]:4000".parse().unwrap(),
      ],
      token: Some([9; 16]),
    };
    assert_eq!(Target::parse(&target.link()).unwrap(), target);
    assert!(Target::parse("https://pair").is_err());
    assert!(Target::parse("aurora://pair?addr=1.2.3.4:5").is_err());
  }
}

//! Bluetooth LE: the desktop advertises [`SERVICE`] and listens for L2CAP
//! connection-oriented channels on [`PSM`]; phones scan and dial. The link
//! is insecure at the Bluetooth level, QUIC's TLS authenticates.

#[cfg(feature = "bluer")]
mod host;

use aurora_proto::Id;
#[cfg(feature = "bluer")]
pub(crate) use host::BtHost;

/// Aurora's GATT service UUID, advertised by the desktop
pub const SERVICE: u128 = 0x6a1f_3c2e_8b4d_4e7a_9c50_aa0b_7d31_e5c4;
/// The same, as Java and Swift parse it
pub const SERVICE_STR: &str = "6a1f3c2e-8b4d-4e7a-9c50-aa0b7d31e5c4";
/// L2CAP LE PSM the desktop listens on (LE dynamic range)
pub const PSM: u16 = 0x0081;

/// Manufacturer id the advert data goes under: 0xFFFF, the id Bluetooth
/// SIG reserves for testing; a legacy advert has no room for service data
/// keyed by a 128-bit UUID next to the UUID list iOS filters on
pub const COMPANY: u16 = 0xffff;
/// First bytes of a desktop's endpoint id in its advert
pub const PREFIX_LEN: usize = 4;
const ADVERT_LEN: usize = 2 + PREFIX_LEN;

/// Manufacturer data: PSM (big-endian) and the start of the desktop's
/// endpoint id, so phones dial only desktops they want. Flags (3) + UUID
/// list (18) + this (4 + 6) fill the 31 byte legacy advert exactly.
pub fn encode_advert(psm: u16, id: &Id) -> Vec<u8> {
  let mut data = Vec::with_capacity(ADVERT_LEN);
  data.extend_from_slice(&psm.to_be_bytes());
  data.extend_from_slice(&id[..PREFIX_LEN]);
  data
}

/// PSM and id prefix, `None` for data from another version or device
pub fn decode_advert(data: &[u8]) -> Option<(u16, [u8; PREFIX_LEN])> {
  if data.len() != ADVERT_LEN {
    return None;
  }
  let psm = u16::from_be_bytes([data[0], data[1]]);
  Some((psm, data[2..].try_into().ok()?))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn adverts_round_trip() {
    let id: Id = std::array::from_fn(|i| i as u8);
    let data = encode_advert(PSM, &id);
    assert_eq!(decode_advert(&data), Some((PSM, [0, 1, 2, 3])));
  }

  #[test]
  fn foreign_adverts_are_ignored() {
    assert_eq!(decode_advert(&[]), None);
    assert_eq!(decode_advert(&encode_advert(PSM, &[1; 32])[..5]), None);
    assert_eq!(decode_advert(&[0; 9]), None);
  }

  #[test]
  fn service_constants_agree() {
    let hex: String = SERVICE_STR.chars().filter(|c| *c != '-').collect();
    assert_eq!(u128::from_str_radix(&hex, 16).unwrap(), SERVICE);
  }
}

//! The frames around the WPA3 exchange (F-HOTSPOT-04), as bytes and nothing else: the SAE authentication frame (algorithm 3) with its
//! commit (sequence 1) and confirm (sequence 2) bodies for group 19 (NIST P-256), the anti-clogging token and the status codes, and
//! the 802.11w pieces of the connection: the SA Query action frames and the integrity group key item (IGTK) of the key data.
//! The curve arithmetic and the key derivation of the exchange itself are NOT here: a card that runs the exchange (told by
//! `EXT_SAE_OFFLOAD_AP`) does that, and this module only lets the access point read and answer what is around it. Bytes from the air
//! are never trusted: every length is checked and a bad frame is `None`, never a panic.
// SPDX-License-Identifier: Apache-2.0

use crate::mgmt::{header, parse_header, HEADER_LEN, SUBTYPE_AUTH};
use crate::Mac;

/// The authentication algorithm number of SAE.
pub const ALGO_SAE: u16 = 3;
pub const SEQ_COMMIT: u16 = 1;
pub const SEQ_CONFIRM: u16 = 2;
/// The only finite cyclic group offered: NIST P-256 (IANA group 19).
pub const GROUP_P256: u16 = 19;
pub const STATUS_SAE_ANTI_CLOGGING: u16 = 76;
pub const STATUS_SAE_UNSUPPORTED_GROUP: u16 = 77;
pub const STATUS_SAE_HASH_TO_ELEMENT: u16 = 126;
/// Bytes of the scalar and of the two coordinates of the element for group 19.
pub const SCALAR_LEN: usize = 32;
pub const ELEMENT_LEN: usize = 64;
/// The confirm value is a 256-bit hash output.
pub const CONFIRM_LEN: usize = 32;
/// The most bytes of an anti-clogging token the access point accepts back.
pub const TOKEN_MAX: usize = 64;

/// What an SAE authentication frame body holds (after the 24-byte header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaeBody {
    /// Sequence 1: the group, an optional anti-clogging token, the scalar and the element.
    Commit { group: u16, token: Vec<u8>, scalar: [u8; SCALAR_LEN], element: [u8; ELEMENT_LEN] },
    /// Sequence 2: the send-confirm counter and the confirm value.
    Confirm { send_confirm: u16, confirm: [u8; CONFIRM_LEN] },
    /// A commit refused with the anti-clogging request (status 76): the group and the token the station has to send back.
    TokenRequest { group: u16, token: Vec<u8> },
    /// A commit refused for its group (status 77): the group numbers the station may use instead are not listed here.
    Rejected { status: u16 },
}

/// One SAE authentication frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaeFrame {
    pub from: Mac,
    pub to: Mac,
    pub bssid: Mac,
    pub status: u16,
    pub body: SaeBody,
}

fn le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// Reads an SAE authentication frame addressed to `bssid`. A frame of another algorithm, a different group, a wrong length or a
/// token larger than [`TOKEN_MAX`] is `None`.
pub fn parse_sae(f: &[u8], bssid: Mac) -> Option<SaeFrame> {
    let h = parse_header(f)?;
    if h.fc0 != SUBTYPE_AUTH || h.bssid != bssid || h.da != bssid {
        return None;
    }
    let b = f.get(HEADER_LEN..)?;
    if le(b, 0)? != ALGO_SAE {
        return None;
    }
    let (seq, status) = (le(b, 2)?, le(b, 4)?);
    let rest = b.get(6..)?;
    let body = match (seq, status) {
        (SEQ_COMMIT, 0) | (SEQ_COMMIT, STATUS_SAE_HASH_TO_ELEMENT) => {
            let group = le(rest, 0)?;
            if group != GROUP_P256 {
                return None;
            }
            let data = rest.get(2..)?;
            if data.len() < SCALAR_LEN + ELEMENT_LEN || data.len() > TOKEN_MAX + SCALAR_LEN + ELEMENT_LEN {
                return None;
            }
            let tlen = data.len() - SCALAR_LEN - ELEMENT_LEN;
            SaeBody::Commit { group, token: data[..tlen].to_vec(), scalar: data[tlen..tlen + SCALAR_LEN].try_into().ok()?, element: data[tlen + SCALAR_LEN..].try_into().ok()? }
        }
        (SEQ_COMMIT, STATUS_SAE_ANTI_CLOGGING) => {
            let group = le(rest, 0)?;
            let token = rest.get(2..)?;
            if token.is_empty() || token.len() > TOKEN_MAX {
                return None;
            }
            SaeBody::TokenRequest { group, token: token.to_vec() }
        }
        (SEQ_COMMIT, STATUS_SAE_UNSUPPORTED_GROUP) => SaeBody::Rejected { status },
        (SEQ_CONFIRM, 0) => {
            if rest.len() != 2 + CONFIRM_LEN {
                return None;
            }
            SaeBody::Confirm { send_confirm: le(rest, 0)?, confirm: rest[2..].try_into().ok()? }
        }
        _ => return None,
    };
    Some(SaeFrame { from: h.sa, to: h.da, bssid: h.bssid, status, body })
}

/// Writes an SAE authentication frame from the access point to a station.
pub fn encode_sae(sta: Mac, bssid: Mac, status: u16, body: &SaeBody) -> Vec<u8> {
    let mut f = header(SUBTYPE_AUTH, sta, bssid, bssid);
    f.extend_from_slice(&ALGO_SAE.to_le_bytes());
    let (seq, status) = match body {
        SaeBody::Commit { .. } => (SEQ_COMMIT, status),
        SaeBody::Confirm { .. } => (SEQ_CONFIRM, status),
        SaeBody::TokenRequest { .. } => (SEQ_COMMIT, STATUS_SAE_ANTI_CLOGGING),
        SaeBody::Rejected { status } => (SEQ_COMMIT, *status),
    };
    f.extend_from_slice(&seq.to_le_bytes());
    f.extend_from_slice(&status.to_le_bytes());
    match body {
        SaeBody::Commit { group, token, scalar, element } => {
            f.extend_from_slice(&group.to_le_bytes());
            f.extend_from_slice(token);
            f.extend_from_slice(scalar);
            f.extend_from_slice(element);
        }
        SaeBody::Confirm { send_confirm, confirm } => {
            f.extend_from_slice(&send_confirm.to_le_bytes());
            f.extend_from_slice(confirm);
        }
        SaeBody::TokenRequest { group, token } => {
            f.extend_from_slice(&group.to_le_bytes());
            f.extend_from_slice(token);
        }
        SaeBody::Rejected { .. } => {}
    }
    f
}

/// The scalar of a commit has to be in 2 ..= order-1 and the element's coordinates below the prime; the full check (the point is on
/// the curve) needs the field arithmetic and is the exchange's job. This refuses what can be refused from the bytes alone: a scalar
/// of 0 or 1, one not below the group order, and coordinates that are not below the field prime.
pub fn commit_bytes_plausible(scalar: &[u8; SCALAR_LEN], element: &[u8; ELEMENT_LEN]) -> bool {
    // the order and the prime of P-256, big endian
    const ORDER: [u8; 32] = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51];
    const PRIME: [u8; 32] = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let zero_or_one = scalar[..31].iter().all(|b| *b == 0) && scalar[31] <= 1;
    !zero_or_one && scalar[..] < ORDER[..] && element[..32] < PRIME[..] && element[32..] < PRIME[..]
}

// ------------------------------------------------------------------------------------------------ 802.11w

pub const CATEGORY_SA_QUERY: u8 = 8;
pub const SA_QUERY_REQUEST: u8 = 0;
pub const SA_QUERY_RESPONSE: u8 = 1;
const SUBTYPE_ACTION: u8 = 0xd0;

/// An SA Query action frame: the request an access point sends before it believes an association request from a station it still
/// holds (or a station sends to check the access point), and the response that echoes the 16-bit transaction identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaQuery {
    pub from: Mac,
    pub to: Mac,
    pub response: bool,
    pub transaction: u16,
}

/// Writes an SA Query frame (the frame has to be sent protected once the pairwise key is installed; protecting it is the card's job).
pub fn encode_sa_query(to: Mac, bssid: Mac, response: bool, transaction: u16) -> Vec<u8> {
    let mut f = header(SUBTYPE_ACTION, to, bssid, bssid);
    f.push(CATEGORY_SA_QUERY);
    f.push(if response { SA_QUERY_RESPONSE } else { SA_QUERY_REQUEST });
    f.extend_from_slice(&transaction.to_le_bytes());
    f
}

pub fn parse_sa_query(f: &[u8], bssid: Mac) -> Option<SaQuery> {
    let h = parse_header(f)?;
    if h.fc0 != SUBTYPE_ACTION || h.bssid != bssid {
        return None;
    }
    let b = f.get(HEADER_LEN..)?;
    if b.len() != 4 || b[0] != CATEGORY_SA_QUERY || b[1] > SA_QUERY_RESPONSE {
        return None;
    }
    Some(SaQuery { from: h.sa, to: h.da, response: b[1] == SA_QUERY_RESPONSE, transaction: u16::from_le_bytes([b[2], b[3]]) })
}

/// A station's request is answered when it echoes our transaction number; a response with another number does not count.
pub fn sa_query_answers(request_transaction: u16, r: &SaQuery) -> bool {
    r.response && r.transaction == request_transaction
}

/// The integrity group key item of the key data (the key that protects broadcast management frames): dd len 00-0f-ac 09 | key id (2,
/// little endian) | packet number (6) | key (16 for BIP-CMAC-128).
pub fn igtk_kde(key_id: u16, ipn: [u8; 6], igtk: &[u8; 16]) -> Vec<u8> {
    let mut v = vec![0xdd, 4 + 2 + 6 + 16, 0x00, 0x0f, 0xac, 9];
    v.extend_from_slice(&key_id.to_le_bytes());
    v.extend_from_slice(&ipn);
    v.extend_from_slice(igtk);
    v
}

/// Reads an IGTK item back: (key id, packet number, key). The key ids of an IGTK are 4 and 5.
pub fn parse_igtk_kde(item: &[u8]) -> Option<(u16, [u8; 6], [u8; 16])> {
    if item.len() != 2 + 4 + 2 + 6 + 16 || item[0] != 0xdd || item[1] as usize != item.len() - 2 || item[2..5] != [0x00, 0x0f, 0xac] || item[5] != 9 {
        return None;
    }
    let id = u16::from_le_bytes([item[6], item[7]]);
    if !(4..=5).contains(&id) {
        return None;
    }
    Some((id, item[8..14].try_into().ok()?, item[14..30].try_into().ok()?))
}

#[cfg(test)]
#[path = "sae_tests.rs"]
mod tests;

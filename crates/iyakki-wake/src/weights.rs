//! The weights file: a versioned header, the numbers, an integrity checksum.
//!
//! ```text
//! "IYWK" | u32 version (1) | u32 n_mel | u32 window_frames | u32 param_count | u32 data_kind | f32 threshold
//!        | u64 seed | u32 epochs | u32 notes_len | notes (UTF-8, at most 512 bytes) | param_count x f32 LE | u64 FNV-1a of all before
//! ```
//! `data_kind`: 0 = synthetic speech only, 1 = includes real recordings. The runtime says which in its status line. The checksum catches
//! a damaged or half-written file; it is not a signature (the file is local data).

use crate::features::{N_MEL, WINDOW_FRAMES};
use crate::model::Net;

pub const MAGIC: &[u8; 4] = b"IYWK";
pub const VERSION: u32 = 1;
pub const DATA_SYNTHETIC: u32 = 0;
pub const DATA_REAL_INCLUDED: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub data_kind: u32,
    /// The suggested threshold from the validation sweep (the person's own setting wins).
    pub threshold: f32,
    pub seed: u64,
    pub epochs: u32,
    pub notes: String,
}

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

pub fn save_bytes(net: &Net, h: &Header) -> Vec<u8> {
    let notes: String = h.notes.chars().take(400).collect();
    let mut v = Vec::new();
    v.extend_from_slice(MAGIC);
    for x in [VERSION, N_MEL as u32, WINDOW_FRAMES as u32, net.params.len() as u32, h.data_kind] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(&h.threshold.to_le_bytes());
    v.extend_from_slice(&h.seed.to_le_bytes());
    v.extend_from_slice(&h.epochs.to_le_bytes());
    v.extend_from_slice(&(notes.len() as u32).to_le_bytes());
    v.extend_from_slice(notes.as_bytes());
    for p in &net.params {
        v.extend_from_slice(&p.to_le_bytes());
    }
    let sum = fnv1a(&v);
    v.extend_from_slice(&sum.to_le_bytes());
    v
}

pub fn load_bytes(b: &[u8]) -> Result<(Net, Header), String> {
    if b.len() < 52 || &b[0..4] != MAGIC {
        return Err("not an IYAKKI wake weights file".into());
    }
    let (body, tail) = b.split_at(b.len() - 8);
    if fnv1a(body) != u64::from_le_bytes(tail.try_into().unwrap()) {
        return Err("the weights file is damaged (checksum does not match)".into());
    }
    let u32at = |o: usize| u32::from_le_bytes(body[o..o + 4].try_into().unwrap());
    if u32at(4) != VERSION {
        return Err(format!("weights file version {} is not supported (this build reads version {VERSION})", u32at(4)));
    }
    if u32at(8) as usize != N_MEL || u32at(12) as usize != WINDOW_FRAMES {
        return Err("the weights were made for different features".into());
    }
    let n = u32at(16) as usize;
    let data_kind = u32at(20);
    let threshold = f32::from_le_bytes(body[24..28].try_into().unwrap());
    let seed = u64::from_le_bytes(body[28..36].try_into().unwrap());
    let epochs = u32at(36);
    let nl = u32at(40) as usize;
    if nl > 512 || 44 + nl + n * 4 != body.len() {
        return Err("the weights file has the wrong size".into());
    }
    let notes = String::from_utf8(body[44..44 + nl].to_vec()).map_err(|_| "notes are not text")?;
    let params = body[44 + nl..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    let net = Net::from_params(params)?;
    Ok((net, Header { data_kind, threshold, seed, epochs, notes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header { data_kind: DATA_SYNTHETIC, threshold: 0.8, seed: 42, epochs: 3, notes: "test".into() }
    }

    #[test]
    fn save_then_load_gives_the_same_network() {
        let net = Net::random(1);
        let b = save_bytes(&net, &header());
        let (n2, h2) = load_bytes(&b).unwrap();
        assert_eq!(n2.params, net.params);
        assert_eq!(h2, header());
        assert!(b.len() < 70 * 1024);
    }

    #[test]
    fn damage_version_and_shape_are_refused() {
        let net = Net::random(1);
        let b = save_bytes(&net, &header());
        let mut c = b.clone();
        c[100] ^= 1;
        assert!(load_bytes(&c).unwrap_err().contains("damaged"));
        assert!(load_bytes(&b[..b.len() - 20]).is_err());
        assert!(load_bytes(b"IYWK").is_err());
        let mut d = b.clone();
        d[4] = 9;
        let n = d.len();
        let sum = fnv1a(&d[..n - 8]).to_le_bytes();
        d[n - 8..].copy_from_slice(&sum);
        assert!(load_bytes(&d).unwrap_err().contains("version"));
    }

    #[test]
    fn the_checksum_is_the_known_fnv1a() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}

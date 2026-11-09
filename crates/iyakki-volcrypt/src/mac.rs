//! HMAC-SHA256 (RFC 2104) and a constant-time comparison, on the `sha2` crate's hash.

use sha2::{Digest, Sha256};

/// HMAC-SHA256 of the pieces of `data` joined together.
pub fn hmac_sha256(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for d in data {
        inner.update(d);
    }
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(ih);
    let out = outer.finalize().into();
    zeroize::Zeroize::zeroize(&mut k);
    out
}

/// Equal contents, taking the same time whatever the contents are (the lengths are public).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    std::hint::black_box(d) == 0
}

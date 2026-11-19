//! AES key wrap (RFC 3394, NIST SP 800-38F "KW"): wraps a key under a key-encryption key and notices a wrong key-encryption key by
//! the 64-bit integrity value, so a wrong password costs nothing more than the failed unwrap.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes256};
use zeroize::Zeroizing;

use crate::mac::ct_eq;

const IV: [u8; 8] = [0xA6; 8];

fn wrap_with<C: BlockEncrypt + KeyInit>(kek: &[u8], plain: &[u8]) -> Option<Vec<u8>> {
    if plain.len() < 16 || plain.len() % 8 != 0 {
        return None;
    }
    let c = C::new_from_slice(kek).ok()?;
    let n = plain.len() / 8;
    let mut a = IV;
    let mut r: Vec<[u8; 8]> = plain.chunks(8).map(|x| x.try_into().unwrap()).collect();
    for j in 0..6u64 {
        for (i, ri) in r.iter_mut().enumerate() {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(ri);
            let mut blk = GenericArray::clone_from_slice(&b);
            c.encrypt_block(&mut blk);
            let t = (n as u64) * j + i as u64 + 1;
            a.copy_from_slice(&blk[..8]);
            for (x, y) in a.iter_mut().zip(t.to_be_bytes()) {
                *x ^= y;
            }
            ri.copy_from_slice(&blk[8..]);
        }
    }
    let mut out = a.to_vec();
    for ri in &r {
        out.extend_from_slice(ri);
    }
    Some(out)
}

fn unwrap_with<C: BlockDecrypt + KeyInit>(kek: &[u8], wrapped: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if wrapped.len() < 24 || wrapped.len() % 8 != 0 {
        return None;
    }
    let c = C::new_from_slice(kek).ok()?;
    let n = wrapped.len() / 8 - 1;
    let mut a: [u8; 8] = wrapped[..8].try_into().unwrap();
    let mut r: Vec<[u8; 8]> = wrapped[8..].chunks(8).map(|x| x.try_into().unwrap()).collect();
    for j in (0..6u64).rev() {
        for i in (0..n).rev() {
            let t = (n as u64) * j + i as u64 + 1;
            let mut b = [0u8; 16];
            for (k, (x, y)) in a.iter().zip(t.to_be_bytes()).enumerate() {
                b[k] = x ^ y;
            }
            b[8..].copy_from_slice(&r[i]);
            let mut blk = GenericArray::clone_from_slice(&b);
            c.decrypt_block(&mut blk);
            a.copy_from_slice(&blk[..8]);
            r[i].copy_from_slice(&blk[8..]);
        }
    }
    let ok = ct_eq(&a, &IV);
    let mut out = Zeroizing::new(Vec::with_capacity(n * 8));
    for ri in r.iter_mut() {
        out.extend_from_slice(ri);
        ri.fill(0);
    }
    if ok {
        Some(out)
    } else {
        None
    }
}

/// Wrap `plain` (a multiple of 8 bytes, at least 16) under a 256-bit key; the result is 8 bytes longer.
pub fn wrap256(kek: &[u8; 32], plain: &[u8]) -> Option<Vec<u8>> {
    wrap_with::<Aes256>(kek, plain)
}

/// Unwrap; `None` for a wrong key or damaged data (the two cannot be told apart, on purpose).
pub fn unwrap256(kek: &[u8; 32], wrapped: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    unwrap_with::<Aes256>(kek, wrapped)
}

/// 128-bit variants, used only to check the implementation against the 128-bit RFC 3394 vector.
pub fn wrap128(kek: &[u8; 16], plain: &[u8]) -> Option<Vec<u8>> {
    wrap_with::<Aes128>(kek, plain)
}

pub fn unwrap128(kek: &[u8; 16], wrapped: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    unwrap_with::<Aes128>(kek, wrapped)
}

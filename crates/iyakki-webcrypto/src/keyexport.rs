//! The Matrix room key export file (client-server specification, "Key exports"): a passphrase-protected container for the
//! list of room keys a device holds, so a person can move them to another device or keep a copy.
//!
//! Layout: version byte 1, salt (16), iv (16), rounds (u32 big endian), AES-256-CTR cipher text, HMAC-SHA-256 of everything
//! before it. The two keys come from PBKDF2-HMAC-SHA-512 over the passphrase (64 bytes: the first half encrypts, the second
//! half authenticates). The bytes travel as base64 between two armour lines. Nothing here reads or writes a file.

use crate::{b64, ct_eq, kdf::hmac_sha256, random};
use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use ring::pbkdf2;
use std::num::NonZeroU32;

const BEGIN: &str = "-----BEGIN MEGOLM SESSION DATA-----";
const END: &str = "-----END MEGOLM SESSION DATA-----";
/// What a new export uses; the specification suggests at least 100 000.
pub const DEFAULT_ROUNDS: u32 = 500_000;
/// The most rounds a file may ask for (a hostile file must not make the device spin for minutes).
pub const MAX_ROUNDS: u32 = 5_000_000;
/// The biggest export read (16 MiB of container).
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

fn keys(pass: &str, salt: &[u8], rounds: u32) -> Result<([u8; 32], [u8; 32]), String> {
    let n = NonZeroU32::new(rounds).ok_or("the export has no rounds")?;
    let mut out = [0u8; 64];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA512, n, salt, pass.as_bytes(), &mut out);
    let mut a = [0u8; 32];
    let mut m = [0u8; 32];
    a.copy_from_slice(&out[..32]);
    m.copy_from_slice(&out[32..]);
    Ok((a, m))
}

/// AES-256-CTR with a 128-bit big endian counter (the same call encrypts and decrypts).
fn ctr(key: &[u8; 32], iv: &[u8; 16], data: &mut [u8]) {
    let c = aes::Aes256::new(GenericArray::from_slice(key));
    let mut counter = u128::from_be_bytes(*iv);
    for chunk in data.chunks_mut(16) {
        let mut block = counter.to_be_bytes();
        c.encrypt_block(GenericArray::from_mut_slice(&mut block));
        for (d, k) in chunk.iter_mut().zip(block) {
            *d ^= k;
        }
        counter = counter.wrapping_add(1);
    }
}

fn seal_with(pass: &str, plain: &[u8], salt: [u8; 16], mut iv: [u8; 16], rounds: u32) -> Result<Vec<u8>, String> {
    iv[8] &= 0x7f; // bit 63 clear so the counter never carries into the nonce half
    let (ak, mk) = keys(pass, &salt, rounds)?;
    let mut ct = plain.to_vec();
    ctr(&ak, &iv, &mut ct);
    let mut out = Vec::with_capacity(1 + 16 + 16 + 4 + ct.len() + 32);
    out.push(1);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&rounds.to_be_bytes());
    out.extend_from_slice(&ct);
    let mac = hmac_sha256(&mk, &out);
    out.extend_from_slice(&mac);
    Ok(out)
}

/// The container bytes for `plain` (normally the JSON list of room keys).
pub fn seal(pass: &str, plain: &[u8], rounds: u32) -> Result<Vec<u8>, String> {
    if pass.is_empty() {
        return Err("a key export needs a passphrase".into());
    }
    if rounds == 0 || rounds > MAX_ROUNDS {
        return Err("the number of rounds is out of range".into());
    }
    let mut salt = [0u8; 16];
    let mut iv = [0u8; 16];
    random(&mut salt)?;
    random(&mut iv)?;
    seal_with(pass, plain, salt, iv, rounds)
}

/// Opens container bytes; a wrong passphrase and a damaged file both say only that the file did not open.
pub fn open(pass: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() > MAX_BYTES {
        return Err("the export is too big".into());
    }
    if data.len() < 1 + 16 + 16 + 4 + 32 || data[0] != 1 {
        return Err("this is not a key export of a version this device reads".into());
    }
    let (body, mac) = data.split_at(data.len() - 32);
    let rounds = u32::from_be_bytes([body[33], body[34], body[35], body[36]]);
    if rounds == 0 || rounds > MAX_ROUNDS {
        return Err("the export asks for an unreasonable number of rounds".into());
    }
    let (ak, mk) = keys(pass, &body[1..17], rounds)?;
    if !ct_eq(&hmac_sha256(&mk, body), mac) {
        return Err("the export did not open: wrong passphrase or a damaged file".into());
    }
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&body[17..33]);
    let mut plain = body[37..].to_vec();
    ctr(&ak, &iv, &mut plain);
    Ok(plain)
}

/// The text of an export file: the container as base64 in lines of 96 characters between the armour lines.
pub fn armor(container: &[u8]) -> String {
    let b = b64::std(container);
    let mut s = String::from(BEGIN);
    s.push('\n');
    let mut rest = b.as_str();
    while !rest.is_empty() {
        let (line, tail) = rest.split_at(rest.len().min(96));
        s.push_str(line);
        s.push('\n');
        rest = tail;
    }
    s.push_str(END);
    s.push('\n');
    s
}

/// The container inside an export file's text.
pub fn unarmor(text: &str) -> Result<Vec<u8>, String> {
    let t = text.trim();
    let inner = t.strip_prefix(BEGIN).and_then(|x| x.trim_end().strip_suffix(END)).ok_or("the file has no key export armour")?;
    let joined: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    b64::decode(&joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_sha512_matches_a_published_vector() {
        // RFC 6070 style vector for HMAC-SHA-512 (password / salt, 1 round, first 32 bytes of a 64 byte output).
        let mut out = [0u8; 64];
        pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA512, NonZeroU32::new(1).unwrap(), b"salt", b"password", &mut out);
        let hex: String = out.iter().take(16).map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "867f70cf1ade02cff3752599a3a53dc4");
    }

    #[test]
    fn ctr_matches_nist_sp800_38a_f_5_5_and_decrypts_itself() {
        let key: [u8; 32] = hex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4").try_into().unwrap();
        let iv: [u8; 16] = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff").try_into().unwrap();
        let mut d = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        ctr(&key, &iv, &mut d);
        assert_eq!(d, hex("601ec313775789a5b7a7f504bbf3d228f443e3ca4d62b59aca84e990cacaf5c5"));
        ctr(&key, &iv, &mut d);
        assert_eq!(d, hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51"));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn an_export_comes_back_with_the_passphrase_and_only_then() {
        let plain = br#"[{"algorithm":"m.megolm.v1.aes-sha2","room_id":"!r:x"}]"#.to_vec();
        let c = seal("correct horse", &plain, 1000).unwrap();
        assert_eq!(c[0], 1);
        assert_eq!(open("correct horse", &c).unwrap(), plain);
        assert!(open("wrong", &c).is_err());
        let text = armor(&c);
        assert!(text.starts_with(BEGIN) && text.trim_end().ends_with(END));
        assert_eq!(open("correct horse", &unarmor(&text).unwrap()).unwrap(), plain);
    }

    #[test]
    fn damage_short_files_odd_versions_and_greedy_rounds_are_refused() {
        let c = seal("p", b"[]", 10).unwrap();
        for i in [0usize, 5, 20, 33, 37, c.len() - 1] {
            let mut bad = c.clone();
            bad[i] ^= 1;
            assert!(open("p", &bad).is_err(), "byte {i}");
        }
        assert!(open("p", &c[..40]).is_err());
        let mut greedy = c.clone();
        greedy[33..37].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(open("p", &greedy).unwrap_err().contains("rounds"));
        assert!(seal("", b"x", 10).is_err());
        assert!(seal("p", b"x", 0).is_err());
        assert!(unarmor("no armour").is_err());
    }

    #[test]
    fn the_counter_bit_is_cleared_so_a_long_export_never_carries_into_the_nonce() {
        let c = seal_with("p", &[7u8; 64], [1; 16], [0xff; 16], 2).unwrap();
        assert_eq!(c[17 + 8] & 0x80, 0);
        assert_eq!(open("p", &c).unwrap(), vec![7u8; 64]);
    }
}

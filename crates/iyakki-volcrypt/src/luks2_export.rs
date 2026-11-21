//! Writing a LUKS2 header (F-STORE-05, "LUKS2 export"): the person's encrypted-home volume key and password become a standard LUKS2
//! header (two 16 KiB copies, one Argon2id key slot with the anti-forensic split, a PBKDF2-SHA256 digest of the volume key) that
//! `cryptsetup` and this crate's own [`crate::luks2`] reader both open. Pure code: the caller supplies the random bytes and gets the
//! whole 16 MiB header area back (to be written ahead of the data); nothing here touches a drive. The volume key and the password are
//! never stored or printed.
// SPDX-License-Identifier: Apache-2.0

use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use crate::luks1::{af_split, pbkdf2, LuksError, LuksHash};
use crate::luks2::BINARY_HEADER_LEN;
use crate::xts::Xts;
use sha2::{Digest, Sha256};

/// One copy of the header: the binary part and the JSON area.
pub const HEADER_COPY: usize = 16 * 1024;
/// Where the key slot area starts (after both copies) and how big it is (the 4000 stripes of a 64-byte key, 4096-aligned).
const SLOT_OFFSET: u64 = 2 * HEADER_COPY as u64;
const SLOT_AREA: u64 = 258_048;
/// The payload starts here, as `cryptsetup` makes it by default.
pub const DATA_OFFSET: u64 = 16 * 1024 * 1024;
const STRIPES: usize = 4000;

/// The costs of the key slot's Argon2id and the digest's PBKDF2.
#[derive(Clone, Copy, Debug)]
pub struct Costs {
    pub time: u32,
    pub memory_kib: u32,
    pub cpus: u32,
    pub digest_iterations: u32,
}

impl Costs {
    /// The cheap costs the tests use; real exports take the calibrated costs of the encrypted home.
    pub const fn light() -> Costs {
        Costs { time: 1, memory_kib: 64, cpus: 1, digest_iterations: 1000 }
    }
}

fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn is_uuid(u: &str) -> bool {
    u.len() == 36 && u.bytes().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() })
}

/// The whole header area (`DATA_OFFSET` bytes) for `volume_key` (64 bytes, AES-256-XTS) opened by `password`. `uuid` is the textual
/// volume UUID; `label` is at most 47 bytes of printable ASCII or empty. `random` fills salts and the split.
pub fn export_header(volume_key: &[u8; 64], password: &[u8], uuid: &str, label: &str, costs: Costs, random: &mut dyn FnMut(&mut [u8])) -> Result<Vec<u8>, LuksError> {
    export_header_sized(volume_key, password, uuid, label, 512, costs, random)
}

/// Like [`export_header`] for a volume whose data unit (dm-crypt `sector_size`) is 512 or 4096 bytes: the same payload bytes then open
/// unchanged, because the segment says the same unit the volume was written with.
pub fn export_header_sized(volume_key: &[u8; 64], password: &[u8], uuid: &str, label: &str, sector_size: u32, costs: Costs, random: &mut dyn FnMut(&mut [u8])) -> Result<Vec<u8>, LuksError> {
    if sector_size != 512 && sector_size != 4096 {
        return Err(LuksError::Invalid("sector size"));
    }
    if !is_uuid(uuid) {
        return Err(LuksError::Invalid("uuid"));
    }
    if label.len() > 47 || !label.bytes().all(|c| c.is_ascii_graphic() || c == b' ') {
        return Err(LuksError::Invalid("label"));
    }
    if password.is_empty() {
        return Err(LuksError::Invalid("password"));
    }
    // the key slot: derive the slot key, split the volume key, encrypt the split
    let mut kdf_salt = [0u8; 32];
    random(&mut kdf_salt);
    let params = Params::new(costs.memory_kib, costs.time, costs.cpus, Some(64)).map_err(|_| LuksError::Invalid("argon2 costs"))?;
    let mut slot_key = Zeroizing::new([0u8; 64]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params).hash_password_into(password, &kdf_salt, &mut *slot_key).map_err(|_| LuksError::Invalid("argon2 costs"))?;
    let split = Zeroizing::new(af_split(LuksHash::Sha256, volume_key, STRIPES, random));
    let mut area = Zeroizing::new(vec![0u8; SLOT_AREA as usize]);
    area[..split.len()].copy_from_slice(&split);
    let xts = Xts::new(&slot_key);
    for (i, sec) in area.chunks_mut(512).enumerate() {
        if !xts.encrypt_sector(i as u64, sec) {
            return Err(LuksError::Invalid("key material size"));
        }
    }
    // the digest of the volume key
    let mut digest_salt = [0u8; 32];
    random(&mut digest_salt);
    let digest = pbkdf2(LuksHash::Sha256, volume_key, &digest_salt, costs.digest_iterations, 32);
    let json = serde_json::json!({
        "keyslots": {"0": {
            "type": "luks2", "key_size": 64,
            "af": {"type": "luks1", "stripes": STRIPES, "hash": "sha256"},
            "area": {"type": "raw", "offset": SLOT_OFFSET.to_string(), "size": SLOT_AREA.to_string(), "encryption": "aes-xts-plain64", "key_size": 64},
            "kdf": {"type": "argon2id", "time": costs.time, "memory": costs.memory_kib, "cpus": costs.cpus, "salt": b64(&kdf_salt)}
        }},
        "tokens": {},
        "segments": {"0": {"type": "crypt", "offset": DATA_OFFSET.to_string(), "iv_tweak": "0", "size": "dynamic", "encryption": "aes-xts-plain64", "sector_size": sector_size}},
        "digests": {"0": {"type": "pbkdf2", "keyslots": ["0"], "segments": ["0"], "hash": "sha256", "iterations": costs.digest_iterations, "salt": b64(&digest_salt), "digest": b64(&digest)}},
        "config": {"json_size": (HEADER_COPY - BINARY_HEADER_LEN).to_string(), "keyslots_size": (DATA_OFFSET - SLOT_OFFSET).to_string()}
    });
    let json = serde_json::to_vec(&json).map_err(|_| LuksError::Invalid("the JSON area"))?;
    if json.len() >= HEADER_COPY - BINARY_HEADER_LEN {
        return Err(LuksError::Invalid("the JSON area"));
    }
    let mut header_salt = [0u8; 64];
    random(&mut header_salt);
    let mut out = vec![0u8; DATA_OFFSET as usize];
    for (n, magic) in [b"LUKS\xba\xbe", b"SKUL\xba\xbe"].into_iter().enumerate() {
        let at = n * HEADER_COPY;
        let h = &mut out[at..at + HEADER_COPY];
        h[..6].copy_from_slice(magic);
        h[6..8].copy_from_slice(&2u16.to_be_bytes());
        h[8..16].copy_from_slice(&(HEADER_COPY as u64).to_be_bytes());
        h[16..24].copy_from_slice(&1u64.to_be_bytes());
        h[24..24 + label.len()].copy_from_slice(label.as_bytes());
        h[72..78].copy_from_slice(b"sha256");
        h[104..168].copy_from_slice(&header_salt);
        h[168..168 + uuid.len()].copy_from_slice(uuid.as_bytes());
        h[256..264].copy_from_slice(&(at as u64).to_be_bytes());
        h[BINARY_HEADER_LEN..BINARY_HEADER_LEN + json.len()].copy_from_slice(&json);
        let sum = Sha256::digest(&*h);
        h[448..480].copy_from_slice(&sum);
    }
    out[SLOT_OFFSET as usize..SLOT_OFFSET as usize + area.len()].copy_from_slice(&area);
    Ok(out)
}

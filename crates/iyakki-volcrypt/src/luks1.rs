//! Opening a LUKS1 volume with our own code (F-DISKS-02): the 592-byte header, the key slots, PBKDF2 with SHA-1, SHA-256 or SHA-512,
//! the anti-forensic merge, AES-XTS (`aes-xts-plain64`) and the master-key check. Pure code: the caller reads the header and the key
//! material from the drive and hands the bytes over; nothing here opens a device. The result is the master key, which a later step hands
//! to device-mapper (the same way `iyakki-storage::dm` does for IYAKVOL1). LUKS2 (JSON header, Argon2) and CBC-ESSIV are not read here;
//! they are refused with a plain reason. The password is never stored or printed by this module.

use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use crate::mac::ct_eq;
use crate::xts::{Xts, Xts128};

/// The header is 592 bytes: 208 fixed, 8 key slots of 48.
pub const HEADER_LEN: usize = 592;
const LUKS1_SIGNATURE: &[u8; 6] = b"LUKS\xba\xbe";
const SLOT_ACTIVE: u32 = 0x00AC_71F3;
const SLOT_DISABLED: u32 = 0x0000_DEAD;
/// Refuse absurd iteration counts before spending minutes on them (a real drive uses 1000 to a few million).
const MAX_ITER: u32 = 100_000_000;
const MAX_STRIPES: u32 = 100_000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LuksError {
    #[error("this is not a LUKS1 header")]
    NotLuks1,
    #[error("this is not a LUKS2 header")]
    NotLuks2,
    #[error("the LUKS header is cut short")]
    Truncated,
    #[error("this LUKS volume uses {0}, which IYAKKI cannot open itself yet")]
    Unsupported(String),
    #[error("the LUKS header holds values that make no sense: {0}")]
    Invalid(&'static str),
    #[error("the password is wrong (or the drive is damaged)")]
    WrongPassword,
    #[error("the key material of a key slot could not be read")]
    Unreadable,
}

/// The hash a LUKS1 header names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuksHash {
    Sha1,
    Sha256,
    Sha512,
}

impl LuksHash {
    pub fn from_spec(s: &str) -> Result<LuksHash, LuksError> {
        match s {
            "sha1" => Ok(LuksHash::Sha1),
            "sha256" => Ok(LuksHash::Sha256),
            "sha512" => Ok(LuksHash::Sha512),
            other => Err(LuksError::Unsupported(format!("the hash {:?}", other.chars().filter(char::is_ascii_graphic).take(16).collect::<String>()))),
        }
    }
    pub fn out_len(self) -> usize {
        match self {
            LuksHash::Sha1 => 20,
            LuksHash::Sha256 => 32,
            LuksHash::Sha512 => 64,
        }
    }
    fn block_len(self) -> usize {
        if self == LuksHash::Sha512 { 128 } else { 64 }
    }
    /// The hash of the pieces joined together.
    pub fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        match self {
            LuksHash::Sha1 => {
                let mut s = Sha1::new();
                for p in parts {
                    s.update(p);
                }
                s.finish().to_vec()
            }
            LuksHash::Sha256 => {
                let mut h = Sha256::new();
                for p in parts {
                    h.update(p);
                }
                h.finalize().to_vec()
            }
            LuksHash::Sha512 => {
                let mut h = Sha512::new();
                for p in parts {
                    h.update(p);
                }
                h.finalize().to_vec()
            }
        }
    }
}

/// SHA-1 (FIPS 180-4), only for reading old LUKS1 headers; nothing new is ever protected with it.
pub struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Sha1 {
        Sha1 { h: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0], buf: Vec::new(), len: 0 }
    }
    pub fn update(&mut self, data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        self.buf.extend_from_slice(data);
        let full = self.buf.len() / 64 * 64;
        let blocks: Vec<u8> = self.buf.drain(..full).collect();
        for b in blocks.chunks_exact(64) {
            self.block(b);
        }
    }
    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 80];
        for (i, c) in b.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut bb, mut c, mut d, mut e] = self.h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((bb & c) | (!bb & d), 0x5A82_7999u32),
                20..=39 => (bb ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((bb & c) | (bb & d) | (c & d), 0x8F1B_BCDC),
                _ => (bb ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = bb.rotate_left(30);
            bb = a;
            a = t;
        }
        for (x, y) in self.h.iter_mut().zip([a, bb, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    pub fn finish(mut self) -> [u8; 20] {
        let bits = self.len.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.buf.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_be_bytes());
        let tail = std::mem::take(&mut self.buf);
        let mut all = tail;
        all.extend_from_slice(&pad);
        for b in all.chunks_exact(64) {
            self.block(b);
        }
        let mut out = [0u8; 20];
        for (i, x) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
        }
        out
    }
}

/// HMAC (RFC 2104) with any of the three hashes.
pub fn hmac(h: LuksHash, key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let bl = h.block_len();
    let mut k = vec![0u8; bl];
    if key.len() > bl {
        let d = h.digest(&[key]);
        k[..d.len()].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut inner_parts: Vec<&[u8]> = vec![&ipad];
    inner_parts.extend_from_slice(parts);
    let inner = h.digest(&inner_parts);
    let out = h.digest(&[&opad, &inner]);
    zeroize::Zeroize::zeroize(&mut k);
    out
}

/// PBKDF2 (RFC 8018) with HMAC of the chosen hash.
pub fn pbkdf2(h: LuksHash, password: &[u8], salt: &[u8], iterations: u32, out_len: usize) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(out_len));
    let mut block = 1u32;
    while out.len() < out_len {
        let mut u = hmac(h, password, &[salt, &block.to_be_bytes()]);
        let mut t = u.clone();
        for _ in 1..iterations.max(1) {
            u = hmac(h, password, &[&u]);
            for (a, b) in t.iter_mut().zip(&u) {
                *a ^= b;
            }
        }
        let take = (out_len - out.len()).min(t.len());
        out.extend_from_slice(&t[..take]);
        block += 1;
    }
    out
}

/// One key slot of the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub active: bool,
    pub iterations: u32,
    pub salt: [u8; 32],
    /// In 512-byte sectors from the start of the drive.
    pub material_sector: u32,
    pub stripes: u32,
}

/// The parts of a LUKS1 header that unlocking needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Luks1 {
    pub cipher: String,
    pub mode: String,
    pub hash: LuksHash,
    pub payload_sector: u32,
    pub key_bytes: u32,
    pub mk_digest: [u8; 20],
    pub mk_salt: [u8; 32],
    pub mk_iterations: u32,
    pub uuid: String,
    pub slots: Vec<Slot>,
}

fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn text(b: &[u8]) -> String {
    b.iter().copied().take_while(|c| *c != 0).filter(u8::is_ascii_graphic).take(40).map(|c| c as char).collect()
}

/// Read the 592 bytes. Hostile input: every length and offset is checked, nothing panics.
pub fn parse_header(b: &[u8]) -> Result<Luks1, LuksError> {
    if b.get(..6) != Some(&LUKS1_SIGNATURE[..]) {
        return Err(LuksError::NotLuks1);
    }
    if b.len() < HEADER_LEN {
        return Err(LuksError::Truncated);
    }
    if u16::from_be_bytes([b[6], b[7]]) != 1 {
        return Err(LuksError::NotLuks1);
    }
    let (cipher, mode, hash_spec) = (text(&b[8..40]), text(&b[40..72]), text(&b[72..104]));
    let hash = LuksHash::from_spec(&hash_spec)?;
    let key_bytes = be32(b, 108);
    if !matches!(key_bytes, 16 | 32 | 64) {
        return Err(LuksError::Invalid("key size"));
    }
    let mut mk_digest = [0u8; 20];
    mk_digest.copy_from_slice(&b[112..132]);
    let mut mk_salt = [0u8; 32];
    mk_salt.copy_from_slice(&b[132..164]);
    let mk_iterations = be32(b, 164);
    if mk_iterations == 0 || mk_iterations > MAX_ITER {
        return Err(LuksError::Invalid("master key check count"));
    }
    let mut slots = Vec::new();
    for i in 0..8 {
        let o = 208 + i * 48;
        let state = be32(b, o);
        if state != SLOT_ACTIVE && state != SLOT_DISABLED {
            return Err(LuksError::Invalid("key slot state"));
        }
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&b[o + 8..o + 40]);
        let s = Slot { active: state == SLOT_ACTIVE, iterations: be32(b, o + 4), salt, material_sector: be32(b, o + 40), stripes: be32(b, o + 44) };
        if s.active && (s.iterations == 0 || s.iterations > MAX_ITER || s.stripes == 0 || s.stripes > MAX_STRIPES) {
            return Err(LuksError::Invalid("key slot values"));
        }
        slots.push(s);
    }
    Ok(Luks1 { cipher, mode, hash, payload_sector: be32(b, 104), key_bytes, mk_digest, mk_salt, mk_iterations, uuid: text(&b[168..208]), slots })
}

impl Luks1 {
    /// Why this volume cannot be opened by our code, if it cannot.
    pub fn unsupported(&self) -> Option<String> {
        if self.cipher != "aes" || self.mode != "xts-plain64" {
            return Some(format!("{}-{}", self.cipher, self.mode));
        }
        None
    }

    /// Bytes of key material of one slot (stripes x key size, rounded up to whole sectors on the drive).
    pub fn material_len(&self, s: &Slot) -> usize {
        (s.stripes as usize) * (self.key_bytes as usize)
    }
}

/// Decrypt key material read from the drive: AES-XTS with the slot key, plain64 sectors counted from the start of the slot's area.
fn decrypt_material(slot_key: &[u8], data: &mut [u8]) -> Result<(), LuksError> {
    let ok = |done: bool| if done { Ok(()) } else { Err(LuksError::Invalid("key material size")) };
    match slot_key.len() {
        32 => {
            let mut k = Zeroizing::new([0u8; 32]);
            k.copy_from_slice(slot_key);
            let x = Xts128::new_128(&k);
            for (i, sec) in data.chunks_mut(512).enumerate() {
                ok(x.decrypt_sector(i as u64, sec))?;
            }
        }
        64 => {
            let mut k = Zeroizing::new([0u8; 64]);
            k.copy_from_slice(slot_key);
            let x = Xts::new(&k);
            for (i, sec) in data.chunks_mut(512).enumerate() {
                ok(x.decrypt_sector(i as u64, sec))?;
            }
        }
        _ => return Err(LuksError::Invalid("key size")),
    }
    Ok(())
}

/// The LUKS diffusion step over a buffer, one digest-sized piece at a time (each piece is hashed with its counter in front).
fn diffuse(h: LuksHash, d: &mut [u8]) {
    let dl = h.out_len();
    for (i, piece) in d.chunks_mut(dl).enumerate() {
        let out = h.digest(&[&(i as u32).to_be_bytes(), piece]);
        piece.copy_from_slice(&out[..piece.len()]);
    }
}

/// Anti-forensic merge (the LUKS1 specification, "AFmerge").
pub fn af_merge(h: LuksHash, material: &[u8], block: usize, stripes: usize) -> Option<Zeroizing<Vec<u8>>> {
    if block == 0 || stripes == 0 || material.len() < block * stripes {
        return None;
    }
    let mut d = Zeroizing::new(vec![0u8; block]);
    for s in 0..stripes - 1 {
        for (x, y) in d.iter_mut().zip(&material[s * block..(s + 1) * block]) {
            *x ^= y;
        }
        diffuse(h, &mut d);
    }
    for (x, y) in d.iter_mut().zip(&material[(stripes - 1) * block..stripes * block]) {
        *x ^= y;
    }
    Some(d)
}

/// Anti-forensic split (the writing side of the same specification); used by the tests to build volumes and kept for a later "add a key".
pub fn af_split(h: LuksHash, key: &[u8], stripes: usize, random: &mut dyn FnMut(&mut [u8])) -> Vec<u8> {
    let block = key.len();
    let mut out = vec![0u8; block * stripes];
    let mut d = vec![0u8; block];
    for s in 0..stripes - 1 {
        random(&mut out[s * block..(s + 1) * block]);
        for (x, y) in d.iter_mut().zip(&out[s * block..(s + 1) * block]) {
            *x ^= y;
        }
        diffuse(h, &mut d);
    }
    for i in 0..block {
        out[(stripes - 1) * block + i] = d[i] ^ key[i];
    }
    out
}

impl Luks1 {
    /// The master key, if `password` opens any active slot. `read_sectors(start, bytes)` returns that many bytes of the drive from the
    /// 512-byte sector `start` (the key material). Slots are tried in order; every active slot costs one PBKDF2 whatever the result.
    pub fn unlock(&self, password: &[u8], read_sectors: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, LuksError> {
        if let Some(what) = self.unsupported() {
            return Err(LuksError::Unsupported(what));
        }
        let kb = self.key_bytes as usize;
        for s in self.slots.iter().filter(|s| s.active) {
            let slot_key = pbkdf2(self.hash, password, &s.salt, s.iterations, kb);
            let len = self.material_len(s);
            let padded = len.div_ceil(512) * 512;
            let mut raw = read_sectors(u64::from(s.material_sector), padded).ok_or(LuksError::Unreadable)?;
            if raw.len() < padded {
                return Err(LuksError::Unreadable);
            }
            raw.truncate(padded);
            decrypt_material(&slot_key, &mut raw)?;
            let merged = af_merge(self.hash, &raw[..len], kb, s.stripes as usize).ok_or(LuksError::Invalid("key material size"))?;
            let check = pbkdf2(self.hash, &merged, &self.mk_salt, self.mk_iterations, 20);
            if ct_eq(&check, &self.mk_digest) {
                return Ok(merged);
            }
        }
        Err(LuksError::WrongPassword)
    }
}

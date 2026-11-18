//! Opening a LUKS2 volume with our own code (F-DISKS-02): the 4096-byte binary header and its checksum, the JSON area, the key slot's
//! key derivation (Argon2id, Argon2i or PBKDF2), the anti-forensic merge and AES-XTS shared with [`crate::luks1`], and the master-key
//! check against the digest. Pure code: the caller reads the header area and the key slot areas from the drive. The result is the master
//! key for device-mapper. Refused with a plain reason: other ciphers, sector-wrapped or non-`luks2` key slots, hashes other than SHA-1,
//! SHA-256 and SHA-512, memory or time costs above the limits below. The password is never stored or printed by this module.
// SPDX-License-Identifier: Apache-2.0

use argon2::{Algorithm, Argon2, Params, Version};
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use crate::luks1::{af_merge, pbkdf2, LuksError, LuksHash};
use crate::mac::ct_eq;
use crate::xts::{Xts, Xts128};

pub const BINARY_HEADER_LEN: usize = 4096;
const SIGNATURE_FIRST: &[u8; 6] = b"LUKS\xba\xbe";
const SIGNATURE_SECOND: &[u8; 6] = b"SKUL\xba\xbe";
/// The largest header (binary plus JSON) taken: cryptsetup writes 16 KiB to 4 MiB.
pub const MAX_HEADER: u64 = 4 * 1024 * 1024;
const MAX_ARGON_KIB: u64 = 4 * 1024 * 1024;
const MAX_ARGON_TIME: u64 = 1000;
const MAX_ARGON_CPUS: u64 = 64;
const MAX_ITER: u64 = 100_000_000;
const MAX_STRIPES: u64 = 100_000;
const MAX_AREA: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kdf {
    Argon { alg: Algorithm, time: u32, memory_kib: u32, cpus: u32, salt: Vec<u8> },
    Pbkdf2 { hash: LuksHash, iterations: u32, salt: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub id: String,
    pub key_size: usize,
    pub stripes: usize,
    pub af_hash: LuksHash,
    /// Where the encrypted key material sits on the drive, in bytes from its start.
    pub area_offset: u64,
    pub area_size: u64,
    pub kdf: Kdf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Digest2 {
    pub keyslots: Vec<String>,
    pub hash: LuksHash,
    pub iterations: u32,
    pub salt: Vec<u8>,
    pub digest: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub offset: u64,
    /// `None` for "dynamic" (the rest of the drive).
    pub size: Option<u64>,
    pub iv_tweak: u64,
    pub sector_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Luks2 {
    pub uuid: String,
    pub label: String,
    pub slots: Vec<Slot>,
    pub digests: Vec<Digest2>,
    pub segments: Vec<Segment>,
}

fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]))
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).chars().filter(|c| !c.is_control()).collect()
}

/// The header size (binary plus JSON) the first 4096 bytes announce, so the caller knows how much to read.
pub fn header_size(first4096: &[u8]) -> Result<u64, LuksError> {
    if first4096.len() < BINARY_HEADER_LEN {
        return Err(LuksError::Truncated);
    }
    if &first4096[..6] != SIGNATURE_FIRST && &first4096[..6] != SIGNATURE_SECOND {
        return Err(LuksError::NotLuks2);
    }
    if u16::from_be_bytes([first4096[6], first4096[7]]) != 2 {
        return Err(LuksError::NotLuks2);
    }
    let n = be64(first4096, 8);
    if !(BINARY_HEADER_LEN as u64 + 512..=MAX_HEADER).contains(&n) || n % 4096 != 0 {
        return Err(LuksError::Invalid("header size"));
    }
    Ok(n)
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let v = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s {
        acc = (acc << 6) | v(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn num(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}

fn parse_slot(id: &str, v: &Value) -> Result<Slot, LuksError> {
    if s(v, "type") != Some("luks2") {
        return Err(LuksError::Unsupported(format!("a key slot of type {}", s(v, "type").unwrap_or("?"))));
    }
    let key_size = num(&v["key_size"]).filter(|k| matches!(k, 32 | 64)).ok_or(LuksError::Invalid("key size"))? as usize;
    let af = &v["af"];
    if s(af, "type") != Some("luks1") {
        return Err(LuksError::Unsupported("an anti-forensic split other than luks1".into()));
    }
    let stripes = num(&af["stripes"]).filter(|n| (1..=MAX_STRIPES).contains(n)).ok_or(LuksError::Invalid("stripes"))? as usize;
    let af_hash = LuksHash::from_spec(s(af, "hash").unwrap_or("")).map_err(|_| LuksError::Unsupported(format!("the hash {}", s(af, "hash").unwrap_or("?"))))?;
    let area = &v["area"];
    if s(area, "type") != Some("raw") {
        return Err(LuksError::Unsupported("a key slot area that is not raw".into()));
    }
    if s(area, "encryption") != Some("aes-xts-plain64") {
        return Err(LuksError::Unsupported(format!("the key slot cipher {}", s(area, "encryption").unwrap_or("?"))));
    }
    if num(&area["key_size"]) != Some(key_size as u64) {
        return Err(LuksError::Invalid("area key size"));
    }
    let (area_offset, area_size) = (num(&area["offset"]).ok_or(LuksError::Invalid("area offset"))?, num(&area["size"]).filter(|n| *n > 0 && *n <= MAX_AREA && n % 512 == 0).ok_or(LuksError::Invalid("area size"))?);
    if area_offset % 512 != 0 || (stripes * key_size) as u64 > area_size {
        return Err(LuksError::Invalid("area"));
    }
    let k = &v["kdf"];
    let salt = s(k, "salt").and_then(b64).filter(|b| !b.is_empty() && b.len() <= 256).ok_or(LuksError::Invalid("salt"))?;
    let kdf = match s(k, "type") {
        Some(t @ ("argon2id" | "argon2i")) => {
            let (time, memory, cpus) = (num(&k["time"]), num(&k["memory"]), num(&k["cpus"]));
            let (time, memory, cpus) = (time.filter(|n| (1..=MAX_ARGON_TIME).contains(n)), memory.filter(|n| (8..=MAX_ARGON_KIB).contains(n)), cpus.filter(|n| (1..=MAX_ARGON_CPUS).contains(n)));
            let (Some(time), Some(memory), Some(cpus)) = (time, memory, cpus) else { return Err(LuksError::Invalid("argon2 costs")) };
            Kdf::Argon { alg: if t == "argon2id" { Algorithm::Argon2id } else { Algorithm::Argon2i }, time: time as u32, memory_kib: memory as u32, cpus: cpus as u32, salt }
        }
        Some("pbkdf2") => {
            let hash = LuksHash::from_spec(s(k, "hash").unwrap_or("")).map_err(|_| LuksError::Unsupported(format!("the hash {}", s(k, "hash").unwrap_or("?"))))?;
            let iterations = num(&k["iterations"]).filter(|n| (1..=MAX_ITER).contains(n)).ok_or(LuksError::Invalid("iterations"))? as u32;
            Kdf::Pbkdf2 { hash, iterations, salt }
        }
        other => return Err(LuksError::Unsupported(format!("the key derivation {}", other.unwrap_or("?")))),
    };
    Ok(Slot { id: id.to_string(), key_size, stripes, af_hash, area_offset, area_size, kdf })
}

/// Read a LUKS2 header: the binary part and the JSON after it (`header` holds all [`header_size`] bytes). The checksum (SHA-256 over
/// the whole header with its checksum field zeroed) is checked; the other checksum algorithms are refused.
pub fn parse_header(header: &[u8]) -> Result<Luks2, LuksError> {
    let size = header_size(header.get(..BINARY_HEADER_LEN).ok_or(LuksError::Truncated)?)? as usize;
    if header.len() < size {
        return Err(LuksError::Truncated);
    }
    let h = &header[..size];
    let alg = cstr(&h[72..104]);
    let mut zeroed = h.to_vec();
    zeroed[448..512].fill(0);
    let sum: Vec<u8> = match alg.as_str() {
        "sha256" => Sha256::digest(&zeroed).to_vec(),
        "sha512" => Sha512::digest(&zeroed).to_vec(),
        other => return Err(LuksError::Unsupported(format!("the header checksum {other}"))),
    };
    if !ct_eq(&sum, &h[448..448 + sum.len()]) || h[448 + sum.len()..512].iter().any(|b| *b != 0) {
        return Err(LuksError::Invalid("header checksum"));
    }
    let json = &h[BINARY_HEADER_LEN..];
    let json = &json[..json.iter().position(|c| *c == 0).unwrap_or(json.len())];
    let j: Value = serde_json::from_slice(json).map_err(|_| LuksError::Invalid("the JSON area"))?;
    let mut slots = Vec::new();
    for (id, v) in j["keyslots"].as_object().ok_or(LuksError::Invalid("keyslots"))? {
        slots.push(parse_slot(id, v)?);
    }
    slots.sort_by(|a, b| a.id.parse::<u32>().unwrap_or(u32::MAX).cmp(&b.id.parse::<u32>().unwrap_or(u32::MAX)));
    let mut digests = Vec::new();
    for v in j["digests"].as_object().ok_or(LuksError::Invalid("digests"))?.values() {
        if s(v, "type") != Some("pbkdf2") {
            return Err(LuksError::Unsupported(format!("a digest of type {}", s(v, "type").unwrap_or("?"))));
        }
        digests.push(Digest2 {
            keyslots: v["keyslots"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            hash: LuksHash::from_spec(s(v, "hash").unwrap_or("")).map_err(|_| LuksError::Unsupported(format!("the hash {}", s(v, "hash").unwrap_or("?"))))?,
            iterations: num(&v["iterations"]).filter(|n| (1..=MAX_ITER).contains(n)).ok_or(LuksError::Invalid("iterations"))? as u32,
            salt: s(v, "salt").and_then(b64).filter(|b| !b.is_empty()).ok_or(LuksError::Invalid("salt"))?,
            digest: s(v, "digest").and_then(b64).filter(|b| !b.is_empty() && b.len() <= 64).ok_or(LuksError::Invalid("digest"))?,
        });
    }
    let mut segments = Vec::new();
    for v in j["segments"].as_object().map(|o| o.values().collect::<Vec<_>>()).unwrap_or_default() {
        if s(v, "type") != Some("crypt") {
            continue;
        }
        if s(v, "encryption") != Some("aes-xts-plain64") {
            return Err(LuksError::Unsupported(format!("the data cipher {}", s(v, "encryption").unwrap_or("?"))));
        }
        segments.push(Segment {
            offset: num(&v["offset"]).ok_or(LuksError::Invalid("segment offset"))?,
            size: s(v, "size").filter(|x| *x != "dynamic").and_then(|x| x.parse().ok()),
            iv_tweak: num(&v["iv_tweak"]).unwrap_or(0),
            sector_size: num(&v["sector_size"]).filter(|n| matches!(n, 512 | 1024 | 2048 | 4096)).unwrap_or(512) as u32,
        });
    }
    Ok(Luks2 { uuid: cstr(&h[168..208]), label: cstr(&h[24..72]), slots, digests, segments })
}

fn derive(kdf: &Kdf, password: &[u8], out_len: usize) -> Result<Zeroizing<Vec<u8>>, LuksError> {
    match kdf {
        Kdf::Pbkdf2 { hash, iterations, salt } => Ok(pbkdf2(*hash, password, salt, *iterations, out_len)),
        Kdf::Argon { alg, time, memory_kib, cpus, salt } => {
            let p = Params::new(*memory_kib, *time, *cpus, Some(out_len)).map_err(|_| LuksError::Invalid("argon2 costs"))?;
            let mut out = Zeroizing::new(vec![0u8; out_len]);
            Argon2::new(*alg, Version::V0x13, p).hash_password_into(password, salt, &mut out).map_err(|_| LuksError::Invalid("argon2 costs"))?;
            Ok(out)
        }
    }
}

fn decrypt_area(key: &[u8], data: &mut [u8]) -> Result<(), LuksError> {
    let bad = || LuksError::Invalid("key material size");
    match key.len() {
        32 => {
            let mut k = Zeroizing::new([0u8; 32]);
            k.copy_from_slice(key);
            let x = Xts128::new_128(&k);
            for (i, sec) in data.chunks_mut(512).enumerate() {
                if !x.decrypt_sector(i as u64, sec) {
                    return Err(bad());
                }
            }
        }
        64 => {
            let mut k = Zeroizing::new([0u8; 64]);
            k.copy_from_slice(key);
            let x = Xts::new(&k);
            for (i, sec) in data.chunks_mut(512).enumerate() {
                if !x.decrypt_sector(i as u64, sec) {
                    return Err(bad());
                }
            }
        }
        _ => return Err(LuksError::Invalid("key size")),
    }
    Ok(())
}

impl Luks2 {
    /// The master key, if `password` opens any key slot. `read(offset, bytes)` returns that many bytes of the drive from byte `offset`.
    /// Every slot costs its full key derivation whatever the result.
    pub fn unlock(&self, password: &[u8], read: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Result<Zeroizing<Vec<u8>>, LuksError> {
        for slot in &self.slots {
            let slot_key = derive(&slot.kdf, password, slot.key_size)?;
            let len = slot.stripes * slot.key_size;
            let padded = len.div_ceil(512) * 512;
            let mut raw = read(slot.area_offset, padded).ok_or(LuksError::Unreadable)?;
            if raw.len() < padded {
                return Err(LuksError::Unreadable);
            }
            raw.truncate(padded);
            decrypt_area(&slot_key, &mut raw)?;
            let merged = af_merge(slot.af_hash, &raw[..len], slot.key_size, slot.stripes).ok_or(LuksError::Invalid("key material size"))?;
            for d in self.digests.iter().filter(|d| d.keyslots.contains(&slot.id)) {
                let check = pbkdf2(d.hash, &merged, &d.salt, d.iterations, d.digest.len());
                if ct_eq(&check, &d.digest) {
                    return Ok(merged);
                }
            }
        }
        Err(LuksError::WrongPassword)
    }

    /// The segment that holds the data (the first one), for handing the master key to device-mapper.
    pub fn data_segment(&self) -> Option<&Segment> {
        self.segments.first()
    }
}

//! A minimal OpenPGP check for **detached signatures of downloaded files** (RFC 4880): one question, "was this file signed by one of the
//! RSA keys pinned in the repository?". Used for the AWS CLI zip (`legal/aws-cli-pgp.asc`).
//!
//! What it does: reads an ASCII-armored or binary public key file (RSA primary key and subkeys, v4), reads a detached signature (v3 or v4,
//! binary or armored, RSA, SHA-256/384/512), hashes the file with the signature's trailer and checks the PKCS#1 v1.5 signature with an own
//! small big-number routine (public exponent only, so speed is no issue). No new dependency (`sha2` is already used).
//!
//! What it deliberately does not do: it does not look at key or signature expiry, revocation or user ids (the trust decision is "this exact
//! key file in the repository"; to withdraw a key, change the file), and it ignores every algorithm except RSA with SHA-2. A signature made
//! by a key that is not in the file, a changed file, a wrong hash algorithm or a malformed packet all give an error.

use sha2::{Digest, Sha256, Sha384, Sha512};

/// Armor to bytes (the CRC-24 line is checked when present); input that is not armored is returned unchanged.
pub fn dearmor(data: &[u8]) -> Result<Vec<u8>, String> {
    let Ok(text) = std::str::from_utf8(data) else { return Ok(data.to_vec()) };
    if !text.trim_start().starts_with("-----BEGIN PGP") {
        return Ok(data.to_vec());
    }
    let mut lines = text.lines().map(str::trim);
    lines.by_ref().find(|l| l.starts_with("-----BEGIN PGP"));
    // header lines (`Version: ...`) end at the first empty line
    let mut body = String::new();
    let mut crc: Option<&str> = None;
    let mut in_body = false;
    for l in lines {
        if l.starts_with("-----END PGP") {
            break;
        }
        if !in_body {
            if l.is_empty() {
                in_body = true;
            }
            continue;
        }
        if let Some(c) = l.strip_prefix('=') {
            crc = Some(c);
        } else {
            body.push_str(l);
        }
    }
    let bytes = b64_decode(&body).ok_or("the armored block is not valid base64")?;
    if let Some(c) = crc {
        let want = b64_decode(c).filter(|v| v.len() == 3).ok_or("the armor checksum is malformed")?;
        let got = crc24(&bytes);
        if want != [(got >> 16) as u8, (got >> 8) as u8, got as u8] {
            return Err("the armor checksum does not match (the file is damaged)".into());
        }
    }
    Ok(bytes)
}

fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0x00B7_04CE;
    for &b in data {
        crc ^= (b as u32) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x0100_0000 != 0 {
                crc ^= 0x0186_4CFB;
            }
        }
    }
    crc & 0x00FF_FFFF
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// (tag, body) of every packet; new and old format headers, definite lengths only.
fn packets(mut d: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut out = Vec::new();
    while !d.is_empty() {
        let c = d[0];
        if c & 0x80 == 0 {
            return Err("not an OpenPGP packet".into());
        }
        let (tag, len, hdr): (u8, usize, usize) = if c & 0x40 != 0 {
            let l = *d.get(1).ok_or("truncated packet")? as usize;
            match l {
                0..=191 => (c & 0x3f, l, 2),
                192..=223 => (c & 0x3f, ((l - 192) << 8) + *d.get(2).ok_or("truncated packet")? as usize + 192, 3),
                255 => {
                    let b = d.get(2..6).ok_or("truncated packet")?;
                    (c & 0x3f, u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize, 6)
                }
                _ => return Err("partial-length packets are not supported".into()),
            }
        } else {
            match c & 3 {
                0 => (((c >> 2) & 0xf), *d.get(1).ok_or("truncated packet")? as usize, 2),
                1 => {
                    let b = d.get(1..3).ok_or("truncated packet")?;
                    (((c >> 2) & 0xf), u16::from_be_bytes([b[0], b[1]]) as usize, 3)
                }
                2 => {
                    let b = d.get(1..5).ok_or("truncated packet")?;
                    (((c >> 2) & 0xf), u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize, 5)
                }
                _ => return Err("indeterminate-length packets are not supported".into()),
            }
        };
        let body = d.get(hdr..hdr.checked_add(len).ok_or("bad length")?).ok_or("truncated packet")?;
        out.push((tag, body));
        d = &d[hdr + len..];
    }
    Ok(out)
}

/// One multi-precision integer (2-byte bit count, then the bytes); returns (value without leading zeros, rest).
fn mpi(d: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let bits = u16::from_be_bytes([*d.first().ok_or("truncated number")?, *d.get(1).ok_or("truncated number")?]) as usize;
    let n = bits.div_ceil(8);
    let v = d.get(2..2 + n).ok_or("truncated number")?;
    Ok((v, &d[2 + n..]))
}

/// The RSA public keys (n, e) of a key file: the primary key and every subkey.
pub fn rsa_keys(key_file: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
    let bin = dearmor(key_file)?;
    let mut keys = Vec::new();
    for (tag, body) in packets(&bin)? {
        if tag != 6 && tag != 14 {
            continue;
        }
        // version 4: version, 4 bytes time, algorithm, then the key material; algorithm 1/2/3 = RSA
        if body.len() < 6 || body[0] != 4 || !matches!(body[5], 1..=3) {
            continue;
        }
        let (n, rest) = mpi(&body[6..])?;
        let (e, _) = mpi(rest)?;
        keys.push((n.to_vec(), e.to_vec()));
    }
    if keys.is_empty() {
        return Err("the key file holds no RSA public key".into());
    }
    Ok(keys)
}

struct Sig {
    sigtype: u8,
    hash_alg: u8,
    left16: [u8; 2],
    /// what is hashed after the data
    trailer: Vec<u8>,
    value: Vec<u8>,
}

fn parse_sig(sig_file: &[u8]) -> Result<Sig, String> {
    let bin = dearmor(sig_file)?;
    let (_, b) = packets(&bin)?.into_iter().find(|(t, _)| *t == 2).ok_or("no signature packet in the .sig file")?;
    match b.first() {
        Some(3) => {
            // version, 5, type, 4 time, 8 key id, pub alg, hash alg, left 16, MPI
            if b.len() < 19 || b[1] != 5 || b[15] != 1 {
                return Err("an unsupported version 3 signature (only RSA is)".into());
            }
            let (v, _) = mpi(&b[19..])?;
            Ok(Sig { sigtype: b[2], hash_alg: b[16], left16: [b[17], b[18]], trailer: b[2..7].to_vec(), value: v.to_vec() })
        }
        Some(4) => {
            if b.len() < 6 {
                return Err("truncated signature".into());
            }
            if !matches!(b[2], 1..=3) {
                return Err("only RSA signatures are supported".into());
            }
            let hl = u16::from_be_bytes([b[4], b[5]]) as usize;
            let hdr_end = 6 + hl;
            let ul_at = hdr_end;
            let ul = u16::from_be_bytes([*b.get(ul_at).ok_or("truncated signature")?, *b.get(ul_at + 1).ok_or("truncated signature")?]) as usize;
            let l16_at = ul_at + 2 + ul;
            let l16 = b.get(l16_at..l16_at + 2).ok_or("truncated signature")?;
            let (v, _) = mpi(&b[l16_at + 2..])?;
            let mut trailer = b[..hdr_end].to_vec();
            trailer.extend_from_slice(&[4, 0xff]);
            trailer.extend_from_slice(&(hdr_end as u32).to_be_bytes());
            Ok(Sig { sigtype: b[1], hash_alg: b[3], left16: [l16[0], l16[1]], trailer, value: v.to_vec() })
        }
        _ => Err("an unsupported signature version".into()),
    }
}

/// Check that `sig_file` (detached, binary or armored) is a valid signature of `data` by one of the RSA keys in `key_file`.
pub fn verify_detached(key_file: &[u8], sig_file: &[u8], data: &[u8]) -> Result<(), String> {
    let mut h = Hasher::start(sig_file)?;
    h.update(data);
    h.finish()?.check(key_file)
}

/// Streaming form for large files: `Hasher::start(sig)`, `update` per chunk, `finish()` then `.check(key)`.
pub struct Hasher {
    sig: Sig,
    inner: Inner,
}

enum Inner {
    S256(Sha256),
    S384(Sha384),
    S512(Sha512),
}

pub struct Finished {
    sig: Sig,
    hash: Vec<u8>,
}

impl Hasher {
    pub fn start(sig_file: &[u8]) -> Result<Hasher, String> {
        let sig = parse_sig(sig_file)?;
        if sig.sigtype > 1 {
            return Err("the signature is not a signature of a file".into());
        }
        let inner = match sig.hash_alg {
            8 => Inner::S256(Sha256::new()),
            9 => Inner::S384(Sha384::new()),
            10 => Inner::S512(Sha512::new()),
            a => return Err(format!("the signature uses hash algorithm {a}; only SHA-256, SHA-384 and SHA-512 are accepted")),
        };
        Ok(Hasher { sig, inner })
    }

    pub fn update(&mut self, d: &[u8]) {
        match &mut self.inner {
            Inner::S256(h) => h.update(d),
            Inner::S384(h) => h.update(d),
            Inner::S512(h) => h.update(d),
        }
    }

    pub fn finish(mut self) -> Result<Finished, String> {
        let t = self.sig.trailer.clone();
        self.update(&t);
        let hash = match self.inner {
            Inner::S256(h) => h.finalize().to_vec(),
            Inner::S384(h) => h.finalize().to_vec(),
            Inner::S512(h) => h.finalize().to_vec(),
        };
        Ok(Finished { sig: self.sig, hash })
    }
}

const PREFIX_256: &[u8] = &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];
const PREFIX_384: &[u8] = &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30];
const PREFIX_512: &[u8] = &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40];

impl Finished {
    pub fn check(&self, key_file: &[u8]) -> Result<(), String> {
        if self.hash[..2] != self.sig.left16 {
            return Err("the signature does not match the file".into());
        }
        let prefix = match self.sig.hash_alg {
            8 => PREFIX_256,
            9 => PREFIX_384,
            _ => PREFIX_512,
        };
        for (n, e) in rsa_keys(key_file)? {
            let k = n.len();
            if self.sig.value.len() > k || k < prefix.len() + self.hash.len() + 11 {
                continue;
            }
            let em = Big::from_be(&self.sig.value).modpow(&Big::from_be(&e), &Big::from_be(&n)).to_be(k);
            // 00 01 FF..FF 00 prefix hash
            let t = prefix.len() + self.hash.len();
            let ok = em[0] == 0 && em[1] == 1 && em[2..k - t - 1].iter().all(|&b| b == 0xff) && em[k - t - 1] == 0 && em[k - t..k - self.hash.len()] == *prefix && em[k - self.hash.len()..] == self.hash[..];
            if ok {
                return Ok(());
            }
        }
        Err("the signature is not valid for the file with any pinned key".into())
    }
}

/// Unsigned big number, little-endian 32-bit limbs; only what an RSA public-key operation needs.
#[derive(Clone)]
struct Big(Vec<u32>);

impl Big {
    fn from_be(b: &[u8]) -> Big {
        let mut v = vec![0u32; b.len().div_ceil(4)];
        for (i, &x) in b.iter().rev().enumerate() {
            v[i / 4] |= (x as u32) << (8 * (i % 4));
        }
        let mut r = Big(v);
        r.trim();
        r
    }

    fn to_be(&self, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        for i in 0..len {
            if let Some(l) = self.0.get(i / 4) {
                out[len - 1 - i] = (l >> (8 * (i % 4))) as u8;
            }
        }
        out
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    fn cmp(&self, o: &Big) -> std::cmp::Ordering {
        self.0.len().cmp(&o.0.len()).then_with(|| self.0.iter().rev().cmp(o.0.iter().rev()))
    }

    fn sub_assign(&mut self, o: &Big) {
        let mut borrow = 0i64;
        for i in 0..self.0.len() {
            let d = self.0[i] as i64 - o.0.get(i).copied().unwrap_or(0) as i64 - borrow;
            self.0[i] = d as u32;
            borrow = (d < 0) as i64;
        }
        self.trim();
    }

    fn add_assign(&mut self, o: &Big) {
        let mut carry = 0u64;
        for i in 0..self.0.len().max(o.0.len()) {
            if i >= self.0.len() {
                self.0.push(0);
            }
            let s = self.0[i] as u64 + o.0.get(i).copied().unwrap_or(0) as u64 + carry;
            self.0[i] = s as u32;
            carry = s >> 32;
        }
        if carry > 0 {
            self.0.push(carry as u32);
        }
    }

    fn shl1(&mut self) {
        let mut carry = 0u32;
        for l in self.0.iter_mut() {
            let n = (*l >> 31) as u32;
            *l = (*l << 1) | carry;
            carry = n;
        }
        if carry > 0 {
            self.0.push(carry);
        }
    }

    fn bits(&self) -> usize {
        self.0.len() * 32 - self.0.last().map_or(32, |l| l.leading_zeros() as usize)
    }

    fn bit(&self, i: usize) -> bool {
        self.0.get(i / 32).is_some_and(|l| l >> (i % 32) & 1 == 1)
    }

    fn reduce(&mut self, m: &Big) {
        if self.cmp(m) != std::cmp::Ordering::Less {
            self.sub_assign(m);
        }
    }

    /// (a * b) mod m by double-and-add; both inputs below m.
    fn mulmod(a: &Big, b: &Big, m: &Big) -> Big {
        let mut r = Big(vec![]);
        for i in (0..b.bits()).rev() {
            r.shl1();
            r.reduce(m);
            if b.bit(i) {
                r.add_assign(a);
                r.reduce(m);
            }
        }
        r
    }

    fn modpow(&self, e: &Big, m: &Big) -> Big {
        if m.0.is_empty() {
            return Big(vec![]);
        }
        // reduce the base first (a signature value larger than n is invalid and comes out wrong, which is fine)
        let mut base = self.clone();
        while base.cmp(m) != std::cmp::Ordering::Less {
            base.sub_assign(m);
        }
        let mut r = Big(vec![1]);
        for i in (0..e.bits()).rev() {
            r = Big::mulmod(&r, &r, m);
            if e.bit(i) {
                r = Big::mulmod(&r, &base, m);
            }
        }
        r
    }
}

#[cfg(test)]
#[path = "openpgp_tests.rs"]
mod tests;

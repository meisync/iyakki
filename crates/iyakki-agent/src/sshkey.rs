//! ed25519 SSH keys, in-house: make one, write and read the OpenSSH private key file format, the public key line and the
//! `SHA256:` fingerprint, and sign. No ssh-keygen is needed (the images have none), and the private half never has to be a
//! file: the vault stores the text, [`crate::gitagent`] signs with the key in memory.
//!
//! * The curve code is `ed25519-dalek` (BSD-3-Clause), already part of the build; everything else is here.
//! * Reading a key that is protected by a passphrase supports what `ssh-keygen` writes by default: `bcrypt` key derivation
//!   (bcrypt_pbkdf, with a Blowfish whose tables are computed from the digits of pi at first use instead of being pasted in) and
//!   `aes256-ctr`. Other ciphers and other key types (RSA, ECDSA) are refused with a plain message.
//! * Writing makes unencrypted keys only: the vault encrypts what it holds, and a passphrase would only be one more thing to type.

use std::sync::OnceLock;

use aes::cipher::{KeyIvInit, StreamCipher};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256, Sha512};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8] = b"openssh-key-v1\0";
const BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const END: &str = "-----END OPENSSH PRIVATE KEY-----";
pub const KEY_TYPE: &str = "ssh-ed25519";

/// Why a key text could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The key is protected by a passphrase and none (or a wrong one) was given.
    NeedPassphrase,
    WrongPassphrase,
    /// Not an OpenSSH private key at all.
    NotAKey,
    /// An OpenSSH key of a kind this build does not handle (RSA, ECDSA, an unusual cipher).
    Unsupported(String),
    Damaged(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::NeedPassphrase => write!(f, "this key is protected by a passphrase: enter it to use the key"),
            KeyError::WrongPassphrase => write!(f, "that passphrase does not open the key"),
            KeyError::NotAKey => write!(f, "this is not an OpenSSH private key (it should start with -----BEGIN OPENSSH PRIVATE KEY-----)"),
            KeyError::Unsupported(s) => write!(f, "{s}"),
            KeyError::Damaged(s) => write!(f, "the key text is damaged: {s}"),
        }
    }
}

/// An ed25519 key pair held in memory (the seed is wiped when it is dropped).
pub struct KeyPair {
    seed: Zeroizing<[u8; 32]>,
    public: [u8; 32],
    pub comment: String,
}

impl Clone for KeyPair {
    fn clone(&self) -> KeyPair {
        KeyPair { seed: Zeroizing::new(*self.seed), public: self.public, comment: self.comment.clone() }
    }
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyPair({})", self.fingerprint())
    }
}

// ------------------------------------------------------------------ wire helpers

pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s);
}

pub(crate) struct Reader<'a> {
    pub b: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn u32(&mut self) -> Option<u32> {
        let (h, t) = self.b.split_first_chunk::<4>()?;
        self.b = t;
        Some(u32::from_be_bytes(*h))
    }
    pub fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > self.b.len() {
            return None;
        }
        let (h, t) = self.b.split_at(n);
        self.b = t;
        Some(h)
    }
}

pub(crate) fn b64(data: &[u8]) -> String {
    crate::story::base64_encode(data)
}

pub(crate) fn unb64(s: &str) -> Option<Vec<u8>> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    crate::models::b64_decode(&clean)
}

/// `SHA256:…` of a public key blob, as `ssh-keygen -l` prints it.
pub fn fingerprint_of_blob(blob: &[u8]) -> String {
    format!("SHA256:{}", b64(&Sha256::digest(blob)).trim_end_matches('='))
}

// ------------------------------------------------------------------ the key pair

impl KeyPair {
    pub fn from_seed(seed: [u8; 32], comment: &str) -> KeyPair {
        let public = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
        KeyPair { seed: Zeroizing::new(seed), public, comment: clean_comment(comment) }
    }

    /// A new random key.
    pub fn generate(comment: &str) -> Result<KeyPair, String> {
        let mut seed = [0u8; 32];
        iyakki_sec::random_fill(&mut seed).map_err(|e| format!("no random numbers: {e}"))?;
        let k = KeyPair::from_seed(seed, comment);
        seed.zeroize();
        Ok(k)
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        self.public
    }

    /// The public key as the SSH wire blob (what a server and the agent protocol use).
    pub fn public_blob(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_str(&mut b, KEY_TYPE.as_bytes());
        put_str(&mut b, &self.public);
        b
    }

    /// `ssh-ed25519 AAAA… comment`: the line to paste into GitHub, Gitea, GitLab or `authorized_keys`.
    pub fn public_line(&self) -> String {
        let c = if self.comment.is_empty() { String::new() } else { format!(" {}", self.comment) };
        format!("{KEY_TYPE} {}{c}", b64(&self.public_blob()))
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_of_blob(&self.public_blob())
    }

    /// A short text to read out or type: the fingerprint's first and last characters and the key's tail.
    pub fn short_text(&self) -> String {
        let fp = self.fingerprint();
        let body = fp.trim_start_matches("SHA256:");
        format!("ed25519 {}…{}", &body[..8.min(body.len())], &body[body.len().saturating_sub(6)..])
    }

    /// The signature of `data` in the SSH wire form (`string "ssh-ed25519"`, `string signature`), as the agent protocol returns it.
    pub fn sign_wire(&self, data: &[u8]) -> Vec<u8> {
        let sig = SigningKey::from_bytes(&self.seed).sign(data).to_bytes();
        let mut b = Vec::new();
        put_str(&mut b, KEY_TYPE.as_bytes());
        put_str(&mut b, &sig);
        b
    }

    /// The OpenSSH private key file (not encrypted).
    pub fn to_openssh(&self) -> Zeroizing<String> {
        let mut check = [0u8; 4];
        let _ = iyakki_sec::random_fill(&mut check);
        let check = u32::from_be_bytes(check);
        let mut private = Vec::new();
        put_u32(&mut private, check);
        put_u32(&mut private, check);
        put_str(&mut private, KEY_TYPE.as_bytes());
        put_str(&mut private, &self.public);
        let mut both = Zeroizing::new(Vec::with_capacity(64));
        both.extend_from_slice(&*self.seed);
        both.extend_from_slice(&self.public);
        put_str(&mut private, &both);
        put_str(&mut private, self.comment.as_bytes());
        let mut pad = 1u8;
        while private.len() % 8 != 0 {
            private.push(pad);
            pad += 1;
        }
        let mut file = Vec::new();
        file.extend_from_slice(MAGIC);
        put_str(&mut file, b"none");
        put_str(&mut file, b"none");
        put_str(&mut file, b"");
        put_u32(&mut file, 1);
        put_str(&mut file, &self.public_blob());
        put_str(&mut file, &private);
        private.zeroize();
        let enc = b64(&file);
        file.zeroize();
        let mut text = String::from(BEGIN);
        text.push('\n');
        for chunk in enc.as_bytes().chunks(70) {
            text.push_str(std::str::from_utf8(chunk).unwrap_or(""));
            text.push('\n');
        }
        text.push_str(END);
        text.push('\n');
        Zeroizing::new(text)
    }

    /// Reads an OpenSSH private key file. `passphrase` is needed for an encrypted one.
    pub fn from_openssh(text: &str, passphrase: Option<&str>) -> Result<KeyPair, KeyError> {
        let t = text.trim();
        let body = t.strip_prefix(BEGIN).and_then(|r| r.trim_end().strip_suffix(END)).ok_or(KeyError::NotAKey)?;
        let raw = Zeroizing::new(unb64(body).ok_or_else(|| KeyError::Damaged("the base64 text does not decode".into()))?);
        let dam = |s: &str| KeyError::Damaged(s.into());
        let rest = raw.strip_prefix(MAGIC).ok_or_else(|| dam("no openssh-key-v1 header"))?;
        let mut r = Reader { b: rest };
        let cipher = r.string().ok_or_else(|| dam("cut short"))?.to_vec();
        let kdf = r.string().ok_or_else(|| dam("cut short"))?.to_vec();
        let kdfopts = r.string().ok_or_else(|| dam("cut short"))?.to_vec();
        let n = r.u32().ok_or_else(|| dam("cut short"))?;
        if n != 1 {
            return Err(KeyError::Unsupported(format!("the file holds {n} keys; import one key per file")));
        }
        let pubblob = r.string().ok_or_else(|| dam("cut short"))?.to_vec();
        let mut enc = Zeroizing::new(r.string().ok_or_else(|| dam("cut short"))?.to_vec());
        match pubblob_type(&pubblob).as_deref() {
            Some(KEY_TYPE) => {}
            Some(other) => return Err(KeyError::Unsupported(format!("this is a {other} key; only ed25519 keys are supported (make a new ed25519 key in Settings → Git and SSH keys)"))),
            None => return Err(dam("the public key is unreadable")),
        }
        match (cipher.as_slice(), kdf.as_slice()) {
            (b"none", b"none") => {}
            (b"aes256-ctr", b"bcrypt") => {
                let pass = passphrase.filter(|p| !p.is_empty()).ok_or(KeyError::NeedPassphrase)?;
                let mut o = Reader { b: &kdfopts };
                let salt = o.string().ok_or_else(|| dam("bad key derivation options"))?;
                let rounds = o.u32().ok_or_else(|| dam("bad key derivation options"))?;
                if rounds == 0 || rounds > 2000 {
                    return Err(KeyError::Unsupported(format!("the key asks for {rounds} bcrypt rounds, which is out of range")));
                }
                if enc.len() % 16 != 0 {
                    return Err(dam("the encrypted part has the wrong length"));
                }
                let mut km = Zeroizing::new([0u8; 48]);
                bcrypt_pbkdf(pass.as_bytes(), salt, rounds, &mut *km);
                let mut c = ctr::Ctr128BE::<aes::Aes256>::new((&km[..32]).into(), (&km[32..48]).into());
                c.apply_keystream(&mut enc);
            }
            (c, k) => {
                return Err(KeyError::Unsupported(format!(
                    "this key uses the cipher {} with {}; IYAKKI reads keys written with the default aes256-ctr (re-save it with `ssh-keygen -p -Z aes256-ctr`) or without a passphrase",
                    String::from_utf8_lossy(c),
                    String::from_utf8_lossy(k)
                )))
            }
        }
        let mut p = Reader { b: &enc };
        let (c1, c2) = (p.u32().ok_or_else(|| dam("cut short"))?, p.u32().ok_or_else(|| dam("cut short"))?);
        if c1 != c2 {
            return Err(if cipher == b"none" { dam("the check numbers differ") } else { KeyError::WrongPassphrase });
        }
        let ty = p.string().ok_or_else(|| dam("cut short"))?;
        if ty != KEY_TYPE.as_bytes() {
            return Err(dam("the private part is not ed25519"));
        }
        let public = p.string().ok_or_else(|| dam("cut short"))?;
        let both = p.string().ok_or_else(|| dam("cut short"))?;
        let comment = String::from_utf8_lossy(p.string().ok_or_else(|| dam("cut short"))?).into_owned();
        if public.len() != 32 || both.len() != 64 {
            return Err(dam("wrong key length"));
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&both[..32]);
        let k = KeyPair::from_seed(seed, &comment);
        seed.zeroize();
        if k.public != public || k.public != both[32..] {
            return Err(dam("the public and private halves do not match"));
        }
        Ok(k)
    }
}

fn clean_comment(c: &str) -> String {
    c.chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_string()
}

fn pubblob_type(blob: &[u8]) -> Option<String> {
    let mut r = Reader { b: blob };
    Some(String::from_utf8_lossy(r.string()?).into_owned())
}

/// The public key line (`ssh-ed25519 AAAA… comment`) of a key file, without needing its passphrase: the public blob is in the clear.
pub fn public_line_of(text: &str) -> Result<String, KeyError> {
    let t = text.trim();
    let body = t.strip_prefix(BEGIN).and_then(|r| r.trim_end().strip_suffix(END)).ok_or(KeyError::NotAKey)?;
    let raw = unb64(body).ok_or_else(|| KeyError::Damaged("the base64 text does not decode".into()))?;
    let mut r = Reader { b: raw.strip_prefix(MAGIC).ok_or_else(|| KeyError::Damaged("no header".into()))? };
    for _ in 0..3 {
        r.string().ok_or_else(|| KeyError::Damaged("cut short".into()))?;
    }
    r.u32();
    let blob = r.string().ok_or_else(|| KeyError::Damaged("cut short".into()))?;
    let ty = pubblob_type(blob).ok_or_else(|| KeyError::Damaged("bad public key".into()))?;
    Ok(format!("{ty} {}", b64(blob)))
}

/// Whether the key file is protected by a passphrase.
pub fn is_encrypted(text: &str) -> bool {
    let Some(body) = text.trim().strip_prefix(BEGIN).and_then(|r| r.trim_end().strip_suffix(END)) else { return false };
    let Some(raw) = unb64(body) else { return false };
    let Some(rest) = raw.strip_prefix(MAGIC) else { return false };
    Reader { b: rest }.string().is_some_and(|c| c != b"none")
}

// ------------------------------------------------------------------ Blowfish and bcrypt_pbkdf

struct Blowfish {
    p: [u32; 18],
    s: [[u32; 256]; 4],
}

/// The first 1042 32-bit words of the fractional part of pi (Blowfish's initial P-array and S-boxes), by Machin's formula in
/// fixed-point arithmetic. Done once (a few milliseconds).
fn pi_words() -> &'static [u32; 1042] {
    static W: OnceLock<[u32; 1042]> = OnceLock::new();
    W.get_or_init(|| {
        const LIMBS: usize = 1042 + 3 + 1; // fraction limbs plus guard limbs, and one limb for the integer part at index 0
        // fixed point: limb 0 is the integer part, limbs 1.. the fraction, most significant first
        fn div_small(a: &mut [u32], d: u32) {
            let mut rem = 0u64;
            for x in a.iter_mut() {
                let cur = (rem << 32) | *x as u64;
                *x = (cur / d as u64) as u32;
                rem = cur % d as u64;
            }
        }
        fn add(a: &mut [u32], b: &[u32]) {
            let mut carry = 0u64;
            for i in (0..a.len()).rev() {
                let s = a[i] as u64 + b[i] as u64 + carry;
                a[i] = s as u32;
                carry = s >> 32;
            }
        }
        fn sub(a: &mut [u32], b: &[u32]) {
            let mut borrow = 0i64;
            for i in (0..a.len()).rev() {
                let mut s = a[i] as i64 - b[i] as i64 - borrow;
                borrow = 0;
                if s < 0 {
                    s += 1 << 32;
                    borrow = 1;
                }
                a[i] = s as u32;
            }
        }
        fn is_zero(a: &[u32]) -> bool {
            a.iter().all(|&x| x == 0)
        }
        // atan(1/x) * mult
        fn atan_inv(x: u32, mult: u32) -> Vec<u32> {
            let mut total = vec![0u32; LIMBS];
            let mut term = vec![0u32; LIMBS];
            term[0] = mult; // mult / x^1 comes after the first division
            div_small(&mut term, x);
            let x2 = x * x;
            let mut k = 0u32;
            let mut cur = term.clone();
            loop {
                let mut t = cur.clone();
                div_small(&mut t, 2 * k + 1);
                if k % 2 == 0 {
                    add(&mut total, &t);
                } else {
                    sub(&mut total, &t);
                }
                div_small(&mut cur, x2);
                k += 1;
                if is_zero(&cur) {
                    break;
                }
            }
            total
        }
        let mut pi = atan_inv(5, 16);
        let b = atan_inv(239, 4);
        sub(&mut pi, &b);
        let mut out = [0u32; 1042];
        out.copy_from_slice(&pi[1..1043]);
        out
    })
}

impl Blowfish {
    fn new() -> Blowfish {
        let w = pi_words();
        let mut p = [0u32; 18];
        p.copy_from_slice(&w[..18]);
        let mut s = [[0u32; 256]; 4];
        for (i, row) in s.iter_mut().enumerate() {
            row.copy_from_slice(&w[18 + i * 256..18 + (i + 1) * 256]);
        }
        Blowfish { p, s }
    }

    fn f(&self, x: u32) -> u32 {
        (self.s[0][(x >> 24) as usize].wrapping_add(self.s[1][((x >> 16) & 0xff) as usize]) ^ self.s[2][((x >> 8) & 0xff) as usize]).wrapping_add(self.s[3][(x & 0xff) as usize])
    }

    fn encipher(&self, l: &mut u32, r: &mut u32) {
        let (mut xl, mut xr) = (*l ^ self.p[0], *r);
        for i in (1..=16).step_by(2) {
            xr ^= self.f(xl) ^ self.p[i];
            xl ^= self.f(xr) ^ self.p[i + 1];
        }
        *l = xr ^ self.p[17];
        *r = xl;
    }

    fn stream2word(data: &[u8], j: &mut usize) -> u32 {
        let mut w = 0u32;
        for _ in 0..4 {
            if *j >= data.len() {
                *j = 0;
            }
            w = (w << 8) | data[*j] as u32;
            *j += 1;
        }
        w
    }

    /// Blowfish_expandstate: the salt is mixed into the chain while the key is XORed into the P-array.
    fn expand(&mut self, data: Option<&[u8]>, key: &[u8]) {
        let mut j = 0;
        for i in 0..18 {
            self.p[i] ^= Self::stream2word(key, &mut j);
        }
        let (mut dl, mut dr, mut dj) = (0u32, 0u32, 0usize);
        for i in (0..18).step_by(2) {
            if let Some(d) = data {
                dl ^= Self::stream2word(d, &mut dj);
                dr ^= Self::stream2word(d, &mut dj);
            }
            self.encipher(&mut dl, &mut dr);
            self.p[i] = dl;
            self.p[i + 1] = dr;
        }
        for i in 0..4 {
            for k in (0..256).step_by(2) {
                if let Some(d) = data {
                    dl ^= Self::stream2word(d, &mut dj);
                    dr ^= Self::stream2word(d, &mut dj);
                }
                self.encipher(&mut dl, &mut dr);
                self.s[i][k] = dl;
                self.s[i][k + 1] = dr;
            }
        }
    }
}

fn bcrypt_hash(sha2pass: &[u8; 64], sha2salt: &[u8; 64]) -> [u8; 32] {
    let mut st = Blowfish::new();
    st.expand(Some(sha2salt), sha2pass);
    for _ in 0..64 {
        st.expand(None, sha2salt);
        st.expand(None, sha2pass);
    }
    let text = b"OxychromaticBlowfishSwatDynamite";
    let mut cdata = [0u32; 8];
    let mut j = 0;
    for w in cdata.iter_mut() {
        *w = Blowfish::stream2word(text, &mut j);
    }
    for _ in 0..64 {
        for b in 0..4 {
            let (mut l, mut r) = (cdata[2 * b], cdata[2 * b + 1]);
            st.encipher(&mut l, &mut r);
            cdata[2 * b] = l;
            cdata[2 * b + 1] = r;
        }
    }
    let mut out = [0u8; 32];
    for (i, w) in cdata.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// OpenBSD's bcrypt_pbkdf, the key derivation of passphrase-protected OpenSSH keys.
pub fn bcrypt_pbkdf(pass: &[u8], salt: &[u8], rounds: u32, key: &mut [u8]) {
    let sha2pass: [u8; 64] = Sha512::digest(pass).into();
    let orig = key.len();
    let stride = orig.div_ceil(32);
    let mut amt = orig.div_ceil(stride);
    let mut remaining = orig;
    let mut count = 1u32;
    while remaining > 0 {
        let mut cs = salt.to_vec();
        cs.extend_from_slice(&count.to_be_bytes());
        let sha2salt: [u8; 64] = Sha512::digest(&cs).into();
        let mut tmp = bcrypt_hash(&sha2pass, &sha2salt);
        let mut out = tmp;
        for _ in 1..rounds {
            let s: [u8; 64] = Sha512::digest(tmp).into();
            tmp = bcrypt_hash(&sha2pass, &s);
            for (o, t) in out.iter_mut().zip(tmp.iter()) {
                *o ^= t;
            }
        }
        amt = amt.min(remaining);
        let mut i = 0;
        while i < amt {
            let dest = i * stride + (count as usize - 1);
            if dest >= orig {
                break;
            }
            key[dest] = out[i];
            i += 1;
        }
        remaining -= i;
        count += 1;
    }
}

#[cfg(test)]
#[path = "sshkey_tests.rs"]
mod tests;

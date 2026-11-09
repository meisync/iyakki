//! The recovery key: 256 random bits printed once as 14 groups of 4 characters (52 for the key, 4 for a checksum), so a typing
//! mistake is noticed before anything is tried. The alphabet is Crockford's base32 (no I, L, O, U); lower case, spaces and dashes
//! are accepted when typing, and O, I, L are read as 0, 1, 1.

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::mac::hmac_sha256;
use crate::{Random, VolError};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const KEY_CHARS: usize = 52;
pub const CHECK_CHARS: usize = 4;

/// 32 random bytes. Not `Clone`, no `Debug` text of the bytes, wiped when dropped.
pub struct RecoveryKey([u8; 32]);

impl std::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryKey(..)")
    }
}

impl Drop for RecoveryKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

fn encode(bytes: &[u8], chars: usize) -> String {
    let mut out = String::with_capacity(chars);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in bytes {
        acc = (acc << 8) | *b as u32;
        bits += 8;
        while bits >= 5 && out.len() < chars {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 && out.len() < chars {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn check_text(key: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(b"iyakki-recovery-check-v1");
    h.update(key);
    encode(&h.finalize()[..3], CHECK_CHARS)
}

impl RecoveryKey {
    pub fn generate(rng: &mut dyn Random) -> Result<RecoveryKey, VolError> {
        let mut k = [0u8; 32];
        rng.fill(&mut k)?;
        Ok(RecoveryKey(k))
    }

    pub fn from_bytes(b: [u8; 32]) -> RecoveryKey {
        RecoveryKey(b)
    }

    /// The text to print: `XXXX-XXXX-...` (14 groups).
    pub fn to_text(&self) -> String {
        let mut s = encode(&self.0, KEY_CHARS);
        s.push_str(&check_text(&self.0));
        let groups: Vec<&str> = (0..s.len()).step_by(4).map(|i| &s[i..i + 4]).collect();
        groups.join("-")
    }

    /// Read what a person typed. Every mistake is a distinct, readable error; a wrong key is told apart from a mistyped one by the checksum.
    pub fn parse(text: &str) -> Result<RecoveryKey, VolError> {
        let mut vals: Vec<u8> = Vec::with_capacity(KEY_CHARS + CHECK_CHARS);
        for c in text.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let u = c.to_ascii_uppercase();
            let u = match u {
                'O' => '0',
                'I' | 'L' => '1',
                x => x,
            };
            match ALPHABET.iter().position(|a| *a as char == u) {
                Some(p) => vals.push(p as u8),
                None => return Err(VolError::BadRecoveryKey("it has a character that is not used in recovery keys")),
            }
            if vals.len() > KEY_CHARS + CHECK_CHARS {
                return Err(VolError::BadRecoveryKey("it is too long"));
            }
        }
        if vals.len() < KEY_CHARS + CHECK_CHARS {
            return Err(VolError::BadRecoveryKey("it is too short"));
        }
        let (mut acc, mut bits) = (0u32, 0u32);
        let mut key = [0u8; 32];
        let mut n = 0;
        for v in &vals[..KEY_CHARS] {
            acc = (acc << 5) | *v as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                if n < 32 {
                    key[n] = (acc >> bits) as u8;
                    n += 1;
                }
                acc &= (1 << bits) - 1;
            }
        }
        // 52 characters carry 260 bits; the 4 left over must be zero
        if acc != 0 {
            key.zeroize();
            return Err(VolError::BadRecoveryKey("the check letters do not match; look for a mistyped character"));
        }
        let typed: String = vals[KEY_CHARS..].iter().map(|v| ALPHABET[*v as usize] as char).collect();
        if typed != check_text(&key) {
            key.zeroize();
            return Err(VolError::BadRecoveryKey("the check letters do not match; look for a mistyped character"));
        }
        Ok(RecoveryKey(key))
    }

    /// The 256-bit key-encryption key of the recovery slot (the key is already random, so no slow hash is needed).
    pub fn kek(&self) -> [u8; 32] {
        hmac_sha256(&self.0, &[b"iyakki-volume-recovery-kek-v1"])
    }
}

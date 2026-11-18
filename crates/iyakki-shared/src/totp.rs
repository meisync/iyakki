//! One-time codes for the AUTH tile: HOTP (RFC 4226) and TOTP (RFC 6238) with SHA-1, SHA-256 and SHA-512, base32 secrets
//! (RFC 4648, read leniently: spaces, dashes, lower case and missing padding are fine) and `otpauth://totp/...` links as the
//! authenticator apps and web sites write them. The hashes are the ones already in the tree (`iyakki_sec::wpa` for SHA-1, `sha2`
//! for the others); the HMAC around them is written here (RFC 2104).
//!
//! Nothing here logs, and no error message ever contains the secret or a code.

use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

/// The longest secret text accepted (base32 characters): 1024 bits is far beyond any real service.
pub const MAX_SECRET_CHARS: usize = 256;
/// The longest link accepted.
pub const MAX_URI_BYTES: usize = 2048;
/// The longest issuer or account name.
pub const MAX_NAME_CHARS: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Sha1,
    Sha256,
    Sha512,
}

impl Algo {
    pub const ALL: [Algo; 3] = [Algo::Sha1, Algo::Sha256, Algo::Sha512];

    /// The name written in links and in the vault.
    pub fn name(self) -> &'static str {
        match self {
            Algo::Sha1 => "SHA1",
            Algo::Sha256 => "SHA256",
            Algo::Sha512 => "SHA512",
        }
    }

    /// `SHA1`, `sha-256`, `HmacSHA512` and so on.
    pub fn parse(s: &str) -> Option<Algo> {
        let t: String = s.trim().to_ascii_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        match t.strip_prefix("HMAC").unwrap_or(&t) {
            "SHA1" => Some(Algo::Sha1),
            "SHA256" => Some(Algo::Sha256),
            "SHA512" => Some(Algo::Sha512),
            _ => None,
        }
    }
}

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Base32 text (RFC 4648, with padding) of some bytes.
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let n = u64::from_be_bytes([0, 0, 0, buf[0], buf[1], buf[2], buf[3], buf[4]]);
        let chars = (chunk.len() * 8).div_ceil(5);
        for i in 0..8 {
            if i < chars {
                out.push(B32[((n >> (35 - 5 * i)) & 31) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes of base32 text. Spaces, dashes and tabs are skipped, lower case is fine and the padding may be missing. Anything
/// else is an error; so is a length no byte string can have.
pub fn base32_decode(text: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut chars: Vec<u8> = Vec::new();
    for c in text.chars() {
        match c {
            ' ' | '-' | '\t' | '\r' | '\n' => {}
            '=' => {}
            _ if chars.len() >= MAX_SECRET_CHARS => return Err("That secret is too long.".into()),
            c => match c.to_ascii_uppercase() {
                u @ 'A'..='Z' => chars.push(u as u8 - b'A'),
                u @ '2'..='7' => chars.push(u as u8 - b'2' + 26),
                _ => return Err("A secret has only the letters A to Z and the digits 2 to 7.".into()),
            },
        }
    }
    if chars.is_empty() {
        return Err("Type the secret first.".into());
    }
    if matches!(chars.len() % 8, 1 | 3 | 6) {
        return Err("That secret is not complete: some characters are missing.".into());
    }
    let mut out = Zeroizing::new(Vec::with_capacity(chars.len() * 5 / 8));
    let (mut acc, mut bits) = (0u32, 0u32);
    for v in chars {
        acc = (acc << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// The secret as it is kept: upper case, no spaces, no padding. An error when it is not base32.
pub fn normalize_secret(text: &str) -> Result<Zeroizing<String>, String> {
    let bytes = base32_decode(text)?;
    Ok(Zeroizing::new(base32_encode(&bytes).trim_end_matches('=').to_string()))
}

fn sha2_of<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
    let mut d = D::new();
    for p in parts {
        d.update(p);
    }
    d.finalize().to_vec()
}

/// HMAC (RFC 2104) with the chosen hash.
pub fn hmac(algo: Algo, key: &[u8], msg: &[u8]) -> Vec<u8> {
    let (block, hash): (usize, fn(&[&[u8]]) -> Vec<u8>) = match algo {
        Algo::Sha1 => return iyakki_sec::wpa::hmac_sha1(key, msg).to_vec(),
        Algo::Sha256 => (64, sha2_of::<Sha256>),
        Algo::Sha512 => (128, sha2_of::<Sha512>),
    };
    let mut k = Zeroizing::new(vec![0u8; block]);
    if key.len() > block {
        let h = hash(&[key]);
        k[..h.len()].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let inner = hash(&[&ipad, msg]);
    hash(&[&opad, &inner])
}

/// HOTP (RFC 4226): the code for a counter, `digits` long (6 to 8), with leading zeros.
pub fn hotp(algo: Algo, key: &[u8], counter: u64, digits: u32) -> String {
    let mac = hmac(algo, key, &counter.to_be_bytes());
    let off = (mac[mac.len() - 1] & 15) as usize;
    let bin = u32::from_be_bytes([mac[off] & 0x7f, mac[off + 1], mac[off + 2], mac[off + 3]]);
    let digits = digits.clamp(1, 9);
    format!("{:0width$}", bin % 10u32.pow(digits), width = digits as usize)
}

/// TOTP (RFC 6238) at a Unix time.
pub fn totp(algo: Algo, key: &[u8], unix: i64, period: u32, digits: u32) -> String {
    hotp(algo, key, (unix.max(0) as u64) / period.max(1) as u64, digits)
}

/// Whole seconds until the code changes (1 to `period`).
pub fn seconds_left(unix: i64, period: u32) -> u32 {
    let p = period.max(1) as i64;
    (p - unix.max(0) % p) as u32
}

/// A code in groups for reading: `123 456`, `1234 5678`, `123 4567`.
pub fn grouped(code: &str) -> String {
    let n = code.chars().count();
    let first = match n {
        6 => 3,
        8 => 4,
        7 => 3,
        _ => return code.to_string(),
    };
    let head: String = code.chars().take(first).collect();
    let tail: String = code.chars().skip(first).collect();
    format!("{head} {tail}")
}

/// What an `otpauth://totp/...` link says.
#[derive(Clone, PartialEq, Eq)]
pub struct Parsed {
    pub issuer: String,
    pub account: String,
    /// Normalised base32 (see [`normalize_secret`]).
    pub secret: Zeroizing<String>,
    pub algo: Algo,
    pub digits: u32,
    pub period: u32,
}

impl std::fmt::Debug for Parsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parsed").field("issuer", &self.issuer).field("account", &self.account).field("secret", &"<hidden>").field("algo", &self.algo).field("digits", &self.digits).field("period", &self.period).finish()
    }
}

/// `%41` becomes `A`; invalid escapes and bytes that are not UTF-8 are errors. `plus` reads `+` as a space (query values).
fn percent_decode(s: &str, plus: bool) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                out.push(hex.ok_or("The link has a broken %-escape.")?);
                i += 3;
            }
            b'+' if plus => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "The link is not valid text.".to_string())
}

/// True for text that starts like a one-time-code link (any case).
pub fn looks_like_link(text: &str) -> bool {
    text.trim_start().get(..10).is_some_and(|p| p.eq_ignore_ascii_case("otpauth://"))
}

/// Read an `otpauth://totp/Issuer:account?secret=...&issuer=...&algorithm=SHA1&digits=6&period=30` link. Codes that count uses
/// (`hotp`) are refused with a plain sentence; so is anything malformed, without repeating the link.
pub fn parse_otpauth(uri: &str) -> Result<Parsed, String> {
    let uri = uri.trim();
    if uri.len() > MAX_URI_BYTES {
        return Err("That link is too long to be a one-time-code link.".into());
    }
    if !looks_like_link(uri) {
        return Err("That is not a one-time-code link (it should start with otpauth://).".into());
    }
    let rest = &uri[10..];
    let (kind, rest) = rest.split_once('/').ok_or("The link has no name part.")?;
    match kind.to_ascii_lowercase().as_str() {
        "totp" => {}
        "hotp" => return Err("That link makes codes that count uses (HOTP). Only codes that change with the time are supported.".into()),
        _ => return Err("That link is not for one-time codes.".into()),
    }
    let rest = rest.split('#').next().unwrap_or("");
    let (label, query) = rest.split_once('?').unwrap_or((rest, ""));
    let label = percent_decode(label, false)?;
    let (mut issuer, account) = match label.split_once(':') {
        Some((i, a)) => (i.trim().to_string(), a.trim().to_string()),
        None => (String::new(), label.trim().to_string()),
    };
    let (mut secret, mut algo, mut digits, mut period) = (None, Algo::Sha1, 6u32, 30u32);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (percent_decode(k, true)?.to_ascii_lowercase(), Zeroizing::new(percent_decode(v, true)?));
        match k.as_str() {
            "secret" => secret = Some(normalize_secret(&v)?),
            "issuer" if !v.trim().is_empty() => issuer = v.trim().to_string(),
            "algorithm" => algo = Algo::parse(&v).ok_or("The link uses a hash this app does not know (SHA1, SHA256 and SHA512 are supported).")?,
            "digits" => digits = v.trim().parse().ok().filter(|d| (6..=8).contains(d)).ok_or("The link wants a code length other than 6, 7 or 8 digits.")?,
            "period" => period = v.trim().parse().ok().filter(|p| (1..=3600).contains(p)).ok_or("The link has a time step this app cannot use.")?,
            _ => {}
        }
    }
    let secret = secret.ok_or("The link has no secret.")?;
    for n in [&issuer, &account] {
        if n.chars().count() > MAX_NAME_CHARS || n.chars().any(char::is_control) {
            return Err("The name in the link is too long or has odd characters.".into());
        }
    }
    if issuer.is_empty() && account.is_empty() {
        return Err("The link has no name.".into());
    }
    Ok(Parsed { issuer, account, secret, algo, digits, period })
}

#[cfg(test)]
#[path = "totp_tests.rs"]
mod tests;

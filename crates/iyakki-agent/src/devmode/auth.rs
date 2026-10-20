//! The two proofs of developer mode, built only from primitives already in the tree (no new cryptography):
//!
//! * **Pairing** binds a developer computer's ed25519 key to this device. The device shows a one-time code (10 characters, five
//!   minutes, five tries) and the SHA-256 fingerprint of its TLS certificate. The computer sends its public key and
//!   `HMAC-SHA256(argon2id(code, fingerprint), "IYAKKI-DEV-PAIR v1" | fingerprint | key | name)`. The fingerprint is the one the
//!   computer saw in the TLS handshake, so a machine in the middle (which must show its own certificate) produces a proof the device
//!   refuses; argon2id makes guessing the code from a captured proof slow. The device answers with a second HMAC that only a holder of
//!   the code can make, so the computer knows it reached the device that shows the code.
//! * **Requests** are signed with the paired key: ed25519 over `"IYAKKI-DEV-REQ v1"`, the method, the path, the sorted query, the time,
//!   a random nonce, the SHA-256 of the body and the device's fingerprint. The device checks the key is paired and not revoked, the time
//!   is within two minutes, the nonce is new, and the signature is valid (`verify_strict`). The transport is the device's TLS.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

pub const PAIR_TAG: &str = "IYAKKI-DEV-PAIR v1";
pub const PAIR_OK_TAG: &str = "IYAKKI-DEV-PAIR-OK v1";
pub const REQ_TAG: &str = "IYAKKI-DEV-REQ v1";
/// How far a request's clock may be from the device's.
pub const SKEW_SECS: i64 = 120;
/// The alphabet of pairing codes: no 0/O, 1/I/L look-alikes.
const CODE_ALPHABET: &[u8; 31] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 10;

pub const H_KEY: &str = "x-iyakki-dev-key";
pub const H_TIME: &str = "x-iyakki-dev-time";
pub const H_NONCE: &str = "x-iyakki-dev-nonce";
pub const H_SIG: &str = "x-iyakki-dev-sig";

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || s.len() > 4096 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// SHA-256 of a certificate, as `AB:CD:...` (the same text as the web front end's `tls::fingerprint`).
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// A fingerprint as people compare it: upper case, colons, whitespace ignored.
pub fn norm_fp(fp: &str) -> String {
    fp.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_uppercase()
}

/// HMAC-SHA256 (RFC 2104) over SHA-256 from `sha2`.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
    outer.update(ih);
    outer.finalize().into()
}

/// Constant-time comparison.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A new pairing code, `ABCDE-FGHJK`.
pub fn new_code() -> Result<String, String> {
    // 31 symbols: draw five bits at a time and skip the one value that has no symbol (no bias)
    let mut s = String::new();
    while s.len() < CODE_LEN {
        let mut b = [0u8; 16];
        iyakki_sec::random_fill(&mut b).map_err(|e| format!("no randomness: {e:?}"))?;
        for x in b.iter().map(|x| (x & 31) as usize).filter(|x| *x < CODE_ALPHABET.len()) {
            if s.len() < CODE_LEN {
                s.push(CODE_ALPHABET[x] as char);
            }
        }
    }
    Ok(format!("{}-{}", &s[..5], &s[5..]))
}

/// What the person typed, as the code is compared: upper case, no dashes or spaces.
pub fn norm_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase()
}

/// The pairing key: argon2id of the code, salted with the device's fingerprint (so a proof made for another certificate is useless).
pub fn pair_key(code: &str, fp: &str) -> Result<[u8; 32], String> {
    let salt = Sha256::digest(format!("{PAIR_TAG}\n{}", norm_fp(fp)).as_bytes());
    let params = argon2::Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| e.to_string())?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    a.hash_password_into(norm_code(code).as_bytes(), &salt[..16], &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn pair_msg(fp: &str, key_hex: &str, name: &str) -> String {
    format!("{PAIR_TAG}\n{}\n{}\n{}", norm_fp(fp), key_hex.to_ascii_lowercase(), name)
}

/// The computer's proof that it knows the code, for this device and this key.
pub fn pair_proof(pk: &[u8; 32], fp: &str, key_hex: &str, name: &str) -> String {
    hex(&hmac_sha256(pk, pair_msg(fp, key_hex, name).as_bytes()))
}

/// The device's answer: it knows the code too.
pub fn pair_answer(pk: &[u8; 32], fp: &str, key_hex: &str) -> String {
    hex(&hmac_sha256(pk, format!("{PAIR_OK_TAG}\n{}\n{}", norm_fp(fp), key_hex.to_ascii_lowercase()).as_bytes()))
}

/// The text a request's signature covers. `query` is the decoded query; it is sorted here.
pub fn canonical(method: &str, path: &str, query: &[(String, String)], time: i64, nonce: &str, body: &[u8], fp: &str) -> String {
    let q: BTreeMap<&str, &str> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let q = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    format!("{REQ_TAG}\n{}\n{path}\n{q}\n{time}\n{nonce}\n{}\n{}", method.to_ascii_uppercase(), sha256_hex(body), norm_fp(fp))
}

/// The four headers of a signed request.
pub fn sign_headers(key: &SigningKey, method: &str, path: &str, query: &[(String, String)], body: &[u8], fp: &str, now: i64) -> Result<Vec<(String, String)>, String> {
    let mut n = [0u8; 16];
    iyakki_sec::random_fill(&mut n).map_err(|e| format!("no randomness: {e:?}"))?;
    let nonce = hex(&n);
    let sig = key.sign(canonical(method, path, query, now, &nonce, body, fp).as_bytes());
    Ok(vec![
        (H_KEY.into(), hex(key.verifying_key().as_bytes())),
        (H_TIME.into(), now.to_string()),
        (H_NONCE.into(), nonce),
        (H_SIG.into(), hex(&sig.to_bytes())),
    ])
}

pub fn valid_key_hex(k: &str) -> bool {
    k.len() == 64 && k.chars().all(|c| c.is_ascii_hexdigit()) && unhex(k).and_then(|b| <[u8; 32]>::try_from(b).ok()).is_some_and(|b| VerifyingKey::from_bytes(&b).is_ok())
}

pub fn verify(key_hex: &str, text: &str, sig_hex: &str) -> bool {
    let Some(k) = unhex(key_hex).and_then(|b| <[u8; 32]>::try_from(b).ok()).and_then(|b| VerifyingKey::from_bytes(&b).ok()) else { return false };
    let Some(s) = unhex(sig_hex).and_then(|b| <[u8; 64]>::try_from(b).ok()) else { return false };
    k.verify_strict(text.as_bytes(), &Signature::from_bytes(&s)).is_ok()
}

/// Nonces seen in the last few minutes (a replayed request is refused).
#[derive(Default)]
pub struct Nonces {
    seen: Mutex<(HashMap<String, i64>, VecDeque<(i64, String)>)>,
}

impl Nonces {
    pub const CAP: usize = 20_000;

    /// True when the nonce is new (and now remembered).
    pub fn fresh(&self, nonce: &str, now: i64) -> bool {
        if nonce.len() < 16 || nonce.len() > 64 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        let Ok(mut g) = self.seen.lock() else { return false };
        let (map, order) = &mut *g;
        while order.front().is_some_and(|(t, _)| now - *t > 2 * SKEW_SECS + 10) || order.len() >= Self::CAP {
            if let Some((_, n)) = order.pop_front() {
                map.remove(&n);
            }
        }
        if map.contains_key(nonce) {
            return false;
        }
        map.insert(nonce.to_string(), now);
        order.push_back((now, nonce.to_string()));
        true
    }
}

/// Why a signed request was refused (said to the computer in a few words; details go to the device's activity log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    Missing,
    Clock,
    Replay,
    UnknownKey,
    BadSignature,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AuthError::Missing => "this request is not signed by a paired developer key",
            AuthError::Clock => "the computer's clock is more than two minutes away from the device's",
            AuthError::Replay => "this request was already used",
            AuthError::UnknownKey => "this computer is not paired with the device (or its key was revoked): run `iyakki dev pair` again",
            AuthError::BadSignature => "the request's signature is not valid",
        })
    }
}

/// Check a signed request. `known` says whether a key is paired; returns the key on success.
#[allow(clippy::too_many_arguments)]
pub fn check(headers: &dyn Fn(&str) -> Option<String>, method: &str, path: &str, query: &[(String, String)], body: &[u8], fp: &str, now: i64, nonces: &Nonces, known: &dyn Fn(&str) -> bool) -> Result<String, AuthError> {
    let (Some(key), Some(time), Some(nonce), Some(sig)) = (headers(H_KEY), headers(H_TIME), headers(H_NONCE), headers(H_SIG)) else { return Err(AuthError::Missing) };
    let key = key.to_ascii_lowercase();
    if !valid_key_hex(&key) {
        return Err(AuthError::Missing);
    }
    let Ok(t) = time.parse::<i64>() else { return Err(AuthError::Missing) };
    if (now - t).abs() > SKEW_SECS {
        return Err(AuthError::Clock);
    }
    if !known(&key) {
        return Err(AuthError::UnknownKey);
    }
    if !verify(&key, &canonical(method, path, query, t, &nonce, body, fp), &sig) {
        return Err(AuthError::BadSignature);
    }
    // only a correctly signed request uses up its nonce (a forger cannot fill the cache)
    if !nonces.fresh(&nonce, now) {
        return Err(AuthError::Replay);
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_rfc_4231_test_vectors() {
        // test case 2
        assert_eq!(hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // test case 6: a key longer than the block
        assert_eq!(hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn codes_are_ten_unambiguous_characters_and_compare_without_dashes_or_case() {
        let c = new_code().unwrap();
        assert_eq!(c.len(), 11);
        assert_eq!(c.as_bytes()[5], b'-');
        assert!(norm_code(&c).chars().all(|x| CODE_ALPHABET.contains(&(x as u8))));
        assert!(!c.contains(['0', 'O', '1', 'I', 'L']));
        assert_eq!(norm_code("abcde-fghjk"), norm_code(" ABCDE FGHJK "));
        assert_ne!(new_code().unwrap(), c);
    }

    #[test]
    fn a_pairing_proof_depends_on_the_code_the_certificate_the_key_and_the_name() {
        let fp = "AA:BB:CC";
        let pk = pair_key("ABCDE-FGHJK", fp).unwrap();
        let p = pair_proof(&pk, fp, "00ff", "laptop");
        assert_eq!(p, pair_proof(&pair_key("abcdefghjk", "aa:bb:cc").unwrap(), "aabbcc", "00FF", "laptop"), "typing and fingerprint format do not matter");
        assert_ne!(p, pair_proof(&pair_key("ABCDE-FGHJM", fp).unwrap(), fp, "00ff", "laptop"), "another code");
        assert_ne!(p, pair_proof(&pair_key("ABCDE-FGHJK", "AA:BB:CD").unwrap(), "AA:BB:CD", "00ff", "laptop"), "another certificate (a machine in the middle)");
        assert_ne!(p, pair_proof(&pk, fp, "00fe", "laptop"), "another key");
        assert_ne!(p, pair_proof(&pk, fp, "00ff", "other"), "another name");
        assert_ne!(pair_answer(&pk, fp, "00ff"), p, "the answer is a different message");
    }

    #[test]
    fn a_signed_request_is_accepted_once_and_any_change_or_replay_is_refused() {
        let k = crate::pkg::new_signing_key().unwrap();
        let fp = "11:22";
        let q = vec![("id".to_string(), "clock".to_string()), ("a".to_string(), "1".to_string())];
        let h = sign_headers(&k, "POST", "/v1/dev/install", &q, b"body", fp, 1000).unwrap();
        let get = |h: &Vec<(String, String)>| {
            let h = h.clone();
            move |n: &str| h.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())
        };
        let nonces = Nonces::default();
        let me = hex(k.verifying_key().as_bytes());
        let known = |x: &str| x == me;
        let reversed: Vec<(String, String)> = q.iter().rev().cloned().collect();
        assert_eq!(check(&get(&h), "POST", "/v1/dev/install", &reversed, b"body", fp, 1010, &nonces, &known), Ok(me.clone()), "query order does not matter");
        assert_eq!(check(&get(&h), "POST", "/v1/dev/install", &q, b"body", fp, 1010, &nonces, &known), Err(AuthError::Replay));
        let h2 = sign_headers(&k, "POST", "/v1/dev/install", &q, b"body", fp, 1000).unwrap();
        assert_eq!(check(&get(&h2), "POST", "/v1/dev/install", &q, b"bodY", fp, 1000, &nonces, &known), Err(AuthError::BadSignature), "the body is covered");
        assert_eq!(check(&get(&h2), "POST", "/v1/dev/uninstall", &q, b"body", fp, 1000, &nonces, &known), Err(AuthError::BadSignature), "the path is covered");
        assert_eq!(check(&get(&h2), "POST", "/v1/dev/install", &q, b"body", "11:23", 1000, &nonces, &known), Err(AuthError::BadSignature), "the device is covered");
        assert_eq!(check(&get(&h2), "POST", "/v1/dev/install", &q, b"body", fp, 1000 + SKEW_SECS + 1, &nonces, &known), Err(AuthError::Clock));
        assert_eq!(check(&get(&h2), "POST", "/v1/dev/install", &q, b"body", fp, 1000, &nonces, &|_: &str| false), Err(AuthError::UnknownKey));
        assert_eq!(check(&|_: &str| None, "GET", "/", &[], b"", fp, 1000, &nonces, &known), Err(AuthError::Missing));
        assert_eq!(check(&get(&h2), "POST", "/v1/dev/install", &q, b"body", fp, 1000, &nonces, &known), Ok(me), "the failed tries did not use up the nonce");
    }
}

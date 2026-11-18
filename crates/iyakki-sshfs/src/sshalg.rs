//! Public-key algorithms beyond Ed25519 for the SSH server (F-FILE-13), over `ring` (already in the lock file through rustls): checking
//! user signatures made with RSA (`rsa-sha2-256`, `rsa-sha2-512`; never the SHA-1 `ssh-rsa` signature; keys of 2048 to 8192 bits) and ECDSA
//! over P-256 (`ecdsa-sha2-nistp256`) and P-384 (`ecdsa-sha2-nistp384`; P-521 is not possible: `ring` has no P-521), and an ECDSA P-256 host key that is made here and kept as a PKCS#8 file. An RSA host key is not
//! offered: `ring` cannot make RSA keys, and a key made elsewhere would have to be imported (F-FILE-14).

use ring::rand::SystemRandom;
use ring::signature::{self, EcdsaKeyPair, KeyPair as _, UnparsedPublicKey};

use crate::sshkey::{fingerprint_of_blob, put_str, Reader};

pub const RSA_KEY: &str = "ssh-rsa";
pub const ECDSA_KEY: &str = "ecdsa-sha2-nistp256";
/// Login keys only (F-FILE-14); the host key stays P-256.
pub const ECDSA384_KEY: &str = "ecdsa-sha2-nistp384";
pub const ED25519_KEY: &str = "ssh-ed25519";
pub const MIN_RSA_BITS: usize = 2048;
pub const MAX_RSA_BITS: usize = 8192;

fn strip(b: &[u8]) -> &[u8] {
    &b[b.iter().position(|x| *x != 0).unwrap_or(b.len())..]
}

/// DER length bytes.
fn der_len(o: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        o.push(n as u8);
    } else if n < 0x100 {
        o.extend([0x81, n as u8]);
    } else {
        o.extend([0x82, (n >> 8) as u8, n as u8]);
    }
}

/// A positive integer as DER (leading zero when the top bit is set).
fn der_uint(o: &mut Vec<u8>, be: &[u8]) {
    let t = strip(be);
    if t.is_empty() {
        o.extend([0x02, 1, 0]);
        return;
    }
    o.push(0x02);
    let pad = t[0] & 0x80 != 0;
    der_len(o, t.len() + pad as usize);
    if pad {
        o.push(0);
    }
    o.extend_from_slice(t);
}

/// The type name inside a public key blob.
pub fn blob_type(blob: &[u8]) -> Option<String> {
    let mut r = Reader { b: blob };
    std::str::from_utf8(r.string()?).ok().map(str::to_string)
}

/// Is this a well-formed public key blob of a type the server accepts for login? (RSA of 2048 to 8192 bits with an odd exponent of at
/// least 3, ECDSA P-256 with an uncompressed point, Ed25519 with 32 bytes.)
pub fn valid_user_blob(blob: &[u8]) -> bool {
    let mut r = Reader { b: blob };
    let Some(ty) = r.string() else { return false };
    match ty {
        b"ssh-ed25519" => r.string().map(<[u8]>::len) == Some(32) && r.b.is_empty(),
        b"ssh-rsa" => {
            let (Some(e), Some(n)) = (r.string(), r.string()) else { return false };
            let (e, n) = (strip(e), strip(n));
            let bits = n.len() * 8 - n.first().map_or(0, |b| b.leading_zeros() as usize);
            r.b.is_empty() && (MIN_RSA_BITS..=MAX_RSA_BITS).contains(&bits) && e.last().is_some_and(|b| b & 1 == 1) && e.len() <= 8 && e != [1]
        }
        b"ecdsa-sha2-nistp256" => {
            let (Some(curve), Some(q)) = (r.string(), r.string()) else { return false };
            curve == b"nistp256" && q.len() == 65 && q[0] == 4 && r.b.is_empty()
        }
        b"ecdsa-sha2-nistp384" => {
            let (Some(curve), Some(q)) = (r.string(), r.string()) else { return false };
            curve == b"nistp384" && q.len() == 97 && q[0] == 4 && r.b.is_empty()
        }
        _ => false,
    }
}

/// Check a login signature. `algo` is the algorithm the client named in the request; `blob` the public key; `sig` the SSH signature
/// (`string algo`, `string bytes`); `data` what was signed. Anything malformed is false.
pub fn verify_user(algo: &[u8], blob: &[u8], sig: &[u8], data: &[u8]) -> bool {
    if !valid_user_blob(blob) {
        return false;
    }
    match algo {
        b"ssh-ed25519" => crate::ssh_server::verify_ed25519(blob, sig, data),
        b"rsa-sha2-256" | b"rsa-sha2-512" => {
            let mut b = Reader { b: blob };
            let (Some(b"ssh-rsa"), Some(e), Some(n)) = (b.string(), b.string(), b.string()) else { return false };
            let mut s = Reader { b: sig };
            let (Some(name), Some(raw)) = (s.string(), s.string()) else { return false };
            if name != algo || !s.b.is_empty() {
                return false;
            }
            let mut body = Vec::new();
            der_uint(&mut body, n);
            der_uint(&mut body, e);
            let mut der = vec![0x30];
            der_len(&mut der, body.len());
            der.extend(body);
            let alg: &'static signature::RsaParameters = if algo == b"rsa-sha2-256" { &signature::RSA_PKCS1_2048_8192_SHA256 } else { &signature::RSA_PKCS1_2048_8192_SHA512 };
            UnparsedPublicKey::new(alg, &der).verify(data, raw).is_ok()
        }
        b"ecdsa-sha2-nistp256" | b"ecdsa-sha2-nistp384" => {
            let (half, alg): (usize, &'static signature::EcdsaVerificationAlgorithm) = if algo == b"ecdsa-sha2-nistp256" { (32, &signature::ECDSA_P256_SHA256_FIXED) } else { (48, &signature::ECDSA_P384_SHA384_FIXED) };
            let mut b = Reader { b: blob };
            let (Some(_), Some(_), Some(q)) = (b.string(), b.string(), b.string()) else { return false };
            let mut s = Reader { b: sig };
            let (Some(name), Some(inner)) = (s.string(), s.string()) else { return false };
            if name != algo || !s.b.is_empty() {
                return false;
            }
            let mut i = Reader { b: inner };
            let (Some(r), Some(sv)) = (i.string(), i.string()) else { return false };
            let (r, sv) = (strip(r), strip(sv));
            if !i.b.is_empty() || r.len() > half || sv.len() > half {
                return false;
            }
            let mut fixed = vec![0u8; half * 2];
            fixed[half - r.len()..half].copy_from_slice(r);
            fixed[half * 2 - sv.len()..].copy_from_slice(sv);
            UnparsedPublicKey::new(alg, q).verify(data, &fixed).is_ok()
        }
        _ => false,
    }
}

/// Is `algo` a login algorithm we check for a key of type `ty`?
pub fn algo_fits(algo: &[u8], ty: &str) -> bool {
    matches!((algo, ty), (b"ssh-ed25519", "ssh-ed25519") | (b"rsa-sha2-256" | b"rsa-sha2-512", "ssh-rsa") | (b"ecdsa-sha2-nistp256", "ecdsa-sha2-nistp256") | (b"ecdsa-sha2-nistp384", "ecdsa-sha2-nistp384"))
}

// ------------------------------------------------------------------ the ECDSA host key

pub struct EcHostKey {
    pair: EcdsaKeyPair,
    pkcs8: zeroize::Zeroizing<Vec<u8>>,
}

impl EcHostKey {
    pub fn generate() -> Result<EcHostKey, String> {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng).map_err(|_| "no random numbers for the ECDSA host key".to_string())?;
        EcHostKey::from_pkcs8(doc.as_ref())
    }

    pub fn from_pkcs8(bytes: &[u8]) -> Result<EcHostKey, String> {
        let pair = EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, bytes, &SystemRandom::new()).map_err(|_| "the ECDSA host key is damaged".to_string())?;
        Ok(EcHostKey { pair, pkcs8: zeroize::Zeroizing::new(bytes.to_vec()) })
    }

    pub fn to_pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }

    pub fn public_blob(&self) -> Vec<u8> {
        let mut b = Vec::new();
        put_str(&mut b, ECDSA_KEY.as_bytes());
        put_str(&mut b, b"nistp256");
        put_str(&mut b, self.pair.public_key().as_ref());
        b
    }

    pub fn fingerprint(&self) -> String {
        fingerprint_of_blob(&self.public_blob())
    }

    /// `string "ecdsa-sha2-nistp256"`, `string (mpint r, mpint s)`.
    pub fn sign_wire(&self, data: &[u8]) -> Vec<u8> {
        let sig = self.pair.sign(&SystemRandom::new(), data).map(|s| s.as_ref().to_vec()).unwrap_or_else(|_| vec![0; 64]);
        let mut inner = Vec::new();
        for part in [&sig[..32], &sig[32..]] {
            let t = strip(part);
            let pad = t.first().is_none_or(|b| b & 0x80 != 0);
            crate::sshkey::put_u32(&mut inner, (t.len() + pad as usize) as u32);
            if pad {
                inner.push(0);
            }
            inner.extend_from_slice(t);
        }
        let mut out = Vec::new();
        put_str(&mut out, ECDSA_KEY.as_bytes());
        put_str(&mut out, &inner);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ec_blob_sig(k: &EcHostKey, data: &[u8]) -> (Vec<u8>, Vec<u8>) {
        (k.public_blob(), k.sign_wire(data))
    }

    #[test]
    fn an_ecdsa_signature_made_by_the_host_key_code_checks_for_the_right_text_only() {
        let k = EcHostKey::generate().unwrap();
        for n in 0..40 {
            let data = format!("exchange hash number {n}");
            let (blob, sig) = ec_blob_sig(&k, data.as_bytes());
            assert!(verify_user(b"ecdsa-sha2-nistp256", &blob, &sig, data.as_bytes()), "n={n}");
            assert!(!verify_user(b"ecdsa-sha2-nistp256", &blob, &sig, b"another text"));
        }
        let other = EcHostKey::generate().unwrap();
        let (blob, sig) = ec_blob_sig(&k, b"x");
        assert!(!verify_user(b"ecdsa-sha2-nistp256", &other.public_blob(), &sig, b"x"), "another key");
        assert!(!verify_user(b"ssh-ed25519", &blob, &sig, b"x"), "the wrong algorithm for the key");
        assert!(!verify_user(b"rsa-sha2-256", &blob, &sig, b"x"));
        // every cut-short or damaged signature is refused, none panics
        for cut in 0..sig.len() {
            assert!(!verify_user(b"ecdsa-sha2-nistp256", &blob, &sig[..cut], b"x"));
        }
        for pos in 0..sig.len() {
            let mut bad = sig.clone();
            bad[pos] ^= 0x10;
            assert!(!verify_user(b"ecdsa-sha2-nistp256", &blob, &bad, b"x"), "pos {pos}");
        }
    }

    #[test]
    fn the_host_key_is_kept_as_pkcs8_and_comes_back_the_same() {
        let k = EcHostKey::generate().unwrap();
        let again = EcHostKey::from_pkcs8(k.to_pkcs8()).unwrap();
        assert_eq!(k.public_blob(), again.public_blob());
        assert!(k.fingerprint().starts_with("SHA256:"));
        assert!(EcHostKey::from_pkcs8(&[1, 2, 3]).is_err());
        assert_eq!(blob_type(&k.public_blob()).as_deref(), Some(ECDSA_KEY));
    }

    fn rsa_blob(e: &[u8], n: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        put_str(&mut b, b"ssh-rsa");
        put_str(&mut b, e);
        put_str(&mut b, n);
        b
    }

    #[test]
    fn rsa_keys_are_accepted_only_from_2048_to_8192_bits_with_a_sane_exponent() {
        let n = |bytes: usize| {
            let mut v = vec![0x80];
            v.extend(std::iter::repeat(0x55).take(bytes - 1));
            v
        };
        assert!(valid_user_blob(&rsa_blob(&[1, 0, 1], &n(256))), "2048 bits");
        assert!(valid_user_blob(&rsa_blob(&[1, 0, 1], &n(1024))), "8192 bits");
        assert!(!valid_user_blob(&rsa_blob(&[1, 0, 1], &n(128))), "1024 bits");
        assert!(!valid_user_blob(&rsa_blob(&[1, 0, 1], &n(1025))), "over 8192 bits");
        assert!(!valid_user_blob(&rsa_blob(&[1], &n(256))), "exponent 1");
        assert!(!valid_user_blob(&rsa_blob(&[2], &n(256))), "even exponent");
        assert!(!valid_user_blob(&rsa_blob(&[1, 0, 1], &[])), "no modulus");
        // a leading zero byte (the mpint form) does not count as a bit
        let mut padded = vec![0];
        padded.extend(n(256));
        assert!(valid_user_blob(&rsa_blob(&[1, 0, 1], &padded)));
    }

    #[test]
    fn other_blobs_are_checked_by_shape_and_garbage_is_refused() {
        let mut ed = Vec::new();
        put_str(&mut ed, b"ssh-ed25519");
        put_str(&mut ed, &[7; 32]);
        assert!(valid_user_blob(&ed));
        let mut short = Vec::new();
        put_str(&mut short, b"ssh-ed25519");
        put_str(&mut short, &[7; 31]);
        assert!(!valid_user_blob(&short));
        let mut ec = Vec::new();
        put_str(&mut ec, b"ecdsa-sha2-nistp256");
        put_str(&mut ec, b"nistp384");
        put_str(&mut ec, &[4; 65]);
        assert!(!valid_user_blob(&ec), "wrong curve name");
        let mut dsa = Vec::new();
        put_str(&mut dsa, b"ssh-dss");
        assert!(!valid_user_blob(&dsa));
        assert!(!valid_user_blob(&[]) && !valid_user_blob(&[0, 0, 0, 9, 1]));
        assert!(algo_fits(b"rsa-sha2-512", "ssh-rsa") && !algo_fits(b"ssh-rsa", "ssh-rsa"), "the SHA-1 signature name is never accepted");
        assert!(!algo_fits(b"rsa-sha2-256", "ssh-ed25519"));
    }

    #[test]
    fn der_integers_are_minimal_and_positive() {
        let mut o = Vec::new();
        der_uint(&mut o, &[0, 0, 0x80, 1]);
        assert_eq!(o, [2, 3, 0, 0x80, 1]);
        let mut o = Vec::new();
        der_uint(&mut o, &[0x7f]);
        assert_eq!(o, [2, 1, 0x7f]);
        let mut o = Vec::new();
        der_uint(&mut o, &[]);
        assert_eq!(o, [2, 1, 0]);
        let mut o = Vec::new();
        der_len(&mut o, 300);
        assert_eq!(o, [0x82, 1, 44]);
    }

    /// A P-384 key as a client would hold it: the SSH public key blob and a function that signs in the SSH wire form.
    fn p384() -> (Vec<u8>, impl Fn(&[u8]) -> Vec<u8>) {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P384_SHA384_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P384_SHA384_FIXED_SIGNING, doc.as_ref(), &rng).unwrap();
        let mut blob = Vec::new();
        put_str(&mut blob, ECDSA384_KEY.as_bytes());
        put_str(&mut blob, b"nistp384");
        put_str(&mut blob, pair.public_key().as_ref());
        let sign = move |data: &[u8]| {
            let sig = pair.sign(&SystemRandom::new(), data).unwrap().as_ref().to_vec();
            let mut inner = Vec::new();
            for part in [&sig[..48], &sig[48..]] {
                let t = strip(part);
                let pad = t.first().is_none_or(|b| b & 0x80 != 0);
                crate::sshkey::put_u32(&mut inner, (t.len() + pad as usize) as u32);
                if pad {
                    inner.push(0);
                }
                inner.extend_from_slice(t);
            }
            let mut out = Vec::new();
            put_str(&mut out, ECDSA384_KEY.as_bytes());
            put_str(&mut out, &inner);
            out
        };
        (blob, sign)
    }

    #[test]
    fn a_p384_login_signature_checks_for_the_right_text_and_key_only() {
        let (blob, sign) = p384();
        assert!(valid_user_blob(&blob));
        assert_eq!(blob_type(&blob).as_deref(), Some(ECDSA384_KEY));
        assert!(algo_fits(b"ecdsa-sha2-nistp384", ECDSA384_KEY) && !algo_fits(b"ecdsa-sha2-nistp256", ECDSA384_KEY) && !algo_fits(b"ecdsa-sha2-nistp384", ECDSA_KEY));
        for n in 0..40 {
            let data = format!("session id and request number {n}");
            let sig = sign(data.as_bytes());
            assert!(verify_user(b"ecdsa-sha2-nistp384", &blob, &sig, data.as_bytes()), "n={n}");
            assert!(!verify_user(b"ecdsa-sha2-nistp384", &blob, &sig, b"another text"));
        }
        let (other, _) = p384();
        let sig = sign(b"x");
        assert!(!verify_user(b"ecdsa-sha2-nistp384", &other, &sig, b"x"), "another key");
        assert!(!verify_user(b"ecdsa-sha2-nistp256", &blob, &sig, b"x"), "the P-256 name for a P-384 key");
        for cut in 0..sig.len() {
            assert!(!verify_user(b"ecdsa-sha2-nistp384", &blob, &sig[..cut], b"x"));
        }
        for pos in 0..sig.len() {
            let mut bad = sig.clone();
            bad[pos] ^= 0x10;
            assert!(!verify_user(b"ecdsa-sha2-nistp384", &blob, &bad, b"x"), "pos {pos}");
        }
    }

    #[test]
    fn a_p384_blob_with_the_wrong_curve_or_point_is_refused_and_p521_is_not_accepted() {
        let (blob, _) = p384();
        let mut bad_curve = Vec::new();
        put_str(&mut bad_curve, ECDSA384_KEY.as_bytes());
        put_str(&mut bad_curve, b"nistp256");
        put_str(&mut bad_curve, &blob[blob.len() - 97..]);
        assert!(!valid_user_blob(&bad_curve));
        let mut short = blob.clone();
        short.truncate(blob.len() - 1);
        assert!(!valid_user_blob(&short));
        let mut p521 = Vec::new();
        put_str(&mut p521, b"ecdsa-sha2-nistp521");
        put_str(&mut p521, b"nistp521");
        put_str(&mut p521, &[4u8; 133]);
        assert!(!valid_user_blob(&p521));
        assert!(!verify_user(b"ecdsa-sha2-nistp521", &p521, &[0; 8], b"x"));
    }
}

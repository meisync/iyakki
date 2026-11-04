//! Member keys and key wrapping for the shared vaults (docs/development/CLOUD-MULTITENANT-PLAN.md, 3.2 and 3a).
//!
//! * A member has **one secret** (32 random bytes, kept by the member, never sent anywhere). From it come two key pairs: an X25519 pair
//!   (to receive wrapped scope keys) and an Ed25519 pair (to sign requests to the vault service). The service stores the two *public* keys.
//! * A **scope key** (32 random bytes) encrypts the values of one vault scope (tenant, namespace, common). It is **wrapped** for each member:
//!   a fresh X25519 key, Diffie-Hellman with the member's public key, HKDF-SHA256 to a key and nonce, XChaCha20-Poly1305. The scope name,
//!   the member's name and the key generation are bound as associated data, so a wrap cannot be moved to another scope, member or generation.
//! * A value is encrypted with the scope key, the scope, the secret's name and the generation as associated data.
//!
//! The vault service only ever holds public keys, wraps and ciphertext (`cloudvaultd.rs`); everything here runs on the member's side.
//! The primitives are the ones already in the tree (`x25519-dalek`, `chacha20poly1305`, `sha2`, `ed25519-dalek`); HKDF and HMAC (RFC 5869,
//! RFC 2104) are a few lines over `sha2`.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

const WRAP_INFO: &[u8] = b"iyakki-cloudkeys-wrap-v1";

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    unhex(s).and_then(|b| <[u8; 32]>::try_from(b).ok())
}

/// Random bytes from the system.
pub fn random<const N: usize>() -> Result<[u8; N], String> {
    use std::io::Read;
    let mut b = [0u8; N];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("no random numbers: {e}"))?;
    Ok(b)
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// HKDF-SHA256 (extract and expand) into `out` (at most 255 * 32 bytes).
fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) {
    let prk = hmac(salt, &[ikm]);
    let mut t: Vec<u8> = Vec::new();
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        let n = [(i + 1) as u8];
        let block = hmac(&prk, &[&t, info, &n]);
        chunk.copy_from_slice(&block[..chunk.len()]);
        t = block.to_vec();
    }
}

/// The public half of a member's keys, as the service stores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicKeys {
    /// X25519, hex: where scope keys are wrapped to.
    pub wrap: String,
    /// Ed25519, hex: what signed requests are checked against.
    pub sign: String,
}

impl PublicKeys {
    pub fn check(&self) -> Result<(), String> {
        let w = unhex32(&self.wrap).ok_or("the wrapping key is not 32 bytes of hex")?;
        if w == [0u8; 32] {
            return Err("the wrapping key is not valid".into());
        }
        let s = unhex32(&self.sign).ok_or("the signing key is not 32 bytes of hex")?;
        VerifyingKey::from_bytes(&s).map_err(|_| "the signing key is not valid".to_string())?;
        Ok(())
    }
}

/// A member's secret and what comes from it. Cleared from memory when dropped (each copy too).
#[derive(Clone)]
pub struct MemberKeys {
    seed: [u8; 32],
}

impl Drop for MemberKeys {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

impl MemberKeys {
    pub fn generate() -> Result<MemberKeys, String> {
        Ok(MemberKeys { seed: random::<32>()? })
    }

    pub fn from_seed(seed: [u8; 32]) -> MemberKeys {
        MemberKeys { seed }
    }

    pub fn from_hex(s: &str) -> Result<MemberKeys, String> {
        unhex32(s.trim()).map(MemberKeys::from_seed).ok_or_else(|| "a member secret is 64 hex digits".to_string())
    }

    pub fn seed_hex(&self) -> String {
        hex(&self.seed)
    }

    fn derive(&self, label: &[u8]) -> [u8; 32] {
        let mut o = [0u8; 32];
        hkdf(b"iyakki-cloudkeys-member-v1", &self.seed, label, &mut o);
        o
    }

    fn wrap_secret(&self) -> [u8; 32] {
        self.derive(b"wrap")
    }

    fn signing(&self) -> SigningKey {
        SigningKey::from_bytes(&self.derive(b"sign"))
    }

    pub fn public(&self) -> PublicKeys {
        let w = x25519_dalek::x25519(self.wrap_secret(), x25519_dalek::X25519_BASEPOINT_BYTES);
        PublicKeys { wrap: hex(&w), sign: hex(self.signing().verifying_key().as_bytes()) }
    }

    /// Open a wrap made for this member.
    pub fn unwrap_key(&self, scope: &str, member: &str, generation: u64, w: &Wrapped) -> Result<ScopeKey, String> {
        let epk = unhex32(&w.epk).ok_or("bad wrap")?;
        let ct = unhex(&w.ct).ok_or("bad wrap")?;
        let mut sk = self.wrap_secret();
        let shared = x25519_dalek::x25519(sk, epk);
        sk.zeroize();
        if shared == [0u8; 32] {
            return Err("bad wrap".into());
        }
        let mine = x25519_dalek::x25519(self.wrap_secret(), x25519_dalek::X25519_BASEPOINT_BYTES);
        let (key, nonce) = wrap_material(&shared, &epk, &mine);
        let plain = XChaCha20Poly1305::new(Key::from_slice(&key))
            .decrypt(XNonce::from_slice(&nonce), Payload { msg: &ct, aad: &wrap_aad(scope, member, generation) })
            .map_err(|_| "this wrap was not made for this member, scope and generation".to_string())?;
        <[u8; 32]>::try_from(plain).map(ScopeKey).map_err(|_| "bad wrap".to_string())
    }

    /// The `Authorization` header value for a request to the service: `who` is the member's id there.
    pub fn sign_request(&self, who: &str, method: &str, path: &str, ts: u64, body: &[u8]) -> String {
        let sig = self.signing().sign(request_text(who, method, path, ts, body).as_bytes());
        format!("IYAKKI-SIG id={who},ts={ts},sig={}", hex(&sig.to_bytes()))
    }
}

fn wrap_material(shared: &[u8; 32], epk: &[u8; 32], recipient: &[u8; 32]) -> ([u8; 32], [u8; 24]) {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(epk);
    salt[32..].copy_from_slice(recipient);
    let mut o = [0u8; 56];
    hkdf(&salt, shared, WRAP_INFO, &mut o);
    let (mut k, mut n) = ([0u8; 32], [0u8; 24]);
    k.copy_from_slice(&o[..32]);
    n.copy_from_slice(&o[32..]);
    (k, n)
}

fn wrap_aad(scope: &str, member: &str, generation: u64) -> Vec<u8> {
    format!("iyakki-wrap\0{scope}\0{member}\0{generation}").into_bytes()
}

/// A scope key wrapped for one member: what the service stores.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wrapped {
    /// The one-time X25519 public key, hex.
    pub epk: String,
    /// The sealed key, hex.
    pub ct: String,
}

impl Wrapped {
    /// Only the shape: whether it opens is for the member to find out.
    pub fn check(&self) -> Result<(), String> {
        let ok = unhex32(&self.epk).is_some() && unhex(&self.ct).is_some_and(|c| c.len() == 32 + 16);
        if ok {
            Ok(())
        } else {
            Err("the wrap is not the right shape".into())
        }
    }
}

/// The key of one vault scope. Cleared from memory when dropped.
pub struct ScopeKey([u8; 32]);

impl Drop for ScopeKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl ScopeKey {
    pub fn generate() -> Result<ScopeKey, String> {
        Ok(ScopeKey(random::<32>()?))
    }

    /// Wrap this key for the member whose public wrapping key is `to`.
    pub fn wrap(&self, scope: &str, member: &str, generation: u64, to: &PublicKeys) -> Result<Wrapped, String> {
        let recipient = unhex32(&to.wrap).ok_or("the member's wrapping key is not valid")?;
        let mut esk = random::<32>()?;
        let epk = x25519_dalek::x25519(esk, x25519_dalek::X25519_BASEPOINT_BYTES);
        let shared = x25519_dalek::x25519(esk, recipient);
        esk.zeroize();
        if shared == [0u8; 32] {
            return Err("the member's wrapping key is not valid".into());
        }
        let (key, nonce) = wrap_material(&shared, &epk, &recipient);
        let ct = XChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &self.0, aad: &wrap_aad(scope, member, generation) })
            .map_err(|_| "wrapping failed".to_string())?;
        Ok(Wrapped { epk: hex(&epk), ct: hex(&ct) })
    }

    fn value_aad(scope: &str, name: &str, generation: u64) -> Vec<u8> {
        format!("iyakki-value\0{scope}\0{name}\0{generation}").into_bytes()
    }

    /// Seal a value: the result is the nonce followed by the ciphertext, hex.
    pub fn seal(&self, scope: &str, name: &str, generation: u64, plain: &[u8]) -> Result<String, String> {
        let nonce = random::<24>()?;
        let ct = XChaCha20Poly1305::new(Key::from_slice(&self.0))
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad: &Self::value_aad(scope, name, generation) })
            .map_err(|_| "encryption failed".to_string())?;
        Ok(hex(&[&nonce[..], &ct].concat()))
    }

    pub fn open(&self, scope: &str, name: &str, generation: u64, sealed: &str) -> Result<Vec<u8>, String> {
        let b = unhex(sealed).filter(|b| b.len() >= 24 + 16).ok_or("the value is not sealed data")?;
        XChaCha20Poly1305::new(Key::from_slice(&self.0))
            .decrypt(XNonce::from_slice(&b[..24]), Payload { msg: &b[24..], aad: &Self::value_aad(scope, name, generation) })
            .map_err(|_| "the value does not open with this key (wrong key, or changed)".to_string())
    }
}

/// What a signature covers: the member, the request and the time, so a signed request cannot be turned into another one.
fn request_text(who: &str, method: &str, path: &str, ts: u64, body: &[u8]) -> String {
    format!("iyakki-vault-v1\n{who}\n{method}\n{path}\n{ts}\n{}", hex(&Sha256::digest(body)))
}

/// A parsed `Authorization: IYAKKI-SIG id=..,ts=..,sig=..` header.
pub struct SignedRequest {
    pub id: String,
    pub ts: u64,
    sig: [u8; 64],
}

pub fn parse_authorization(h: &str) -> Option<SignedRequest> {
    let rest = h.strip_prefix("IYAKKI-SIG ")?;
    let (mut id, mut ts, mut sig) = (None, None, None);
    for p in rest.split(',') {
        let (k, v) = p.trim().split_once('=')?;
        match k {
            "id" => id = Some(v.to_string()),
            "ts" => ts = v.parse().ok(),
            "sig" => sig = unhex(v).and_then(|b| <[u8; 64]>::try_from(b).ok()),
            _ => return None,
        }
    }
    Some(SignedRequest { id: id?, ts: ts?, sig: sig? })
}

impl SignedRequest {
    pub fn verify(&self, key: &PublicKeys, method: &str, path: &str, body: &[u8]) -> bool {
        let Some(vk) = unhex32(&key.sign).and_then(|b| VerifyingKey::from_bytes(&b).ok()) else { return false };
        vk.verify_strict(request_text(&self.id, method, path, self.ts, body).as_bytes(), &Signature::from_bytes(&self.sig)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(n: u8) -> MemberKeys {
        MemberKeys::from_seed([n; 32])
    }

    #[test]
    fn hkdf_matches_the_rfc_5869_test_case_1() {
        let ikm = [0x0bu8; 22];
        let salt = unhex("000102030405060708090a0b0c").unwrap();
        let info = unhex("f0f1f2f3f4f5f6f7f8f9").unwrap();
        let mut okm = [0u8; 42];
        hkdf(&salt, &ikm, &info, &mut okm);
        assert_eq!(hex(&okm), "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865");
    }

    #[test]
    fn x25519_matches_the_rfc_7748_vector_so_the_dependency_is_the_real_curve() {
        let k = unhex32("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4").unwrap();
        let u = unhex32("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c").unwrap();
        assert_eq!(hex(&x25519_dalek::x25519(k, u)), "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552");
    }

    #[test]
    fn a_scope_key_wrapped_for_a_member_opens_for_that_member_and_opens_the_values() {
        let (alice, key) = (member(1), ScopeKey::generate().unwrap());
        let w = key.wrap("tenant", "alice", 1, &alice.public()).unwrap();
        w.check().unwrap();
        let got = alice.unwrap_key("tenant", "alice", 1, &w).unwrap();
        let sealed = key.seal("tenant", "db_password", 1, b"s3cret").unwrap();
        assert!(!sealed.contains(&hex(b"s3cret")));
        assert_eq!(got.open("tenant", "db_password", 1, &sealed).unwrap(), b"s3cret");
        assert_ne!(key.wrap("tenant", "alice", 1, &alice.public()).unwrap(), w, "a fresh one-time key every time");
    }

    #[test]
    fn another_member_s_key_cannot_open_a_wrap() {
        let (alice, bob, key) = (member(1), member(2), ScopeKey::generate().unwrap());
        let w = key.wrap("tenant", "alice", 1, &alice.public()).unwrap();
        assert!(bob.unwrap_key("tenant", "alice", 1, &w).is_err());
    }

    #[test]
    fn a_wrap_does_not_open_for_another_scope_member_or_generation() {
        let (alice, key) = (member(1), ScopeKey::generate().unwrap());
        let w = key.wrap("namespace:dev", "alice", 3, &alice.public()).unwrap();
        assert!(alice.unwrap_key("namespace:dev", "alice", 3, &w).is_ok());
        assert!(alice.unwrap_key("namespace:ops", "alice", 3, &w).is_err());
        assert!(alice.unwrap_key("namespace:dev", "bob", 3, &w).is_err());
        assert!(alice.unwrap_key("namespace:dev", "alice", 4, &w).is_err());
    }

    #[test]
    fn changed_ciphertext_or_wrap_is_refused() {
        let (alice, key) = (member(1), ScopeKey::generate().unwrap());
        let mut w = key.wrap("tenant", "alice", 1, &alice.public()).unwrap();
        let mut s = key.seal("tenant", "a", 1, b"value").unwrap();
        let flip = |t: &mut String| {
            let last = t.pop().unwrap();
            t.push(if last == '0' { '1' } else { '0' });
        };
        flip(&mut s);
        assert!(key.open("tenant", "a", 1, &s).is_err());
        flip(&mut w.ct);
        assert!(alice.unwrap_key("tenant", "alice", 1, &w).is_err());
        let ok = key.seal("tenant", "a", 1, b"value").unwrap();
        assert!(key.open("tenant", "b", 1, &ok).is_err(), "bound to the secret's name");
        assert!(key.open("tenant", "a", 2, &ok).is_err(), "bound to the generation");
        assert!(key.open("namespace:x", "a", 1, &ok).is_err(), "bound to the scope");
        assert!(key.open("tenant", "a", 1, "zz").is_err() && key.open("tenant", "a", 1, "").is_err());
        assert!(ScopeKey::generate().unwrap().open("tenant", "a", 1, &ok).is_err(), "another key");
    }

    #[test]
    fn public_keys_are_checked_and_a_signed_request_verifies_only_as_signed() {
        let m = member(5);
        let p = m.public();
        p.check().unwrap();
        assert!(PublicKeys { wrap: "00".repeat(32), sign: p.sign.clone() }.check().is_err());
        assert!(PublicKeys { wrap: p.wrap.clone(), sign: "12".into() }.check().is_err());
        let h = m.sign_request("acme/alice", "PUT", "/v1/scope/tenant/secrets/x", 1000, b"{}");
        let r = parse_authorization(&h).unwrap();
        assert!(r.verify(&p, "PUT", "/v1/scope/tenant/secrets/x", b"{}"));
        assert!(!r.verify(&p, "GET", "/v1/scope/tenant/secrets/x", b"{}"));
        assert!(!r.verify(&p, "PUT", "/v1/scope/tenant/secrets/y", b"{}"));
        assert!(!r.verify(&p, "PUT", "/v1/scope/tenant/secrets/x", b"{ }"));
        assert!(!r.verify(&member(6).public(), "PUT", "/v1/scope/tenant/secrets/x", b"{}"));
        assert!(parse_authorization("Bearer abc").is_none() && parse_authorization("IYAKKI-SIG id=a,ts=x,sig=00").is_none());
    }
}

//! Device sign-in for the engine: the `IYAKKI-SIG` header over a connection's challenge, and the fallback Ed25519 key.
//!
//! The text that is signed and the header are byte for byte those of `iyakki_link::sigscheme` (which `iyakki_agent::clientlink::sign_answer`
//! calls): `iyakki-vault-v2 \n id \n GET \n /client/ws \n ts \n SHA-256(challenge) \n nonce`. `tests/signing.rs` holds golden vectors and
//! checks the engine against the instance's own verifier.
//!
//! Two ways to make the signature. The page prefers the browser's WebCrypto with a non-extractable Ed25519 key: the engine only builds the
//! text ([`request_text`]) and the header ([`header`]) and the browser signs. If the browser has no Ed25519 there, the page keeps a seed in
//! IndexedDB and the engine signs with [`sign_with_seed`]; that key can be read by any script of the page, so the page calls it weaker.

use ed25519_dalek::{Signer, SigningKey};
use iyakki_core_api::client::WS_PATH;
use sha2::{Digest, Sha256};

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// What a signature covers (the vault's `iyakki-vault-v2`, method GET, path `/client/ws`, body the challenge text).
pub fn request_text(device: &str, challenge: &str, ts: u64, nonce: &str) -> String {
    format!("iyakki-vault-v2\n{device}\nGET\n{WS_PATH}\n{ts}\n{}\n{nonce}", hex(&Sha256::digest(challenge.as_bytes())))
}

/// The `Authorization` value for a signature (64 bytes) over [`request_text`].
pub fn header(device: &str, ts: u64, nonce: &str, sig: &[u8]) -> String {
    format!("IYAKKI-SIG id={device},ts={ts},nonce={nonce},sig={}", hex(sig))
}

/// A nonce the instance accepts: 16 to 64 hex digits.
pub fn nonce_ok(n: &str) -> bool {
    (16..=64).contains(&n.len()) && n.bytes().all(|c| c.is_ascii_hexdigit())
}

pub fn public_from_seed(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

pub fn sign_with_seed(seed: &[u8; 32], device: &str, challenge: &str, ts: u64, nonce: &str) -> String {
    let sig = SigningKey::from_bytes(seed).sign(request_text(device, challenge, ts, nonce).as_bytes());
    header(device, ts, nonce, &sig.to_bytes())
}

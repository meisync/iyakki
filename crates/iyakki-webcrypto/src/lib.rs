//! Protocol cryptography written in-house on the primitives already in the tree.
//!
//! * [`webpush`]: RFC 8291 message encryption (aes128gcm, RFC 8188) and RFC 8292 VAPID, F-INBOX-09.
//! * [`webauthn`]: a software authenticator (P-256, attestation none) and the relying-party checks, F-BROX-03.
//! * [`olm`], [`megolm`]: the Matrix end-to-end encryption ratchets from the public specification, F-MSG-08.
//! * [`sas`]: Matrix device verification by short authentication string (emoji and decimals, commitment, MACs), F-MSG-08.
//! * [`keyexport`]: the passphrase-protected room key export file (PBKDF2, AES-CTR, HMAC), F-MSG-08.
//!
//! Nothing here opens a connection or touches a file; keys come in and go out as bytes, and the agent keeps them in the vault.

pub mod b64;
pub mod cbor;
pub mod ctap;
pub mod kdf;
pub mod keyexport;
pub mod megolm;
pub mod olm;
pub mod p256;
pub mod sas;
pub mod webauthn;
pub mod webpush;

/// Random bytes from the system (ring's generator).
pub fn random(buf: &mut [u8]) -> Result<(), String> {
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new().fill(buf).map_err(|_| "no random numbers".to_string())
}

/// Equal in time that does not depend on where the first difference is.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

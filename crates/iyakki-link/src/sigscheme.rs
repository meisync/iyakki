//! Device sign-in: the `IYAKKI-SIG` scheme (docs/development/CONNECTOR-APP-PLAN.md, 3a) for a plain Ed25519 key.
//!
//! The header and the signed text are byte for byte those of the vault (`iyakki-agent/src/cloudkeys.rs`, `iyakki-vault-v2`):
//! `Authorization: IYAKKI-SIG id=NAME,ts=SECONDS,nonce=HEX,sig=HEX`, where the signature covers
//! `iyakki-vault-v2 \n id \n METHOD \n PATH \n ts \n SHA-256(body) \n nonce`. A test in `iyakki-remote` signs with the vault's
//! `MemberKeys` and verifies here (and the other way round), so the two cannot drift apart unnoticed. Here the key is a bare Ed25519
//! pair, not derived from a member secret: it is what a device, a script or `iyakki-connect` holds, or an `ssh-ed25519` key imported
//! from OpenSSH. RSA and other key types are not supported, and there is no JWT.
//!
//! Also here: the OpenSSH formats that matter (a `ssh-ed25519` public key line, and an unencrypted `openssh-key-v1` private key).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

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

/// What a signature covers (the vault's `iyakki-vault-v2`).
pub fn request_text(who: &str, method: &str, path: &str, ts: u64, body: &[u8], nonce: &str) -> String {
    format!("iyakki-vault-v2\n{who}\n{method}\n{path}\n{ts}\n{}\n{nonce}", hex(&Sha256::digest(body)))
}

/// An Ed25519 key pair held by a device. The seed is cleared from memory when dropped.
pub struct DeviceKey {
    seed: [u8; 32],
}

impl Drop for DeviceKey {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceKey({})", self.public_hex()) // never the seed
    }
}

impl DeviceKey {
    pub fn generate() -> Result<DeviceKey, String> {
        let mut seed = [0u8; 32];
        iyakki_sec::random_fill(&mut seed).map_err(|e| format!("no random numbers: {e:?}"))?;
        Ok(DeviceKey { seed })
    }

    pub fn from_seed(seed: [u8; 32]) -> DeviceKey {
        DeviceKey { seed }
    }

    pub fn from_hex(s: &str) -> Result<DeviceKey, String> {
        unhex32(s.trim()).map(DeviceKey::from_seed).ok_or_else(|| "a key seed is 64 hex digits".to_string())
    }

    pub fn seed_hex(&self) -> String {
        hex(&self.seed)
    }

    fn signing(&self) -> SigningKey {
        SigningKey::from_bytes(&self.seed)
    }

    /// The public key (32 bytes).
    pub fn public(&self) -> [u8; 32] {
        self.signing().verifying_key().to_bytes()
    }

    pub fn public_hex(&self) -> String {
        hex(&self.public())
    }

    /// `ssh-ed25519 AAAA... comment`: what an OpenSSH `authorized_keys` line looks like, and what the tenant's owner enrols.
    pub fn ssh_public_line(&self, comment: &str) -> String {
        ssh_public_line(&self.public(), comment)
    }

    /// The `Authorization` header value, with a fresh random nonce.
    pub fn sign_request(&self, who: &str, method: &str, path: &str, ts: u64, body: &[u8]) -> Result<String, String> {
        let mut n = [0u8; 16];
        iyakki_sec::random_fill(&mut n).map_err(|e| format!("no random numbers: {e:?}"))?;
        Ok(self.sign_request_with_nonce(who, method, path, ts, body, &hex(&n)))
    }

    pub fn sign_request_with_nonce(&self, who: &str, method: &str, path: &str, ts: u64, body: &[u8], nonce: &str) -> String {
        let sig = self.signing().sign(request_text(who, method, path, ts, body, nonce).as_bytes());
        format!("IYAKKI-SIG id={who},ts={ts},nonce={nonce},sig={}", hex(&sig.to_bytes()))
    }
}

/// A parsed `Authorization: IYAKKI-SIG ...` header (a nonce is required here: a sign-in without one is refused).
pub struct SignedRequest {
    pub id: String,
    pub ts: u64,
    /// Hex, 16 to 64 digits.
    pub nonce: String,
    sig: [u8; 64],
}

pub fn parse_authorization(h: &str) -> Option<SignedRequest> {
    let rest = h.strip_prefix("IYAKKI-SIG ")?;
    let (mut id, mut ts, mut sig, mut nonce) = (None, None, None, None);
    for p in rest.split(',') {
        let (k, v) = p.trim().split_once('=')?;
        match k {
            "id" => id = Some(v.to_string()).filter(|i| !i.is_empty() && i.len() <= 128),
            "ts" => ts = v.parse().ok(),
            "nonce" => nonce = Some(v.to_string()).filter(|n| (16..=64).contains(&n.len()) && n.bytes().all(|c| c.is_ascii_hexdigit())),
            "sig" => sig = unhex(v).and_then(|b| <[u8; 64]>::try_from(b).ok()),
            _ => return None,
        }
    }
    Some(SignedRequest { id: id?, ts: ts?, nonce: nonce?, sig: sig? })
}

impl SignedRequest {
    /// True when the signature is `public`'s over exactly this request (strict verification).
    pub fn verify(&self, public: &[u8; 32], method: &str, path: &str, body: &[u8]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(public) else { return false };
        vk.verify_strict(request_text(&self.id, method, path, self.ts, body, &self.nonce).as_bytes(), &Signature::from_bytes(&self.sig)).is_ok()
    }
}

// ------------------------------------------------------------------------------------------------------------------ OpenSSH

pub fn base64(data: &[u8]) -> String {
    crate::wsproto::base64(data)
}

/// Standard base64 (padding optional, whitespace ignored); `None` for anything else.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn u32(&mut self) -> Option<u32> {
        let (h, t) = self.0.split_first_chunk::<4>()?;
        self.0 = t;
        Some(u32::from_be_bytes(*h))
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > self.0.len() {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
}

pub fn ssh_public_line(public: &[u8; 32], comment: &str) -> String {
    let mut blob = Vec::new();
    put_string(&mut blob, b"ssh-ed25519");
    put_string(&mut blob, public);
    let c: String = comment.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(64).collect();
    format!("ssh-ed25519 {}{}", base64(&blob), if c.is_empty() { String::new() } else { format!(" {c}") })
}

/// `SHA256:` and the unpadded base64 of the SHA-256 of the key blob: what `ssh-keygen -l` prints.
pub fn fingerprint(public: &[u8; 32]) -> String {
    let line = ssh_public_line(public, "");
    let blob = line.split_whitespace().nth(1).and_then(unbase64).unwrap_or_default();
    format!("SHA256:{}", base64(&Sha256::digest(&blob)).trim_end_matches('='))
}

/// The public key of an `ssh-ed25519 AAAA... [comment]` line (also accepts 64 hex digits). Any other key type is refused.
pub fn parse_public(line: &str) -> Result<[u8; 32], String> {
    let line = line.trim();
    if let Some(k) = unhex32(line) {
        return Ok(k);
    }
    let mut it = line.split_whitespace();
    match it.next() {
        Some("ssh-ed25519") => {}
        Some(other) if other.starts_with("ssh-") || other.starts_with("ecdsa-") || other.starts_with("sk-") => {
            return Err(format!("{other} keys are not supported: only ssh-ed25519 (no RSA, no ECDSA)"));
        }
        _ => return Err("not a public key line (expected: ssh-ed25519 AAAA... or 64 hex digits)".into()),
    }
    let blob = unbase64(it.next().ok_or("the key line has no key")?).ok_or("the key is not valid base64")?;
    let mut r = Rd(&blob);
    if r.string() != Some(b"ssh-ed25519") {
        return Err("the key line says ssh-ed25519 but holds another key".into());
    }
    let k = r.string().and_then(|k| <[u8; 32]>::try_from(k).ok()).ok_or("the key is not 32 bytes")?;
    if !r.0.is_empty() {
        return Err("the key line has trailing data".into());
    }
    VerifyingKey::from_bytes(&k).map_err(|_| "the key is not a valid Ed25519 key".to_string())?;
    Ok(k)
}

/// An unencrypted OpenSSH private key (`-----BEGIN OPENSSH PRIVATE KEY-----`) of type ssh-ed25519.
/// Encrypted keys and other types are refused with the reason.
pub fn parse_openssh_private(text: &str) -> Result<DeviceKey, String> {
    let body: String = text
        .lines()
        .skip_while(|l| l.trim() != "-----BEGIN OPENSSH PRIVATE KEY-----")
        .skip(1)
        .take_while(|l| l.trim() != "-----END OPENSSH PRIVATE KEY-----")
        .collect();
    if body.is_empty() {
        return Err("not an OpenSSH private key (RSA and PEM keys are not supported)".into());
    }
    let raw = unbase64(&body).ok_or("the key is not valid base64")?;
    let rest = raw.strip_prefix(b"openssh-key-v1\0").ok_or("not an openssh-key-v1 file")?;
    let mut r = Rd(rest);
    let (cipher, kdf, _opts) = (r.string().ok_or("cut short")?, r.string().ok_or("cut short")?, r.string().ok_or("cut short")?);
    if cipher != b"none" || kdf != b"none" {
        return Err("this key is protected by a passphrase: remove it into a new key first (ssh-keygen -p -N '' -f COPY), or generate a key with `iyakki-connect key new`".into());
    }
    if r.u32() != Some(1) {
        return Err("the file must hold exactly one key".into());
    }
    let public_blob = r.string().ok_or("cut short")?;
    let mut pr = Rd(public_blob);
    if pr.string() != Some(b"ssh-ed25519") {
        return Err("only ssh-ed25519 keys are supported (no RSA, no ECDSA)".into());
    }
    let public = pr.string().and_then(|k| <[u8; 32]>::try_from(k).ok()).ok_or("bad public key")?;
    let private = r.string().ok_or("cut short")?;
    let mut p = Rd(private);
    let (a, b) = (p.u32().ok_or("cut short")?, p.u32().ok_or("cut short")?);
    if a != b {
        return Err("the key's check numbers differ: damaged".into());
    }
    if p.string() != Some(b"ssh-ed25519") {
        return Err("only ssh-ed25519 keys are supported".into());
    }
    let pub2 = p.string().ok_or("cut short")?;
    let sk = p.string().ok_or("cut short")?;
    if pub2 != public || sk.len() != 64 || sk[32..] != public {
        return Err("the key's two halves do not agree: damaged".into());
    }
    let seed: [u8; 32] = sk[..32].try_into().map_err(|_| "bad key")?;
    let key = DeviceKey::from_seed(seed);
    if key.public() != public {
        return Err("the private key does not belong to the public key: damaged".into());
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> DeviceKey {
        DeviceKey::from_seed([n; 32])
    }

    #[test]
    fn a_signed_request_verifies_only_as_signed() {
        let k = key(1);
        let h = k.sign_request("ann", "POST", "/api/login-key", 1000, b"").unwrap();
        let r = parse_authorization(&h).unwrap();
        assert_eq!((r.id.as_str(), r.ts), ("ann", 1000));
        assert!(r.verify(&k.public(), "POST", "/api/login-key", b""));
        assert!(!r.verify(&k.public(), "GET", "/api/login-key", b""));
        assert!(!r.verify(&k.public(), "POST", "/api/other", b""));
        assert!(!r.verify(&k.public(), "POST", "/api/login-key", b"x"));
        assert!(!r.verify(&key(2).public(), "POST", "/api/login-key", b""));
        assert_ne!(h, k.sign_request("ann", "POST", "/api/login-key", 1000, b"").unwrap(), "a fresh nonce each time");
    }

    #[test]
    fn a_header_without_a_good_nonce_or_with_extra_fields_is_not_parsed() {
        let k = key(1);
        let h = k.sign_request_with_nonce("ann", "POST", "/p", 5, b"", &"ab".repeat(16));
        assert!(parse_authorization(&h).is_some());
        assert!(parse_authorization(&h.replace("nonce=", "extra=1,nonce=")).is_none());
        for bad in ["zz", "ab", &"a".repeat(65)] {
            assert!(parse_authorization(&h.replace(&"ab".repeat(16), bad)).is_none(), "{bad}");
        }
        assert!(parse_authorization(&h.replace(&format!("nonce={},", "ab".repeat(16)), "")).is_none(), "the nonce is required");
        assert!(parse_authorization("Bearer abc").is_none());
        assert!(parse_authorization("IYAKKI-SIG id=,ts=1,nonce=00000000000000000000000000000000,sig=00").is_none());
    }

    #[test]
    fn ed25519_matches_the_rfc_8032_test_vector_1() {
        let k = DeviceKey::from_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap();
        assert_eq!(k.public_hex(), "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
    }

    #[test]
    fn the_ssh_public_line_round_trips_and_other_types_are_refused() {
        let k = key(7);
        let line = k.ssh_public_line("laptop");
        assert!(line.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI"), "{line}");
        assert_eq!(parse_public(&line), Ok(k.public()));
        assert_eq!(parse_public(&k.public_hex()), Ok(k.public()));
        assert!(parse_public("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ comment").unwrap_err().contains("RSA"));
        assert!(parse_public("ecdsa-sha2-nistp256 AAAA").unwrap_err().contains("not supported"));
        assert!(parse_public("ssh-ed25519").is_err() && parse_public("ssh-ed25519 !!!").is_err() && parse_public("hello").is_err());
        assert!(parse_public(&format!("{line} extra words kept as comment")).is_ok());
    }

    /// A real key made by `ssh-keygen -t ed25519 -N ''` (a throw-away test key).
    const SSH_PRIVATE: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACDBFdOcQethiwbFQaclY0v+gI6QoIOpKg7POcHd3uzySAAAAJhf8C2gX/At\noAAAAAtzc2gtZWQyNTUxOQAAACDBFdOcQethiwbFQaclY0v+gI6QoIOpKg7POcHd3uzySA\nAAAEDjAjfXJY3fPBVR0hh6MpgtJ8FtWg7JjdManPOQy5mVicEV05xB62GLBsVBpyVjS/6A\njpCgg6kqDs85wd3e7PJIAAAAD2l5YWtraS10ZXN0LWtleQECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";
    const SSH_PUBLIC: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMEV05xB62GLBsVBpyVjS/6AjpCgg6kqDs85wd3e7PJI iyakki-test-key";

    #[test]
    fn an_openssh_private_key_is_imported_and_its_public_half_matches() {
        let k = parse_openssh_private(SSH_PRIVATE).expect("parsed");
        assert_eq!(k.public(), parse_public(SSH_PUBLIC).unwrap());
        // signing works with the imported key
        let h = k.sign_request("ann", "POST", "/x", 1, b"").unwrap();
        assert!(parse_authorization(&h).unwrap().verify(&k.public(), "POST", "/x", b""));
    }

    #[test]
    fn damaged_encrypted_or_foreign_private_keys_are_refused() {
        assert!(parse_openssh_private("hello").unwrap_err().contains("not an OpenSSH"));
        assert!(parse_openssh_private("-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\n").is_err());
        // flip a byte inside the key material
        let mut bad = SSH_PRIVATE.to_string();
        bad = bad.replace("CDBFdOc", "CDBFdOd");
        assert!(parse_openssh_private(&bad).is_err());
        // an encrypted key says so
        let mut raw = b"openssh-key-v1\0".to_vec();
        put_string(&mut raw, b"aes256-ctr");
        put_string(&mut raw, b"bcrypt");
        put_string(&mut raw, b"");
        raw.extend_from_slice(&1u32.to_be_bytes());
        let enc = format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n", base64(&raw));
        assert!(parse_openssh_private(&enc).unwrap_err().contains("passphrase"));
    }

    #[test]
    fn base64_decodes_what_it_encodes() {
        for n in 0..40usize {
            let d: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(unbase64(&base64(&d)), Some(d));
        }
        assert_eq!(unbase64("a$b"), None);
    }
}

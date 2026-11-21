//! A software WebAuthn authenticator (P-256, ES256, attestation `none`) and the relying-party checks that go with it (F-BROX-03).
//!
//! The authenticator keeps no state of its own: the caller passes the stored [`Credential`] records in and out (the agent keeps them in
//! the person's vault) and says whether the person confirmed user presence. [`make_credential`] and [`get_assertion`] REFUSE when presence
//! is not confirmed, so a page can never get a signature without the person's card being approved. The relying-party functions
//! ([`verify_registration`], [`verify_assertion`]) are what a site does with the answers, written from the W3C WebAuthn Level 2 steps; they
//! are used to test against independent authenticators and are also what an IYAKKI-hosted site could use.

use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING};
use sha2::{Digest, Sha256};

use crate::cbor::{self, Value};
use crate::{b64, ct_eq};

pub const FLAG_UP: u8 = 0x01;
pub const FLAG_UV: u8 = 0x04;
pub const FLAG_AT: u8 = 0x40;
/// COSE algorithm number of ES256.
pub const ES256: i64 = -7;

/// One credential kept for a relying party.
#[derive(Clone, Debug, PartialEq)]
pub struct Credential {
    pub id: Vec<u8>,
    pub rp_id: String,
    pub user_handle: Vec<u8>,
    pub user_name: String,
    /// The private key as PKCS#8.
    pub pkcs8: Vec<u8>,
    pub sign_count: u32,
    pub created: u64,
}

pub fn rp_id_hash(rp_id: &str) -> [u8; 32] {
    Sha256::digest(rp_id.as_bytes()).into()
}

/// The relying party id must be the origin's host or a parent domain of it (no public-suffix list here: the page's browser enforces
/// that), never an address, and an origin must be https (http only for localhost).
pub fn rp_id_allowed(rp_id: &str, origin: &str) -> Result<(), String> {
    let (https, rest) = match origin.strip_prefix("https://") {
        Some(r) => (true, r),
        None => (false, origin.strip_prefix("http://").ok_or("the page's origin is not http or https")?),
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host_port.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(host_port, |(h, _)| h);
    if host.is_empty() || host.contains('@') {
        return Err("bad origin".into());
    }
    if !https && host != "localhost" {
        return Err("passkeys work on https pages (or localhost) only".into());
    }
    let valid = !rp_id.is_empty() && rp_id.len() <= 253 && rp_id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-') && !rp_id.starts_with('.') && !rp_id.ends_with('.');
    let is_ip = rp_id.bytes().all(|b| b.is_ascii_digit() || b == b'.') || host.contains(':');
    if !valid || is_ip {
        return Err("the relying party id is not a domain name".into());
    }
    if host == rp_id || host.strip_suffix(rp_id).is_some_and(|p| p.ends_with('.')) {
        Ok(())
    } else {
        Err(format!("the page at {host} may not use the relying party id {rp_id}"))
    }
}

/// `clientDataJSON` for `webauthn.create` or `webauthn.get`.
pub fn client_data_json(kind: &str, challenge: &[u8], origin: &str) -> String {
    format!(r#"{{"type":"{kind}","challenge":"{}","origin":"{}","crossOrigin":false}}"#, b64::url(challenge), origin.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn client_data_hash(json: &str) -> [u8; 32] {
    Sha256::digest(json.as_bytes()).into()
}

fn cose_key(public: &[u8]) -> Value {
    Value::Map(vec![
        (Value::Int(1), Value::Int(2)),
        (Value::Int(3), Value::Int(ES256)),
        (Value::Int(-1), Value::Int(1)),
        (Value::Int(-2), Value::Bytes(public[1..33].to_vec())),
        (Value::Int(-3), Value::Bytes(public[33..65].to_vec())),
    ])
}

fn pair(c: &Credential) -> Result<EcdsaKeyPair, String> {
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &c.pkcs8, &SystemRandom::new()).map_err(|_| "the passkey is damaged".to_string())
}

/// The uncompressed public point of a credential.
pub fn public_key(c: &Credential) -> Result<Vec<u8>, String> {
    Ok(pair(c)?.public_key().as_ref().to_vec())
}

fn auth_data(rp_id: &str, flags: u8, count: u32, attested: Option<(&[u8], &[u8])>) -> Vec<u8> {
    let mut d = rp_id_hash(rp_id).to_vec();
    d.push(flags);
    d.extend_from_slice(&count.to_be_bytes());
    if let Some((id, public)) = attested {
        d.extend_from_slice(&[0u8; 16]);
        d.extend_from_slice(&(id.len() as u16).to_be_bytes());
        d.extend_from_slice(id);
        d.extend_from_slice(&cbor::encode(&cose_key(public)));
    }
    d
}

/// What a page gets back from `navigator.credentials.create()` (before it is wrapped in an ArrayBuffer).
#[derive(Debug)]
pub struct Created {
    pub credential: Credential,
    pub attestation_object: Vec<u8>,
}

/// Makes a credential: ES256 only (`algs` must allow -7), attestation `none`, the new key never leaves the record. Refuses without
/// confirmed user presence, for an id on the exclude list and when the relying party already has a credential for this user handle in
/// `existing` that is listed in `exclude`.
#[allow(clippy::too_many_arguments)]
pub fn make_credential(rp_id: &str, cd_hash: &[u8; 32], user_handle: &[u8], user_name: &str, algs: &[i64], exclude: &[Vec<u8>], existing: &[Credential], presence: bool, now: u64) -> Result<Created, String> {
    make_credential_uv(rp_id, cd_hash, user_handle, user_name, algs, exclude, existing, Check { presence, verified: false, required: false }, now)
}

/// What the person did and what the site demands (WebAuthn `userVerification`): presence is the card; `verified` is user verification, the
/// person proving who they are with something more than a tap (their password); `required` is the site's `userVerification: "required"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub presence: bool,
    pub verified: bool,
    pub required: bool,
}

impl Check {
    fn flags(&self) -> Result<u8, String> {
        if !self.presence {
            return Err("the person did not confirm".into());
        }
        if self.required && !self.verified {
            return Err("this site requires the person to prove who they are, and that was not done".into());
        }
        Ok(FLAG_UP | if self.verified { FLAG_UV } else { 0 })
    }
}

/// [`make_credential`] with user verification: the UV flag is set in the authenticator data only when `check.verified`, and a site that
/// requires it never gets a credential without.
#[allow(clippy::too_many_arguments)]
pub fn make_credential_uv(rp_id: &str, cd_hash: &[u8; 32], user_handle: &[u8], user_name: &str, algs: &[i64], exclude: &[Vec<u8>], existing: &[Credential], check: Check, now: u64) -> Result<Created, String> {
    let flags = check.flags()?;
    if !algs.is_empty() && !algs.contains(&ES256) {
        return Err("only ES256 (P-256) is supported".into());
    }
    if user_handle.is_empty() || user_handle.len() > 64 {
        return Err("the user handle must be 1 to 64 bytes".into());
    }
    if existing.iter().any(|c| c.rp_id == rp_id && exclude.contains(&c.id)) {
        return Err("a passkey for this account already exists here".into());
    }
    let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new()).map_err(|_| "no random numbers")?;
    let mut id = [0u8; 32];
    crate::random(&mut id)?;
    let credential = Credential { id: id.to_vec(), rp_id: rp_id.into(), user_handle: user_handle.to_vec(), user_name: user_name.chars().take(64).collect(), pkcs8: doc.as_ref().to_vec(), sign_count: 0, created: now };
    let _ = cd_hash; // registration signs nothing with attestation none; the hash is the page's to check
    let public = public_key(&credential)?;
    let ad = auth_data(rp_id, flags | FLAG_AT, 0, Some((&id, &public)));
    let attestation_object = cbor::encode(&Value::Map(vec![(Value::text("fmt"), Value::text("none")), (Value::text("attStmt"), Value::Map(vec![])), (Value::text("authData"), Value::Bytes(ad))]));
    Ok(Created { credential, attestation_object })
}

#[derive(Debug)]
pub struct Assertion {
    pub credential_id: Vec<u8>,
    pub auth_data: Vec<u8>,
    /// ASN.1 DER ECDSA signature over `auth_data || client_data_hash`.
    pub signature: Vec<u8>,
    pub user_handle: Vec<u8>,
}

/// Finds the credential for `rp_id` (one named in `allow`, or, when the list is empty, `pick` by id or the newest), signs, and raises its
/// sign count. Refuses without confirmed presence.
pub fn get_assertion(rp_id: &str, cd_hash: &[u8; 32], allow: &[Vec<u8>], pick: Option<&[u8]>, creds: &mut [Credential], presence: bool) -> Result<Assertion, String> {
    get_assertion_uv(rp_id, cd_hash, allow, pick, creds, Check { presence, verified: false, required: false })
}

/// The passkeys a sign-in for `rp_id` could use (the allowed ones, or every one of the site when the list is empty), newest first: id, account
/// name, creation time. A site that lists no credentials (a discoverable-credential sign-in) leaves the choice of account to the person.
pub fn candidates(rp_id: &str, allow: &[Vec<u8>], creds: &[Credential]) -> Vec<(Vec<u8>, String, u64)> {
    let mut v: Vec<(Vec<u8>, String, u64)> = creds.iter().filter(|c| c.rp_id == rp_id && (allow.is_empty() || allow.contains(&c.id))).map(|c| (c.id.clone(), c.user_name.clone(), c.created)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    v
}

/// The user verification flag of authenticator data (what a site reads to know the person proved who they are).
pub fn user_verified(auth_data: &[u8]) -> bool {
    auth_data.get(32).is_some_and(|f| f & FLAG_UV != 0)
}

/// [`get_assertion`] with user verification (see [`Check`]).
pub fn get_assertion_uv(rp_id: &str, cd_hash: &[u8; 32], allow: &[Vec<u8>], pick: Option<&[u8]>, creds: &mut [Credential], check: Check) -> Result<Assertion, String> {
    let flags = check.flags()?;
    let idx = creds
        .iter()
        .enumerate()
        .filter(|(_, c)| c.rp_id == rp_id && (allow.is_empty() || allow.contains(&c.id)) && pick.is_none_or(|p| p == c.id.as_slice()))
        .max_by_key(|(_, c)| c.created)
        .map(|(i, _)| i)
        .ok_or("there is no passkey for this site")?;
    let c = &mut creds[idx];
    c.sign_count = c.sign_count.checked_add(1).ok_or("sign count overflow")?;
    let ad = auth_data(rp_id, flags, c.sign_count, None);
    let mut msg = ad.clone();
    msg.extend_from_slice(cd_hash);
    let signature = pair(c)?.sign(&SystemRandom::new(), &msg).map_err(|_| "signing failed")?.as_ref().to_vec();
    Ok(Assertion { credential_id: c.id.clone(), auth_data: ad, signature, user_handle: c.user_handle.clone() })
}

// ---- the relying party's side ------------------------------------------------------------------------------------------------------

fn check_client_data(json: &[u8], kind: &str, challenge: &[u8], origin: &str) -> Result<(), String> {
    let s = std::str::from_utf8(json).map_err(|_| "clientDataJSON is not text")?;
    let field = |name: &str| -> Option<String> {
        let at = s.find(&format!("\"{name}\":\""))? + name.len() + 4;
        Some(s[at..].split('"').next()?.to_string())
    };
    if field("type").as_deref() != Some(kind) {
        return Err(format!("the client data is not a {kind}"));
    }
    if field("challenge").map(|c| b64::decode(&c)).transpose()?.as_deref().map(|c| ct_eq(c, challenge)) != Some(true) {
        return Err("the challenge is not the one that was given".into());
    }
    if field("origin").as_deref() != Some(origin) {
        return Err("the origin is not the expected one".into());
    }
    Ok(())
}

/// A registered public key (uncompressed point) and its id.
#[derive(Debug, PartialEq)]
pub struct Registered {
    pub credential_id: Vec<u8>,
    pub public_key: Vec<u8>,
    pub sign_count: u32,
}

/// W3C WebAuthn 7.1 for attestation `none` (a statement of another format is refused, not trusted).
pub fn verify_registration(attestation_object: &[u8], client_data: &[u8], challenge: &[u8], origin: &str, rp_id: &str) -> Result<Registered, String> {
    check_client_data(client_data, "webauthn.create", challenge, origin)?;
    let ao = cbor::decode_all(attestation_object)?;
    if ao.get_text("fmt") != Some(&Value::text("none")) {
        return Err("only attestation none is accepted".into());
    }
    let ad = ao.get_text("authData").and_then(Value::as_bytes).ok_or("no authData")?;
    if ad.len() < 55 || ad[..32] != rp_id_hash(rp_id) {
        return Err("the relying party id hash does not match".into());
    }
    if ad[32] & FLAG_UP == 0 || ad[32] & FLAG_AT == 0 {
        return Err("user presence or attested data flag missing".into());
    }
    let idlen = u16::from_be_bytes([ad[53], ad[54]]) as usize;
    let id = ad.get(55..55 + idlen).ok_or("authData too short")?.to_vec();
    let (key, used) = cbor::decode(&ad[55 + idlen..])?;
    if 55 + idlen + used != ad.len() {
        return Err("extensions are not accepted".into());
    }
    if key.get(&Value::Int(1)).and_then(Value::as_int) != Some(2) || key.get(&Value::Int(3)).and_then(Value::as_int) != Some(ES256) || key.get(&Value::Int(-1)).and_then(Value::as_int) != Some(1) {
        return Err("the key is not an ES256 P-256 key".into());
    }
    let (x, y) = (key.get(&Value::Int(-2)).and_then(Value::as_bytes).ok_or("no x")?, key.get(&Value::Int(-3)).and_then(Value::as_bytes).ok_or("no y")?);
    if x.len() != 32 || y.len() != 32 {
        return Err("bad coordinates".into());
    }
    let mut public_key = vec![4u8];
    public_key.extend_from_slice(x);
    public_key.extend_from_slice(y);
    crate::p256::ecdh(&[1u8; 32], &public_key).map_err(|_| "the public key is not on the curve".to_string())?;
    Ok(Registered { credential_id: id, public_key, sign_count: u32::from_be_bytes([ad[33], ad[34], ad[35], ad[36]]) })
}

/// W3C WebAuthn 7.2: returns the new sign count. A count that did not rise (when either is not 0) is refused as a possible clone.
#[allow(clippy::too_many_arguments)]
pub fn verify_assertion(public_key: &[u8], auth_data: &[u8], client_data: &[u8], signature: &[u8], challenge: &[u8], origin: &str, rp_id: &str, stored_count: u32) -> Result<u32, String> {
    check_client_data(client_data, "webauthn.get", challenge, origin)?;
    if auth_data.len() < 37 || auth_data[..32] != rp_id_hash(rp_id) {
        return Err("the relying party id hash does not match".into());
    }
    if auth_data[32] & FLAG_UP == 0 {
        return Err("user presence flag missing".into());
    }
    let mut msg = auth_data.to_vec();
    msg.extend_from_slice(&client_data_hash(std::str::from_utf8(client_data).map_err(|_| "text")?));
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_key).verify(&msg, signature).map_err(|_| "the signature is wrong".to_string())?;
    let count = u32::from_be_bytes([auth_data[33], auth_data[34], auth_data[35], auth_data[36]]);
    if (count != 0 || stored_count != 0) && count <= stored_count {
        return Err("the sign count did not rise: this passkey may have been copied".into());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ORIGIN: &str = "https://login.example.com";

    #[test]
    fn user_verification_sets_the_flag_only_when_done_and_a_site_that_requires_it_gets_nothing_without() {
        let challenge = [9u8; 32];
        let cd = client_data_json("webauthn.create", &challenge, ORIGIN);
        let h = client_data_hash(&cd);
        let tap = Check { presence: true, verified: false, required: false };
        let proved = Check { presence: true, verified: true, required: false };
        let need = Check { presence: true, verified: false, required: true };
        // only a tap: no UV flag; proved: the flag; required without proof: refused and nothing made
        let a = make_credential_uv("example.com", &h, b"u", "ana", &[ES256], &[], &[], tap, 1).unwrap();
        let b = make_credential_uv("example.com", &h, b"u", "ana", &[ES256], &[], &[], proved, 1).unwrap();
        let ad = |att: &[u8]| cbor::decode_all(att).unwrap().get_text("authData").unwrap().as_bytes().unwrap().to_vec();
        assert!(!user_verified(&ad(&a.attestation_object)) && user_verified(&ad(&b.attestation_object)));
        assert!(make_credential_uv("example.com", &h, b"u", "ana", &[ES256], &[], &[], need, 1).unwrap_err().contains("prove who they are"));
        assert!(make_credential_uv("example.com", &h, b"u", "ana", &[ES256], &[], &[], Check { presence: false, ..proved }, 1).is_err());
        // sign-in: the same rules, and the assertion still verifies at the relying party
        let mut creds = vec![b.credential.clone()];
        let cd2 = client_data_json("webauthn.get", &challenge, ORIGIN);
        let h2 = client_data_hash(&cd2);
        assert!(get_assertion_uv("example.com", &h2, &[], None, &mut creds, need).is_err());
        assert_eq!(creds[0].sign_count, 0, "a refusal does not use the passkey");
        let x = get_assertion_uv("example.com", &h2, &[], None, &mut creds, Check { required: true, ..proved }).unwrap();
        assert!(user_verified(&x.auth_data));
        assert_eq!(verify_assertion(&public_key(&creds[0]).unwrap(), &x.auth_data, cd2.as_bytes(), &x.signature, &challenge, ORIGIN, "example.com", 0), Ok(1));
        let y = get_assertion_uv("example.com", &h2, &[], None, &mut creds, tap).unwrap();
        assert!(!user_verified(&y.auth_data));
    }

    #[test]
    fn the_candidates_of_a_site_are_its_passkeys_newest_first_or_only_the_allowed_ones() {
        let mk = |id: u8, rp: &str, name: &str, created: u64| Credential { id: vec![id], rp_id: rp.into(), user_handle: vec![id], user_name: name.into(), pkcs8: vec![], sign_count: 0, created };
        let creds = vec![mk(1, "a.org", "old", 5), mk(2, "a.org", "new", 9), mk(3, "b.org", "other", 7), mk(4, "a.org", "same time", 9)];
        let names: Vec<String> = candidates("a.org", &[], &creds).into_iter().map(|c| c.1).collect();
        assert_eq!(names, ["new", "same time", "old"], "newest first, ties by name");
        assert_eq!(candidates("a.org", &[vec![1]], &creds).len(), 1);
        assert!(candidates("c.org", &[], &creds).is_empty());
        assert!(!user_verified(&[0u8; 10]), "short data has no flag");
    }

    #[test]
    fn registration_and_sign_in_round_trip_through_the_relying_party_checks() {
        let challenge = [9u8; 32];
        let cd = client_data_json("webauthn.create", &challenge, ORIGIN);
        let made = make_credential("example.com", &client_data_hash(&cd), b"user-1", "ana", &[ES256, -257], &[], &[], true, 10).unwrap();
        let reg = verify_registration(&made.attestation_object, cd.as_bytes(), &challenge, ORIGIN, "example.com").unwrap();
        assert_eq!(reg.credential_id, made.credential.id);
        assert_eq!(reg.public_key, public_key(&made.credential).unwrap());
        let mut creds = vec![made.credential];
        let ch2 = [3u8; 32];
        let cd2 = client_data_json("webauthn.get", &ch2, ORIGIN);
        let a = get_assertion("example.com", &client_data_hash(&cd2), &[], None, &mut creds, true).unwrap();
        assert_eq!(a.user_handle, b"user-1");
        assert_eq!(verify_assertion(&reg.public_key, &a.auth_data, cd2.as_bytes(), &a.signature, &ch2, ORIGIN, "example.com", 0).unwrap(), 1);
        // a replay with the old count, another challenge, origin, relying party or a damaged signature all fail
        assert!(verify_assertion(&reg.public_key, &a.auth_data, cd2.as_bytes(), &a.signature, &ch2, ORIGIN, "example.com", 1).is_err());
        assert!(verify_assertion(&reg.public_key, &a.auth_data, cd2.as_bytes(), &a.signature, &[4u8; 32], ORIGIN, "example.com", 0).is_err());
        assert!(verify_assertion(&reg.public_key, &a.auth_data, cd2.as_bytes(), &a.signature, &ch2, "https://evil.example", "example.com", 0).is_err());
        assert!(verify_assertion(&reg.public_key, &a.auth_data, cd2.as_bytes(), &a.signature, &ch2, ORIGIN, "other.com", 0).is_err());
        let mut bad = a.signature.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(verify_assertion(&reg.public_key, &a.auth_data, cd2.as_bytes(), &bad, &ch2, ORIGIN, "example.com", 0).is_err());
        assert!(verify_registration(&made.attestation_object, cd.as_bytes(), &[1u8; 32], ORIGIN, "example.com").is_err());
        // the count rises on every use
        let a2 = get_assertion("example.com", &client_data_hash(&cd2), &[], None, &mut creds, true).unwrap();
        assert_eq!(u32::from_be_bytes(a2.auth_data[33..37].try_into().unwrap()), 2);
    }

    #[test]
    fn nothing_is_made_or_signed_without_the_persons_confirmation_and_unknown_cases_are_refused() {
        let h = [0u8; 32];
        assert!(make_credential("example.com", &h, b"u", "a", &[ES256], &[], &[], false, 0).is_err());
        assert!(make_credential("example.com", &h, b"u", "a", &[-257], &[], &[], true, 0).is_err(), "RS256 only");
        assert!(make_credential("example.com", &h, b"", "a", &[], &[], &[], true, 0).is_err());
        let mut c = make_credential("example.com", &h, b"u", "a", &[], &[], &[], true, 0).unwrap().credential;
        assert!(make_credential("example.com", &h, b"u", "a", &[], &[c.id.clone()], std::slice::from_ref(&c), true, 0).is_err(), "excluded");
        let mut v = vec![c.clone()];
        assert!(get_assertion("example.com", &h, &[], None, &mut v, false).is_err());
        assert_eq!(v[0].sign_count, 0, "a refused use does not change the count");
        assert!(get_assertion("other.com", &h, &[], None, &mut v, true).is_err());
        assert!(get_assertion("example.com", &h, &[vec![1, 2, 3]], None, &mut v, true).is_err(), "not on the allow list");
        c.pkcs8 = vec![1, 2, 3];
        assert!(get_assertion("example.com", &h, &[], None, &mut [c], true).is_err());
    }

    #[test]
    fn relying_party_ids_follow_the_origin() {
        assert!(rp_id_allowed("example.com", "https://login.example.com").is_ok());
        assert!(rp_id_allowed("example.com", "https://example.com:8443").is_ok());
        assert!(rp_id_allowed("login.example.com", "https://example.com").is_err());
        assert!(rp_id_allowed("ample.com", "https://example.com").is_err());
        assert!(rp_id_allowed("example.com", "http://example.com").is_err());
        assert!(rp_id_allowed("localhost", "http://localhost:8080").is_ok());
        assert!(rp_id_allowed("127.0.0.1", "https://127.0.0.1").is_err());
        assert!(rp_id_allowed("", "https://example.com").is_err());
        assert!(rp_id_allowed("example.com", "https://user@example.com").is_err());
    }

    #[test]
    fn the_authenticator_data_has_the_layout_of_the_specification() {
        let made = make_credential("example.com", &[0; 32], b"u", "a", &[], &[], &[], true, 0).unwrap();
        let ao = cbor::decode_all(&made.attestation_object).unwrap();
        let ad = ao.get_text("authData").unwrap().as_bytes().unwrap();
        assert_eq!(&ad[..32], &Sha256::digest(b"example.com")[..]);
        assert_eq!(ad[32], 0x41);
        assert_eq!(&ad[33..37], &[0, 0, 0, 0]);
        assert_eq!(&ad[37..53], &[0u8; 16], "aaguid");
        assert_eq!(u16::from_be_bytes([ad[53], ad[54]]), 32);
        assert_eq!(ao.get_text("fmt"), Some(&Value::text("none")));
        assert_eq!(ao.get_text("attStmt"), Some(&Value::Map(vec![])));
        // the COSE key is the canonical map a(5): 1:2, 3:-7, -1:1, -2:x, -3:y
        assert_eq!(&ad[55 + 32..55 + 32 + 10], &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20]);
    }
}

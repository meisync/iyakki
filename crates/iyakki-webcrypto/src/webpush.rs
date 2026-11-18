//! Browser Web Push, sender side and a receiver for tests and for the person's own pages.
//!
//! * RFC 8291 message encryption with the `aes128gcm` content coding of RFC 8188, one record.
//! * RFC 8292 VAPID: an ES256 JWT in `Authorization: vapid t=<jwt>, k=<public key>`.
//! * The subscription a browser gives (`endpoint`, `keys.p256dh`, `keys.auth`) is checked before it is kept.
//!
//! Nothing here sends anything: [`build_request`] makes the method, address, headers and body and the caller sends them through the
//! core's network path.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_128_GCM};
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, UnparsedPublicKey as SigPub, ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING};

use crate::{b64, kdf, p256};

/// What a push service accepts: 4096 bytes of body, of which the header takes 86 and the tag and the delimiter 17.
pub const MAX_PLAIN: usize = 4096 - 86 - 17;
const RS: u32 = 4096;

fn key_info(ua_public: &[u8], as_public: &[u8]) -> Vec<u8> {
    let mut v = b"WebPush: info\0".to_vec();
    v.extend_from_slice(ua_public);
    v.extend_from_slice(as_public);
    v
}

fn cek_nonce(ecdh: &[u8], auth: &[u8], ua_public: &[u8], as_public: &[u8], salt: &[u8]) -> ([u8; 16], [u8; 12]) {
    let ikm = kdf::hkdf(auth, ecdh, &key_info(ua_public, as_public), 32);
    let mut cek = [0u8; 16];
    cek.copy_from_slice(&kdf::hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16));
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&kdf::hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12));
    (cek, nonce)
}

fn seal(plain: &[u8], ecdh: &[u8], auth: &[u8], ua_public: &[u8], as_public: &[u8], salt: &[u8; 16]) -> Result<Vec<u8>, String> {
    if plain.len() > MAX_PLAIN {
        return Err(format!("a push message may carry {MAX_PLAIN} bytes at most"));
    }
    let (cek, nonce) = cek_nonce(ecdh, auth, ua_public, as_public, salt);
    let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| "key")?);
    let mut data = plain.to_vec();
    data.push(2);
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data).map_err(|_| "seal")?;
    let mut body = Vec::with_capacity(86 + data.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RS.to_be_bytes());
    body.push(65);
    body.extend_from_slice(as_public);
    body.extend_from_slice(&data);
    Ok(body)
}

fn check_subscription_keys(ua_public: &[u8], auth: &[u8]) -> Result<(), String> {
    if auth.len() != 16 {
        return Err("the auth secret must be 16 bytes".into());
    }
    // a throw-away agreement checks the point is on the curve
    p256::ecdh(&[1u8; 32], ua_public).map(|_| ())
}

/// Encrypts one message for a browser: a fresh sender key and salt every time.
pub fn encrypt(plain: &[u8], ua_public: &[u8], auth: &[u8]) -> Result<Vec<u8>, String> {
    check_subscription_keys(ua_public, auth)?;
    let rng = SystemRandom::new();
    let sk = EphemeralPrivateKey::generate(&ECDH_P256, &rng).map_err(|_| "no random numbers")?;
    let as_public = sk.compute_public_key().map_err(|_| "key")?.as_ref().to_vec();
    let mut salt = [0u8; 16];
    crate::random(&mut salt)?;
    let ecdh = agree_ephemeral(sk, &UnparsedPublicKey::new(&ECDH_P256, ua_public), |z| z.to_vec()).map_err(|_| "the browser's key is not usable")?;
    seal(plain, &ecdh, auth, ua_public, &as_public, &salt)
}

/// The same with the sender's private key and the salt given: for the RFC 8291 test vector.
pub fn encrypt_with(plain: &[u8], ua_public: &[u8], auth: &[u8], as_private: &[u8; 32], salt: &[u8; 16]) -> Result<Vec<u8>, String> {
    check_subscription_keys(ua_public, auth)?;
    let as_public = p256::public_from_private(as_private)?;
    let ecdh = p256::ecdh(as_private, ua_public)?;
    seal(plain, &ecdh, auth, ua_public, &as_public, salt)
}

/// The browser's side: opens a body with the private key and auth secret of the subscription.
pub fn decrypt(body: &[u8], ua_private: &[u8; 32], auth: &[u8]) -> Result<Vec<u8>, String> {
    if body.len() < 86 + 17 {
        return Err("the body is too short".into());
    }
    let salt = &body[..16];
    let rs = u32::from_be_bytes([body[16], body[17], body[18], body[19]]);
    let idlen = body[20] as usize;
    if idlen != 65 || rs < 18 || body.len() < 21 + idlen || body.len() - 21 - idlen > rs as usize {
        return Err("only one record with a 65 byte key id is supported".into());
    }
    let as_public = &body[21..21 + idlen];
    let ua_public = p256::public_from_private(ua_private)?;
    let ecdh = p256::ecdh(ua_private, as_public)?;
    let (cek, nonce) = cek_nonce(&ecdh, auth, &ua_public, as_public, salt);
    let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| "key")?);
    let mut data = body[21 + idlen..].to_vec();
    let plain = key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut data).map_err(|_| "the message does not open (wrong key, auth secret or damaged)")?;
    let end = plain.iter().rposition(|&b| b != 0).ok_or("no delimiter")?;
    if plain[end] != 2 {
        return Err("the record is not the last one".into());
    }
    Ok(plain[..end].to_vec())
}

/// The application server's VAPID key (P-256, kept as PKCS#8 in the vault).
pub struct Vapid {
    pair: EcdsaKeyPair,
    pkcs8: Vec<u8>,
}

impl Vapid {
    pub fn generate() -> Result<Vapid, String> {
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new()).map_err(|_| "no random numbers")?;
        Vapid::from_pkcs8(doc.as_ref())
    }
    pub fn from_pkcs8(b: &[u8]) -> Result<Vapid, String> {
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, b, &SystemRandom::new()).map_err(|_| "the VAPID key is damaged")?;
        Ok(Vapid { pair, pkcs8: b.to_vec() })
    }
    pub fn to_pkcs8(&self) -> &[u8] {
        &self.pkcs8
    }
    /// The 65 byte public key a browser needs as `applicationServerKey`.
    pub fn public_key(&self) -> Vec<u8> {
        self.pair.public_key().as_ref().to_vec()
    }
    pub fn public_key_b64(&self) -> String {
        b64::url(&self.public_key())
    }
    /// `vapid t=<jwt>, k=<key>` for a push service at `endpoint`; `exp` is a Unix time at most 24 hours ahead.
    pub fn authorization(&self, endpoint: &str, exp: u64, subject: &str) -> Result<String, String> {
        let aud = origin(endpoint)?;
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let signing = format!("{}.{}", b64::url(br#"{"typ":"JWT","alg":"ES256"}"#), b64::url(format!(r#"{{"aud":"{}","exp":{},"sub":"{}"}}"#, esc(&aud), exp, esc(subject)).as_bytes()));
        let sig = self.pair.sign(&SystemRandom::new(), signing.as_bytes()).map_err(|_| "signing failed")?;
        Ok(format!("vapid t={}.{}, k={}", signing, b64::url(sig.as_ref()), self.public_key_b64()))
    }
}

/// `https://host[:port]` of an endpoint, which is also the JWT audience.
pub fn origin(endpoint: &str) -> Result<String, String> {
    let rest = endpoint.strip_prefix("https://").ok_or("a push endpoint must be https")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') || host.bytes().any(|b| b <= b' ' || b == b'\\') {
        return Err("bad push endpoint".into());
    }
    Ok(format!("https://{host}"))
}

/// The claims of a checked VAPID header.
#[derive(Debug, PartialEq)]
pub struct VapidClaims {
    pub aud: String,
    pub sub: String,
    pub exp: u64,
    pub key: Vec<u8>,
}

fn json_str(j: &str, name: &str) -> Option<String> {
    let at = j.find(&format!("\"{name}\":\""))? + name.len() + 4;
    let mut o = String::new();
    let mut it = j[at..].chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(o),
            '\\' => o.push(it.next()?),
            c => o.push(c),
        }
    }
    None
}
fn json_num(j: &str, name: &str) -> Option<u64> {
    let at = j.find(&format!("\"{name}\":"))? + name.len() + 3;
    j[at..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}

/// What a push service does with the header: checks the signature with the key it names, the audience, and the time.
pub fn verify_vapid(header: &str, endpoint: &str, now: u64) -> Result<VapidClaims, String> {
    let rest = header.strip_prefix("vapid ").ok_or("not a vapid header")?;
    let (mut t, mut k) = (None, None);
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("t=") {
            t = Some(v);
        } else if let Some(v) = part.strip_prefix("k=") {
            k = Some(v);
        }
    }
    let (jwt, key) = (t.ok_or("no t")?, b64::decode(k.ok_or("no k")?)?);
    let mut it = jwt.split('.');
    let (h, c, s) = (it.next().ok_or("jwt")?, it.next().ok_or("jwt")?, it.next().ok_or("jwt")?);
    if it.next().is_some() {
        return Err("jwt".into());
    }
    let header_json = String::from_utf8(b64::decode(h)?).map_err(|_| "jwt")?;
    if json_str(&header_json, "alg").as_deref() != Some("ES256") {
        return Err("the JWT must be ES256".into());
    }
    SigPub::new(&ECDSA_P256_SHA256_FIXED, &key).verify(format!("{h}.{c}").as_bytes(), &b64::decode(s)?).map_err(|_| "the VAPID signature is wrong")?;
    let claims = String::from_utf8(b64::decode(c)?).map_err(|_| "jwt")?;
    let aud = json_str(&claims, "aud").ok_or("no aud")?;
    let exp = json_num(&claims, "exp").ok_or("no exp")?;
    if aud != origin(endpoint)? {
        return Err("the audience is not this push service".into());
    }
    if exp <= now {
        return Err("the VAPID token has expired".into());
    }
    if exp > now + 24 * 3600 {
        return Err("the VAPID token lives longer than 24 hours".into());
    }
    Ok(VapidClaims { aud, sub: json_str(&claims, "sub").unwrap_or_default(), exp, key })
}

/// A browser subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Subscription {
    pub endpoint: String,
    pub p256dh: Vec<u8>,
    pub auth: Vec<u8>,
}

impl Subscription {
    /// Checks the endpoint (https, no credentials, not too long) and both keys (a point on the curve, 16 bytes).
    pub fn new(endpoint: &str, p256dh_b64: &str, auth_b64: &str) -> Result<Subscription, String> {
        if endpoint.len() > 2048 {
            return Err("endpoint too long".into());
        }
        origin(endpoint)?;
        let (p256dh, auth) = (b64::decode(p256dh_b64)?, b64::decode(auth_b64)?);
        check_subscription_keys(&p256dh, &auth)?;
        Ok(Subscription { endpoint: endpoint.to_string(), p256dh, auth })
    }
}

/// The request to make: always a POST to `url`.
#[derive(Debug)]
pub struct PushRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Urgency of RFC 8030.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Urgency {
    VeryLow,
    Low,
    Normal,
    High,
}

pub fn build_request(sub: &Subscription, vapid: &Vapid, payload: &[u8], ttl: u32, urgency: Urgency, now: u64, contact: &str) -> Result<PushRequest, String> {
    let body = encrypt(payload, &sub.p256dh, &sub.auth)?;
    let urgency = match urgency {
        Urgency::VeryLow => "very-low",
        Urgency::Low => "low",
        Urgency::Normal => "normal",
        Urgency::High => "high",
    };
    Ok(PushRequest {
        url: sub.endpoint.clone(),
        headers: vec![
            ("Content-Encoding".into(), "aes128gcm".into()),
            ("Content-Type".into(), "application/octet-stream".into()),
            ("Content-Length".into(), body.len().to_string()),
            ("TTL".into(), ttl.to_string()),
            ("Urgency".into(), urgency.into()),
            ("Authorization".into(), vapid.authorization(&sub.endpoint, now + 12 * 3600, contact)?),
        ],
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn d32(s: &str) -> [u8; 32] {
        b64::decode(s).unwrap().try_into().unwrap()
    }
    const UA_PUB: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const UA_PRIV: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const AS_PRIV: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
    const TEXT: &[u8] = b"When I grow up, I want to be a watermelon";

    #[test]
    fn rfc8291_appendix_a_the_sender_makes_exactly_the_published_body() {
        let salt: [u8; 16] = b64::decode(SALT).unwrap().try_into().unwrap();
        let body = encrypt_with(TEXT, &b64::decode(UA_PUB).unwrap(), &b64::decode(AUTH).unwrap(), &d32(AS_PRIV), &salt).unwrap();
        assert_eq!(b64::url(&body), BODY);
        assert_eq!(b64::url(&p256::public_from_private(&d32(UA_PRIV)).unwrap()), UA_PUB);
    }
    #[test]
    fn rfc8291_appendix_a_the_receiver_opens_the_published_body() {
        assert_eq!(decrypt(&b64::decode(BODY).unwrap(), &d32(UA_PRIV), &b64::decode(AUTH).unwrap()).unwrap(), TEXT);
    }
    #[test]
    fn a_fresh_sender_key_round_trips_and_damage_or_wrong_secrets_are_refused() {
        let ua = d32(UA_PRIV);
        let (public, auth) = (p256::public_from_private(&ua).unwrap(), b64::decode(AUTH).unwrap());
        let a = encrypt(b"hello", &public, &auth).unwrap();
        let b = encrypt(b"hello", &public, &auth).unwrap();
        assert_ne!(a, b, "salt and sender key are fresh");
        assert_eq!(decrypt(&a, &ua, &auth).unwrap(), b"hello");
        let mut bad = a.clone();
        let n = bad.len();
        bad[n - 3] ^= 1;
        assert!(decrypt(&bad, &ua, &auth).is_err());
        assert!(decrypt(&a, &ua, &[0u8; 16]).is_err());
        assert!(decrypt(&a, &[9u8; 32], &auth).is_err());
        assert!(encrypt(&vec![0u8; MAX_PLAIN + 1], &public, &auth).is_err());
        assert_eq!(decrypt(&encrypt(&vec![7u8; MAX_PLAIN], &public, &auth).unwrap(), &ua, &auth).unwrap().len(), MAX_PLAIN);
    }
    #[test]
    fn vapid_tokens_verify_and_wrong_audience_expiry_lifetime_and_forgery_are_refused() {
        let v = Vapid::generate().unwrap();
        let a = v.authorization("https://push.example.net/x", 1000 + 3600, "mailto:a@b.example").unwrap();
        assert_eq!(verify_vapid(&a, "https://push.example.net/y", 1000).unwrap().key, v.public_key());
        assert!(verify_vapid(&a, "https://other.example/y", 1000).is_err(), "audience");
        assert!(verify_vapid(&a, "https://push.example.net/y", 1000 + 3600 + 1).is_err());
        assert!(verify_vapid(&v.authorization("https://push.example.net/x", 1000 + 90_000, "s").unwrap(), "https://push.example.net/", 1000).is_err(), "over 24 h");
        let mut forged = a.clone().into_bytes();
        let at = forged.iter().position(|&b| b == b',').unwrap() - 3;
        forged[at] = if forged[at] == b'A' { b'B' } else { b'A' };
        assert!(verify_vapid(&String::from_utf8(forged).unwrap(), "https://push.example.net/y", 1000).is_err());
        let again = Vapid::from_pkcs8(v.to_pkcs8()).unwrap();
        assert_eq!(again.public_key(), v.public_key());
    }
    #[test]
    fn subscriptions_are_checked_before_they_are_kept() {
        assert!(Subscription::new("https://push.example.net/abc", UA_PUB, AUTH).is_ok());
        assert!(Subscription::new("http://push.example.net/abc", UA_PUB, AUTH).is_err());
        assert!(Subscription::new("https://user@push.example.net/abc", UA_PUB, AUTH).is_err());
        assert!(Subscription::new("https://push.example.net/abc", UA_PUB, "AAAA").is_err());
        assert!(Subscription::new("https://push.example.net/abc", "BAAA", AUTH).is_err());
        assert!(Subscription::new(&format!("https://push.example.net/{}", "a".repeat(3000)), UA_PUB, AUTH).is_err());
    }

    /// A push service on the loopback: checks the VAPID header, answers 201 and hands the body to the receiver.
    #[test]
    fn a_loopback_push_service_gets_a_request_it_accepts_and_the_browser_side_opens_the_message() {
        let ua = d32(UA_PRIV);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head_end, len) = loop {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                    let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length: ")).unwrap().trim().parse().unwrap();
                    break (p + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            s.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n").unwrap();
            (head, buf[head_end..head_end + len].to_vec())
        });
        let endpoint = format!("https://127.0.0.1:{port}/push/1");
        let sub = Subscription::new(&endpoint, UA_PUB, AUTH).unwrap();
        let vapid = Vapid::generate().unwrap();
        let req = build_request(&sub, &vapid, br#"{"title":"Backup done"}"#, 3600, Urgency::Normal, 5000, "mailto:ops@example.invalid").unwrap();
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut wire = format!("POST /push/1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n").into_bytes();
        for (k, v) in &req.headers {
            wire.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
        wire.extend_from_slice(b"\r\n");
        wire.extend_from_slice(&req.body);
        c.write_all(&wire).unwrap();
        let mut answer = String::new();
        c.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 201"));
        let (head, body) = server.join().unwrap();
        let auth = head.lines().find_map(|l| l.strip_prefix("Authorization: ")).unwrap();
        assert_eq!(verify_vapid(auth, &endpoint, 5000).unwrap().key, vapid.public_key());
        assert!(head.contains("Content-Encoding: aes128gcm") && head.contains("TTL: 3600"));
        assert!(!body.windows(5).any(|w| w == b"Backu"), "the payload is not in the clear");
        assert_eq!(decrypt(&body, &ua, &b64::decode(AUTH).unwrap()).unwrap(), br#"{"title":"Backup done"}"#);
    }
}

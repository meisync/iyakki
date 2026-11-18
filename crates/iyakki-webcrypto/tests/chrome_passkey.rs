//! Passkeys against independent implementations (test only, nothing shipped):
//! * a real Chrome with a WebAuthn virtual authenticator makes a registration and a sign-in that IYAKKI's relying-party code accepts;
//! * Python's `cryptography` (OpenSSL) reads what IYAKKI's authenticator makes (with a small CBOR reader written in the script) and
//!   checks the signature.
//! Both skip with a line on stderr when Chrome or python3 or the module is missing.

use iyakki_webcrypto::webauthn::{self as wa, ES256};
use iyakki_webcrypto::{b64, cbor};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn chromes_virtual_authenticator_registers_and_signs_in_and_our_relying_party_checks_accept_it() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/browsertest/webauthn_chrome.py");
    let out = Command::new("python3").arg(&script).output().expect("python3");
    if out.status.code() == Some(3) {
        eprintln!("skipped: no Chrome here");
        return;
    }
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let kv: HashMap<String, String> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let d = |k: &str| b64::decode(&kv[k]).unwrap();
    let origin = &kv["origin"];
    let reg = wa::verify_registration(&d("create_att"), &d("create_cd"), &[0x11; 32], origin, "localhost").unwrap();
    assert_eq!(reg.credential_id, d("create_id"));
    let count = wa::verify_assertion(&reg.public_key, &d("get_ad"), &d("get_cd"), &d("get_sig"), &[0x22; 32], origin, "localhost", reg.sign_count).unwrap();
    assert!(count >= reg.sign_count);
    // a signature of Chrome's with another origin or challenge is refused by the same code
    assert!(wa::verify_assertion(&reg.public_key, &d("get_ad"), &d("get_cd"), &d("get_sig"), &[0x23; 32], origin, "localhost", 0).is_err());
    assert!(wa::verify_assertion(&reg.public_key, &d("get_ad"), &d("get_cd"), &d("get_sig"), &[0x22; 32], "http://localhost:1", "localhost", 0).is_err());
}

const PY: &str = r#"
import sys, json, base64
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives import hashes
def d(s): return base64.urlsafe_b64decode(s + '=' * (-len(s) % 4))
def cbor(b, i=0):
    t, n = b[i] >> 5, b[i] & 31; i += 1
    if n >= 24:
        ln = 1 << (n - 24); n = int.from_bytes(b[i:i+ln], 'big'); i += ln
    if t == 0: return n, i
    if t == 1: return -1 - n, i
    if t == 2: return b[i:i+n], i + n
    if t == 3: return b[i:i+n].decode(), i + n
    if t == 5:
        m = {}
        for _ in range(n):
            k, i = cbor(b, i); v, i = cbor(b, i); m[k] = v
        return m, i
    raise Exception('type %d' % t)
req = json.loads(sys.stdin.read())
ao, used = cbor(d(req['att'])); assert used == len(d(req['att']))
assert ao['fmt'] == 'none' and ao['attStmt'] == {}
ad = ao['authData']
import hashlib
assert ad[:32] == hashlib.sha256(b'example.com').digest() and ad[32] == 0x41
idlen = int.from_bytes(ad[53:55], 'big'); cid = ad[55:55+idlen]
key, used = cbor(ad, 55 + idlen); assert used == len(ad)
assert key[1] == 2 and key[3] == -7 and key[-1] == 1
pub = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), b'\x04' + key[-2] + key[-3])
a = d(req['ad']); assert a[:32] == hashlib.sha256(b'example.com').digest() and a[32] == 0x01
cdh = hashlib.sha256(d(req['cd'])).digest()
pub.verify(d(req['sig']), a + cdh, ec.ECDSA(hashes.SHA256()))
print('ok', base64.urlsafe_b64encode(cid).decode() == req['cid'])
"#;

#[test]
fn openssl_verifies_the_registration_and_the_sign_in_our_authenticator_makes() {
    let origin = "https://login.example.com";
    let cd = wa::client_data_json("webauthn.create", &[5u8; 32], origin);
    let made = wa::make_credential("example.com", &wa::client_data_hash(&cd), b"u1", "ana", &[ES256], &[], &[], true, 1).unwrap();
    // our own CBOR reader and the script's agree on the object
    assert!(cbor::decode_all(&made.attestation_object).is_ok());
    let mut creds = vec![made.credential.clone()];
    let cd2 = wa::client_data_json("webauthn.get", &[6u8; 32], origin);
    let a = wa::get_assertion("example.com", &wa::client_data_hash(&cd2), &[], None, &mut creds, true).unwrap();
    let req = format!(r#"{{"att":"{}","ad":"{}","sig":"{}","cd":"{}","cid":"{}"}}"#, b64::url(&made.attestation_object), b64::url(&a.auth_data), b64::url(&a.signature), b64::url(cd2.as_bytes()), b64::std(&made.credential.id).replace('+', "-").replace('/', "_"));
    let mut c = match Command::new("python3").args(["-I", "-c", PY]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(_) => return,
    };
    c.stdin.take().unwrap().write_all(req.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    if err.contains("ModuleNotFoundError") {
        eprintln!("skipped: {err}");
        return;
    }
    assert!(o.status.success(), "{err}");
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("ok True"), "{}", String::from_utf8_lossy(&o.stdout));
}

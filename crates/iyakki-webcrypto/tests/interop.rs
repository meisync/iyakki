//! Checks against independent implementations: Python's `cryptography` (OpenSSL) and PyJWT. Skipped, with a line on stderr, when
//! python3 or the modules are missing. Used to TEST only; nothing of it is shipped.

use iyakki_webcrypto::{b64, p256, webpush};
use std::io::Write;
use std::process::{Command, Stdio};

fn python(script: &str, input: &str) -> Option<String> {
    let mut c = Command::new("python3").args(["-I", "-c", script]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    c.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let o = c.wait_with_output().ok()?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr);
        if e.contains("ModuleNotFoundError") {
            eprintln!("skipped: {e}");
            return None;
        }
        panic!("python failed: {e}");
    }
    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

const PUSH_PY: &str = r#"
import sys, base64, json
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
def d(s): return base64.urlsafe_b64decode(s + '=' * (-len(s) % 4))
def e(b): return base64.urlsafe_b64encode(b).rstrip(b'=').decode()
req = json.loads(sys.stdin.read())
ua_priv = ec.derive_private_key(int.from_bytes(d(req['ua_priv']), 'big'), ec.SECP256R1())
ua_pub = ua_priv.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
auth = d(req['auth'])
def derive(secret, ua_pub, as_pub, salt):
    ikm = HKDF(hashes.SHA256(), 32, auth, b'WebPush: info\0' + ua_pub + as_pub).derive(secret)
    cek = HKDF(hashes.SHA256(), 16, salt, b'Content-Encoding: aes128gcm\0').derive(ikm)
    nonce = HKDF(hashes.SHA256(), 12, salt, b'Content-Encoding: nonce\0').derive(ikm)
    return cek, nonce
# 1. open the body made by the Rust sender
body = d(req['body'])
salt, rs, idlen = body[:16], int.from_bytes(body[16:20], 'big'), body[20]
as_pub = body[21:21 + idlen]
secret = ua_priv.exchange(ec.ECDH(), ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), as_pub))
cek, nonce = derive(secret, ua_pub, as_pub, salt)
plain = AESGCM(cek).decrypt(nonce, body[21 + idlen:], None)
assert plain.endswith(b'\x02'), plain
# 2. make a body the way a push library would, for the Rust receiver
sp = ec.generate_private_key(ec.SECP256R1())
sp_pub = sp.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
import os
salt2 = os.urandom(16)
cek, nonce = derive(sp.exchange(ec.ECDH(), ua_priv.public_key()), ua_pub, sp_pub, salt2)
out = salt2 + (4096).to_bytes(4, 'big') + bytes([65]) + sp_pub + AESGCM(cek).encrypt(nonce, b'from python' + b'\x02', None)
print(json.dumps({'plain': plain[:-1].decode(), 'body': e(out)}))
"#;

#[test]
fn openssl_opens_our_web_push_body_and_we_open_one_made_by_python() {
    let mut ua = [0u8; 32];
    iyakki_webcrypto::random(&mut ua).unwrap();
    ua[0] &= 0x7f;
    let public = p256::public_from_private(&ua).unwrap();
    let auth = [0x42u8; 16];
    let body = webpush::encrypt(b"a message from IYAKKI", &public, &auth).unwrap();
    let req = format!(r#"{{"ua_priv":"{}","auth":"{}","body":"{}"}}"#, b64::url(&ua), b64::url(&auth), b64::url(&body));
    let Some(out) = python(PUSH_PY, &req) else { return };
    assert!(out.contains(r#""plain": "a message from IYAKKI""#), "{out}");
    let at = out.find(r#""body": ""#).unwrap() + 9;
    let theirs = b64::decode(&out[at..out.len() - 2]).unwrap();
    assert_eq!(webpush::decrypt(&theirs, &ua, &auth).unwrap(), b"from python");
}

const VAPID_PY: &str = r#"
import sys, base64, json, jwt
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature
def d(s): return base64.urlsafe_b64decode(s + '=' * (-len(s) % 4))
req = json.loads(sys.stdin.read())
hdr = req['header']
t = hdr.split('t=')[1].split(',')[0]
k = hdr.split('k=')[1].strip()
pub = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), d(k))
h, c, s = t.split('.')
sig = d(s)
assert len(sig) == 64
pub.verify(encode_dss_signature(int.from_bytes(sig[:32], 'big'), int.from_bytes(sig[32:], 'big')), (h + '.' + c).encode(), ec.ECDSA(hashes.SHA256()))
claims = jwt.decode(t, pub, algorithms=['ES256'], audience=req['aud'], options={'verify_exp': False})
print(claims['sub'], claims['exp'])
"#;

#[test]
fn openssl_and_pyjwt_accept_our_vapid_token() {
    let v = webpush::Vapid::generate().unwrap();
    let a = v.authorization("https://push.example.net/abc", 1_900_000_000, "mailto:ops@example.invalid").unwrap();
    let req = format!(r#"{{"header":"{a}","aud":"https://push.example.net"}}"#);
    let Some(out) = python(VAPID_PY, &req) else { return };
    assert_eq!(out, "mailto:ops@example.invalid 1900000000");
}

const ECDH_PY: &str = r#"
import sys, json
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives import serialization
for line in sys.stdin.read().split():
    a, b = line.split(':')
    pa = ec.derive_private_key(int(a, 16), ec.SECP256R1())
    pb = ec.derive_private_key(int(b, 16), ec.SECP256R1())
    pub = pb.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
    print(pa.exchange(ec.ECDH(), pb.public_key()).hex(), pub.hex())
"#;

#[test]
fn our_p256_key_agreement_equals_openssls_for_random_keys() {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut pairs = Vec::new();
    let mut input = String::new();
    for _ in 0..8 {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        iyakki_webcrypto::random(&mut a).unwrap();
        iyakki_webcrypto::random(&mut b).unwrap();
        a[0] &= 0x7f;
        b[0] &= 0x7f;
        input.push_str(&format!("{}:{}\n", hex(&a), hex(&b)));
        pairs.push((a, b));
    }
    let Some(out) = python(ECDH_PY, &input) else { return };
    for ((a, b), line) in pairs.iter().zip(out.lines()) {
        let (z, pubb) = line.split_once(' ').unwrap();
        assert_eq!(hex(&p256::ecdh(a, &p256::public_from_private(b).unwrap()).unwrap()), z);
        assert_eq!(hex(&p256::public_from_private(b).unwrap()), pubb);
    }
}

//! Olm and Megolm against a second implementation: Python with `cryptography` (OpenSSL), written separately from the same public
//! specification (no Olm or Megolm library exists here to compare with). It plays the other device in a conversation with ours, and
//! the other way round for a Megolm session. Test only. Skipped with a line on stderr when python3 or the module is missing.

use iyakki_webcrypto::megolm::{InboundGroup, OutboundGroup};
use iyakki_webcrypto::olm::{key_from_b64, Account, Session};
use iyakki_webcrypto::b64;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

const PY: &str = r#"
import sys, json, base64, hmac, hashlib, os
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.padding import PKCS7
RAW = dict(encoding=serialization.Encoding.Raw, format=serialization.PublicFormat.Raw)
def d(s): return base64.b64decode(s + '=' * (-len(s) % 4))
def e(b): return base64.b64encode(b).decode().rstrip('=')
def hk(salt, ikm, info, n): return HKDF(hashes.SHA256(), n, salt if salt else None, info).derive(ikm)
def H(k, m): return hmac.new(k, m, hashlib.sha256).digest()
def cbc_enc(k, iv, p):
    pad = PKCS7(128).padder(); x = pad.update(p) + pad.finalize()
    c = Cipher(algorithms.AES(k), modes.CBC(iv)).encryptor(); return c.update(x) + c.finalize()
def cbc_dec(k, iv, ct):
    c = Cipher(algorithms.AES(k), modes.CBC(iv)).decryptor(); x = c.update(ct) + c.finalize()
    u = PKCS7(128).unpadder(); return u.update(x) + u.finalize()
def pub(sk): return sk.public_key().public_bytes(**RAW)
def vi(n):
    o = b''
    while n >= 0x80: o += bytes([n & 0x7f | 0x80]); n >>= 7
    return o + bytes([n])
def rv(b, i):
    n = s = 0
    while True:
        x = b[i]; i += 1; n |= (x & 0x7f) << s; s += 7
        if not x & 0x80: return n, i
# ---------------- Megolm
def mg_step(R, c):
    c += 1
    h = 0; mask = 0x00FFFFFF
    while h < 4:
        if not (c & mask): break
        h += 1; mask >>= 8
    R = list(R)
    for i in range(3, h - 1, -1): R[i] = H(R[h], bytes([i]))
    return R, c
def mg_keys(R):
    k = hk(b'', b''.join(R), b'MEGOLM_KEYS', 80); return k[:32], k[32:64], k[64:]
def mg_decrypt(session_key, msg):
    b = d(session_key); assert b[0] == 2
    idx0 = int.from_bytes(b[1:5], 'big'); R = [b[5 + 32*i: 37 + 32*i] for i in range(4)]; pk = b[133:165]
    Ed25519PublicKey.from_public_bytes(pk).verify(b[165:], b[:165])
    m = d(msg); assert m[0] == 3
    Ed25519PublicKey.from_public_bytes(pk).verify(m[-64:], m[:-64])
    body = m[:-72]; mac = m[-72:-64]
    assert body[1] == 0x08; idx, i = rv(body, 2); assert body[i] == 0x12; n, i = rv(body, i + 1); ct = body[i:i+n]; assert i + n == len(body)
    c = idx0
    while c < idx: R, c = mg_step(R, c)
    a, mk, iv = mg_keys(R)
    assert hmac.compare_digest(H(mk, body)[:8], mac)
    return cbc_dec(a, iv, ct), idx
def mg_session(text, skip):
    R = [os.urandom(32) for _ in range(4)]; c = 0
    sk = Ed25519PrivateKey.generate(); pk = pub(sk)
    key = b'\x02' + c.to_bytes(4, 'big') + b''.join(R) + pk
    key += sk.sign(key)
    for _ in range(skip): R, c = mg_step(R, c)
    a, mk, iv = mg_keys(R); ct = cbc_enc(a, iv, text)
    m = b'\x03\x08' + vi(c) + b'\x12' + vi(len(ct)) + ct; m += H(mk, m)[:8]; m += sk.sign(m)
    return e(key), e(m)
# ---------------- Olm (the device that starts the conversation)
def olm_keys(K):
    k = hk(b'', K, b'OLM_KEYS', 80); return k[:32], k[32:64], k[64:]
def olm_msg(chain, ratchet_pub, index, text):
    key = H(chain, b'\x01'); a, mk, iv = olm_keys(key); ct = cbc_enc(a, iv, text)
    m = b'\x03\x0a\x20' + ratchet_pub + b'\x10' + vi(index) + b'\x22' + vi(len(ct)) + ct
    return m + H(mk, m)[:8], H(chain, b'\x02')
def olm_open(chain_for_index, msg):
    assert msg[0] == 3; body, mac = msg[:-8], msg[-8:]
    assert body[1] == 0x0a and body[2] == 32; rk = body[3:35]; assert body[35] == 0x10
    idx, i = rv(body, 36); assert body[i] == 0x22; n, i = rv(body, i + 1); ct = body[i:i+n]
    chain = chain_for_index(rk)
    for _ in range(idx): chain = H(chain, b'\x02')
    a, mk, iv = olm_keys(H(chain, b'\x01'))
    assert hmac.compare_digest(H(mk, body)[:8], mac)
    return cbc_dec(a, iv, ct), rk
def main():
    req = json.loads(sys.stdin.readline())
    out = {}
    # 1. Megolm both ways
    pt, idx = mg_decrypt(req['mg_key'], req['mg_msg']); out['mg_plain'] = pt.decode(); out['mg_index'] = idx
    out['mg_key2'], out['mg_msg2'] = mg_session(b'from python megolm', 5)
    # 2. Olm: python is Alice
    bob_id, bob_otk = d(req['bob_identity']), d(req['bob_otk'])
    ia, ea, ra = X25519PrivateKey.generate(), X25519PrivateKey.generate(), X25519PrivateKey.generate()
    xb = lambda sk, p: sk.exchange(X25519PublicKey.from_public_bytes(p))
    S = xb(ia, bob_otk) + xb(ea, bob_id) + xb(ea, bob_otk)
    r = hk(b'', S, b'OLM_ROOT', 64); root, chain = r[:32], r[32:]
    m, chain = olm_msg(chain, pub(ra), 0, b'hello from python olm')
    pk = b'\x03\x0a\x20' + bob_otk + b'\x12\x20' + pub(ea) + b'\x1a\x20' + pub(ia) + b'\x22' + vi(len(m)) + m
    out['olm_prekey'] = e(pk)
    sys.stdout.write(json.dumps(out) + '\n'); sys.stdout.flush()
    # 3. Bob's (our) answer arrives: ratchet step as Alice
    rep = json.loads(sys.stdin.readline())
    reply = d(rep['reply'])
    def chain_for(rk):
        nonlocal root
        r = hk(root, xb(ra, rk), b'OLM_RATCHET', 64); root = r[:32]; return r[32:]
    plain, bob_rk = olm_open(chain_for, reply)
    out2 = {'olm_plain': plain.decode()}
    # python now sends a normal message on a new ratchet key
    nr = X25519PrivateKey.generate()
    r = hk(root, xb(nr, bob_rk), b'OLM_RATCHET', 64); root2, chain2 = r[:32], r[32:]
    m2, _ = olm_msg(chain2, pub(nr), 0, b'second from python')
    out2['olm_second'] = e(m2)
    sys.stdout.write(json.dumps(out2) + '\n'); sys.stdout.flush()
main()
"#;

fn field(line: &str, key: &str) -> String {
    let at = line.find(&format!("\"{key}\": \"")).unwrap_or_else(|| panic!("{key} in {line}")) + key.len() + 5;
    line[at..].split('"').next().unwrap().to_string()
}

#[test]
fn a_second_implementation_reads_our_megolm_and_olm_messages_and_we_read_its() {
    let mut child = match Command::new("python3").args(["-I", "-c", PY]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());

    let mut group = OutboundGroup::new().unwrap();
    group.encrypt(b"burn one").unwrap();
    let key = group.session_key();
    for _ in 0..299 {
        group.encrypt(b"x").unwrap();
    }
    let msg = group.encrypt("from rust megolm".as_bytes()).unwrap();
    let mut bob = Account::new().unwrap();
    bob.generate_one_time_keys(1).unwrap();
    let otk = bob.unpublished_one_time_keys()[0].1.clone();
    writeln!(stdin, r#"{{"mg_key":"{key}","mg_msg":"{msg}","bob_identity":"{}","bob_otk":"{otk}"}}"#, bob.curve25519_key()).unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    if out.read_line(&mut line).unwrap() == 0 {
        let mut e = String::new();
        std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut e).unwrap();
        if e.contains("ModuleNotFoundError") {
            eprintln!("skipped: {e}");
            return;
        }
        panic!("python failed: {e}");
    }
    // Megolm: python read ours (index 300 after a jump over a byte boundary), and we read its session
    assert_eq!(field(&line, "mg_plain"), "from rust megolm");
    assert!(line.contains("\"mg_index\": 300"));
    let mut inb = InboundGroup::from_session_key(&field(&line, "mg_key2")).unwrap();
    assert_eq!(inb.decrypt(&field(&line, "mg_msg2")).unwrap(), (b"from python megolm".to_vec(), 5));
    // Olm: python started the session; we open its pre-key message and answer; python answers back
    let prekey = b64::decode(&field(&line, "olm_prekey")).unwrap();
    let (mut session, first) = Session::inbound(&mut bob, &prekey).unwrap();
    assert_eq!(first, b"hello from python olm");
    let reply = session.encrypt(b"hello from rust olm").unwrap();
    assert_eq!(reply.kind, 1);
    writeln!(stdin, r#"{{"reply":"{}"}}"#, reply.body_b64()).unwrap();
    stdin.flush().unwrap();
    let mut line2 = String::new();
    out.read_line(&mut line2).unwrap();
    assert_eq!(field(&line2, "olm_plain"), "hello from rust olm");
    let second = b64::decode(&field(&line2, "olm_second")).unwrap();
    assert_eq!(session.decrypt(1, &second).unwrap(), b"second from python");
    let _ = key_from_b64;
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

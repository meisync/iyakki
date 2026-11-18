//! Olm, the pairwise double ratchet of Matrix end-to-end encryption, written from the public Olm specification (F-MSG-08).
//!
//! * Identity: a Curve25519 key and an Ed25519 key per device; one-time Curve25519 keys are published for others to use.
//! * Session start (triple Diffie-Hellman): `S = ECDH(I_A, O_B) | ECDH(E_A, I_B) | ECDH(E_A, O_B)`; root key and first chain key are
//!   `HKDF-SHA-256(S, salt empty, "OLM_ROOT")`, 64 bytes.
//! * Ratchet step: `HKDF(ECDH(ratchet keys), salt = root key, "OLM_RATCHET")`, 64 bytes = new root key, new chain key.
//! * Chain: message key `HMAC(C, 0x01)`, next chain key `HMAC(C, 0x02)`.
//! * Message keys: `HKDF(K, salt empty, "OLM_KEYS")`, 80 bytes = AES-256 key, HMAC key, IV; AES-256-CBC with PKCS#7; message
//!   `0x03 | 0x0A len ratchet key | 0x10 index | 0x22 len ciphertext | mac (8 bytes)`; the first messages of a session travel as pre-key
//!   messages `0x03 | 0x0A one-time key | 0x12 base key | 0x1A identity key | 0x22 inner message` until the other side has answered.
//!
//! Keys, messages and base64 are the wire forms Matrix clients exchange. The saved forms are this crate's own.

use std::collections::BTreeMap;

use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use x25519_dalek::{x25519, X25519_BASEPOINT_BYTES};

use crate::kdf::{cbc_decrypt, cbc_encrypt, hkdf, hmac_sha256};
use crate::{b64, ct_eq};

const MAC_LEN: usize = 8;
/// Skipped message keys kept per receiving chain, and the biggest gap one message may open (libolm: 40 and 2000).
const MAX_SKIPPED: usize = 40;
const MAX_GAP: u32 = 2000;
const MAX_CHAINS: usize = 5;

type Key = [u8; 32];

fn random_key() -> Result<Key, String> {
    let mut k = [0u8; 32];
    crate::random(&mut k)?;
    Ok(k)
}
fn public(secret: &Key) -> Key {
    x25519(*secret, X25519_BASEPOINT_BYTES)
}
fn dh(secret: &Key, their: &Key) -> Result<Key, String> {
    let z = x25519(*secret, *their);
    if z == [0u8; 32] {
        return Err("the key is of low order".into());
    }
    Ok(z)
}

/// One device's identity and its one-time keys.
pub struct Account {
    curve: Key,
    ed_seed: Key,
    one_time: BTreeMap<u32, (Key, bool)>,
    next_id: u32,
}

impl Account {
    pub fn new() -> Result<Account, String> {
        Ok(Account { curve: random_key()?, ed_seed: random_key()?, one_time: BTreeMap::new(), next_id: 1 })
    }
    pub fn curve25519_key(&self) -> String {
        b64::std_nopad(&public(&self.curve))
    }
    pub fn ed25519_key(&self) -> String {
        b64::std_nopad(SigningKey::from_bytes(&self.ed_seed).verifying_key().as_bytes())
    }
    /// A signature in unpadded base64 (Matrix signs the canonical JSON of device keys and one-time keys with this).
    pub fn sign(&self, message: &[u8]) -> String {
        b64::std_nopad(&SigningKey::from_bytes(&self.ed_seed).sign(message).to_bytes())
    }
    pub fn generate_one_time_keys(&mut self, n: usize) -> Result<(), String> {
        for _ in 0..n {
            self.one_time.insert(self.next_id, (random_key()?, false));
            self.next_id += 1;
        }
        // libolm keeps at most 100
        while self.one_time.len() > 100 {
            let first = *self.one_time.keys().next().ok_or("none")?;
            self.one_time.remove(&first);
        }
        Ok(())
    }
    /// The one-time keys not yet published: (key id, public key).
    pub fn unpublished_one_time_keys(&self) -> Vec<(String, String)> {
        self.one_time.iter().filter(|(_, (_, p))| !*p).map(|(id, (k, _))| (b64::std_nopad(&id.to_be_bytes()), b64::std_nopad(&public(k)))).collect()
    }
    pub fn mark_keys_as_published(&mut self) {
        self.one_time.values_mut().for_each(|(_, p)| *p = true);
    }
    pub fn one_time_key_count(&self) -> usize {
        self.one_time.len()
    }
    fn take_one_time(&mut self, public_key: &Key) -> Option<Key> {
        let id = self.one_time.iter().find(|(_, (k, _))| public(k) == *public_key).map(|(id, _)| *id)?;
        self.one_time.remove(&id).map(|(k, _)| k)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&self.curve);
        o.extend_from_slice(&self.ed_seed);
        o.extend_from_slice(&self.next_id.to_be_bytes());
        o.extend_from_slice(&(self.one_time.len() as u32).to_be_bytes());
        for (id, (k, p)) in &self.one_time {
            o.extend_from_slice(&id.to_be_bytes());
            o.extend_from_slice(k);
            o.push(*p as u8);
        }
        o
    }
    pub fn from_bytes(b: &[u8]) -> Result<Account, String> {
        let bad = || "a saved Olm account is damaged".to_string();
        if b.len() < 72 {
            return Err(bad());
        }
        let n = u32::from_be_bytes(b[68..72].try_into().map_err(|_| bad())?) as usize;
        if n > 100 || b.len() != 72 + n * 37 {
            return Err(bad());
        }
        let mut one_time = BTreeMap::new();
        for i in 0..n {
            let r = &b[72 + i * 37..72 + (i + 1) * 37];
            one_time.insert(u32::from_be_bytes(r[..4].try_into().map_err(|_| bad())?), (r[4..36].try_into().map_err(|_| bad())?, r[36] != 0));
        }
        Ok(Account { curve: b[..32].try_into().map_err(|_| bad())?, ed_seed: b[32..64].try_into().map_err(|_| bad())?, next_id: u32::from_be_bytes(b[64..68].try_into().map_err(|_| bad())?), one_time })
    }
}

struct SenderChain {
    secret: Key,
    public: Key,
    chain: Key,
    index: u32,
}
#[derive(Clone)]
struct ReceiverChain {
    ratchet: Key,
    chain: Key,
    index: u32,
    skipped: Vec<(u32, Key)>,
}
#[derive(Clone)]
struct PreKey {
    one_time: Key,
    base: Key,
    identity: Key,
}

/// A pairwise session between two devices.
pub struct Session {
    root: Key,
    sender: Option<SenderChain>,
    receivers: Vec<ReceiverChain>,
    pre_key: Option<PreKey>,
    their_identity: Key,
    id: Key,
}

fn put_varint(o: &mut Vec<u8>, mut n: u32) {
    while n >= 0x80 {
        o.push((n & 0x7f) as u8 | 0x80);
        n >>= 7;
    }
    o.push(n as u8);
}
fn get_varint(b: &[u8], at: &mut usize) -> Result<u32, String> {
    let mut n = 0u32;
    for shift in (0..35).step_by(7) {
        let x = *b.get(*at).ok_or("message ends early")?;
        *at += 1;
        n |= ((x & 0x7f) as u32).checked_shl(shift).ok_or("number too big")?;
        if x & 0x80 == 0 {
            return Ok(n);
        }
    }
    Err("number too big".into())
}
fn field<'a>(b: &'a [u8], at: &mut usize) -> Result<&'a [u8], String> {
    let n = get_varint(b, at)? as usize;
    let s = b.get(*at..*at + n).ok_or("message ends early")?;
    *at += n;
    Ok(s)
}

fn message_keys(k: &Key) -> (Key, Key, [u8; 16]) {
    let d = hkdf(&[], k, b"OLM_KEYS", 80);
    (d[..32].try_into().unwrap_or([0; 32]), d[32..64].try_into().unwrap_or([0; 32]), d[64..80].try_into().unwrap_or([0; 16]))
}
fn ratchet_step(root: &Key, shared: &Key) -> (Key, Key) {
    let d = hkdf(root, shared, b"OLM_RATCHET", 64);
    (d[..32].try_into().unwrap_or([0; 32]), d[32..].try_into().unwrap_or([0; 32]))
}

/// A parsed normal message.
struct Parsed<'a> {
    ratchet: Key,
    index: u32,
    ciphertext: &'a [u8],
    mac_over: &'a [u8],
    mac: &'a [u8],
}
fn parse_message(b: &[u8]) -> Result<Parsed<'_>, String> {
    if b.len() < 1 + MAC_LEN || b[0] != 3 {
        return Err("not an Olm message".into());
    }
    let (body, mac) = b.split_at(b.len() - MAC_LEN);
    let (mut at, mut ratchet, mut index, mut ct) = (1usize, None, None, None);
    while at < body.len() {
        let tag = body[at];
        at += 1;
        match tag {
            0x0a => ratchet = Some(Key::try_from(field(body, &mut at)?).map_err(|_| "bad ratchet key")?),
            0x10 => index = Some(get_varint(body, &mut at)?),
            0x22 => ct = Some(field(body, &mut at)?),
            _ => return Err("unknown field in an Olm message".into()),
        }
    }
    Ok(Parsed { ratchet: ratchet.ok_or("no ratchet key")?, index: index.ok_or("no index")?, ciphertext: ct.ok_or("no ciphertext")?, mac_over: body, mac })
}

/// The type number Matrix gives: 0 pre-key, 1 normal.
pub struct OlmMessage {
    pub kind: u8,
    pub body: Vec<u8>,
}

impl OlmMessage {
    pub fn body_b64(&self) -> String {
        b64::std_nopad(&self.body)
    }
}

impl Session {
    fn session_id(identity_a: &Key, base: &Key, one_time: &Key) -> Key {
        let mut h = Sha256::new();
        h.update(identity_a);
        h.update(base);
        h.update(one_time);
        h.finalize().into()
    }
    pub fn id(&self) -> String {
        b64::std_nopad(&self.id)
    }

    /// Alice: starts a session to a device from its identity key and one of its published one-time keys (both Curve25519, raw bytes).
    pub fn outbound(account: &Account, their_identity: &Key, their_one_time: &Key) -> Result<Session, String> {
        let base = random_key()?;
        let ratchet = random_key()?;
        let mut s = Vec::with_capacity(96);
        s.extend_from_slice(&dh(&account.curve, their_one_time)?);
        s.extend_from_slice(&dh(&base, their_identity)?);
        s.extend_from_slice(&dh(&base, their_one_time)?);
        let d = hkdf(&[], &s, b"OLM_ROOT", 64);
        let my_identity = public(&account.curve);
        Ok(Session {
            root: d[..32].try_into().map_err(|_| "kdf")?,
            sender: Some(SenderChain { secret: ratchet, public: public(&ratchet), chain: d[32..].try_into().map_err(|_| "kdf")?, index: 0 }),
            receivers: vec![],
            pre_key: Some(PreKey { one_time: *their_one_time, base: public(&base), identity: my_identity }),
            their_identity: *their_identity,
            id: Self::session_id(&my_identity, &public(&base), their_one_time),
        })
    }

    /// Bob: opens a session from a pre-key message (type 0) and returns the first plain text. The one-time key is used up.
    pub fn inbound(account: &mut Account, pre_key_message: &[u8]) -> Result<(Session, Vec<u8>), String> {
        let b = pre_key_message;
        if b.first() != Some(&3) {
            return Err("not an Olm pre-key message".into());
        }
        let (mut at, mut one_time, mut base, mut identity, mut inner) = (1usize, None, None, None, None);
        while at < b.len() {
            let tag = b[at];
            at += 1;
            let f = field(b, &mut at)?;
            match tag {
                0x0a => one_time = Some(Key::try_from(f).map_err(|_| "bad one-time key")?),
                0x12 => base = Some(Key::try_from(f).map_err(|_| "bad base key")?),
                0x1a => identity = Some(Key::try_from(f).map_err(|_| "bad identity key")?),
                0x22 => inner = Some(f),
                _ => return Err("unknown field in an Olm pre-key message".into()),
            }
        }
        let (one_time, base, identity, inner) = (one_time.ok_or("no one-time key")?, base.ok_or("no base key")?, identity.ok_or("no identity key")?, inner.ok_or("no message")?);
        let parsed = parse_message(inner)?;
        let ot_secret = account.one_time.iter().find(|(_, (k, _))| public(k) == one_time).map(|(_, (k, _))| *k).ok_or("this pre-key message uses a one-time key we do not have (or already used)")?;
        let mut s = Vec::with_capacity(96);
        s.extend_from_slice(&dh(&ot_secret, &identity)?);
        s.extend_from_slice(&dh(&account.curve, &base)?);
        s.extend_from_slice(&dh(&ot_secret, &base)?);
        let d = hkdf(&[], &s, b"OLM_ROOT", 64);
        let mut session = Session {
            root: d[..32].try_into().map_err(|_| "kdf")?,
            sender: None,
            receivers: vec![ReceiverChain { ratchet: parsed.ratchet, chain: d[32..].try_into().map_err(|_| "kdf")?, index: 0, skipped: vec![] }],
            pre_key: None,
            their_identity: identity,
            id: Self::session_id(&identity, &base, &one_time),
        };
        let plain = session.decrypt(1, inner)?;
        // only now, with a message that opened, is the one-time key spent
        account.take_one_time(&one_time);
        Ok((session, plain))
    }

    /// Whether a pre-key message belongs to this session (the same base key, identity and one-time key).
    pub fn matches_pre_key(&self, pre_key_message: &[u8], their_identity: &Key) -> bool {
        let Some(p) = &self.pre_key else { return false };
        *their_identity == self.their_identity && !pre_key_message.is_empty() && pre_key_message.windows(32).any(|w| w == p.base) && pre_key_message.windows(32).any(|w| w == p.one_time)
    }

    pub fn encrypt(&mut self, plain: &[u8]) -> Result<OlmMessage, String> {
        if self.sender.is_none() {
            let their = self.receivers.last().ok_or("no chain to answer on")?.ratchet;
            let secret = random_key()?;
            let (root, chain) = ratchet_step(&self.root, &dh(&secret, &their)?);
            self.root = root;
            self.sender = Some(SenderChain { secret, public: public(&secret), chain, index: 0 });
        }
        let s = self.sender.as_mut().ok_or("no sender chain")?;
        let key = hmac_sha256(&s.chain, &[1]);
        let index = s.index;
        s.chain = hmac_sha256(&s.chain, &[2]);
        s.index += 1;
        let (aes, mac_key, iv) = message_keys(&key);
        let ct = cbc_encrypt(&aes, &iv, plain);
        let mut m = vec![3u8, 0x0a, 32];
        m.extend_from_slice(&s.public);
        m.push(0x10);
        put_varint(&mut m, index);
        m.push(0x22);
        put_varint(&mut m, ct.len() as u32);
        m.extend_from_slice(&ct);
        let mac = hmac_sha256(&mac_key, &m);
        m.extend_from_slice(&mac[..MAC_LEN]);
        match &self.pre_key {
            Some(p) => {
                let mut o = vec![3u8, 0x0a, 32];
                o.extend_from_slice(&p.one_time);
                o.extend_from_slice(&[0x12, 32]);
                o.extend_from_slice(&p.base);
                o.extend_from_slice(&[0x1a, 32]);
                o.extend_from_slice(&p.identity);
                o.push(0x22);
                put_varint(&mut o, m.len() as u32);
                o.extend_from_slice(&m);
                Ok(OlmMessage { kind: 0, body: o })
            }
            None => Ok(OlmMessage { kind: 1, body: m }),
        }
    }

    /// Opens a normal message (`kind` 1, or the inner message of a pre-key message). Nothing changes if it does not open.
    pub fn decrypt(&mut self, kind: u8, body: &[u8]) -> Result<Vec<u8>, String> {
        let inner: &[u8] = if kind == 0 {
            let mut at = 1usize;
            let mut found = None;
            while at < body.len() {
                let tag = body[at];
                at += 1;
                let f = field(body, &mut at)?;
                if tag == 0x22 {
                    found = Some(f);
                }
            }
            found.ok_or("no message in the pre-key message")?
        } else {
            body
        };
        let p = parse_message(inner)?;
        let (mut root, mut sender_gone) = (self.root, false);
        let mut receivers = self.receivers.clone();
        let pos = match receivers.iter().position(|r| r.ratchet == p.ratchet) {
            Some(i) => i,
            None => {
                let s = self.sender.as_ref().ok_or("a message on an unknown chain")?;
                let (r2, chain) = ratchet_step(&root, &dh(&s.secret, &p.ratchet)?);
                root = r2;
                sender_gone = true;
                receivers.push(ReceiverChain { ratchet: p.ratchet, chain, index: 0, skipped: vec![] });
                if receivers.len() > MAX_CHAINS {
                    receivers.remove(0);
                }
                receivers.len() - 1
            }
        };
        let chain = &mut receivers[pos];
        let key = if p.index < chain.index {
            let at = chain.skipped.iter().position(|(i, _)| *i == p.index).ok_or("this message was already read or is too old")?;
            chain.skipped[at].1
        } else {
            if p.index - chain.index > MAX_GAP {
                return Err("the message is too far ahead".into());
            }
            while chain.index < p.index {
                chain.skipped.push((chain.index, hmac_sha256(&chain.chain, &[1])));
                if chain.skipped.len() > MAX_SKIPPED {
                    chain.skipped.remove(0);
                }
                chain.chain = hmac_sha256(&chain.chain, &[2]);
                chain.index += 1;
            }
            let k = hmac_sha256(&chain.chain, &[1]);
            chain.chain = hmac_sha256(&chain.chain, &[2]);
            chain.index += 1;
            k
        };
        let (aes, mac_key, iv) = message_keys(&key);
        if !ct_eq(&hmac_sha256(&mac_key, p.mac_over)[..MAC_LEN], p.mac) {
            return Err("the message authentication code is wrong".into());
        }
        let plain = cbc_decrypt(&aes, &iv, p.ciphertext)?;
        // success: commit
        if p.index < receivers[pos].index {
            receivers[pos].skipped.retain(|(i, _)| *i != p.index);
        }
        self.receivers = receivers;
        self.root = root;
        if sender_gone {
            self.sender = None;
        }
        self.pre_key = None;
        Ok(plain)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&self.root);
        o.extend_from_slice(&self.their_identity);
        o.extend_from_slice(&self.id);
        match &self.sender {
            Some(s) => {
                o.push(1);
                o.extend_from_slice(&s.secret);
                o.extend_from_slice(&s.chain);
                o.extend_from_slice(&s.index.to_be_bytes());
            }
            None => o.push(0),
        }
        match &self.pre_key {
            Some(p) => {
                o.push(1);
                for k in [&p.one_time, &p.base, &p.identity] {
                    o.extend_from_slice(k);
                }
            }
            None => o.push(0),
        }
        o.push(self.receivers.len() as u8);
        for r in &self.receivers {
            o.extend_from_slice(&r.ratchet);
            o.extend_from_slice(&r.chain);
            o.extend_from_slice(&r.index.to_be_bytes());
            o.push(r.skipped.len() as u8);
            for (i, k) in &r.skipped {
                o.extend_from_slice(&i.to_be_bytes());
                o.extend_from_slice(k);
            }
        }
        o
    }

    pub fn from_bytes(b: &[u8]) -> Result<Session, String> {
        let bad = || "a saved Olm session is damaged".to_string();
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let s = b.get(at..at + n).ok_or_else(bad)?;
            at += n;
            Ok(s)
        };
        let key = |s: &[u8]| -> Result<Key, String> { s.try_into().map_err(|_| bad()) };
        let u32_of = |s: &[u8]| -> Result<u32, String> { Ok(u32::from_be_bytes(s.try_into().map_err(|_| bad())?)) };
        let (root, their_identity, id) = (key(take(32)?)?, key(take(32)?)?, key(take(32)?)?);
        let sender = if take(1)?[0] == 1 {
            let secret = key(take(32)?)?;
            let chain = key(take(32)?)?;
            Some(SenderChain { secret, public: public(&secret), chain, index: u32_of(take(4)?)? })
        } else {
            None
        };
        let pre_key = if take(1)?[0] == 1 { Some(PreKey { one_time: key(take(32)?)?, base: key(take(32)?)?, identity: key(take(32)?)? }) } else { None };
        let n = take(1)?[0] as usize;
        let mut receivers = Vec::new();
        for _ in 0..n {
            let (ratchet, chain, index) = (key(take(32)?)?, key(take(32)?)?, u32_of(take(4)?)?);
            let sn = take(1)?[0] as usize;
            let mut skipped = Vec::new();
            for _ in 0..sn {
                skipped.push((u32_of(take(4)?)?, key(take(32)?)?));
            }
            receivers.push(ReceiverChain { ratchet, chain, index, skipped });
        }
        if at != b.len() {
            return Err(bad());
        }
        Ok(Session { root, sender, receivers, pre_key, their_identity, id })
    }
}

/// A raw Curve25519 key from its unpadded base64 form.
pub fn key_from_b64(s: &str) -> Result<Key, String> {
    b64::decode(s)?.try_into().map_err(|_| "a Curve25519 key is 32 bytes".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Account, Account, Session, OlmMessage) {
        let alice = Account::new().unwrap();
        let mut bob = Account::new().unwrap();
        bob.generate_one_time_keys(3).unwrap();
        let otk = key_from_b64(&bob.unpublished_one_time_keys()[0].1).unwrap();
        let mut a = Session::outbound(&alice, &key_from_b64(&bob.curve25519_key()).unwrap(), &otk).unwrap();
        let first = a.encrypt(b"hello bob").unwrap();
        assert_eq!(first.kind, 0);
        (alice, bob, a, first)
    }

    #[test]
    fn a_conversation_goes_back_and_forth_and_the_ratchet_turns_on_every_change_of_direction() {
        let (_alice, mut bob, mut a, first) = pair();
        let (mut b, plain) = Session::inbound(&mut bob, &first.body).unwrap();
        assert_eq!(plain, b"hello bob");
        assert_eq!(a.id(), b.id());
        assert_eq!(bob.one_time_key_count(), 2, "the used one-time key is gone");
        // Alice keeps sending pre-key messages until Bob has answered
        let second = a.encrypt(b"again").unwrap();
        assert_eq!(second.kind, 0);
        assert!(a.matches_pre_key(&second.body, &key_from_b64(&_alice.curve25519_key()).map(|_| a.their_identity).unwrap()));
        assert_eq!(b.decrypt(0, &second.body).unwrap(), b"again");
        let r1 = b.encrypt(b"hi alice").unwrap();
        assert_eq!(r1.kind, 1);
        assert_eq!(a.decrypt(1, &r1.body).unwrap(), b"hi alice");
        let m = a.encrypt(b"now normal").unwrap();
        assert_eq!(m.kind, 1);
        assert_eq!(b.decrypt(1, &m.body).unwrap(), b"now normal");
        for i in 0..20u8 {
            let (x, y) = if i % 2 == 0 { (&mut a, &mut b) } else { (&mut b, &mut a) };
            for k in 0..3u8 {
                let t = [i, k];
                assert_eq!(y.decrypt(1, &x.encrypt(&t).unwrap().body).unwrap(), t);
            }
        }
    }

    #[test]
    fn messages_that_arrive_late_or_out_of_order_open_once_and_replays_damage_and_gaps_are_refused() {
        let (_a0, mut bob, mut a, first) = pair();
        let (mut b, _) = Session::inbound(&mut bob, &first.body).unwrap();
        let r = b.encrypt(b"x").unwrap();
        a.decrypt(1, &r.body).unwrap();
        let m: Vec<_> = (0..5u8).map(|i| a.encrypt(&[i; 3]).unwrap()).collect();
        assert_eq!(b.decrypt(1, &m[3].body).unwrap(), [3; 3]);
        assert_eq!(b.decrypt(1, &m[1].body).unwrap(), [1; 3]);
        assert!(b.decrypt(1, &m[1].body).is_err(), "a replay");
        assert_eq!(b.decrypt(1, &m[0].body).unwrap(), [0; 3]);
        assert_eq!(b.decrypt(1, &m[4].body).unwrap(), [4; 3]);
        let before = b.to_bytes();
        let mut bad = m[2].body.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(b.decrypt(1, &bad).is_err());
        assert_eq!(b.to_bytes(), before, "a failed message changes nothing");
        assert_eq!(b.decrypt(1, &m[2].body).unwrap(), [2; 3]);
        // too far ahead
        for _ in 0..(MAX_GAP + 1) {
            a.encrypt(b"y").unwrap();
        }
        let far = a.encrypt(b"far").unwrap();
        assert!(b.decrypt(1, &far.body).is_err());
        assert!(b.decrypt(1, &[3]).is_err() && b.decrypt(1, &[]).is_err() && b.decrypt(1, &[9; 80]).is_err());
    }

    #[test]
    fn a_pre_key_message_for_a_spent_or_unknown_one_time_key_is_refused_and_keeps_the_key_if_it_does_not_open() {
        let (_a0, mut bob, _a, first) = pair();
        let mut tampered = first.body.clone();
        let n = tampered.len();
        tampered[n - 1] ^= 1;
        assert!(Session::inbound(&mut bob, &tampered).is_err());
        assert_eq!(bob.one_time_key_count(), 3, "a message that does not open does not spend the key");
        Session::inbound(&mut bob, &first.body).unwrap();
        assert!(Session::inbound(&mut bob, &first.body).is_err(), "the key is spent");
        let mut other = Account::new().unwrap();
        assert!(Session::inbound(&mut other, &first.body).is_err());
    }

    #[test]
    fn sessions_and_accounts_survive_being_saved_and_a_damaged_save_is_refused() {
        let (_a0, mut bob, mut a, first) = pair();
        let (mut b, _) = Session::inbound(&mut bob, &first.body).unwrap();
        let r = b.encrypt(b"x").unwrap();
        a.decrypt(1, &r.body).unwrap();
        let m = a.encrypt(b"later").unwrap();
        let mut b2 = Session::from_bytes(&b.to_bytes()).unwrap();
        assert_eq!(b2.decrypt(1, &m.body).unwrap(), b"later");
        assert!(Session::from_bytes(&b.to_bytes()[..50]).is_err());
        let acc = Account::from_bytes(&bob.to_bytes()).unwrap();
        assert_eq!((acc.curve25519_key(), acc.ed25519_key(), acc.one_time_key_count()), (bob.curve25519_key(), bob.ed25519_key(), bob.one_time_key_count()));
        assert!(Account::from_bytes(&bob.to_bytes()[..60]).is_err());
        let mut mark = Account::from_bytes(&bob.to_bytes()).unwrap();
        assert_eq!(mark.unpublished_one_time_keys().len(), 2);
        mark.mark_keys_as_published();
        assert!(mark.unpublished_one_time_keys().is_empty());
        mark.generate_one_time_keys(150).unwrap();
        assert_eq!(mark.one_time_key_count(), 100);
    }

    #[test]
    fn the_signature_of_an_account_checks_with_its_ed25519_key() {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let a = Account::new().unwrap();
        let sig = Signature::from_bytes(&b64::decode(&a.sign(b"{\"a\":1}")).unwrap().try_into().unwrap());
        let key = VerifyingKey::from_bytes(&b64::decode(&a.ed25519_key()).unwrap().try_into().unwrap()).unwrap();
        assert!(key.verify(b"{\"a\":1}", &sig).is_ok() && key.verify(b"{\"a\":2}", &sig).is_err());
    }
}

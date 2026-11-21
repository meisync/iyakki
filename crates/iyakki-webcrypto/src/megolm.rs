//! Megolm, the group ratchet of Matrix end-to-end encrypted rooms, written from the public Megolm specification (F-MSG-08).
//!
//! * The ratchet has four 32-byte parts and a 32-bit counter. Advancing re-hashes with HMAC-SHA-256 (the part is the key, the part number
//!   the one-byte input) from the highest part that changed.
//! * Message keys: HKDF-SHA-256 over the 128 ratchet bytes, empty salt, info `MEGOLM_KEYS`, 80 bytes = AES-256 key, HMAC key, CBC IV.
//! * A message is `0x03 | 0x08 index | 0x12 len ciphertext | mac (8 bytes) | Ed25519 signature (64 bytes)`, in unpadded base64.
//! * A session key is `0x02 | index (4 bytes, big endian) | 128 ratchet bytes | Ed25519 public key | signature`.
//!
//! The wire formats are the ones Matrix clients exchange; the in-memory and saved forms are this crate's own.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::kdf::{cbc_decrypt, cbc_encrypt, hkdf, hmac_sha256};
use crate::{b64, ct_eq};

const MAC_LEN: usize = 8;

#[derive(Clone, PartialEq, Eq)]
pub struct Ratchet {
    data: [u8; 128],
    counter: u32,
}

impl Ratchet {
    pub fn new() -> Result<Ratchet, String> {
        let mut data = [0u8; 128];
        crate::random(&mut data)?;
        Ok(Ratchet { data, counter: 0 })
    }
    pub fn from_parts(data: [u8; 128], counter: u32) -> Ratchet {
        Ratchet { data, counter }
    }
    pub fn counter(&self) -> u32 {
        self.counter
    }
    pub fn data(&self) -> &[u8; 128] {
        &self.data
    }

    fn rehash(&mut self, from: usize, to: usize) {
        let key: [u8; 32] = self.data[from * 32..from * 32 + 32].try_into().unwrap_or([0; 32]);
        self.data[to * 32..to * 32 + 32].copy_from_slice(&hmac_sha256(&key, &[to as u8]));
    }

    /// The highest part that changes when the counter has just become `c`.
    fn level(c: u32) -> usize {
        if c & 0x00ff_ffff == 0 {
            0
        } else if c & 0xffff == 0 {
            1
        } else if c & 0xff == 0 {
            2
        } else {
            3
        }
    }

    pub fn advance(&mut self) -> Result<(), String> {
        self.counter = self.counter.checked_add(1).ok_or("the Megolm session is used up (2^32 messages)")?;
        let h = Self::level(self.counter);
        for i in (h..4).rev() {
            self.rehash(h, i);
        }
        Ok(())
    }

    /// Moves forward to `target` by the largest jumps that do not overshoot (a jump over a block gives the same parts as the steps in it).
    pub fn advance_to(&mut self, target: u32) -> Result<(), String> {
        if target < self.counter {
            return Err("a ratchet cannot go back".into());
        }
        while self.counter != target {
            let c = self.counter as u64;
            let mut stepped = false;
            for j in 0..3 {
                let shift = (3 - j) * 8;
                let next = ((c >> shift) + 1) << shift;
                if next <= target as u64 {
                    self.counter = next as u32;
                    let h = Self::level(self.counter);
                    for i in (h..4).rev() {
                        self.rehash(h, i);
                    }
                    stepped = true;
                    break;
                }
            }
            if !stepped {
                self.advance()?;
            }
        }
        Ok(())
    }

    fn keys(&self) -> ([u8; 32], [u8; 32], [u8; 16]) {
        let k = hkdf(&[], &self.data, b"MEGOLM_KEYS", 80);
        (k[..32].try_into().unwrap_or([0; 32]), k[32..64].try_into().unwrap_or([0; 32]), k[64..80].try_into().unwrap_or([0; 16]))
    }
}

fn varint(out: &mut Vec<u8>, mut n: u32) {
    while n >= 0x80 {
        out.push((n & 0x7f) as u8 | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

fn read_varint(b: &[u8], at: &mut usize) -> Result<u32, String> {
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

/// Our own sending session in a room.
pub struct OutboundGroup {
    ratchet: Ratchet,
    signing: SigningKey,
}

impl OutboundGroup {
    pub fn new() -> Result<OutboundGroup, String> {
        let mut seed = [0u8; 32];
        crate::random(&mut seed)?;
        Ok(OutboundGroup { ratchet: Ratchet::new()?, signing: SigningKey::from_bytes(&seed) })
    }
    pub fn from_parts(ratchet: Ratchet, seed: [u8; 32]) -> OutboundGroup {
        OutboundGroup { ratchet, signing: SigningKey::from_bytes(&seed) }
    }
    /// The saved form: ratchet bytes, counter and the signing seed.
    pub fn to_parts(&self) -> (Ratchet, [u8; 32]) {
        (self.ratchet.clone(), self.signing.to_bytes())
    }
    /// The session id is the base64 of the signing public key.
    pub fn session_id(&self) -> String {
        b64::std_nopad(self.signing.verifying_key().as_bytes())
    }
    pub fn message_index(&self) -> u32 {
        self.ratchet.counter
    }
    /// What goes to the room's devices in an `m.room_key` event (`session_key`).
    pub fn session_key(&self) -> String {
        let mut b = vec![2u8];
        b.extend_from_slice(&self.ratchet.counter.to_be_bytes());
        b.extend_from_slice(&self.ratchet.data);
        b.extend_from_slice(self.signing.verifying_key().as_bytes());
        let sig = self.signing.sign(&b);
        b.extend_from_slice(&sig.to_bytes());
        b64::std_nopad(&b)
    }
    pub fn encrypt(&mut self, plain: &[u8]) -> Result<String, String> {
        let (aes, mac_key, iv) = self.ratchet.keys();
        let ct = cbc_encrypt(&aes, &iv, plain);
        let mut m = vec![3u8, 0x08];
        varint(&mut m, self.ratchet.counter);
        m.push(0x12);
        varint(&mut m, ct.len() as u32);
        m.extend_from_slice(&ct);
        m.extend_from_slice(&hmac_sha256(&mac_key, &m)[..MAC_LEN]);
        let sig = self.signing.sign(&m);
        m.extend_from_slice(&sig.to_bytes());
        self.ratchet.advance()?;
        Ok(b64::std_nopad(&m))
    }
}

/// A session of somebody else's that we can read, from the first index we were given.
#[derive(Clone)]
pub struct InboundGroup {
    initial: Ratchet,
    latest: Ratchet,
    signing_public: [u8; 32],
}

impl InboundGroup {
    /// From the `session_key` of an `m.room_key` event (version 2, signed) or an exported session (version 1, not signed).
    pub fn from_session_key(s: &str) -> Result<InboundGroup, String> {
        let b = b64::decode(s)?;
        let signed = match b.first() {
            Some(2) => true,
            Some(1) => false,
            _ => return Err("not a Megolm session key".into()),
        };
        let body = 1 + 4 + 128 + 32;
        if b.len() != body + if signed { 64 } else { 0 } {
            return Err("a Megolm session key has the wrong length".into());
        }
        let public: [u8; 32] = b[body - 32..body].try_into().map_err(|_| "key")?;
        if signed {
            let key = VerifyingKey::from_bytes(&public).map_err(|_| "the signing key is not valid")?;
            let sig = Signature::from_bytes(b[body..].try_into().map_err(|_| "sig")?);
            key.verify(&b[..body], &sig).map_err(|_| "the session key is not signed by its own key")?;
        }
        let counter = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
        let r = Ratchet::from_parts(b[5..133].try_into().map_err(|_| "ratchet")?, counter);
        Ok(InboundGroup { initial: r.clone(), latest: r, signing_public: public })
    }
    pub fn session_id(&self) -> String {
        b64::std_nopad(&self.signing_public)
    }
    /// The exported form (version 1: first known index, its ratchet, the signing key; no signature) of the session from the first
    /// index this device can read, as a room key export file carries it. `from_session_key` reads it back.
    pub fn export_session_key(&self) -> String {
        let mut o = Vec::with_capacity(1 + 4 + 128 + 32);
        o.push(1);
        o.extend_from_slice(&self.initial.counter.to_be_bytes());
        o.extend_from_slice(&self.initial.data);
        o.extend_from_slice(&self.signing_public);
        b64::std_nopad(&o)
    }
    pub fn first_known_index(&self) -> u32 {
        self.initial.counter
    }
    /// Plain text and the message index. A message before the first known index, a bad signature or a bad MAC is refused. Whether an
    /// index was seen before (a replay) is for the caller to decide: it knows the event ids.
    pub fn decrypt(&mut self, msg: &str) -> Result<(Vec<u8>, u32), String> {
        let b = b64::decode(msg)?;
        if b.len() < 1 + 2 + 2 + MAC_LEN + 64 || b[0] != 3 {
            return Err("not a Megolm message".into());
        }
        let (signed, sig) = b.split_at(b.len() - 64);
        let key = VerifyingKey::from_bytes(&self.signing_public).map_err(|_| "the signing key is not valid")?;
        key.verify(signed, &Signature::from_bytes(sig.try_into().map_err(|_| "sig")?)).map_err(|_| "the message is not signed by this session")?;
        let body = &signed[..signed.len() - MAC_LEN];
        let (mut at, mut index, mut ct) = (1usize, None, None);
        while at < body.len() {
            let tag = body[at];
            at += 1;
            match tag {
                0x08 => index = Some(read_varint(body, &mut at)?),
                0x12 => {
                    let n = read_varint(body, &mut at)? as usize;
                    ct = Some(body.get(at..at + n).ok_or("message ends early")?);
                    at += n;
                }
                _ => return Err("unknown field in a Megolm message".into()),
            }
        }
        let (index, ct) = (index.ok_or("no message index")?, ct.ok_or("no ciphertext")?);
        if index < self.initial.counter {
            return Err("this message is from before the session key we were given".into());
        }
        let mut r = if index >= self.latest.counter { self.latest.clone() } else { self.initial.clone() };
        r.advance_to(index)?;
        let (aes, mac_key, iv) = r.keys();
        if !ct_eq(&hmac_sha256(&mac_key, body)[..MAC_LEN], &signed[signed.len() - MAC_LEN..]) {
            return Err("the message authentication code is wrong".into());
        }
        let plain = cbc_decrypt(&aes, &iv, ct)?;
        if index > self.latest.counter {
            self.latest = r;
        }
        Ok((plain, index))
    }
    /// Saved form: both ratchets and the public key.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(32 + 2 * 132);
        o.extend_from_slice(&self.signing_public);
        for r in [&self.initial, &self.latest] {
            o.extend_from_slice(&r.counter.to_be_bytes());
            o.extend_from_slice(&r.data);
        }
        o
    }
    pub fn from_bytes(b: &[u8]) -> Result<InboundGroup, String> {
        if b.len() != 32 + 2 * 132 {
            return Err("a saved Megolm session has the wrong length".into());
        }
        let r = |at: usize| -> Result<Ratchet, String> { Ok(Ratchet::from_parts(b[at + 4..at + 132].try_into().map_err(|_| "ratchet")?, u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))) };
        Ok(InboundGroup { signing_public: b[..32].try_into().map_err(|_| "key")?, initial: r(32)?, latest: r(164)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ratchet_rekeys_at_the_byte_boundaries_and_a_jump_equals_the_steps() {
        let mut data = [0u8; 128];
        for (i, d) in data.iter_mut().enumerate() {
            *d = i as u8;
        }
        let mut one = Ratchet::from_parts(data, 0);
        for _ in 0..257 {
            one.advance().unwrap();
        }
        let mut jump = Ratchet::from_parts(data, 0);
        jump.advance_to(257).unwrap();
        assert!(one == jump);
        // below the first boundary only the last part changes
        let mut r = Ratchet::from_parts(data, 0);
        r.advance().unwrap();
        assert_eq!(r.data[..96], data[..96]);
        assert_ne!(r.data[96..], data[96..]);
        // at 256 the parts 2 and 3 are rebuilt from part 2; at 65536 parts 1..3 from part 1
        let mut r = Ratchet::from_parts(data, 255);
        r.advance().unwrap();
        assert_eq!(r.data[..64], data[..64]);
        assert_eq!(r.data[64..96], hmac_sha256(&data[64..96], &[2]));
        assert_eq!(r.data[96..], hmac_sha256(&data[64..96], &[3]));
        let mut a = Ratchet::from_parts(data, 65_000);
        a.advance_to(70_000).unwrap();
        let mut b = Ratchet::from_parts(data, 65_000);
        for _ in 65_000..70_000 {
            b.advance().unwrap();
        }
        assert!(a == b);
        // and across the 2^24 boundary
        let mut a = Ratchet::from_parts(data, (1 << 24) - 3);
        a.advance_to((1 << 24) + 300).unwrap();
        let mut b = Ratchet::from_parts(data, (1 << 24) - 3);
        for _ in 0..303 {
            b.advance().unwrap();
        }
        assert!(a == b);
        assert!(Ratchet::from_parts(data, u32::MAX).advance().is_err());
        assert!(a.advance_to(5).is_err());
    }

    #[test]
    fn a_session_key_lets_another_device_read_every_message_from_its_index_on_in_any_order() {
        let mut out = OutboundGroup::new().unwrap();
        let first = out.encrypt(b"zero").unwrap();
        let shared = out.session_key();
        let m1 = out.encrypt(b"one").unwrap();
        let m2 = out.encrypt(b"two").unwrap();
        for _ in 0..300 {
            out.encrypt(b"x").unwrap();
        }
        let m_late = out.encrypt("late \u{1F600}".as_bytes()).unwrap();
        let mut inb = InboundGroup::from_session_key(&shared).unwrap();
        assert_eq!(inb.session_id(), out.session_id());
        assert_eq!(inb.first_known_index(), 1);
        assert!(inb.decrypt(&first).is_err(), "before the session key");
        assert_eq!(inb.decrypt(&m_late).unwrap(), ("late \u{1F600}".as_bytes().to_vec(), 303));
        assert_eq!(inb.decrypt(&m2).unwrap(), (b"two".to_vec(), 2), "an earlier index still opens after a later one");
        assert_eq!(inb.decrypt(&m1).unwrap(), (b"one".to_vec(), 1));
        let again = InboundGroup::from_bytes(&inb.to_bytes()).unwrap();
        assert_eq!(again.clone().decrypt(&m1).unwrap().0, b"one");
        let (r, seed) = out.to_parts();
        let mut resumed = OutboundGroup::from_parts(r, seed);
        assert_eq!(resumed.session_id(), out.session_id());
        assert_eq!(inb.decrypt(&resumed.encrypt(b"after restart").unwrap()).unwrap().0, b"after restart");
    }

    #[test]
    fn damage_foreign_sessions_and_unsigned_or_bad_keys_are_refused() {
        let mut out = OutboundGroup::new().unwrap();
        let key = out.session_key();
        let m = out.encrypt(b"secret").unwrap();
        let mut inb = InboundGroup::from_session_key(&key).unwrap();
        let raw = b64::decode(&m).unwrap();
        for at in [1, 4, raw.len() - 70, raw.len() - 10, raw.len() - 1] {
            let mut bad = raw.clone();
            bad[at] ^= 1;
            assert!(inb.decrypt(&b64::std_nopad(&bad)).is_err(), "byte {at}");
        }
        let mut other = OutboundGroup::new().unwrap();
        assert!(inb.decrypt(&other.encrypt(b"x").unwrap()).is_err(), "another session's message");
        let mut k = b64::decode(&key).unwrap();
        k[10] ^= 1;
        assert!(InboundGroup::from_session_key(&b64::std_nopad(&k)).is_err(), "signed ratchet bytes");
        assert!(InboundGroup::from_session_key("AAAA").is_err());
        assert!(inb.decrypt("AAAA").is_err());
        // version 1 (exported, unsigned) is accepted when the length fits
        let mut v1 = b64::decode(&key).unwrap();
        v1.truncate(v1.len() - 64);
        v1[0] = 1;
        assert!(InboundGroup::from_session_key(&b64::std_nopad(&v1)).unwrap().decrypt(&m).is_ok());
    }
}

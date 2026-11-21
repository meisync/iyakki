//! A security key over USB HID (F-BROX-03): the CTAPHID framing and the CTAP2 requests a browser sends, written from the public FIDO
//! Alliance CTAP 2.1 description. [`Hid`] is the 64-byte report pipe of one key (the device node is the caller's; a test gives a fake that
//! is itself a CTAP2 authenticator built on the software one), [`Key`] opens a channel and makes and uses credentials.
//!
//! What it does: `authenticatorMakeCredential` (ES256 only, an optional discoverable credential, an optional exclude list, user
//! verification asked for when the site requires it), `authenticatorGetAssertion` with every account a discoverable sign-in finds
//! (`getNextAssertion` until the count is read), keep-alive frames skipped, every status byte turned into a sentence. It does NOT do the
//! PIN protocol: a key that answers "PIN required" is told to the person in words and nothing else is tried.

use crate::cbor::{self, Value};

/// A report is 64 bytes (the CTAPHID packet size on USB full speed).
pub const PACKET: usize = 64;
const BROADCAST: u32 = 0xFFFF_FFFF;
const CMD_INIT: u8 = 0x06;
const CMD_CBOR: u8 = 0x10;
const CMD_KEEPALIVE: u8 = 0x3B;
const CMD_ERROR: u8 = 0x3F;
/// The capability bit "implements CBOR (CTAP2)".
const CAP_CBOR: u8 = 0x04;
/// The most keep-alive frames waited through for one answer (a key blinking for a touch sends one every 100 ms).
const MAX_KEEPALIVE: usize = 1200;
/// The longest message: one initial packet and 128 continuation packets.
const MAX_MESSAGE: usize = 57 + 128 * 59;

/// One key's report pipe: a packet out, a packet in (waiting up to the device's own timeout; an `Err` is a timeout or a gone key).
pub trait Hid {
    fn send(&mut self, packet: &[u8; PACKET]) -> Result<(), String>;
    fn recv(&mut self) -> Result<[u8; PACKET], String>;
}

impl<T: Hid + ?Sized> Hid for Box<T> {
    fn send(&mut self, packet: &[u8; PACKET]) -> Result<(), String> {
        (**self).send(packet)
    }
    fn recv(&mut self) -> Result<[u8; PACKET], String> {
        (**self).recv()
    }
}

/// The packets of one message: an initial packet and as many continuation packets as the data needs.
pub fn frames(cid: u32, cmd: u8, data: &[u8]) -> Result<Vec<[u8; PACKET]>, String> {
    if data.len() > MAX_MESSAGE {
        return Err("the message is too long for a key".into());
    }
    let mut out = Vec::new();
    let mut p = [0u8; PACKET];
    p[..4].copy_from_slice(&cid.to_be_bytes());
    p[4] = 0x80 | cmd;
    p[5..7].copy_from_slice(&(data.len() as u16).to_be_bytes());
    let first = data.len().min(PACKET - 7);
    p[7..7 + first].copy_from_slice(&data[..first]);
    out.push(p);
    for (seq, chunk) in data[first..].chunks(PACKET - 5).enumerate() {
        let mut p = [0u8; PACKET];
        p[..4].copy_from_slice(&cid.to_be_bytes());
        p[4] = seq as u8;
        p[5..5 + chunk.len()].copy_from_slice(chunk);
        out.push(p);
    }
    Ok(out)
}

/// Puts the packets of one message back together.
pub struct Reassembler {
    cid: u32,
    cmd: u8,
    want: usize,
    data: Vec<u8>,
    seq: u8,
    started: bool,
}

impl Reassembler {
    pub fn new(cid: u32) -> Reassembler {
        Reassembler { cid, cmd: 0, want: 0, data: Vec::new(), seq: 0, started: false }
    }

    /// Takes one packet; `Ok(Some((command, data)))` when the message is whole. Packets of another channel are ignored.
    pub fn push(&mut self, p: &[u8; PACKET]) -> Result<Option<(u8, Vec<u8>)>, String> {
        if u32::from_be_bytes([p[0], p[1], p[2], p[3]]) != self.cid {
            return Ok(None);
        }
        if !self.started {
            if p[4] & 0x80 == 0 {
                return Err("the key sent a continuation packet first".into());
            }
            self.cmd = p[4] & 0x7F;
            self.want = u16::from_be_bytes([p[5], p[6]]) as usize;
            if self.want > MAX_MESSAGE {
                return Err("the key announced a message that is too long".into());
            }
            self.started = true;
            self.data.extend_from_slice(&p[7..7 + self.want.min(PACKET - 7)]);
        } else {
            if p[4] != self.seq {
                return Err("the key's packets came out of order".into());
            }
            self.seq = self.seq.wrapping_add(1);
            let need = self.want - self.data.len();
            self.data.extend_from_slice(&p[5..5 + need.min(PACKET - 5)]);
        }
        Ok((self.data.len() >= self.want).then(|| (self.cmd, std::mem::take(&mut self.data))))
    }
}

/// What a CTAP2 status byte means, in words for the person.
pub fn status_words(status: u8) -> String {
    match status {
        0x19 => "this key already has a passkey for this account".into(),
        0x26 | 0x2B => "this key does not support the kind of passkey the site wants".into(),
        0x27 => "the key's owner refused (or the key did not accept the request)".into(),
        0x2E => "this key has no passkey for this site".into(),
        0x2F => "the key was not touched in time".into(),
        0x31 | 0x32 | 0x33 => "the key's PIN is wrong or blocked".into(),
        0x36 => "this key wants its PIN first; the PIN is not supported here yet".into(),
        0x35 => "this key has no PIN set, but the site needs the person to prove who they are".into(),
        0x34 => "the key is full".into(),
        s => format!("the key answered with the error 0x{s:02x}"),
    }
}

/// One discoverable or listed credential answered by the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyAssertion {
    pub credential_id: Vec<u8>,
    pub auth_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub user_handle: Vec<u8>,
    pub user_name: String,
}

/// An open channel to one key.
pub struct Key<H: Hid> {
    hid: H,
    cid: u32,
}

fn text(v: &str) -> Value {
    Value::text(v)
}

fn desc(id: &[u8]) -> Value {
    Value::Map(vec![(text("id"), Value::Bytes(id.to_vec())), (text("type"), text("public-key"))])
}

impl<H: Hid> Key<H> {
    /// Opens a channel (CTAPHID_INIT on the broadcast channel with a random nonce) and checks that the key speaks CTAP2.
    pub fn open(hid: H, nonce: [u8; 8]) -> Result<Key<H>, String> {
        let mut k = Key { hid, cid: BROADCAST };
        for f in frames(BROADCAST, CMD_INIT, &nonce)? {
            k.hid.send(&f)?;
        }
        // other keys' answers to their own nonce are ignored: wait for ours
        for _ in 0..8 {
            let mut r = Reassembler::new(BROADCAST);
            let (cmd, data) = loop {
                if let Some(m) = r.push(&k.hid.recv()?)? {
                    break m;
                }
            };
            if cmd == CMD_INIT && data.len() >= 17 && data[..8] == nonce {
                k.cid = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
                if data[16] & CAP_CBOR == 0 {
                    return Err("this key does not speak CTAP2 (only an old U2F key)".into());
                }
                return Ok(k);
            }
        }
        Err("the key did not answer the greeting".into())
    }

    /// Sends one CTAP2 command and gives the data after the status byte; keep-alive frames are skipped, an error status is a sentence.
    fn command(&mut self, ctap_cmd: u8, params: Option<&Value>) -> Result<Vec<u8>, String> {
        let mut msg = vec![ctap_cmd];
        if let Some(p) = params {
            msg.extend(cbor::encode(p));
        }
        for f in frames(self.cid, CMD_CBOR, &msg)? {
            self.hid.send(&f)?;
        }
        let mut keepalives = 0;
        loop {
            let mut r = Reassembler::new(self.cid);
            let (cmd, data) = loop {
                if let Some(m) = r.push(&self.hid.recv()?)? {
                    break m;
                }
            };
            match cmd {
                CMD_KEEPALIVE => {
                    keepalives += 1;
                    if keepalives > MAX_KEEPALIVE {
                        return Err("the key never answered".into());
                    }
                }
                CMD_ERROR => return Err(format!("the key reported a transport error 0x{:02x}", data.first().copied().unwrap_or(0))),
                CMD_CBOR => {
                    let status = *data.first().ok_or("the key sent an empty answer")?;
                    return if status == 0 { Ok(data[1..].to_vec()) } else { Err(status_words(status)) };
                }
                _ => return Err("the key sent something else than an answer".into()),
            }
        }
    }

    /// `authenticatorMakeCredential`: ES256, `resident` asks for a discoverable credential, `uv` for user verification. Gives the attestation
    /// object a page expects (`fmt`, `attStmt`, `authData`) and the credential id.
    #[allow(clippy::too_many_arguments)]
    pub fn make_credential(&mut self, rp_id: &str, rp_name: &str, user_id: &[u8], user_name: &str, cd_hash: &[u8; 32], resident: bool, uv: bool, exclude: &[Vec<u8>]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut options = vec![];
        if resident {
            options.push((text("rk"), Value::Bool(true)));
        }
        if uv {
            options.push((text("uv"), Value::Bool(true)));
        }
        let mut m = vec![
            (Value::Int(1), Value::Bytes(cd_hash.to_vec())),
            (Value::Int(2), Value::Map(vec![(text("id"), text(rp_id)), (text("name"), text(rp_name))])),
            (Value::Int(3), Value::Map(vec![(text("id"), Value::Bytes(user_id.to_vec())), (text("name"), text(user_name)), (text("displayName"), text(user_name))])),
            (Value::Int(4), Value::Array(vec![Value::Map(vec![(text("alg"), Value::Int(crate::webauthn::ES256)), (text("type"), text("public-key"))])])),
        ];
        if !exclude.is_empty() {
            m.push((Value::Int(5), Value::Array(exclude.iter().map(|i| desc(i)).collect())));
        }
        if !options.is_empty() {
            m.push((Value::Int(7), Value::Map(options)));
        }
        let ans = cbor::decode_all(&self.command(0x01, Some(&Value::Map(m)))?)?;
        let fmt = ans.get(&Value::Int(1)).ok_or("the key's answer has no format")?.clone();
        let auth = ans.get(&Value::Int(2)).and_then(Value::as_bytes).ok_or("the key's answer has no authenticator data")?.to_vec();
        let stmt = ans.get(&Value::Int(3)).cloned().unwrap_or(Value::Map(vec![]));
        let id = credential_id(&auth).ok_or("the key's authenticator data holds no credential")?;
        Ok((cbor::encode(&Value::Map(vec![(text("fmt"), fmt), (text("attStmt"), stmt), (text("authData"), Value::Bytes(auth))])), id))
    }

    /// `authenticatorGetAssertion` and, when the key says it found several accounts, `getNextAssertion` for each: all of them, newest first as
    /// the key orders them, so the person can choose.
    pub fn get_assertions(&mut self, rp_id: &str, cd_hash: &[u8; 32], allow: &[Vec<u8>], uv: bool) -> Result<Vec<KeyAssertion>, String> {
        let mut m = vec![(Value::Int(1), text(rp_id)), (Value::Int(2), Value::Bytes(cd_hash.to_vec()))];
        if !allow.is_empty() {
            m.push((Value::Int(3), Value::Array(allow.iter().map(|i| desc(i)).collect())));
        }
        if uv {
            m.push((Value::Int(5), Value::Map(vec![(text("uv"), Value::Bool(true))])));
        }
        let first = cbor::decode_all(&self.command(0x02, Some(&Value::Map(m)))?)?;
        let count = first.get(&Value::Int(5)).and_then(Value::as_int).unwrap_or(1).clamp(1, 20) as usize;
        let mut out = vec![parse_assertion(&first, allow)?];
        for _ in 1..count {
            let next = cbor::decode_all(&self.command(0x08, None)?)?;
            out.push(parse_assertion(&next, allow)?);
        }
        Ok(out)
    }
}

/// The credential id inside authenticator data that carries attested credential data.
pub fn credential_id(auth_data: &[u8]) -> Option<Vec<u8>> {
    let n = u16::from_be_bytes([*auth_data.get(53)?, *auth_data.get(54)?]) as usize;
    auth_data.get(55..55 + n).map(<[u8]>::to_vec)
}

fn parse_assertion(v: &Value, allow: &[Vec<u8>]) -> Result<KeyAssertion, String> {
    let id = v.get(&Value::Int(1)).and_then(|c| c.get_text("id")).and_then(Value::as_bytes).map(<[u8]>::to_vec).or_else(|| (allow.len() == 1).then(|| allow[0].clone())).ok_or("the key's answer names no credential")?;
    let auth_data = v.get(&Value::Int(2)).and_then(Value::as_bytes).ok_or("the key's answer has no authenticator data")?.to_vec();
    let signature = v.get(&Value::Int(3)).and_then(Value::as_bytes).ok_or("the key's answer has no signature")?.to_vec();
    let user = v.get(&Value::Int(4));
    let user_handle = user.and_then(|u| u.get_text("id")).and_then(Value::as_bytes).map(<[u8]>::to_vec).unwrap_or_default();
    let user_name = match user.and_then(|u| u.get_text("name")) {
        Some(Value::Text(t)) => t.chars().filter(|c| !c.is_control()).take(64).collect(),
        _ => String::new(),
    };
    Ok(KeyAssertion { credential_id: id, auth_data, signature, user_handle, user_name })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webauthn as wa;

    /// A key made of the software authenticator: it speaks CTAPHID and CTAP2 on the packets it is given.
    struct FakeKey {
        cid: u32,
        incoming: Option<Reassembler>,
        queue: std::collections::VecDeque<[u8; PACKET]>,
        creds: Vec<wa::Credential>,
        resident: Vec<Vec<u8>>,
        pending: Vec<KeyAssertion>,
        /// Send a keep-alive frame before every answer.
        busy: bool,
        /// Make the key refuse everything with this status.
        status: Option<u8>,
        seen_uv: Vec<bool>,
        no_cbor: bool,
    }

    impl FakeKey {
        fn new() -> FakeKey {
            FakeKey { cid: 0x0102_0304, incoming: None, queue: Default::default(), creds: vec![], resident: vec![], pending: vec![], busy: false, status: None, seen_uv: vec![], no_cbor: false }
        }
        fn reply(&mut self, cid: u32, cmd: u8, data: &[u8]) {
            for f in frames(cid, cmd, data).unwrap() {
                self.queue.push_back(f);
            }
        }
        fn answer(&mut self, status: u8, body: Option<Value>) {
            let mut d = vec![status];
            if let Some(b) = body {
                d.extend(cbor::encode(&b));
            }
            if self.busy {
                self.reply(self.cid, CMD_KEEPALIVE, &[1]);
            }
            self.reply(self.cid, CMD_CBOR, &d);
        }
        fn assertion_value(a: &KeyAssertion, more: Option<usize>) -> Value {
            let mut m = vec![(Value::Int(1), desc(&a.credential_id)), (Value::Int(2), Value::Bytes(a.auth_data.clone())), (Value::Int(3), Value::Bytes(a.signature.clone()))];
            if !a.user_handle.is_empty() {
                m.push((Value::Int(4), Value::Map(vec![(text("id"), Value::Bytes(a.user_handle.clone())), (text("name"), text(&a.user_name))])));
            }
            if let Some(n) = more {
                m.push((Value::Int(5), Value::Int(n as i64)));
            }
            Value::Map(m)
        }
        fn handle(&mut self, cmd: u8, data: Vec<u8>) {
            if cmd == CMD_INIT {
                let mut d = data[..8].to_vec();
                d.extend_from_slice(&self.cid.to_be_bytes());
                d.extend_from_slice(&[2, 1, 0, 0, if self.no_cbor { 0 } else { CAP_CBOR }]);
                self.reply(BROADCAST, CMD_INIT, &d);
                return;
            }
            if let Some(s) = self.status {
                return self.answer(s, None);
            }
            let params = if data.len() > 1 { cbor::decode_all(&data[1..]).ok() } else { None };
            match data[0] {
                0x01 => {
                    let p = params.unwrap();
                    let get = |k: i64| p.get(&Value::Int(k)).cloned();
                    let hash: [u8; 32] = get(1).unwrap().as_bytes().unwrap().try_into().unwrap();
                    let rp = match get(2).unwrap().get_text("id") { Some(Value::Text(t)) => t.clone(), _ => String::new() };
                    let user = get(3).unwrap();
                    let handle = user.get_text("id").and_then(Value::as_bytes).unwrap().to_vec();
                    let name = match user.get_text("name") { Some(Value::Text(t)) => t.clone(), _ => String::new() };
                    let exclude: Vec<Vec<u8>> = match get(5) { Some(Value::Array(a)) => a.iter().filter_map(|d| d.get_text("id").and_then(Value::as_bytes).map(<[u8]>::to_vec)).collect(), _ => vec![] };
                    let opts = get(7);
                    let flag = |k: &str| opts.as_ref().and_then(|o| o.get_text(k)).is_some_and(|v| matches!(v, Value::Bool(true)));
                    self.seen_uv.push(flag("uv"));
                    if self.creds.iter().any(|c| c.rp_id == rp && exclude.contains(&c.id)) {
                        return self.answer(0x19, None);
                    }
                    let check = wa::Check { presence: true, verified: flag("uv"), required: false };
                    let made = wa::make_credential_uv(&rp, &hash, &handle, &name, &[wa::ES256], &[], &[], check, 1 + self.creds.len() as u64).unwrap();
                    if flag("rk") {
                        self.resident.push(made.credential.id.clone());
                    }
                    self.creds.push(made.credential);
                    let ao = cbor::decode_all(&made.attestation_object).unwrap();
                    let body = Value::Map(vec![(Value::Int(1), text("none")), (Value::Int(2), ao.get_text("authData").unwrap().clone()), (Value::Int(3), Value::Map(vec![]))]);
                    self.answer(0, Some(body));
                }
                0x02 => {
                    let p = params.unwrap();
                    let hash: [u8; 32] = p.get(&Value::Int(2)).unwrap().as_bytes().unwrap().try_into().unwrap();
                    let rp = match p.get(&Value::Int(1)) { Some(Value::Text(t)) => t.clone(), _ => String::new() };
                    let allow: Vec<Vec<u8>> = match p.get(&Value::Int(3)) { Some(Value::Array(a)) => a.iter().filter_map(|d| d.get_text("id").and_then(Value::as_bytes).map(<[u8]>::to_vec)).collect(), _ => vec![] };
                    let uv = p.get(&Value::Int(5)).and_then(|o| o.get_text("uv")).is_some_and(|v| matches!(v, Value::Bool(true)));
                    self.seen_uv.push(uv);
                    let pool: Vec<wa::Credential> = self.creds.iter().filter(|c| !allow.is_empty() || self.resident.contains(&c.id)).cloned().collect();
                    let cands = wa::candidates(&rp, &allow, &pool);
                    if cands.is_empty() {
                        return self.answer(0x2E, None);
                    }
                    let mut all = vec![];
                    for (id, ..) in &cands {
                        let a = wa::get_assertion_uv(&rp, &hash, &allow, Some(id), &mut self.creds, wa::Check { presence: true, verified: uv, required: false }).unwrap();
                        let name = self.creds.iter().find(|c| &c.id == id).map(|c| c.user_name.clone()).unwrap_or_default();
                        all.push(KeyAssertion { credential_id: a.credential_id, auth_data: a.auth_data, signature: a.signature, user_handle: if allow.len() == 1 { vec![] } else { a.user_handle }, user_name: name });
                    }
                    let first = all.remove(0);
                    let n = 1 + all.len();
                    self.pending = all;
                    self.pending.reverse();
                    self.answer(0, Some(Self::assertion_value(&first, (n > 1).then_some(n))));
                }
                0x08 => match self.pending.pop() {
                    Some(a) => self.answer(0, Some(Self::assertion_value(&a, None))),
                    None => self.answer(0x2E, None),
                },
                _ => self.answer(0x01, None),
            }
        }
    }

    impl Hid for FakeKey {
        fn send(&mut self, p: &[u8; PACKET]) -> Result<(), String> {
            let cid = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
            let r = self.incoming.get_or_insert_with(|| Reassembler::new(cid));
            if let Some((cmd, data)) = r.push(p)? {
                self.incoming = None;
                self.handle(cmd, data);
            }
            Ok(())
        }
        fn recv(&mut self) -> Result<[u8; PACKET], String> {
            self.queue.pop_front().ok_or_else(|| "the key did not answer".to_string())
        }
    }

    const H: [u8; 32] = [7u8; 32];

    #[test]
    fn a_message_of_any_length_is_cut_into_packets_and_put_back_and_bad_order_or_size_is_an_error() {
        for n in [0usize, 1, 56, 57, 58, 115, 116, 117, 1000, MAX_MESSAGE] {
            let data: Vec<u8> = (0..n).map(|i| i as u8).collect();
            let fr = frames(0xAABB_CCDD, CMD_CBOR, &data).unwrap();
            let mut r = Reassembler::new(0xAABB_CCDD);
            let mut done = None;
            for f in &fr {
                done = r.push(f).unwrap();
            }
            assert_eq!(done, Some((CMD_CBOR, data)), "{n} bytes in {} packets", fr.len());
        }
        assert!(frames(1, CMD_CBOR, &vec![0; MAX_MESSAGE + 1]).is_err());
        let fr = frames(5, CMD_CBOR, &[1u8; 200]).unwrap();
        let mut r = Reassembler::new(5);
        r.push(&fr[0]).unwrap();
        assert!(r.push(&fr[2]).is_err(), "a packet out of order");
        let mut r = Reassembler::new(5);
        assert!(r.push(&fr[1]).is_err(), "a continuation first");
        let mut other = fr[0];
        other[0] = 9;
        assert_eq!(Reassembler::new(5).push(&other), Ok(None), "another channel is ignored");
        let mut huge = fr[0];
        huge[5] = 0xFF;
        huge[6] = 0xFF;
        assert!(Reassembler::new(5).push(&huge).is_err());
    }

    #[test]
    fn a_key_makes_a_credential_and_signs_in_and_the_sites_own_checks_accept_both() {
        let challenge = [3u8; 32];
        let origin = "https://login.example.com";
        let cd = wa::client_data_json("webauthn.create", &challenge, origin);
        let mut key = Key::open(FakeKey::new(), *b"nonce-01").unwrap();
        key.hid.busy = true;
        let (att, id) = key.make_credential("example.com", "Example", b"user-1", "ana", &wa::client_data_hash(&cd), true, false, &[]).unwrap();
        let reg = wa::verify_registration(&att, cd.as_bytes(), &challenge, origin, "example.com").unwrap();
        assert_eq!(reg.credential_id, id);
        let cd2 = wa::client_data_json("webauthn.get", &challenge, origin);
        let got = key.get_assertions("example.com", &wa::client_data_hash(&cd2), &[], false).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].user_name.as_str(), got[0].user_handle.as_slice()), ("ana", &b"user-1"[..]));
        assert_eq!(wa::verify_assertion(&reg.public_key, &got[0].auth_data, cd2.as_bytes(), &got[0].signature, &challenge, origin, "example.com", 0), Ok(1));
        // a listed credential works without a user in the answer
        let listed = key.get_assertions("example.com", &wa::client_data_hash(&cd2), &[id.clone()], false).unwrap();
        assert_eq!(listed[0].credential_id, id);
    }

    #[test]
    fn a_discoverable_sign_in_finds_every_account_of_the_site_for_the_person_to_choose_and_user_verification_reaches_the_key() {
        let mut key = Key::open(FakeKey::new(), *b"nonce-02").unwrap();
        for who in ["ana", "bo", "cy"] {
            key.make_credential("example.com", "Example", who.as_bytes(), who, &H, true, true, &[]).unwrap();
        }
        key.make_credential("example.com", "Example", b"nobody", "not discoverable", &H, false, false, &[]).unwrap();
        let got = key.get_assertions("example.com", &H, &[], true).unwrap();
        let names: Vec<&str> = got.iter().map(|a| a.user_name.as_str()).collect();
        assert_eq!(names, ["cy", "bo", "ana"], "the key's order, newest first; a non-discoverable one is not found without a list");
        assert!(key.hid.seen_uv.iter().filter(|u| **u).count() >= 4, "{:?}", key.hid.seen_uv);
        assert!(key.get_assertions("other.org", &H, &[], false).unwrap_err().contains("no passkey for this site"));
    }

    #[test]
    fn every_status_a_key_can_answer_is_a_sentence_and_an_old_or_silent_key_is_said_so() {
        let mut key = Key::open(FakeKey::new(), *b"nonce-03").unwrap();
        key.make_credential("example.com", "E", b"u", "ana", &H, false, false, &[]).unwrap();
        // the same account again with the exclude list
        let id = key.hid.creds[0].id.clone();
        assert!(key.make_credential("example.com", "E", b"u", "ana", &H, false, false, &[id]).unwrap_err().contains("already has a passkey"));
        for (status, word) in [(0x2Eu8, "no passkey"), (0x2F, "not touched"), (0x36, "PIN"), (0x35, "no PIN"), (0x27, "refused"), (0x99, "0x99")] {
            key.hid.status = Some(status);
            let e = key.get_assertions("example.com", &H, &[], false).unwrap_err();
            assert!(e.contains(word), "{status:02x}: {e}");
        }
        let mut old = FakeKey::new();
        old.no_cbor = true;
        assert!(Key::open(old, *b"nonce-04").err().unwrap().contains("U2F"));
        struct Mute;
        impl Hid for Mute {
            fn send(&mut self, _: &[u8; PACKET]) -> Result<(), String> {
                Ok(())
            }
            fn recv(&mut self) -> Result<[u8; PACKET], String> {
                Err("timed out".into())
            }
        }
        assert!(Key::open(Mute, [0; 8]).err().unwrap().contains("timed out"));
    }

    #[test]
    fn a_key_that_keeps_blinking_is_given_up_on_and_hostile_answers_never_panic() {
        struct Blink;
        impl Hid for Blink {
            fn send(&mut self, _: &[u8; PACKET]) -> Result<(), String> {
                Ok(())
            }
            fn recv(&mut self) -> Result<[u8; PACKET], String> {
                let mut p = [0u8; PACKET];
                p[..4].copy_from_slice(&1u32.to_be_bytes());
                p[4] = 0x80 | CMD_KEEPALIVE;
                p[6] = 1;
                p[7] = 2;
                Ok(p)
            }
        }
        let mut k = Key { hid: Blink, cid: 1 };
        assert!(k.command(0x02, None).unwrap_err().contains("never answered"));
        // garbage CBOR and short authenticator data
        assert!(parse_assertion(&Value::Int(3), &[]).is_err());
        assert_eq!(credential_id(&[0u8; 10]), None);
        assert!(parse_assertion(&cbor::decode_all(&[0xA0]).unwrap(), &[]).is_err());
    }
}

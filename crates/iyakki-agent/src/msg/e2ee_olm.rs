//! The Olm and Megolm add-on (F-MSG-08), implementing the hook of [`super::e2ee`] on the in-house cryptography of `iyakki-webcrypto`.
//!
//! Built here: the device's Olm account and its signed device keys and one-time keys (uploaded through [`AddonRequest`]s from
//! [`E2eeAddon::on_sync`]); reading `m.room_key` to-device events that arrive Olm-encrypted; decrypting `m.megolm.v1.aes-sha2` room events
//! with the sender key and session checked, replays of a message index refused; Megolm outbound sessions per room and the Olm encryption
//! of a room key for one device ([`OlmAddon::share_room_key`]).
//!
//! NOT built (the remainder of the row): asking the homeserver which devices the members of a room have (`/keys/query`), claiming their
//! one-time keys (`/keys/claim`) and posting the room key to them (`/sendToDevice`), which needs the host to hand the answers back to the
//! add-on; device verification (cross-signing, SAS, QR); key backup and room-key export. Until a room's key has been shared, [`encrypt`]
//! refuses: IYAKKI never sends a message nobody could read, and never sends text in the clear into an encrypted room.
//! The add-on is registered by the add-on loader (`e2ee::register`); no loader or download is built, so nothing registers it in the image.
//!
//! The whole state is one JSON text kept in the person's vault under [`vault_name`]; it holds private keys and is never logged.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iyakki_webcrypto::megolm::{InboundGroup, OutboundGroup, Ratchet};
use iyakki_webcrypto::olm::{key_from_b64, Account, Session};
use iyakki_webcrypto::b64;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::e2ee::{AddonRequest, E2eeAddon};
use crate::gitkeys::SecretStore;

pub const OLM_ALGORITHM: &str = "m.olm.v1.curve25519-aes-sha2";
pub const MEGOLM_ALGORITHM: &str = "m.megolm.v1.aes-sha2";
/// Fewer unused one-time keys than this on the server: make more (the server keeps up to 50 of ours).
const LOW_WATER: u64 = 25;
const BATCH: usize = 50;

/// The vault entry of an account's end-to-end state.
pub fn vault_name(account_id: &str) -> String {
    format!("msg.e2ee.{account_id}")
}

/// Matrix canonical JSON: keys sorted, no spaces, strings as UTF-8 (what device keys and one-time keys are signed over).
pub fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            format!("{{{}}}", keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical_json(&m[*k]))).collect::<Vec<_>>().join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    account: String,
    device_keys_uploaded: bool,
    sessions: Vec<(String, String)>,
    inbound: Vec<(String, String, String, String)>,
    outbound: Vec<(String, String, u32, String, bool)>,
    seen: Vec<(String, u32, String)>,
}

struct Inner {
    account: Account,
    device_keys_uploaded: bool,
    sessions: Vec<(String, Session)>,
    /// (room, session id) -> (sender key, session)
    inbound: BTreeMap<(String, String), (String, InboundGroup)>,
    /// room -> (session, shared)
    outbound: BTreeMap<String, (OutboundGroup, bool)>,
    /// (session id, message index) -> event id, to refuse a replay
    seen: BTreeMap<(String, u32), String>,
    /// To-device messages the last sync refused (reasons only, no keys)
    problems: Vec<String>,
}

pub struct OlmAddon {
    user_id: String,
    device_id: String,
    store: Arc<dyn SecretStore>,
    vault_key: String,
    inner: Mutex<Inner>,
}

impl OlmAddon {
    /// Opens the saved state of an account, or makes a new device identity. Needs the vault to be unlocked.
    pub fn open(store: Arc<dyn SecretStore>, account_id: &str, user_id: &str, device_id: &str) -> Result<OlmAddon, String> {
        let key = vault_name(account_id);
        let inner = match store.get(&key) {
            Ok(text) => Self::load(&text)?,
            Err(e) if e.contains("locked") => return Err("your vault is locked: sign in again, then try again".into()),
            Err(_) => Inner { account: Account::new()?, device_keys_uploaded: false, sessions: vec![], inbound: BTreeMap::new(), outbound: BTreeMap::new(), seen: BTreeMap::new(), problems: vec![] },
        };
        let a = OlmAddon { user_id: user_id.into(), device_id: device_id.into(), store, vault_key: key, inner: Mutex::new(inner) };
        a.save(&a.lock())?;
        Ok(a)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn load(text: &str) -> Result<Inner, String> {
        let s: Saved = serde_json::from_str(text).map_err(|_| "the saved encryption state is damaged".to_string())?;
        let bad = |_| "the saved encryption state is damaged".to_string();
        let mut inner = Inner { account: Account::from_bytes(&b64::decode(&s.account).map_err(bad)?)?, device_keys_uploaded: s.device_keys_uploaded, sessions: vec![], inbound: BTreeMap::new(), outbound: BTreeMap::new(), seen: BTreeMap::new(), problems: vec![] };
        for (k, d) in s.sessions {
            inner.sessions.push((k, Session::from_bytes(&b64::decode(&d).map_err(bad)?)?));
        }
        for (room, sid, sender, d) in s.inbound {
            inner.inbound.insert((room, sid), (sender, InboundGroup::from_bytes(&b64::decode(&d).map_err(bad)?)?));
        }
        for (room, data, counter, seed, shared) in s.outbound {
            let (data, seed) = (b64::decode(&data).map_err(bad)?, b64::decode(&seed).map_err(bad)?);
            let r = Ratchet::from_parts(data.try_into().map_err(|_| "damaged")?, counter);
            inner.outbound.insert(room, (OutboundGroup::from_parts(r, seed.try_into().map_err(|_| "damaged")?), shared));
        }
        for (sid, idx, ev) in s.seen {
            inner.seen.insert((sid, idx), ev);
        }
        Ok(inner)
    }

    fn save(&self, i: &Inner) -> Result<(), String> {
        let s = Saved {
            account: b64::std(&i.account.to_bytes()),
            device_keys_uploaded: i.device_keys_uploaded,
            sessions: i.sessions.iter().map(|(k, s)| (k.clone(), b64::std(&s.to_bytes()))).collect(),
            inbound: i.inbound.iter().map(|((r, sid), (sender, g))| (r.clone(), sid.clone(), sender.clone(), b64::std(&g.to_bytes()))).collect(),
            outbound: i
                .outbound
                .iter()
                .map(|(room, (g, shared))| {
                    let (r, seed) = g.to_parts();
                    (room.clone(), b64::std(r.data()), r.counter(), b64::std(&seed), *shared)
                })
                .collect(),
            // the replay memory is bounded
            seen: i.seen.iter().rev().take(5000).map(|((s, n), e)| (s.clone(), *n, e.clone())).collect(),
        };
        self.store.set(&self.vault_key, &serde_json::to_string(&s).map_err(|e| e.to_string())?)
    }

    pub fn curve25519_key(&self) -> String {
        self.lock().account.curve25519_key()
    }
    pub fn ed25519_key(&self) -> String {
        self.lock().account.ed25519_key()
    }

    fn signed(&self, i: &Inner, body: Value) -> Value {
        let sig = i.account.sign(canonical_json(&body).as_bytes());
        let mut o = body;
        o["signatures"] = json!({ &self.user_id: { format!("ed25519:{}", self.device_id): sig } });
        o
    }

    fn device_keys(&self, i: &Inner) -> Value {
        self.signed(
            i,
            json!({
                "user_id": self.user_id, "device_id": self.device_id,
                "algorithms": [OLM_ALGORITHM, MEGOLM_ALGORITHM],
                "keys": { format!("curve25519:{}", self.device_id): i.account.curve25519_key(), format!("ed25519:{}", self.device_id): i.account.ed25519_key() },
            }),
        )
    }

    /// The Megolm session key to send to the devices of the room, and the session id. Makes the room's session if there is none.
    pub fn room_key_content(&self, room_id: &str) -> Result<Value, String> {
        let mut i = self.lock();
        if !i.outbound.contains_key(room_id) {
            i.outbound.insert(room_id.into(), (OutboundGroup::new()?, false));
        }
        let (g, _) = &i.outbound[room_id];
        let c = json!({"algorithm": MEGOLM_ALGORITHM, "room_id": room_id, "session_id": g.session_id(), "session_key": g.session_key()});
        self.save(&i)?;
        Ok(c)
    }

    /// The Olm-encrypted `m.room_key` for ONE device, given what `/keys/query` and `/keys/claim` returned for it. Returns the content of
    /// the `m.room.encrypted` to-device event. The caller sends it and then calls [`OlmAddon::mark_shared`].
    pub fn share_room_key(&self, room_id: &str, user_id: &str, their_curve: &str, their_ed: &str, one_time_key: Option<&str>) -> Result<Value, String> {
        let room_key = self.room_key_content(room_id)?;
        let mut i = self.lock();
        let at = i.sessions.iter().position(|(k, _)| k == their_curve);
        let at = match at {
            Some(a) => a,
            None => {
                let otk = one_time_key.ok_or("no Olm session with that device and no one-time key to start one")?;
                let s = Session::outbound(&i.account, &key_from_b64(their_curve)?, &key_from_b64(otk)?)?;
                i.sessions.push((their_curve.into(), s));
                i.sessions.len() - 1
            }
        };
        let payload = json!({"type": "m.room_key", "content": room_key, "sender": self.user_id, "sender_device": self.device_id, "keys": {"ed25519": i.account.ed25519_key()}, "recipient": user_id, "recipient_keys": {"ed25519": their_ed}});
        let m = i.sessions[at].1.encrypt(payload.to_string().as_bytes())?;
        let content = json!({"algorithm": OLM_ALGORITHM, "sender_key": i.account.curve25519_key(), "ciphertext": { their_curve: {"type": m.kind, "body": m.body_b64()} }});
        self.save(&i)?;
        Ok(content)
    }

    /// The reasons the last sync refused some to-device messages (taken once; the requests of that sync are still returned).
    pub fn take_problems(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().problems)
    }

    pub fn mark_shared(&self, room_id: &str) -> Result<(), String> {
        let mut i = self.lock();
        i.outbound.get_mut(room_id).ok_or("no session for that room")?.1 = true;
        self.save(&i)
    }

    fn handle_to_device(&self, i: &mut Inner, ev: &Value) -> Result<(), String> {
        let c = &ev["content"];
        if ev["type"] != "m.room.encrypted" || c["algorithm"] != OLM_ALGORITHM {
            return Ok(());
        }
        let sender_key = c["sender_key"].as_str().ok_or("no sender key")?.to_string();
        let mine = i.account.curve25519_key();
        let ct = &c["ciphertext"][&mine];
        let (kind, body) = (ct["type"].as_u64().ok_or("not for this device")? as u8, b64::decode(ct["body"].as_str().ok_or("no body")?)?);
        let mut plain = None;
        for (k, s) in i.sessions.iter_mut().filter(|(k, _)| *k == sender_key) {
            let _ = k;
            if let Ok(p) = s.decrypt(kind, &body) {
                plain = Some(p);
                break;
            }
        }
        if plain.is_none() && kind == 0 {
            let (s, p) = Session::inbound(&mut i.account, &body)?;
            i.sessions.push((sender_key.clone(), s));
            plain = Some(p);
        }
        let payload: Value = serde_json::from_slice(&plain.ok_or("the to-device message did not open")?).map_err(|_| "bad payload")?;
        // the spec's checks: it was meant for this user and device key
        if payload["recipient"] != self.user_id || payload["recipient_keys"]["ed25519"] != i.account.ed25519_key() {
            return Err("an encrypted to-device message that was meant for someone else".into());
        }
        if payload["type"] == "m.room_key" && payload["content"]["algorithm"] == MEGOLM_ALGORITHM {
            let (room, sid, key) = (payload["content"]["room_id"].as_str().ok_or("no room")?, payload["content"]["session_id"].as_str().ok_or("no session id")?, payload["content"]["session_key"].as_str().ok_or("no session key")?);
            let g = InboundGroup::from_session_key(key)?;
            if g.session_id() != sid {
                return Err("the session id does not match its key".into());
            }
            // an existing session of that id is never replaced (it could be an attempt to rewrite what we can read)
            i.inbound.entry((room.into(), sid.into())).or_insert((sender_key, g));
        }
        Ok(())
    }
}

impl E2eeAddon for OlmAddon {
    fn name(&self) -> String {
        "Matrix end-to-end encryption (Olm and Megolm, built in) 0.1".into()
    }

    fn encrypt(&self, room_id: &str, event_type: &str, content: &Value) -> Result<(String, Value), String> {
        let mut i = self.lock();
        let shared = i.outbound.get(room_id).is_some_and(|(_, s)| *s);
        if !shared {
            return Err("this room's encryption key has not been shared with its devices yet (looking up the members' devices and sending them the key is not built), so nothing is sent".into());
        }
        let sender_key = i.account.curve25519_key();
        let (g, _) = i.outbound.get_mut(room_id).ok_or("no session")?;
        let plain = json!({"type": event_type, "content": content, "room_id": room_id}).to_string();
        let ciphertext = g.encrypt(plain.as_bytes())?;
        let out = json!({"algorithm": MEGOLM_ALGORITHM, "sender_key": sender_key, "ciphertext": ciphertext, "session_id": g.session_id(), "device_id": self.device_id});
        self.save(&i)?;
        Ok(("m.room.encrypted".into(), out))
    }

    fn decrypt(&self, room_id: &str, event: &Value) -> Result<Value, String> {
        let c = &event["content"];
        if c["algorithm"] != MEGOLM_ALGORITHM {
            return Err("an encryption this add-on does not know".into());
        }
        let (sid, ct) = (c["session_id"].as_str().ok_or("no session id")?, c["ciphertext"].as_str().ok_or("no ciphertext")?);
        let mut i = self.lock();
        let (sender, g) = i.inbound.get_mut(&(room_id.to_string(), sid.to_string())).ok_or("the key for this message has not arrived (ask the sender's device to share it)")?;
        if c["sender_key"].as_str().is_some_and(|k| k != sender) {
            return Err("the sender key of the message is not the one that gave us the session".into());
        }
        let (plain, index) = g.decrypt(ct)?;
        let id = event["event_id"].as_str().unwrap_or("").to_string();
        match i.seen.get(&(sid.to_string(), index)) {
            Some(prev) if *prev != id || id.is_empty() => return Err("a message with this index was read before: a replay is refused".into()),
            _ => {
                i.seen.insert((sid.to_string(), index), id);
            }
        }
        let v: Value = serde_json::from_slice(&plain).map_err(|_| "the decrypted event is not JSON")?;
        if v["room_id"] != room_id {
            return Err("the decrypted event names another room".into());
        }
        self.save(&i)?;
        Ok(v["content"].clone())
    }

    fn on_sync(&self, sync: &Value) -> Result<Vec<AddonRequest>, String> {
        let mut i = self.lock();
        let mut problems = Vec::new();
        for ev in sync["to_device"]["events"].as_array().into_iter().flatten() {
            if let Err(e) = self.handle_to_device(&mut i, ev) {
                problems.push(e);
            }
        }
        let mut reqs = Vec::new();
        let on_server = sync["device_one_time_keys_count"]["signed_curve25519"].as_u64().unwrap_or(0);
        let need_otk = on_server < LOW_WATER;
        if !i.device_keys_uploaded || need_otk {
            let mut body = Map::new();
            if !i.device_keys_uploaded {
                body.insert("device_keys".into(), self.device_keys(&i));
            }
            if need_otk {
                i.account.generate_one_time_keys(BATCH)?;
                let mut keys = Map::new();
                for (id, k) in i.account.unpublished_one_time_keys() {
                    keys.insert(format!("signed_curve25519:{id}"), self.signed(&i, json!({"key": k})));
                }
                body.insert("one_time_keys".into(), Value::Object(keys));
                i.account.mark_keys_as_published();
            }
            i.device_keys_uploaded = true;
            reqs.push(AddonRequest { method: "POST".into(), path: "v3/keys/upload".into(), body: Value::Object(body) });
        }
        self.save(&i)?;
        let refused = !problems.is_empty();
        i.problems = problems;
        if refused && reqs.is_empty() {
            return Err(format!("{} to-device message(s) were refused: {}", i.problems.len(), i.problems[0]));
        }
        Ok(reqs)
    }
}

#[cfg(test)]
#[path = "e2ee_olm_tests.rs"]
mod tests;

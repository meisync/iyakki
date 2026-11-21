//! The Olm and Megolm add-on (F-MSG-08), implementing the hook of [`super::e2ee`] on the in-house cryptography of `iyakki-webcrypto`.
//!
//! Built here: the device's Olm account and its signed device keys and one-time keys (uploaded through [`AddonRequest`]s from
//! [`E2eeAddon::on_sync`]); reading `m.room_key` to-device events that arrive Olm-encrypted; decrypting `m.megolm.v1.aes-sha2` room events
//! with the sender key and session checked, replays of a message index refused; Megolm outbound sessions per room and the Olm encryption
//! of a room key for one device ([`OlmAddon::share_room_key`]).
//!
//! Key sharing (alpha.7): before a message goes into an encrypted room the host asks [`E2eeAddon::prepare_send`], carries out the
//! requests (`/keys/query`, `/keys/claim`, `sendToDevice`) and gives each answer back through [`E2eeAddon::on_response`]; devices must be
//! signed by their own Ed25519 key, one-time keys by the device, a device whose signing key changes is refused, a new member turns the
//! session over, and when devices exist but none could be given the key nothing is sent.
//!
//! NOT built: device verification (cross-signing, SAS, QR: so every device is trusted on first use and says so nowhere yet); key backup
//! and room-key export; requesting a missing key from other devices. Until a room's key has been shared, [`encrypt`] refuses: IYAKKI
//! never sends a message nobody could read, and never sends text in the clear into an encrypted room.
//! The add-on is registered by the add-on loader (`e2ee::register`); no loader or download is built, so nothing registers it in the image.
//!
//! The whole state is one JSON text kept in the person's vault under [`vault_name`]; it holds private keys and is never logged.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iyakki_webcrypto::keyexport;
use iyakki_webcrypto::megolm::{InboundGroup, OutboundGroup, Ratchet};
use iyakki_webcrypto::olm::{key_from_b64, Account, Session};
use iyakki_webcrypto::b64;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::e2ee::{AddonRequest, E2eeAddon, KeyAsk, Verification};

#[path = "crosssign.rs"]
mod crosssign;
#[path = "keyshare_flow.rs"]
mod keyshare_flow;
#[path = "sas_flow.rs"]
mod sas_flow;
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

/// The most sessions one export file may add.
const MAX_IMPORT: usize = 20_000;

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    account: String,
    device_keys_uploaded: bool,
    sessions: Vec<(String, String)>,
    inbound: Vec<(String, String, String, String)>,
    outbound: Vec<(String, String, u32, String, bool)>,
    seen: Vec<(String, u32, String)>,
    /// Devices this one verified by SAS: (user, device).
    #[serde(default)]
    verified: Vec<(String, String)>,
    /// The cross-signing seeds (master, self-signing, user-signing) and the masters met.
    #[serde(default)]
    cross: Option<(String, String, String, Vec<(String, String)>)>,
    /// Other people whose master key this person signed: (user, master public key).
    #[serde(default)]
    user_signed: Vec<(String, String)>,
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
    /// Devices of other people learned from `/keys/query` (user, device) -> (curve25519, ed25519); kept for this run only. A device whose
    /// signing key changes later is refused, never silently taken.
    devices: BTreeMap<(String, String), (String, String)>,
    /// The key sharing in progress (one at a time).
    job: Option<Job>,
    /// The members each room's key went to in this run (a member who is not in the list makes the session turn over).
    shared_with: BTreeMap<String, Vec<String>>,
    /// How many key messages were made (for transaction ids).
    txns: u64,
    /// The device verifications (SAS) and what waits to be sent for them.
    flows: sas_flow::Flows,
    /// Megolm sessions a key request was already made for in this run: (room, session id).
    requested: std::collections::BTreeSet<(String, String)>,
    /// Other devices' requests for a room key and where each stands.
    asks: BTreeMap<String, keyshare_flow::Ask>,
    /// Cross-signing keys and what the chain vouches for.
    cross: crosssign::CrossState,
}

struct Job {
    room: String,
    members: Vec<String>,
    stage: Stage,
}

#[derive(PartialEq)]
enum Stage {
    Querying,
    Claiming,
    Delivering,
}

/// Most people whose devices one room key goes to, and most devices in all.
const MAX_MEMBERS: usize = 200;
const MAX_DEVICES: usize = 400;

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
            Err(_) => Inner { account: Account::new()?, device_keys_uploaded: false, sessions: vec![], inbound: BTreeMap::new(), outbound: BTreeMap::new(), seen: BTreeMap::new(), problems: vec![], devices: BTreeMap::new(), job: None, shared_with: BTreeMap::new(), txns: 0, flows: Default::default(), requested: Default::default(), asks: Default::default(), cross: Default::default() },
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
        let mut inner = Inner { account: Account::from_bytes(&b64::decode(&s.account).map_err(bad)?)?, device_keys_uploaded: s.device_keys_uploaded, sessions: vec![], inbound: BTreeMap::new(), outbound: BTreeMap::new(), seen: BTreeMap::new(), problems: vec![], devices: BTreeMap::new(), job: None, shared_with: BTreeMap::new(), txns: 0, flows: Default::default(), requested: Default::default(), asks: Default::default(), cross: Default::default() };
        inner.flows.verified = s.verified.iter().cloned().collect();
        if let Some(c) = &s.cross {
            Self::cross_restore(&mut inner, c)?;
        }
        inner.cross.user_signed = s.user_signed.iter().cloned().collect();
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
            verified: i.flows.verified.iter().cloned().collect(),
            cross: self.cross_saved(i),
            user_signed: i.cross.user_signed.iter().map(|(a, b)| (a.clone(), b.clone())).collect(),
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
            let g = OutboundGroup::new()?;
            // a copy this device can read and export: its own sessions are part of the key export
            let own = (i.account.curve25519_key(), InboundGroup::from_session_key(&g.session_key())?);
            i.inbound.insert((room_id.to_string(), g.session_id()), own);
            i.outbound.insert(room_id.into(), (g, false));
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

    /// A device object of `/keys/query`, checked: for this user and device, with both keys, signed by its own Ed25519 key. Returns
    /// (curve25519, ed25519).
    fn checked_device(user: &str, dev: &str, dk: &Value) -> Result<(String, String), String> {
        if dk["user_id"] != user || dk["device_id"] != dev {
            return Err(format!("a device entry of {user} names another user or device"));
        }
        let (Some(curve), Some(ed)) = (dk["keys"][format!("curve25519:{dev}")].as_str(), dk["keys"][format!("ed25519:{dev}")].as_str()) else {
            return Err(format!("device {dev} of {user} has no keys"));
        };
        let sig = dk["signatures"][user][format!("ed25519:{dev}")].as_str().ok_or_else(|| format!("device {dev} of {user} is not signed"))?;
        let mut body = dk.clone();
        let o = body.as_object_mut().ok_or("not an object")?;
        o.remove("signatures");
        o.remove("unsigned");
        iyakki_webcrypto::olm::verify_ed25519(ed, canonical_json(&body).as_bytes(), sig).map_err(|e| format!("device {dev} of {user}: {e}"))?;
        key_from_b64(curve)?;
        Ok((curve.to_string(), ed.to_string()))
    }

    fn after_query(&self, resp: &Value) -> Result<Vec<AddonRequest>, String> {
        let mut i = self.lock();
        let Some(job) = i.job.take().filter(|j| j.stage == Stage::Querying) else {
            // not a key-sharing lookup: a device asked for by a verification
            self.learn_devices(&mut i, resp);
            self.sas_after_query(&mut i);
            self.ks_after_query(&mut i);
            let out = std::mem::take(&mut i.flows.outbox);
            self.save(&i)?;
            return Ok(out);
        };
        let mut problems = Vec::new();
        let mut found: Vec<(String, String, String, String)> = Vec::new();
        for user in &job.members {
            for (dev, dk) in resp["device_keys"][user].as_object().into_iter().flatten() {
                if *user == self.user_id && *dev == self.device_id {
                    continue;
                }
                match Self::checked_device(user, dev, dk) {
                    Ok((curve, ed)) => {
                        // trust on first use: a device that shows up again with another signing key is refused
                        if i.devices.get(&(user.clone(), dev.clone())).is_some_and(|(_, old)| *old != ed) {
                            problems.push(format!("device {dev} of {user} changed its signing key and is left out"));
                        } else {
                            found.push((user.clone(), dev.clone(), curve, ed));
                        }
                    }
                    Err(e) => problems.push(e),
                }
            }
        }
        if found.len() > MAX_DEVICES {
            return Err(format!("the members have more than {MAX_DEVICES} devices: the key is not shared with so many, so nothing is sent"));
        }
        for (u, d, c, e) in &found {
            i.devices.insert((u.clone(), d.clone()), (c.clone(), e.clone()));
        }
        i.problems = problems;
        let mut claim: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        for (u, d, c, _) in &found {
            if !i.sessions.iter().any(|(k, _)| k == c) {
                claim.entry(u.clone()).or_default().insert(d.clone(), json!("signed_curve25519"));
            }
        }
        let room = job.room.clone();
        let members = job.members.clone();
        if claim.is_empty() {
            i.job = Some(Job { room, members, stage: Stage::Claiming });
            drop(i);
            return self.deliver(&BTreeMap::new());
        }
        i.job = Some(Job { room, members, stage: Stage::Claiming });
        let body: Map<String, Value> = claim.into_iter().map(|(u, m)| (u, Value::Object(m))).collect();
        Ok(vec![AddonRequest { method: "POST".into(), path: "v3/keys/claim".into(), body: json!({"one_time_keys": Value::Object(body), "timeout": 10_000}) }])
    }

    /// Remember the devices of a `/keys/query` answer that checked out (own signature, user and device named, trust on first use).
    fn learn_devices(&self, i: &mut Inner, resp: &Value) {
        self.cross_learn(i, resp);
        for (user, devs) in resp["device_keys"].as_object().into_iter().flatten() {
            for (dev, dk) in devs.as_object().into_iter().flatten().take(MAX_DEVICES) {
                if *user == self.user_id && *dev == self.device_id {
                    continue;
                }
                match Self::checked_device(user, dev, dk) {
                    Ok((curve, ed)) => {
                        if i.devices.get(&(user.clone(), dev.clone())).is_some_and(|(_, old)| *old != ed) {
                            i.problems.push(format!("device {dev} of {user} changed its signing key and is left out"));
                        } else {
                            i.devices.insert((user.clone(), dev.clone()), (curve, ed));
                        }
                    }
                    Err(e) => i.problems.push(e),
                }
            }
        }
    }

    fn after_claim(&self, resp: &Value) -> Result<Vec<AddonRequest>, String> {
        let mut otks: BTreeMap<(String, String), String> = BTreeMap::new();
        {
            let mut i = self.lock();
            if i.job.as_ref().is_none_or(|j| j.stage != Stage::Claiming) {
                return Ok(Vec::new());
            }
            let mut problems = Vec::new();
            for (user, devs) in resp["one_time_keys"].as_object().into_iter().flatten() {
                for (dev, keys) in devs.as_object().into_iter().flatten() {
                    let Some((_, ed)) = i.devices.get(&(user.clone(), dev.clone())).cloned() else { continue };
                    let Some((_, obj)) = keys.as_object().and_then(|m| m.iter().find(|(k, _)| k.starts_with("signed_curve25519:"))) else { continue };
                    let mut body = obj.clone();
                    let Some(o) = body.as_object_mut() else { continue };
                    o.remove("signatures");
                    o.remove("unsigned");
                    let sig = obj["signatures"][user][format!("ed25519:{dev}")].as_str().unwrap_or("");
                    match iyakki_webcrypto::olm::verify_ed25519(&ed, canonical_json(&body).as_bytes(), sig) {
                        Ok(()) => {
                            if let Some(k) = obj["key"].as_str() {
                                otks.insert((user.clone(), dev.clone()), k.to_string());
                            }
                        }
                        Err(_) => problems.push(format!("a one-time key of device {dev} of {user} is not signed by the device and was not used")),
                    }
                }
            }
            i.problems.extend(problems);
            if let Some(j) = i.job.as_mut() {
                j.stage = Stage::Delivering;
            }
        }
        self.deliver(&otks)
    }

    /// Make the Olm-encrypted room key for every device of the job and the one `sendToDevice` request that carries them.
    fn deliver(&self, otks: &BTreeMap<(String, String), String>) -> Result<Vec<AddonRequest>, String> {
        let (room, members, devices) = {
            let mut i = self.lock();
            let Some(j) = i.job.as_mut() else { return Ok(Vec::new()) };
            j.stage = Stage::Delivering;
            let (room, members) = (j.room.clone(), j.members.clone());
            let devices: Vec<((String, String), (String, String))> = i.devices.iter().filter(|((u, _), _)| members.contains(u)).map(|(k, v)| (k.clone(), v.clone())).collect();
            (room, members, devices)
        };
        let mut messages: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        let had_devices = !devices.is_empty();
        for ((user, dev), (curve, ed)) in devices {
            match self.share_room_key(&room, &user, &curve, &ed, otks.get(&(user.clone(), dev.clone())).map(String::as_str)) {
                Ok(c) => {
                    messages.entry(user).or_default().insert(dev, c);
                }
                Err(e) => self.lock().problems.push(format!("the key could not be made for device {dev} of {user}: {e}")),
            }
        }
        let mut i = self.lock();
        if messages.is_empty() {
            i.job = None;
            if had_devices {
                // devices exist but none could be given the key: a message now would be one nobody could read
                let why = i.problems.first().cloned().unwrap_or_else(|| "no key could be made".into());
                return Err(format!("the room key reached none of the members' devices ({why}), so nothing is sent"));
            }
            // nobody else has a device: the key stays with this device
            if let Some(g) = i.outbound.get_mut(&room) {
                g.1 = true;
            }
            i.shared_with.insert(room, members);
            self.save(&i)?;
            return Ok(Vec::new());
        }
        i.txns += 1;
        let txn = format!("iykey{}x{}", i.txns, std::process::id());
        let body: Map<String, Value> = messages.into_iter().map(|(u, m)| (u, Value::Object(m))).collect();
        Ok(vec![AddonRequest { method: "PUT".into(), path: format!("v3/sendToDevice/m.room.encrypted/{txn}"), body: json!({"messages": Value::Object(body)}) }])
    }

    /// The reasons the last sync refused some to-device messages (taken once; the requests of that sync are still returned).
    /// The room key export file: every session this device can read, in the Matrix export container under a passphrase the person
    /// chose. The keys leave the vault only inside it. Sessions this device started itself are in it too.
    pub fn export_keys(&self, passphrase: &str, rounds: u32) -> Result<String, String> {
        let i = self.lock();
        let mut list: Vec<Value> = i
            .inbound
            .iter()
            .map(|((room, sid), (sender, g))| json!({"algorithm": MEGOLM_ALGORITHM, "room_id": room, "sender_key": sender, "sender_claimed_keys": {}, "session_id": sid, "session_key": g.export_session_key()}))
            .collect();
        // sessions this device started itself in an earlier run have no inbound copy: they are exported from where they stand now
        let own = i.account.curve25519_key();
        for (room, (g, _)) in &i.outbound {
            if !i.inbound.contains_key(&(room.clone(), g.session_id())) {
                if let Ok(copy) = InboundGroup::from_session_key(&g.session_key()) {
                    list.push(json!({"algorithm": MEGOLM_ALGORITHM, "room_id": room, "sender_key": own, "sender_claimed_keys": {}, "session_id": g.session_id(), "session_key": copy.export_session_key()}));
                }
            }
        }
        let plain = serde_json::to_vec(&list).map_err(|e| e.to_string())?;
        Ok(keyexport::armor(&keyexport::seal(passphrase, &plain, rounds)?))
    }

    /// Reads an export file: every entry is checked (algorithm, session id equal to the key's own, size) and a session already known is
    /// kept unless the file reaches further back. Returns how many sessions were added or improved.
    pub fn import_keys(&self, passphrase: &str, text: &str) -> Result<usize, String> {
        let plain = keyexport::open(passphrase, &keyexport::unarmor(text)?)?;
        let list: Vec<Value> = serde_json::from_slice(&plain).map_err(|_| "the export holds no list of keys")?;
        if list.len() > MAX_IMPORT {
            return Err("the export holds more keys than this device reads at once".into());
        }
        let mut i = self.lock();
        let mut n = 0;
        for e in &list {
            if e["algorithm"] != MEGOLM_ALGORITHM {
                continue;
            }
            let (Some(room), Some(sid), Some(key), Some(sender)) = (e["room_id"].as_str(), e["session_id"].as_str(), e["session_key"].as_str(), e["sender_key"].as_str()) else { continue };
            let Ok(g) = InboundGroup::from_session_key(key) else { continue };
            if g.session_id() != sid {
                continue;
            }
            let k = (room.to_string(), sid.to_string());
            match i.inbound.get(&k) {
                Some((_, old)) if old.first_known_index() <= g.first_known_index() => {}
                _ => {
                    i.inbound.insert(k, (sender.to_string(), g));
                    n += 1;
                }
            }
        }
        if n > 0 {
            self.save(&i)?;
        }
        Ok(n)
    }

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
        if payload["type"] == "m.forwarded_room_key" {
            return self.accept_forwarded(i, ev, &payload["content"]);
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

    fn shares_keys(&self) -> bool {
        true
    }

    fn key_requests(&self, room_id: &str, event: &Value) -> Vec<AddonRequest> {
        let c = &event["content"];
        let (Some(sid), Some(sender_key)) = (c["session_id"].as_str(), c["sender_key"].as_str()) else { return Vec::new() };
        let mut i = self.lock();
        if i.inbound.contains_key(&(room_id.to_string(), sid.to_string())) || !i.requested.insert((room_id.to_string(), sid.to_string())) || i.requested.len() > 500 {
            return Vec::new();
        }
        i.txns += 1;
        let txn = format!("iyreq{}x{}", i.txns, std::process::id());
        let body = json!({"messages": {&self.user_id: {"*": {"action": "request", "request_id": txn, "requesting_device_id": self.device_id, "body": {"algorithm": MEGOLM_ALGORITHM, "room_id": room_id, "sender_key": sender_key, "session_id": sid}}}}});
        vec![AddonRequest { method: "PUT".into(), path: format!("v3/sendToDevice/m.room_key_request/{txn}"), body }]
    }

    fn verifications(&self) -> Vec<Verification> {
        self.sas_cards()
    }
    fn key_asks(&self) -> Vec<KeyAsk> {
        self.ks_cards()
    }
    fn key_ask_answer(&self, id: &str, allow: bool) -> Result<(), String> {
        self.ks_answer(id, allow)
    }
    fn verify_start(&self, user: &str, device: &str) -> Result<(), String> {
        self.sas_start(user, device)
    }
    fn verify_accept(&self, txn: &str) -> Result<(), String> {
        self.sas_accept(txn)
    }
    fn verify_confirm(&self, txn: &str, matches: bool) -> Result<(), String> {
        self.sas_confirm(txn, matches)
    }
    fn verify_cancel(&self, txn: &str) -> Result<(), String> {
        self.sas_cancel(txn)
    }
    fn is_verified(&self, user: &str, device: &str) -> bool {
        self.lock().flows.verified.contains(&(user.to_string(), device.to_string())) || self.cross_trusted(user, device)
    }
    fn cross_signing_setup(&self, password: &str) -> Result<(), String> {
        self.cross_setup(password)
    }
    fn cross_signing_state(&self) -> String {
        self.cross_state_text()
    }
    fn cross_signing_trust_user(&self, user: &str) -> Result<(), String> {
        self.cross_sign_user(user)
    }
    fn export_keys(&self, passphrase: &str, rounds: u32) -> Result<String, String> {
        OlmAddon::export_keys(self, passphrase, rounds)
    }
    fn import_keys(&self, passphrase: &str, text: &str) -> Result<usize, String> {
        OlmAddon::import_keys(self, passphrase, text)
    }

    fn prepare_send(&self, room_id: &str, members: &[String]) -> Result<Vec<AddonRequest>, String> {
        if members.len() > MAX_MEMBERS {
            return Err(format!("this room has more than {MAX_MEMBERS} members: the key is not shared with so many, so nothing is sent"));
        }
        let mut i = self.lock();
        let shared = i.outbound.get(room_id).is_some_and(|(_, s)| *s);
        if shared {
            match i.shared_with.get(room_id) {
                // nobody new since the key went out (or a restart: the earlier run's list is gone, so it is trusted as it was)
                None => return Ok(Vec::new()),
                Some(have) if members.iter().all(|m| have.contains(m)) => return Ok(Vec::new()),
                // somebody joined: a new session, so that the newcomer cannot read what was said before and everybody gets the new key
                Some(_) => {
                    i.outbound.remove(room_id);
                    i.shared_with.remove(room_id);
                }
            }
        }
        let mut want = Map::new();
        for m in members {
            want.insert(m.clone(), json!([]));
        }
        i.job = Some(Job { room: room_id.into(), members: members.to_vec(), stage: Stage::Querying });
        Ok(vec![AddonRequest { method: "POST".into(), path: "v3/keys/query".into(), body: json!({"device_keys": Value::Object(want), "timeout": 10_000}) }])
    }

    fn on_response(&self, request: &AddonRequest, status: u16, response: &Value) -> Result<Vec<AddonRequest>, String> {
        let path = request.path.trim_start_matches('/');
        if path == "v3/keys/device_signing/upload" {
            let out = self.cross_response(request, status, response)?;
            return Ok(out);
        }
        if path == "v3/keys/signatures/upload" {
            return self.cross_signature_response(request, status, response);
        }
        if !(200..300).contains(&status) {
            self.lock().job = None;
            return Err(format!("the homeserver refused a key request ({status}), so nothing is sent"));
        }
        if path == "v3/keys/query" && request.body["timeout"] == 10_001 {
            // a device asked for by a verification (not the key sharing): learn it and run the checks that waited for it
            let mut i = self.lock();
            self.learn_devices(&mut i, response);
            self.sas_after_query(&mut i);
            self.ks_after_query(&mut i);
            let out = std::mem::take(&mut i.flows.outbox);
            self.save(&i)?;
            return Ok(out);
        }
        if path == "v3/keys/query" {
            return self.after_query(response);
        }
        if path == "v3/keys/claim" && request.body["timeout"] == 10_002 {
            let mut i = self.lock();
            self.ks_after_claim(&mut i, response);
            let out = std::mem::take(&mut i.flows.outbox);
            self.save(&i)?;
            return Ok(out);
        }
        if path == "v3/keys/claim" {
            return self.after_claim(response);
        }
        if path.starts_with("v3/sendToDevice/") {
            let mut i = self.lock();
            let Some(j) = i.job.take().filter(|j| j.stage == Stage::Delivering) else { return Ok(Vec::new()) };
            i.outbound.get_mut(&j.room).ok_or("no session for that room")?.1 = true;
            i.shared_with.insert(j.room, j.members);
            self.save(&i)?;
        }
        Ok(Vec::new())
    }

    fn on_sync(&self, sync: &Value) -> Result<Vec<AddonRequest>, String> {
        let mut i = self.lock();
        let mut problems = Vec::new();
        for ev in sync["to_device"]["events"].as_array().into_iter().flatten() {
            if ev["type"].as_str().is_some_and(|t| t.starts_with("m.key.verification.")) {
                if let Err(e) = self.sas_event(&mut i, ev) {
                    problems.push(e);
                }
            } else if ev["type"] == "m.room_key_request" {
                if let Err(e) = self.ks_event(&mut i, ev) {
                    problems.push(e);
                }
            } else if let Err(e) = self.handle_to_device(&mut i, ev) {
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
        reqs.extend(std::mem::take(&mut i.flows.outbox));
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

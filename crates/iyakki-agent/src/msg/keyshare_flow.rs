//! Answering the key requests of the person's other devices (F-MSG-08), a child of `e2ee_olm` so it reads the add-on's state.
//!
//! Another device of the same person that cannot read a room's messages sends `m.room_key_request` to all of the person's devices.
//! Here such a request is only noted and shown to the person as a card ([`E2eeAddon::key_asks`]); NOTHING is sent until the person
//! answers yes ([`E2eeAddon::key_ask_answer`]), which the chat tool asks on a hard card. On yes the device keys of the asking device are
//! looked up (`/keys/query`: signed by its own key, the device must belong to this very user), an Olm session is used or started with a
//! claimed one-time key (checked signature) and the session key goes out as an Olm-encrypted `m.forwarded_room_key` to that one device.
//!
//! Only the person's own devices are answered (a request from another user is dropped), only for a session this device can read, never
//! to this device itself, and at most [`MAX_ASKS`] requests wait at once. A cancellation removes a waiting card.

use serde_json::{json, Value};

use super::{canonical_json, Inner, OlmAddon, MEGOLM_ALGORITHM, OLM_ALGORITHM};
use crate::msg::e2ee::{AddonRequest, KeyAsk};
use iyakki_webcrypto::megolm::InboundGroup;
use iyakki_webcrypto::olm::{key_from_b64, Session};

pub(super) const MAX_ASKS: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AskStage {
    Waiting,
    /// The person said yes; the asking device's keys are being looked up.
    Approved,
    /// A one-time key of that device was asked for.
    Claiming,
    Sent,
    Refused,
    Failed(String),
}

pub(super) struct Ask {
    pub device: String,
    pub room: String,
    pub session: String,
    pub stage: AskStage,
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@' | '!' | '=' | '+' | '/'))
}

impl OlmAddon {
    /// One `m.room_key_request` to-device event.
    pub(super) fn ks_event(&self, i: &mut Inner, ev: &Value) -> Result<(), String> {
        let c = &ev["content"];
        // only the person's own devices are ever answered
        if ev["sender"] != self.user_id.as_str() {
            return Ok(());
        }
        let (Some(device), Some(request)) = (c["requesting_device_id"].as_str().filter(|d| plain(d, 128)), c["request_id"].as_str().filter(|d| plain(d, 128))) else {
            return Err("a key request with no usable device or request id".into());
        };
        if device == self.device_id {
            return Ok(());
        }
        let id = format!("{device}/{request}");
        match c["action"].as_str() {
            Some("request_cancellation") => {
                if i.asks.get(&id).is_some_and(|a| a.stage == AskStage::Waiting) {
                    i.asks.remove(&id);
                }
                Ok(())
            }
            Some("request") => {
                let b = &c["body"];
                let (Some(room), Some(sid)) = (b["room_id"].as_str().filter(|r| plain(r, 255)), b["session_id"].as_str().filter(|s| plain(s, 128))) else {
                    return Err("a key request names no room or session".into());
                };
                if b["algorithm"] != MEGOLM_ALGORITHM || !i.inbound.contains_key(&(room.to_string(), sid.to_string())) || i.asks.contains_key(&id) {
                    return Ok(()); // not ours to give, or already known
                }
                if i.asks.len() >= MAX_ASKS {
                    return Err("too many key requests are waiting for you".into());
                }
                i.asks.insert(id, Ask { device: device.to_string(), room: room.to_string(), session: sid.to_string(), stage: AskStage::Waiting });
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(super) fn ks_cards(&self) -> Vec<KeyAsk> {
        let i = self.lock();
        i.asks
            .iter()
            .map(|(id, a)| {
                let (state, next) = match &a.stage {
                    AskStage::Waiting => ("another device of yours asks for the key to a room: waiting for your answer".to_string(), "answer"),
                    AskStage::Approved | AskStage::Claiming => ("you said yes; the key is on its way".to_string(), ""),
                    AskStage::Sent => ("sent".to_string(), ""),
                    AskStage::Refused => ("you refused".to_string(), ""),
                    AskStage::Failed(why) => (format!("not sent: {why}"), ""),
                };
                KeyAsk { id: id.clone(), device: a.device.clone(), room: a.room.clone(), state, next, verified: i.flows.verified.contains(&(self.user_id.clone(), a.device.clone())) || i.cross.trusted.contains(&(self.user_id.clone(), a.device.clone())) }
            })
            .collect()
    }

    pub(super) fn ks_answer(&self, id: &str, allow: bool) -> Result<(), String> {
        let mut i = self.lock();
        let a = i.asks.get_mut(id).ok_or("no key request under that number")?;
        if a.stage != AskStage::Waiting {
            return Err("that key request was answered already".into());
        }
        if !allow {
            a.stage = AskStage::Refused;
            return Ok(());
        }
        a.stage = AskStage::Approved;
        let device = a.device.clone();
        if i.devices.contains_key(&(self.user_id.clone(), device.clone())) {
            self.ks_proceed(&mut i, id);
        } else {
            i.flows.outbox.push(AddonRequest { method: "POST".into(), path: "v3/keys/query".into(), body: json!({"device_keys": {&self.user_id: [device]}, "timeout": 10_001}) });
        }
        Ok(())
    }

    /// After a device lookup: go on with every approved request whose device is known now.
    pub(super) fn ks_after_query(&self, i: &mut Inner) {
        let ids: Vec<String> = i.asks.iter().filter(|(_, a)| a.stage == AskStage::Approved).map(|(k, _)| k.clone()).collect();
        for id in ids {
            let device = i.asks[&id].device.clone();
            if i.devices.contains_key(&(self.user_id.clone(), device)) {
                self.ks_proceed(i, &id);
            } else {
                i.asks.get_mut(&id).map(|a| a.stage = AskStage::Failed("the homeserver does not list that device for you, or its keys are not signed by itself".into()));
            }
        }
    }

    /// The device is known: use its Olm session, or ask for a one-time key to start one.
    fn ks_proceed(&self, i: &mut Inner, id: &str) {
        let device = i.asks[id].device.clone();
        let Some((curve, _)) = i.devices.get(&(self.user_id.clone(), device.clone())).cloned() else { return };
        if i.sessions.iter().any(|(k, _)| *k == curve) {
            self.ks_send(i, id, None);
        } else {
            if let Some(a) = i.asks.get_mut(id) {
                a.stage = AskStage::Claiming;
            }
            i.flows.outbox.push(AddonRequest { method: "POST".into(), path: "v3/keys/claim".into(), body: json!({"one_time_keys": {&self.user_id: {device: "signed_curve25519"}}, "timeout": 10_002}) });
        }
    }

    /// The answer of the one-time key claim of a key request.
    pub(super) fn ks_after_claim(&self, i: &mut Inner, resp: &Value) {
        let ids: Vec<String> = i.asks.iter().filter(|(_, a)| a.stage == AskStage::Claiming).map(|(k, _)| k.clone()).collect();
        for id in ids {
            let device = i.asks[&id].device.clone();
            let Some((_, ed)) = i.devices.get(&(self.user_id.clone(), device.clone())).cloned() else { continue };
            let obj = resp["one_time_keys"][&self.user_id][&device].as_object().and_then(|m| m.iter().find(|(k, _)| k.starts_with("signed_curve25519:"))).map(|(_, v)| v.clone());
            let otk = obj.and_then(|obj| {
                let mut body = obj.clone();
                let o = body.as_object_mut()?;
                o.remove("signatures");
                o.remove("unsigned");
                let sig = obj["signatures"][&self.user_id][format!("ed25519:{device}")].as_str()?;
                iyakki_webcrypto::olm::verify_ed25519(&ed, canonical_json(&body).as_bytes(), sig).ok()?;
                obj["key"].as_str().map(String::from)
            });
            match otk {
                Some(k) => self.ks_send(i, &id, Some(&k)),
                None => {
                    if let Some(a) = i.asks.get_mut(&id) {
                        a.stage = AskStage::Failed("the device gave no one-time key signed by itself".into());
                    }
                }
            }
        }
    }

    /// Make the Olm-encrypted `m.forwarded_room_key` for the asking device and queue it.
    fn ks_send(&self, i: &mut Inner, id: &str, otk: Option<&str>) {
        let (device, room, sid) = {
            let a = &i.asks[id];
            (a.device.clone(), a.room.clone(), a.session.clone())
        };
        let res = (|| -> Result<Value, String> {
            let (curve, ed) = i.devices.get(&(self.user_id.clone(), device.clone())).cloned().ok_or("device unknown")?;
            let (sender_key, key) = {
                let (s, g) = i.inbound.get(&(room.clone(), sid.clone())).ok_or("this device cannot read that session any more")?;
                (s.clone(), g.export_session_key())
            };
            let at = match i.sessions.iter().position(|(k, _)| *k == curve) {
                Some(a) => a,
                None => {
                    let otk = otk.ok_or("no Olm session with that device and no one-time key to start one")?;
                    let s = Session::outbound(&i.account, &key_from_b64(&curve)?, &key_from_b64(otk)?)?;
                    i.sessions.push((curve.clone(), s));
                    i.sessions.len() - 1
                }
            };
            let own_ed = i.account.ed25519_key();
            let payload = json!({
                "type": "m.forwarded_room_key",
                "content": {"algorithm": MEGOLM_ALGORITHM, "room_id": room, "sender_key": sender_key, "session_id": sid, "session_key": key, "sender_claimed_ed25519_key": "", "forwarding_curve25519_key_chain": []},
                "sender": self.user_id, "sender_device": self.device_id, "keys": {"ed25519": own_ed},
                "recipient": self.user_id, "recipient_keys": {"ed25519": ed},
            });
            let m = i.sessions[at].1.encrypt(payload.to_string().as_bytes())?;
            Ok(json!({"algorithm": OLM_ALGORITHM, "sender_key": i.account.curve25519_key(), "ciphertext": { curve: {"type": m.kind, "body": m.body_b64()} }}))
        })();
        match res {
            Ok(content) => {
                i.txns += 1;
                let txn = format!("iyfwd{}x{}", i.txns, std::process::id());
                i.flows.outbox.push(AddonRequest { method: "PUT".into(), path: format!("v3/sendToDevice/m.room.encrypted/{txn}"), body: json!({"messages": {&self.user_id: {&device: content}}}) });
                if let Some(a) = i.asks.get_mut(id) {
                    a.stage = AskStage::Sent;
                }
            }
            Err(e) => {
                if let Some(a) = i.asks.get_mut(id) {
                    a.stage = AskStage::Failed(e);
                }
            }
        }
    }

    /// An `m.forwarded_room_key` that arrived Olm-encrypted: taken only from one of the person's own devices, only for a session this
    /// device asked for in this run, and a session already known is replaced only when the new one reaches further back.
    pub(super) fn accept_forwarded(&self, i: &mut Inner, ev: &Value, c: &Value) -> Result<(), String> {
        if ev["sender"] != self.user_id.as_str() {
            return Err("a forwarded room key from another user is not taken".into());
        }
        let (Some(room), Some(sid), Some(key), Some(sender)) = (c["room_id"].as_str(), c["session_id"].as_str(), c["session_key"].as_str(), c["sender_key"].as_str()) else {
            return Err("a forwarded room key with missing parts".into());
        };
        if c["algorithm"] != MEGOLM_ALGORITHM || !i.requested.contains(&(room.to_string(), sid.to_string())) {
            return Err("a forwarded room key that this device did not ask for".into());
        }
        let g = InboundGroup::from_session_key(key)?;
        if g.session_id() != sid {
            return Err("the session id of a forwarded key does not match its key".into());
        }
        let k = (room.to_string(), sid.to_string());
        match i.inbound.get(&k) {
            Some((_, old)) if old.first_known_index() <= g.first_known_index() => {}
            _ => {
                i.inbound.insert(k, (sender.to_string(), g));
            }
        }
        Ok(())
    }
}

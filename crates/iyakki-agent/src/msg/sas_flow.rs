//! The `m.key.verification.*` event flow of device verification by short authentication string (F-MSG-08), a child of `e2ee_olm` so it
//! reads the add-on's state. The cryptography is `iyakki_webcrypto::sas`; this file carries the messages between it and the homeserver
//! (they leave as `sendToDevice` requests with the next sync) and keeps what the person sees: who asked, how far it got, and the strings.
//!
//! * A request from another device waits for the person's own accept ([`OlmAddon::verification_accept`]); nothing is answered before.
//!   A `start` that arrives with no accepted request is kept the same way and answered only after that accept.
//! * Either side may start. The accepting side commits to its key before it shows it; the starting side checks the commitment.
//! * The person compares the seven emoji (or three numbers) with the other device and says so ([`OlmAddon::verification_confirm`]):
//!   no tool or event can say it for them. Only then is the MAC sent; the device is marked verified when the other side's MAC checks out
//!   against the device key this add-on learned from `/keys/query` (a device whose key is unknown is looked up first).
//! * At most [`MAX_FLOWS`] verifications at once; every field of an incoming event is checked and a bad one cancels with a code.

use std::collections::BTreeSet;

use iyakki_webcrypto::sas::{Parties, Sas};
use serde_json::{json, Value};

use super::{canonical_json, Inner, OlmAddon};
use crate::msg::e2ee::{AddonRequest, Verification};

pub(super) const MAX_FLOWS: usize = 8;
const METHOD: &str = "m.sas.v1";
const KEY_AGREEMENT: &str = "curve25519-hkdf-sha256";
const HASH: &str = "sha256";
const MAC_METHOD: &str = "hkdf-hmac-sha256.v2";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    /// We asked; waiting for their `ready`.
    Asked,
    /// They asked (or started); waiting for the person's accept.
    Incoming,
    /// We said ready; waiting for their `start`.
    Ready,
    /// We sent `start`; waiting for their `accept`.
    Started,
    /// Accepted; keys are being exchanged.
    Keys,
    /// Both keys are in: the strings can be compared.
    Compare,
    /// The person said they match and our MAC is out.
    Confirmed,
    Verified,
    Cancelled(String),
}

pub(super) struct Verif {
    pub user: String,
    pub device: String,
    pub started_by_us: bool,
    pub stage: Stage,
    pub sas: Option<Sas>,
    pub start_canon: Option<String>,
    pub commitment: Option<String>,
    pub pending_start: Option<Value>,
    pub their_mac: Option<(Vec<(String, String)>, String)>,
    pub confirmed: bool,
}

#[derive(Default)]
pub(super) struct Flows {
    pub by_txn: std::collections::BTreeMap<String, Verif>,
    /// Pairs (user, device) this device verified (kept in the vault by the add-on).
    pub verified: BTreeSet<(String, String)>,
    /// Requests waiting for the next sync.
    pub outbox: Vec<AddonRequest>,
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@' | '!' | '=' | '+' | '/'))
}

fn list_has(v: &Value, want: &str) -> bool {
    v.as_array().is_some_and(|a| a.iter().any(|x| x == want))
}

impl OlmAddon {
    fn send_td(&self, i: &mut Inner, ty: &str, user: &str, device: &str, content: Value) {
        i.txns += 1;
        let txn = format!("iysas{}x{}", i.txns, std::process::id());
        i.flows.outbox.push(AddonRequest { method: "PUT".into(), path: format!("v3/sendToDevice/{ty}/{txn}"), body: json!({"messages": {user: {device: content}}}) });
    }

    fn cancel_flow(&self, i: &mut Inner, txn: &str, code: &str, reason: &str) {
        let Some(v) = i.flows.by_txn.get_mut(txn) else { return };
        if matches!(v.stage, Stage::Verified | Stage::Cancelled(_)) {
            return;
        }
        v.stage = Stage::Cancelled(reason.to_string());
        let (u, d) = (v.user.clone(), v.device.clone());
        self.send_td(i, "m.key.verification.cancel", &u, &d, json!({"transaction_id": txn, "code": code, "reason": reason}));
    }

    /// Ask for the device keys of `user` when this run has not seen them (the MAC is checked against the key the server shows).
    fn lookup_device(&self, i: &mut Inner, user: &str, device: &str) {
        if i.devices.contains_key(&(user.to_string(), device.to_string())) {
            return;
        }
        i.txns += 1;
        i.flows.outbox.push(AddonRequest { method: "POST".into(), path: "v3/keys/query".into(), body: json!({"device_keys": {user: [device]}, "timeout": 10_001}) });
    }

    /// One `m.key.verification.*` to-device event. An error cancels that verification (when there is one) and is reported as a problem.
    pub(super) fn sas_event(&self, i: &mut Inner, ev: &Value) -> Result<(), String> {
        let ty = ev["type"].as_str().unwrap_or("");
        let c = &ev["content"];
        let sender = ev["sender"].as_str().filter(|s| plain(s, 255)).ok_or("a verification event with no sender")?.to_string();
        let txn = c["transaction_id"].as_str().filter(|t| plain(t, 128)).ok_or("a verification event with no usable transaction id")?.to_string();
        if sender == self.user_id && c["from_device"].as_str() == Some(self.device_id.as_str()) {
            return Ok(()); // our own echo
        }
        let res = self.sas_step(i, ty, &sender, &txn, c);
        if let Err(e) = &res {
            self.cancel_flow(i, &txn, "m.unexpected_message", e);
        }
        res
    }

    fn sas_step(&self, i: &mut Inner, ty: &str, sender: &str, txn: &str, c: &Value) -> Result<(), String> {
        match ty {
            "m.key.verification.request" => {
                if i.flows.by_txn.len() >= MAX_FLOWS && !i.flows.by_txn.contains_key(txn) {
                    return Err("too many verifications at once".into());
                }
                let device = c["from_device"].as_str().filter(|d| plain(d, 128)).ok_or("the request names no device")?.to_string();
                if !list_has(&c["methods"], METHOD) {
                    return Err("the other device offers no verification method this one knows".into());
                }
                self.lookup_device(i, sender, &device);
                i.flows.by_txn.insert(txn.to_string(), Verif { user: sender.to_string(), device, started_by_us: false, stage: Stage::Incoming, sas: None, start_canon: None, commitment: None, pending_start: None, their_mac: None, confirmed: false });
                Ok(())
            }
            "m.key.verification.ready" => {
                let v = i.flows.by_txn.get(txn).filter(|v| v.stage == Stage::Asked && v.user == sender).ok_or("a ready for a verification this device did not ask for")?;
                if c["from_device"].as_str().is_some_and(|d| d != v.device) {
                    return Err("the answer came from another device than the one asked".into());
                }
                if !list_has(&c["methods"], METHOD) {
                    return Err("the other device offers no verification method this one knows".into());
                }
                let (user, device) = (v.user.clone(), v.device.clone());
                let start = json!({"from_device": self.device_id, "method": METHOD, "transaction_id": txn, "key_agreement_protocols": [KEY_AGREEMENT], "hashes": [HASH], "message_authentication_codes": [MAC_METHOD], "short_authentication_string": ["decimal", "emoji"]});
                let sas = Sas::new(Parties { start_user: self.user_id.clone(), start_device: self.device_id.clone(), accept_user: user.clone(), accept_device: device.clone(), transaction: txn.to_string() }, true)?;
                let v = i.flows.by_txn.get_mut(txn).ok_or("gone")?;
                v.started_by_us = true;
                v.start_canon = Some(canonical_json(&start));
                v.sas = Some(sas);
                v.stage = Stage::Started;
                self.send_td(i, "m.key.verification.start", &user, &device, start);
                Ok(())
            }
            "m.key.verification.start" => {
                if c["method"] != METHOD {
                    return Err("a verification method this device does not know".into());
                }
                let device = c["from_device"].as_str().filter(|d| plain(d, 128)).ok_or("the start names no device")?.to_string();
                if !(list_has(&c["key_agreement_protocols"], KEY_AGREEMENT) && list_has(&c["hashes"], HASH) && list_has(&c["message_authentication_codes"], MAC_METHOD) && (list_has(&c["short_authentication_string"], "emoji") || list_has(&c["short_authentication_string"], "decimal"))) {
                    return Err("the start offers no protocol, hash, MAC or strings this device uses".into());
                }
                let at = i.flows.by_txn.get(txn).map(|v| (v.user == sender && v.device == device, v.stage.clone()));
                match at {
                    Some((true, Stage::Ready)) => {
                        if let Some(v) = i.flows.by_txn.get_mut(txn) {
                            v.pending_start = Some(c.clone());
                        }
                        self.answer_start(i, txn)
                    }
                    Some((true, Stage::Incoming)) => {
                        if let Some(v) = i.flows.by_txn.get_mut(txn) {
                            v.pending_start = Some(c.clone());
                        }
                        Ok(())
                    }
                    Some(_) => Err("a start that does not fit this verification".into()),
                    None => {
                        if i.flows.by_txn.len() >= MAX_FLOWS {
                            return Err("too many verifications at once".into());
                        }
                        self.lookup_device(i, sender, &device);
                        i.flows.by_txn.insert(txn.to_string(), Verif { user: sender.to_string(), device, started_by_us: false, stage: Stage::Incoming, sas: None, start_canon: None, commitment: None, pending_start: Some(c.clone()), their_mac: None, confirmed: false });
                        Ok(())
                    }
                }
            }
            "m.key.verification.accept" => {
                let v = i.flows.by_txn.get_mut(txn).filter(|v| v.stage == Stage::Started && v.user == sender).ok_or("an accept for a verification that was not started here")?;
                if c["key_agreement_protocol"] != KEY_AGREEMENT || c["hash"] != HASH || c["message_authentication_code"] != MAC_METHOD {
                    return Err("the other device chose a protocol this device did not offer".into());
                }
                v.commitment = Some(c["commitment"].as_str().filter(|x| plain(x, 128)).ok_or("the accept has no commitment")?.to_string());
                let key = v.sas.as_ref().ok_or("no key")?.public_b64();
                let (u, d) = (v.user.clone(), v.device.clone());
                v.stage = Stage::Keys;
                self.send_td(i, "m.key.verification.key", &u, &d, json!({"transaction_id": txn, "key": key}));
                Ok(())
            }
            "m.key.verification.key" => {
                let their = c["key"].as_str().filter(|k| plain(k, 64)).ok_or("the key message has no key")?.to_string();
                let v = i.flows.by_txn.get_mut(txn).filter(|v| v.user == sender && matches!(v.stage, Stage::Keys | Stage::Started)).ok_or("a key for a verification that is not at that step")?;
                let sas = v.sas.as_mut().ok_or("no key of ours yet")?;
                if v.started_by_us {
                    let (cm, canon) = (v.commitment.clone().ok_or("the key came before the commitment")?, v.start_canon.clone().ok_or("no start")?);
                    sas.set_their_key(&their, Some((&cm, &canon)))?;
                    v.stage = Stage::Compare;
                } else {
                    sas.set_their_key(&their, None)?;
                    let mine = sas.public_b64();
                    let (u, d) = (v.user.clone(), v.device.clone());
                    v.stage = Stage::Compare;
                    self.send_td(i, "m.key.verification.key", &u, &d, json!({"transaction_id": txn, "key": mine}));
                }
                Ok(())
            }
            "m.key.verification.mac" => {
                let v = i.flows.by_txn.get_mut(txn).filter(|v| v.user == sender && matches!(v.stage, Stage::Compare | Stage::Confirmed)).ok_or("a MAC for a verification that is not at that step")?;
                let macs: Vec<(String, String)> = c["mac"].as_object().ok_or("the MAC message has no MACs")?.iter().filter_map(|(k, m)| Some((k.clone(), m.as_str()?.to_string()))).take(20).collect();
                let keys = c["keys"].as_str().ok_or("the MAC message has no list MAC")?.to_string();
                v.their_mac = Some((macs, keys));
                self.try_finish(i, txn)
            }
            "m.key.verification.done" => Ok(()),
            "m.key.verification.cancel" => {
                if let Some(v) = i.flows.by_txn.get_mut(txn).filter(|v| v.user == sender) {
                    if !matches!(v.stage, Stage::Verified) {
                        let why: String = c["reason"].as_str().unwrap_or("the other device ended it").chars().filter(|x| !x.is_control()).take(120).collect();
                        v.stage = Stage::Cancelled(format!("the other device ended it: {why}"));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// We got a `start` (kept in `pending_start`) and the person accepted: commit to our key and send `accept`.
    fn answer_start(&self, i: &mut Inner, txn: &str) -> Result<(), String> {
        let v = i.flows.by_txn.get_mut(txn).ok_or("no such verification")?;
        let start = v.pending_start.take().ok_or("no start to answer")?;
        let sas = Sas::new(Parties { start_user: v.user.clone(), start_device: v.device.clone(), accept_user: self.user_id.clone(), accept_device: self.device_id.clone(), transaction: txn.to_string() }, false)?;
        let commitment = sas.commitment(&canonical_json(&start));
        let (u, d) = (v.user.clone(), v.device.clone());
        v.sas = Some(sas);
        v.start_canon = Some(canonical_json(&start));
        v.stage = Stage::Keys;
        self.send_td(i, "m.key.verification.accept", &u, &d, json!({"transaction_id": txn, "method": METHOD, "key_agreement_protocol": KEY_AGREEMENT, "hash": HASH, "message_authentication_code": MAC_METHOD, "short_authentication_string": ["decimal", "emoji"], "commitment": commitment}));
        Ok(())
    }

    /// When the person confirmed and the other side's MAC is here: check it against the device key this add-on knows and mark the device.
    fn try_finish(&self, i: &mut Inner, txn: &str) -> Result<(), String> {
        let (user, device, confirmed, mac) = {
            let v = i.flows.by_txn.get(txn).ok_or("no such verification")?;
            (v.user.clone(), v.device.clone(), v.confirmed, v.their_mac.clone())
        };
        let (Some((macs, keys)), true) = (mac, confirmed) else { return Ok(()) };
        let Some((_, ed)) = i.devices.get(&(user.clone(), device.clone())).cloned() else {
            // the key lookup is on its way (asked when the verification began); the check runs again after its answer
            self.lookup_device(i, &user, &device);
            return Ok(());
        };
        let known = vec![(format!("ed25519:{device}"), ed)];
        i.flows.by_txn.get(txn).and_then(|v| v.sas.as_ref()).ok_or("no key")?.check_mac(&macs, &keys, &known)?;
        i.flows.verified.insert((user.clone(), device.clone()));
        if let Some(v) = i.flows.by_txn.get_mut(txn) {
            v.stage = Stage::Verified;
        }
        self.send_td(i, "m.key.verification.done", &user, &device, json!({"transaction_id": txn}));
        Ok(())
    }

    /// Run again the checks that waited for device keys (after a `/keys/query` answer).
    pub(super) fn sas_after_query(&self, i: &mut Inner) {
        let waiting: Vec<String> = i.flows.by_txn.iter().filter(|(_, v)| v.confirmed && v.their_mac.is_some() && v.stage == Stage::Confirmed).map(|(t, _)| t.clone()).collect();
        for t in waiting {
            if let Err(e) = self.try_finish(i, &t) {
                self.cancel_flow(i, &t, "m.key_mismatch", &e);
                i.problems.push(e);
            }
        }
    }

    // ---- what the person and the tools do

    pub(super) fn sas_cards(&self) -> Vec<Verification> {
        let i = self.lock();
        i.flows
            .by_txn
            .iter()
            .map(|(txn, v)| {
                let (state, next) = match &v.stage {
                    Stage::Asked => ("asked; waiting for the other device to answer".to_string(), ""),
                    Stage::Incoming => ("the other device wants to verify: waiting for your answer".to_string(), "accept"),
                    Stage::Ready | Stage::Started | Stage::Keys => ("in progress; no strings yet".to_string(), ""),
                    Stage::Compare => ("compare the strings with the other device".to_string(), "confirm"),
                    Stage::Confirmed => ("you said they match; waiting for the other device".to_string(), ""),
                    Stage::Verified => ("verified".to_string(), ""),
                    Stage::Cancelled(why) => (format!("ended: {why}"), ""),
                };
                let (emoji, decimals) = match (&v.stage, &v.sas) {
                    (Stage::Compare | Stage::Confirmed, Some(s)) => (s.emoji().map(|e| e.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()).unwrap_or_default(), s.decimals().ok()),
                    _ => (Vec::new(), None),
                };
                Verification { txn: txn.clone(), user: v.user.clone(), device: v.device.clone(), state, next, emoji, decimals }
            })
            .collect()
    }

    pub(super) fn sas_start(&self, user: &str, device: &str) -> Result<(), String> {
        if !plain(user, 255) || !plain(device, 128) {
            return Err("the user or device name is not usable".into());
        }
        let mut i = self.lock();
        if i.flows.by_txn.len() >= MAX_FLOWS {
            return Err("too many verifications at once; end one first".into());
        }
        i.txns += 1;
        let txn = format!("iyver{}x{}", i.txns, std::process::id());
        self.lookup_device(&mut i, user, device);
        i.flows.by_txn.insert(txn.clone(), Verif { user: user.into(), device: device.into(), started_by_us: true, stage: Stage::Asked, sas: None, start_canon: None, commitment: None, pending_start: None, their_mac: None, confirmed: false });
        let ts = crate::clock::now_unix().max(0) * 1000;
        self.send_td(&mut i, "m.key.verification.request", user, device, json!({"from_device": self.device_id, "methods": [METHOD], "timestamp": ts, "transaction_id": txn}));
        Ok(())
    }

    pub(super) fn sas_accept(&self, txn: &str) -> Result<(), String> {
        let mut i = self.lock();
        let (user, device, has_start) = {
            let v = i.flows.by_txn.get(txn).filter(|v| v.stage == Stage::Incoming).ok_or("no verification is waiting for your answer under that number")?;
            (v.user.clone(), v.device.clone(), v.pending_start.is_some())
        };
        if has_start {
            return self.answer_start(&mut i, txn);
        }
        if let Some(v) = i.flows.by_txn.get_mut(txn) {
            v.stage = Stage::Ready;
        }
        self.send_td(&mut i, "m.key.verification.ready", &user, &device, json!({"from_device": self.device_id, "methods": [METHOD], "transaction_id": txn}));
        Ok(())
    }

    pub(super) fn sas_confirm(&self, txn: &str, matches: bool) -> Result<(), String> {
        let mut i = self.lock();
        if !i.flows.by_txn.get(txn).is_some_and(|v| v.stage == Stage::Compare) {
            return Err("no verification is at the step where the strings are compared under that number".into());
        }
        if !matches {
            self.cancel_flow(&mut i, txn, "m.mismatched_sas", "the strings did not match");
            return Ok(());
        }
        let (user, device, mac) = {
            let v = i.flows.by_txn.get(txn).ok_or("gone")?;
            let keys = vec![(format!("ed25519:{}", self.device_id), i.account.ed25519_key())];
            let (macs, keys_mac) = v.sas.as_ref().ok_or("no key")?.mac_message(&keys)?;
            let obj: serde_json::Map<String, Value> = macs.into_iter().map(|(k, m)| (k, Value::String(m))).collect();
            (v.user.clone(), v.device.clone(), json!({"transaction_id": txn, "mac": Value::Object(obj), "keys": keys_mac}))
        };
        if let Some(v) = i.flows.by_txn.get_mut(txn) {
            v.confirmed = true;
            v.stage = Stage::Confirmed;
        }
        self.send_td(&mut i, "m.key.verification.mac", &user, &device, mac);
        if let Err(e) = self.try_finish(&mut i, txn) {
            self.cancel_flow(&mut i, txn, "m.key_mismatch", &e);
            return Err(e);
        }
        self.save(&i)
    }

    pub(super) fn sas_cancel(&self, txn: &str) -> Result<(), String> {
        let mut i = self.lock();
        if !i.flows.by_txn.contains_key(txn) {
            return Err("no verification under that number".into());
        }
        self.cancel_flow(&mut i, txn, "m.user", "you ended it");
        Ok(())
    }
}

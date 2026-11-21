//! Cross-signing keys (F-MSG-08), a child of `e2ee_olm`: the person's master, self-signing and user-signing Ed25519 keys, made on this
//! device, uploaded once (the homeserver asks for the account password again, typed by the person on the screen, never through the chat),
//! and this device's own key signed by the self-signing key so the person's other devices can trust it from the chain alone.
//!
//! Trust is narrow on purpose. A device of the person is trusted through the chain (device signed by the self-signing key, that key signed
//! by the master key) only when the master key is the person's OWN: made here, or pinned while a device of the person was already verified
//! by SAS. A master key of another person is pinned the first time it is seen (a changed one is refused and reported) but their devices are
//! not called verified from it: that needs this person's own signature on their master key, which `cross_sign_user` makes only when the person
//! asks (a card, after checking the other person's key some other way): the user-signing key signs the master key object, the signature is
//! uploaded, and once the homeserver took it the person's devices count as verified for as long as that master key stays the same.

use std::collections::BTreeSet;

use ed25519_dalek::{Signer, SigningKey};
use iyakki_webcrypto::b64;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{canonical_json, Inner, OlmAddon};
use crate::msg::e2ee::AddonRequest;

pub(super) struct Cross {
    pub master: [u8; 32],
    pub selfsign: [u8; 32],
    pub usersign: [u8; 32],
}

#[derive(Default)]
pub(super) struct CrossState {
    pub keys: Option<Cross>,
    /// Masters seen: user -> master public key (first one seen is kept; another one later is refused).
    pub pinned: std::collections::BTreeMap<String, String>,
    /// Devices whose chain checks out up to a master key this person trusts: (user, device).
    pub trusted: BTreeSet<(String, String)>,
    /// The password for the one retry with authentication (kept in memory for that retry only).
    pub password: Option<Zeroizing<String>>,
    pub upload_body: Option<Value>,
    /// Master key objects of other people as `/keys/query` gave them (checked against their self-signing key); kept for this run, needed to sign one.
    pub masters: std::collections::BTreeMap<String, Value>,
    /// Other people whose master key THIS person signed with the user-signing key (user -> the master public key that was signed). Their devices
    /// count as verified through the chain from then on, only while their master key is still that one.
    pub user_signed: std::collections::BTreeMap<String, String>,
}

fn public(seed: &[u8; 32]) -> String {
    b64::std_nopad(SigningKey::from_bytes(seed).verifying_key().as_bytes())
}

fn sign(seed: &[u8; 32], v: &Value) -> String {
    b64::std_nopad(&SigningKey::from_bytes(seed).sign(canonical_json(v).as_bytes()).to_bytes())
}

/// The one public key of a cross-signing key object (`keys` holds exactly one entry whose name is `ed25519:<key>`).
fn key_of(o: &Value) -> Option<String> {
    let m = o["keys"].as_object()?;
    if m.len() != 1 {
        return None;
    }
    let (k, v) = m.iter().next()?;
    let v = v.as_str()?;
    (k == &format!("ed25519:{v}")).then(|| v.to_string())
}

fn unsigned(o: &Value) -> Value {
    let mut b = o.clone();
    if let Some(m) = b.as_object_mut() {
        m.remove("signatures");
        m.remove("unsigned");
    }
    b
}

impl OlmAddon {
    fn cross_body(&self, c: &Cross) -> Value {
        let user = &self.user_id;
        let (m, s, u) = (public(&c.master), public(&c.selfsign), public(&c.usersign));
        let master = json!({"user_id": user, "usage": ["master"], "keys": {format!("ed25519:{m}"): m}});
        let mut self_key = json!({"user_id": user, "usage": ["self_signing"], "keys": {format!("ed25519:{s}"): s}});
        self_key["signatures"] = json!({user: {format!("ed25519:{m}"): sign(&c.master, &unsigned(&self_key))}});
        let mut user_key = json!({"user_id": user, "usage": ["user_signing"], "keys": {format!("ed25519:{u}"): u}});
        user_key["signatures"] = json!({user: {format!("ed25519:{m}"): sign(&c.master, &unsigned(&user_key))}});
        json!({"master_key": master, "self_signing_key": self_key, "user_signing_key": user_key})
    }

    /// Make the keys (once) and queue their upload. The account password answers the homeserver's second question; it is held in memory
    /// for that one retry and then dropped.
    pub(super) fn cross_setup(&self, password: &str) -> Result<(), String> {
        if password.is_empty() {
            return Err("the account password is needed: the homeserver asks for it again before it accepts new cross-signing keys".into());
        }
        let mut i = self.lock();
        if i.cross.keys.is_some() {
            return Err("cross-signing keys exist already on this device".into());
        }
        let (mut m, mut s, mut u) = ([0u8; 32], [0u8; 32], [0u8; 32]);
        for b in [&mut m, &mut s, &mut u] {
            iyakki_webcrypto::random(b)?;
        }
        let keys = Cross { master: m, selfsign: s, usersign: u };
        let body = self.cross_body(&keys);
        let (mpub, user) = (public(&keys.master), self.user_id.clone());
        i.cross.pinned.insert(user, mpub);
        i.cross.keys = Some(keys);
        i.cross.password = Some(Zeroizing::new(password.to_string()));
        i.cross.upload_body = Some(body.clone());
        i.flows.outbox.push(AddonRequest { method: "POST".into(), path: "v3/keys/device_signing/upload".into(), body });
        self.save(&i)
    }

    pub(super) fn cross_state_text(&self) -> String {
        let i = self.lock();
        match &i.cross.keys {
            None => "not set up on this device".to_string(),
            Some(c) => format!("set up (master key {})", public(&c.master)),
        }
    }

    /// The homeserver's answer to the upload: the password step, then this device's own signature.
    pub(super) fn cross_response(&self, request: &AddonRequest, status: u16, response: &Value) -> Result<Vec<AddonRequest>, String> {
        let mut i = self.lock();
        if request.path.trim_start_matches('/') == "v3/keys/device_signing/upload" {
            if status == 401 && request.body.get("auth").is_none() {
                let session = response["session"].as_str().ok_or("the homeserver asked for authentication without a session")?.to_string();
                let flows = response["flows"].as_array().cloned().unwrap_or_default();
                let by_password = flows.iter().any(|f| f["stages"].as_array().is_some_and(|s| s.len() == 1 && s[0] == "m.login.password"));
                if !by_password {
                    i.cross.password = None;
                    return Err("the homeserver wants another way of signing in than the password for new cross-signing keys: do it in another client".into());
                }
                let pw = i.cross.password.take().ok_or("no password to answer the homeserver with")?;
                let mut body = i.cross.upload_body.clone().ok_or("no keys to upload")?;
                body["auth"] = json!({"type": "m.login.password", "identifier": {"type": "m.id.user", "user": self.user_id}, "password": pw.as_str(), "session": session});
                return Ok(vec![AddonRequest { method: "POST".into(), path: "v3/keys/device_signing/upload".into(), body }]);
            }
            i.cross.password = None;
            i.cross.upload_body = None;
            if !(200..300).contains(&status) {
                // the keys were never accepted: forget them so the person can try again
                i.cross.keys = None;
                i.cross.pinned.remove(&self.user_id);
                self.save(&i)?;
                return Err(format!("the homeserver did not accept the cross-signing keys ({status})"));
            }
            // this device's own key signed by the self-signing key
            let c = i.cross.keys.as_ref().ok_or("no keys")?;
            let mut dk = self.device_keys(&i);
            let spub = public(&c.selfsign);
            let sig = sign(&c.selfsign, &unsigned(&dk));
            dk["signatures"][&self.user_id][format!("ed25519:{spub}")] = json!(sig);
            i.cross.trusted.insert((self.user_id.clone(), self.device_id.clone()));
            self.save(&i)?;
            return Ok(vec![AddonRequest { method: "POST".into(), path: "v3/keys/signatures/upload".into(), body: json!({&self.user_id: {&self.device_id: dk}}) }]);
        }
        Ok(Vec::new())
    }

    /// Read the cross-signing keys of a `/keys/query` answer and say which devices the chain vouches for.
    pub(super) fn cross_learn(&self, i: &mut Inner, resp: &Value) {
        for (user, master) in resp["master_keys"].as_object().into_iter().flatten() {
            let (Some(mpub), Some(sk)) = (key_of(master), resp["self_signing_keys"].get(user)) else { continue };
            if master["user_id"] != user.as_str() || sk["user_id"] != user.as_str() {
                continue;
            }
            let Some(spub) = key_of(sk) else { continue };
            // the self-signing key must be signed by that master key
            let by_master = sk["signatures"][user][format!("ed25519:{mpub}")].as_str();
            if by_master.is_none_or(|sig| iyakki_webcrypto::olm::verify_ed25519(&mpub, canonical_json(&unsigned(sk)).as_bytes(), sig).is_err()) {
                i.problems.push(format!("the self-signing key of {user} is not signed by its master key and was not used"));
                continue;
            }
            match i.cross.pinned.get(user) {
                Some(old) if *old != mpub => {
                    i.problems.push(format!("the master key of {user} changed and was not used"));
                    continue;
                }
                Some(_) => {}
                None => {
                    // another device of the person may be trusted only through a master key made here or met after a SAS verification
                    if *user != self.user_id || i.cross.keys.is_some() || i.flows.verified.iter().any(|(u, _)| *u == self.user_id) {
                        i.cross.pinned.insert(user.clone(), mpub.clone());
                    }
                }
            }
            i.cross.masters.insert(user.clone(), master.clone());
            let own_trusted = (*user == self.user_id && i.cross.pinned.get(user) == Some(&mpub)) || (*user != self.user_id && i.cross.user_signed.get(user) == Some(&mpub));
            for (dev, dk) in resp["device_keys"][user].as_object().into_iter().flatten() {
                let ok = dk["signatures"][user][format!("ed25519:{spub}")].as_str().is_some_and(|sig| iyakki_webcrypto::olm::verify_ed25519(&spub, canonical_json(&unsigned(dk)).as_bytes(), sig).is_ok());
                if ok && own_trusted && Self::checked_device(user, dev, dk).is_ok() {
                    i.cross.trusted.insert((user.clone(), dev.clone()));
                }
            }
        }
    }

    /// Sign another person's master key with this person's user-signing key and queue the upload. The person checked that key some other
    /// way (a card says so); until the homeserver accepts the signature nothing is trusted.
    pub(super) fn cross_sign_user(&self, user: &str) -> Result<(), String> {
        if user == self.user_id {
            return Err("your own keys are signed by the setup, not by this step".into());
        }
        if !user.starts_with('@') || !user.contains(':') || user.len() > 255 {
            return Err("give the full Matrix id of the person, like @name:server".into());
        }
        let mut i = self.lock();
        let c = i.cross.keys.as_ref().ok_or("make your own cross-signing keys first (message_verify cross_signing)")?;
        let master = i.cross.masters.get(user).cloned().ok_or_else(|| format!("no cross-signing key of {user} is known on this device yet: it is learned when a message of theirs or a verification with them is read; try again then"))?;
        let mpub = key_of(&master).ok_or("their master key is not in a usable shape")?;
        if i.cross.pinned.get(user) != Some(&mpub) {
            return Err(format!("the master key of {user} is not the one first seen, so it is not signed"));
        }
        let (upub, seed) = (public(&c.usersign), c.usersign);
        let mut signed = unsigned(&master);
        signed["signatures"] = master["signatures"].clone();
        if !signed["signatures"].is_object() {
            signed["signatures"] = json!({});
        }
        signed["signatures"][&self.user_id] = json!({format!("ed25519:{upub}"): sign(&seed, &unsigned(&master))});
        i.flows.outbox.push(AddonRequest { method: "POST".into(), path: "v3/keys/signatures/upload".into(), body: json!({user: {&mpub: signed}}) });
        self.save(&i)
    }

    /// The homeserver's answer to a signature upload. For a signature on another person's master key: when it was taken, remember it.
    pub(super) fn cross_signature_response(&self, request: &AddonRequest, status: u16, response: &Value) -> Result<Vec<AddonRequest>, String> {
        let mut i = self.lock();
        let mut said = Vec::new();
        for (user, keys) in request.body.as_object().into_iter().flatten() {
            if *user == self.user_id {
                continue;
            }
            for (mpub, _) in keys.as_object().into_iter().flatten() {
                let refused = !(200..300).contains(&status) || response["failures"].get(user).is_some_and(|f| !f.is_null());
                if refused {
                    said.push(format!("the homeserver did not take the signature on the key of {user}"));
                } else if i.cross.pinned.get(user) == Some(mpub) {
                    i.cross.user_signed.insert(user.clone(), mpub.clone());
                }
            }
        }
        self.save(&i)?;
        if said.is_empty() {
            Ok(Vec::new())
        } else {
            Err(said.join("; "))
        }
    }

    pub(super) fn cross_trusted(&self, user: &str, device: &str) -> bool {
        self.lock().cross.trusted.contains(&(user.to_string(), device.to_string()))
    }

    pub(super) fn cross_saved(&self, i: &Inner) -> Option<(String, String, String, Vec<(String, String)>)> {
        let c = i.cross.keys.as_ref()?;
        Some((b64::std(&c.master), b64::std(&c.selfsign), b64::std(&c.usersign), i.cross.pinned.iter().map(|(a, b)| (a.clone(), b.clone())).collect()))
    }

    pub(super) fn cross_restore(i: &mut Inner, saved: &(String, String, String, Vec<(String, String)>)) -> Result<(), String> {
        let bad = |_| "the saved encryption state is damaged".to_string();
        let seed = |s: &str| -> Result<[u8; 32], String> { b64::decode(s).map_err(bad)?.try_into().map_err(|_| "the saved encryption state is damaged".to_string()) };
        i.cross.keys = Some(Cross { master: seed(&saved.0)?, selfsign: seed(&saved.1)?, usersign: seed(&saved.2)? });
        i.cross.pinned = saved.3.iter().cloned().collect();
        Ok(())
    }
}

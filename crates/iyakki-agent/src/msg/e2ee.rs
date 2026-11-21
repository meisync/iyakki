//! The hook for end-to-end encrypted Matrix rooms. IYAKKI itself writes only to unencrypted rooms. Olm and Megolm (device keys, key
//! sharing, verification, a key store) are a large piece, so they are meant to come as an optional, separately downloaded add-on that
//! the person clicks to install (like the media add-on). This file is the whole contract such an add-on implements; nothing in the image
//! implements it, [`installed`] answers `None`, and a room that is encrypted is refused with a plain sentence that points here.
//!
//! What the add-on may and may not do:
//! * it gets plain JSON in and gives plain JSON out ([`E2eeAddon::encrypt`], [`E2eeAddon::decrypt`], [`E2eeAddon::on_sync`]);
//! * it never opens a connection. When it needs the homeserver (upload device keys, claim one-time keys, send room keys to devices) it
//!   returns [`AddonRequest`]s and IYAKKI performs them through the same client, so the "only the configured homeserver" rule holds;
//! * it never sees the access token; its key store is its own, under the person's data folder, protected by the vault.

use std::sync::{Arc, RwLock};

use serde_json::Value;

/// A call to the homeserver the add-on wants made on its behalf (path under `/_matrix/client/`, JSON body).
#[derive(Clone, Debug, PartialEq)]
pub struct AddonRequest {
    pub method: String,
    pub path: String,
    pub body: Value,
}

/// One device verification (SAS) as the person sees it: who, how far it got and, once both keys are in, the strings to compare with the
/// other device (seven emoji with their English names, or three numbers). The strings are not secret: they are meant to be read aloud.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    pub txn: String,
    pub user: String,
    pub device: String,
    /// One short sentence: asked, waiting for you, compare the strings, verified, cancelled (with the reason).
    pub state: String,
    /// What the person may do next: `accept`, `confirm`, or nothing.
    pub next: &'static str,
    pub emoji: Vec<(String, String)>,
    pub decimals: Option<[u16; 3]>,
}

/// Another device of the same person asked this one for the key to a room's messages (`m.room_key_request`). Nothing is sent before the
/// person says yes on a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyAsk {
    /// What the answer names (device and request together).
    pub id: String,
    pub device: String,
    pub room: String,
    pub state: String,
    /// `answer` while the person's yes or no is awaited, else nothing.
    pub next: &'static str,
    /// Did this device verify the asking device by SAS?
    pub verified: bool,
}

pub trait E2eeAddon: Send + Sync {
    /// Shown in Settings ("Matrix end-to-end encryption add-on 0.3").
    fn name(&self) -> String;
    /// Encrypt an event for a room. Returns the event type to send (`m.room.encrypted`) and its content.
    fn encrypt(&self, room_id: &str, event_type: &str, content: &Value) -> Result<(String, Value), String>;
    /// Decrypt one `m.room.encrypted` event of a sync response; returns the clear content (with `msgtype` and `body`).
    fn decrypt(&self, room_id: &str, event: &Value) -> Result<Value, String>;
    /// Look at a whole sync response (to-device events, device lists, one-time-key counts) and say what the homeserver should be told.
    fn on_sync(&self, sync: &Value) -> Result<Vec<AddonRequest>, String>;
    /// Does this add-on share the room key with the members' devices itself (it then wants [`prepare_send`](Self::prepare_send) and
    /// [`on_response`](Self::on_response) used before a message goes into an encrypted room)?
    fn shares_keys(&self) -> bool {
        false
    }
    /// Before a message goes into an encrypted room: what must be asked of the homeserver so that every device of the joined `members`
    /// has the room's key (device lookup, one-time key claim, delivery). Empty when nothing is needed.
    fn prepare_send(&self, _room_id: &str, _members: &[String]) -> Result<Vec<AddonRequest>, String> {
        Ok(Vec::new())
    }
    /// Undecryptable `event` of `room_id` (its key has not arrived): what to ask the person's own other devices so that one of them sends
    /// the key (`m.room_key_request`). At most once per session in a run. Empty when the add-on does not ask.
    fn key_requests(&self, _room_id: &str, _event: &Value) -> Vec<AddonRequest> {
        Vec::new()
    }
    /// The verifications in progress or finished in this run (newest last).
    fn verifications(&self) -> Vec<Verification> {
        Vec::new()
    }
    /// Ask one device of `user` to verify (`m.key.verification.request`); the messages go out with the next sync.
    fn verify_start(&self, _user: &str, _device: &str) -> Result<(), String> {
        Err("this add-on does not verify devices".into())
    }
    /// The person accepts a verification the other device asked for.
    fn verify_accept(&self, _txn: &str) -> Result<(), String> {
        Err("this add-on does not verify devices".into())
    }
    /// The person says the strings do match (`true`) or do not (`false`: the verification is cancelled as mismatched).
    fn verify_confirm(&self, _txn: &str, _matches: bool) -> Result<(), String> {
        Err("this add-on does not verify devices".into())
    }
    /// The person ends a verification.
    fn verify_cancel(&self, _txn: &str) -> Result<(), String> {
        Err("this add-on does not verify devices".into())
    }
    /// Make this person's cross-signing keys on this device and upload them (the homeserver asks for the account password again: it is
    /// typed by the person on the screen and passed here, never through the chat). The messages go out with the next sync.
    fn cross_signing_setup(&self, _password: &str) -> Result<(), String> {
        Err("this add-on does not make cross-signing keys".into())
    }
    /// Sign another person's master key with the user-signing key (the person has checked that key some other way) and upload the signature
    /// with the next sync; their devices then count as verified through the chain while that master key stays the same.
    fn cross_signing_trust_user(&self, _user: &str) -> Result<(), String> {
        Err("this add-on does not sign other people's keys".into())
    }
    /// One sentence on the cross-signing keys of this device.
    fn cross_signing_state(&self) -> String {
        "this add-on does not use cross-signing".into()
    }
    /// Requests of the person's other devices for a room key, waiting for the person or answered.
    fn key_asks(&self) -> Vec<KeyAsk> {
        Vec::new()
    }
    /// The person's answer to a [`KeyAsk`]: `true` sends the key (encrypted to that device only, with the next sync), `false` refuses.
    fn key_ask_answer(&self, _id: &str, _allow: bool) -> Result<(), String> {
        Err("this add-on does not answer key requests".into())
    }
    /// Whether this device verified that device of that user (kept in the vault).
    fn is_verified(&self, _user: &str, _device: &str) -> bool {
        false
    }
    /// The room key export file under a passphrase (the person typed it on the screen).
    fn export_keys(&self, _passphrase: &str, _rounds: u32) -> Result<String, String> {
        Err("this add-on cannot export keys".into())
    }
    /// Read an export file; returns how many sessions were added or improved.
    fn import_keys(&self, _passphrase: &str, _text: &str) -> Result<usize, String> {
        Err("this add-on cannot import keys".into())
    }
    /// The homeserver's answer (`status`, JSON body) to a request the add-on made; it may ask for the next one.
    fn on_response(&self, _request: &AddonRequest, _status: u16, _response: &Value) -> Result<Vec<AddonRequest>, String> {
        Ok(Vec::new())
    }
}

static INSTALLED: RwLock<Option<Arc<dyn E2eeAddon>>> = RwLock::new(None);

/// The add-on the person installed, if any (none in this build).
pub fn installed() -> Option<Arc<dyn E2eeAddon>> {
    INSTALLED.read().ok().and_then(|g| g.clone())
}

/// Called by the add-on loader after it verified and loaded the add-on.
pub fn register(a: Arc<dyn E2eeAddon>) {
    if let Ok(mut g) = INSTALLED.write() {
        *g = Some(a);
    }
}

/// What a person reads when a room is encrypted and no add-on is there.
pub const REFUSAL: &str = "this room is end-to-end encrypted and IYAKKI does not send plain text into an encrypted room. Use a room without encryption, or wait for the optional Matrix encryption add-on (not available yet)";

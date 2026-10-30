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

pub trait E2eeAddon: Send + Sync {
    /// Shown in Settings ("Matrix end-to-end encryption add-on 0.3").
    fn name(&self) -> String;
    /// Encrypt an event for a room. Returns the event type to send (`m.room.encrypted`) and its content.
    fn encrypt(&self, room_id: &str, event_type: &str, content: &Value) -> Result<(String, Value), String>;
    /// Decrypt one `m.room.encrypted` event of a sync response; returns the clear content (with `msgtype` and `body`).
    fn decrypt(&self, room_id: &str, event: &Value) -> Result<Value, String>;
    /// Look at a whole sync response (to-device events, device lists, one-time-key counts) and say what the homeserver should be told.
    fn on_sync(&self, sync: &Value) -> Result<Vec<AddonRequest>, String>;
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

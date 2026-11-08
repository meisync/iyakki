//! The system announcement of a private store: `GET <store>/v1/system.json` (docs/STORE.md, section 13; docs/UPDATES.md).
//!
//! The store's administrator puts a small file `system.json` in the store's data folder:
//! `{"version": "...", "variant": "...", "notes": "...", "channel": "..."}`. The server signs it when it is asked, with the catalog
//! key and the same signing helper the catalog and the withdrawn list use (purpose `system`), and serves the signed document. The
//! device accepts it only when the document names exactly the key that was pinned when the store was added (the person compared its
//! fingerprint) and the signature checks: an unsigned, wrongly signed, re-keyed or tampered file is refused. No file means nothing
//! is announced (404, not an error). It only says what is new; it never installs anything.

use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use super::trust::{sign_doc, verify_doc};
use crate::pkg::public_key_hex;

pub const PATH: &str = "/v1/system.json";
pub const PURPOSE: &str = "system";
/// The most bytes read from the file or the answer.
pub const MAX: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub version: String,
    pub variant: String,
    pub notes: String,
    pub channel: String,
}

fn text(v: &Value, k: &str, max: usize) -> String {
    v[k].as_str().unwrap_or("").chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

/// Server side: the signed document for the content of `system.json`.
pub fn sign(raw: &[u8], store_id: &str, now: i64, key: &SigningKey) -> Result<String, String> {
    if raw.len() > MAX {
        return Err("system.json is too large".into());
    }
    let v: Value = serde_json::from_slice(raw).map_err(|e| format!("system.json is not valid: {e}"))?;
    let version = text(&v, "version", 40);
    if version.is_empty() {
        return Err("system.json has no version".into());
    }
    let mut doc = json!({"v": 1, "id": store_id, "issued": now, "version": version, "variant": text(&v, "variant", 40), "notes": text(&v, "notes", 300), "channel": text(&v, "channel", 40), "key": public_key_hex(key)});
    let sig = sign_doc(PURPOSE, &doc, key);
    doc["sig"] = Value::String(sig);
    Ok(doc.to_string())
}

/// Device side: accept the document only when it is signed by `pin` (the key pinned for that store).
pub fn verify(body: &[u8], pin: &str) -> Result<Announcement, String> {
    const REFUSED: &str = "the store's system announcement is not signed by the key you confirmed: it is refused";
    let v: Value = serde_json::from_slice(body).map_err(|_| "the answer is not a system announcement".to_string())?;
    let (key, sig) = (v["key"].as_str().unwrap_or(""), v["sig"].as_str().unwrap_or(""));
    if pin.is_empty() || !key.eq_ignore_ascii_case(pin) || sig.is_empty() || !verify_doc(PURPOSE, &v, key, sig) {
        return Err(REFUSED.into());
    }
    let a = Announcement { version: text(&v, "version", 40), variant: text(&v, "variant", 40), notes: text(&v, "notes", 300), channel: text(&v, "channel", 40) };
    if a.version.is_empty() {
        return Err("the system announcement has no version".into());
    }
    Ok(a)
}

/// `<store>/v1/system.json` for a catalog address (`.../catalog.json` or a base address).
pub fn url_for_catalog(catalog_location: &str) -> String {
    let t = catalog_location.trim().trim_end_matches('/');
    let base = t.strip_suffix("/catalog.json").unwrap_or(t);
    format!("{base}{PATH}")
}

/// Is `url` the announcement address of a store (so it must be verified against a pinned key)?
pub fn is_store_announcement(url: &str) -> bool {
    url.trim().ends_with(PATH)
}

//! The Alexa voice provider's switch and consent (F-VOICE-43), kept where the Settings page and the chat tools both reach it.
//! The protocol is `iyakki_agent::avs` / `avs_h2`; this file holds only the decisions that stand in front of it:
//!
//! * FAIL CLOSED: the owner's AVS client id and product id (`alexa-avs.json`) must exist and be plain text before the provider can be
//!   switched on or a yes recorded. With none, [`NO_CLIENT_ID`] is the answer and nothing is ever dialled. There is no default id.
//! * On or off is the machine's choice (administrators). A session also needs the person's OWN yes on the consent card, which only a
//!   person on the Settings page can give: no chat tool calls [`grant`].
//! * Amazon's assistant is not IYAKKI's: it does not use IYAKKI's tools or approval cards, and the card says so.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "alexa-avs.json";
pub const STATE_FILE: &str = "alexa-voice.json";
/// The words when the owner's product is not set (the same sentence the transport says).
pub const NO_CLIENT_ID: &str = "Alexa is not set up: the owner has not registered an Alexa Voice Service product on this machine (no client id). Nothing was sent to Amazon.";
const MAX_PEOPLE: usize = 200;

/// The owner's product is named in the machine's file with a plain client id and product id (the same rule as the transport's reading).
pub fn configured(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join(CONFIG_FILE)) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    let plain = |k: &str| v[k].as_str().map(str::trim).is_some_and(|s| !s.is_empty() && s.len() <= 200 && s.chars().all(|c| c.is_ascii_graphic()));
    plain("client_id") && plain("product_id")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The machine's choice: the provider may be used at all.
    #[serde(default)]
    pub enabled: bool,
    /// Who said yes to the card, and on what day.
    #[serde(default)]
    pub consent: BTreeMap<String, String>,
}

fn path(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE)
}

pub fn load(dir: &Path) -> State {
    std::fs::read_to_string(path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(dir: &Path, s: &State) -> Result<(), String> {
    let p = path(dir);
    let tmp = p.with_extension("tmp");
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("could not save: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("could not save: {e}"))
}

/// Switch the provider on or off. Off always works; on needs the owner's client id.
pub fn set_enabled(dir: &Path, on: bool) -> Result<(), String> {
    if on && !configured(dir) {
        return Err(NO_CLIENT_ID.into());
    }
    let mut s = load(dir);
    s.enabled = on;
    save(dir, &s)
}

/// The person answered YES to the card (a key press by the person on the Settings page; no assistant tool calls this).
pub fn grant(dir: &Path, user: &str, date: &str) -> Result<(), String> {
    if user.is_empty() {
        return Err("nobody is signed in".into());
    }
    if !configured(dir) {
        return Err(NO_CLIENT_ID.into());
    }
    let mut s = load(dir);
    if !s.consent.contains_key(user) && s.consent.len() >= MAX_PEOPLE {
        return Err("too many people have answered on this computer".into());
    }
    s.consent.insert(user.to_string(), date.to_string());
    save(dir, &s)
}

/// Take the person's yes back (always allowed).
pub fn revoke(dir: &Path, user: &str) -> Result<(), String> {
    let mut s = load(dir);
    if s.consent.remove(user).is_some() {
        save(dir, &s)?;
    }
    Ok(())
}

pub fn consent_date(dir: &Path, user: &str) -> Option<String> {
    load(dir).consent.get(user).cloned()
}

/// `Ok` only when a turn may be sent to Amazon for `user`: the owner's product exists, the machine switched it on and the person said yes.
pub fn allowed(dir: &Path, user: &str) -> Result<(), String> {
    if !configured(dir) {
        return Err(NO_CLIENT_ID.into());
    }
    let s = load(dir);
    if !s.enabled {
        return Err("Alexa is switched off on this computer (Settings -> Alexa). Nothing was sent to Amazon.".into());
    }
    if !s.consent.contains_key(user) {
        return Err("You have not said yes to sending your voice to Amazon (Settings -> Alexa, key C). Nothing was sent.".into());
    }
    Ok(())
}

/// One short sentence of where things stand, for the Settings page and the chat tool.
pub fn status_text(dir: &Path, user: &str) -> String {
    if !configured(dir) {
        return NO_CLIENT_ID.into();
    }
    let s = load(dir);
    match (s.enabled, s.consent.get(user)) {
        (false, _) => "Alexa is off (the default). Nothing is sent to Amazon.".into(),
        (true, None) => "Alexa is available, but you have not said yes: nothing is sent to Amazon (Settings -> Alexa, key C).".into(),
        (true, Some(d)) => format!("Alexa is on and you said yes on {d}. Your voice goes to Amazon only during a turn you start; Amazon's assistant does not use IYAKKI's tools or approvals."),
    }
}

/// The consent card. "No" is the first choice in the page's form, so Enter alone never agrees.
pub fn consent_card() -> String {
    "Send your voice to Amazon? When you start an Alexa turn, what the microphone hears is sent to Amazon's Alexa Voice Service (alexa.na.gateway.devices.a2z.com) and Amazon answers with its own voice. Amazon's assistant is not IYAKKI's: it cannot use your files, tools or approval cards here, and what Amazon keeps is governed by Amazon's terms, not this computer. Nothing is sent before you start a turn, and no recording is kept on this computer. Answer Yes to allow it, No to keep your voice on this computer. You can take your answer back at any time.".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-alexa-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn without_the_owners_client_id_nothing_can_be_switched_on_or_agreed_to() {
        let d = dir("closed");
        assert!(!configured(&d));
        assert_eq!(set_enabled(&d, true), Err(NO_CLIENT_ID.to_string()));
        assert_eq!(grant(&d, "ada", "2026-11-19"), Err(NO_CLIENT_ID.to_string()));
        assert_eq!(allowed(&d, "ada"), Err(NO_CLIENT_ID.to_string()));
        assert_eq!(status_text(&d, "ada"), NO_CLIENT_ID);
        assert!(set_enabled(&d, false).is_ok(), "off always works");
        std::fs::write(d.join(CONFIG_FILE), r#"{"client_id":"","product_id":"p"}"#).unwrap();
        assert!(!configured(&d), "an empty id is no id");
        std::fs::write(d.join(CONFIG_FILE), r#"{"client_id":"a b","product_id":"p"}"#).unwrap();
        assert!(!configured(&d), "an id that is not plain text is no id");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_turn_needs_the_product_the_machines_switch_and_the_persons_own_yes() {
        let d = dir("open");
        std::fs::write(d.join(CONFIG_FILE), r#"{"client_id":"amzn1.application-oa2-client.x","product_id":"prod"}"#).unwrap();
        assert!(configured(&d));
        assert!(allowed(&d, "ada").unwrap_err().contains("switched off"));
        set_enabled(&d, true).unwrap();
        assert!(allowed(&d, "ada").unwrap_err().contains("not said yes"));
        assert!(status_text(&d, "ada").contains("not said yes"));
        grant(&d, "ada", "2026-11-19").unwrap();
        assert!(allowed(&d, "ada").is_ok());
        assert!(allowed(&d, "bob").unwrap_err().contains("not said yes"), "the yes is the person's own");
        assert!(status_text(&d, "ada").contains("2026-11-19"));
        revoke(&d, "ada").unwrap();
        assert!(allowed(&d, "ada").is_err());
        set_enabled(&d, false).unwrap();
        assert!(!load(&d).enabled);
        let _ = std::fs::remove_dir_all(&d);
    }
}

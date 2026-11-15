//! Chat tools for Settings -> Models -> Google Gemini (EXPERIMENTAL, optional, off by default; docs/GEMINI-PROVIDER.md):
//! `gemini_status` (free), `gemini_set` (asks every time, administrators: the tier, on or off, the model names, and whether the chat
//! and speech-to-text use it) and `gemini_consent_revoke` (asks every time: takes back the signed-in person's yes).
//!
//! The assistant can never type the key (the person does that in Settings, like every provider key) and can never give the consent
//! to send audio and text to Google: there is no tool for it, the card is answered on the Settings page by the person.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::gemini::{self, Change, Tier};
use crate::settingstools::{no_args, Entry, Env};

const TAB: &str = "Models";

pub const TOOLS: [&str; 3] = ["gemini_status", "gemini_set", "gemini_consent_revoke"];

pub const PROMPT_RULE: &str = "\nGOOGLE GEMINI (Settings -> Models, EXPERIMENTAL, off by default): an optional cloud provider for speech-to-text and for the assistant itself. gemini_status says whether it is on, whether a key is stored (never the key), the tier (free or paid: it only changes the notice and the advice when Google's limit is reached), the models, whether the chat and speech use it, and whether the person has said yes to sending audio and text to Google. gemini_set (tier, enabled, chat_model, stt_model, use_for_chat, use_for_speech) asks the person every time. gemini_consent_revoke takes the person's yes back and asks every time. You cannot type the key (the person types it in Settings -> Models, key g) and you cannot give the consent: the person answers the consent card on the Settings page (key C). Until they say yes nothing is sent to Google, and on the free tier Google may use what is sent to improve its products (according to Google's published terms; the person should check them), so do not suggest it for private material.";

fn parse(a: &Value) -> Result<Change, String> {
    let b = |k: &str| a.get(k).map(|v| v.as_bool().ok_or_else(|| format!("`{k}` must be true or false"))).transpose();
    let s = |k: &str| a.get(k).map(|v| v.as_str().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).ok_or_else(|| format!("`{k}` must be a model name"))).transpose();
    let tier = a.get("tier").map(|v| v.as_str().and_then(Tier::parse).ok_or("`tier` must be free or paid")).transpose()?;
    let ch = Change { tier, enabled: b("enabled")?, chat_model: s("chat_model")?, stt_model: s("stt_model")?, use_chat: b("use_for_chat")?, use_stt: b("use_for_speech")? };
    if ch.is_empty() {
        return Err("give at least one of `tier`, `enabled`, `chat_model`, `stt_model`, `use_for_chat`, `use_for_speech`".into());
    }
    Ok(ch)
}

fn describe(ch: &Change) -> String {
    let mut p = Vec::new();
    if let Some(t) = ch.tier {
        p.push(format!("set the Gemini tier to {} (the consent question is asked again)", t.label()));
    }
    match ch.enabled {
        Some(true) => p.push("turn Google Gemini on (nothing is sent until you say yes in Settings)".to_string()),
        Some(false) => p.push("turn Google Gemini off and put your earlier chat model and speech engine back".to_string()),
        None => {}
    }
    if let Some(m) = &ch.chat_model {
        p.push(format!("use the Gemini chat model {m}"));
    }
    if let Some(m) = &ch.stt_model {
        p.push(format!("use the Gemini speech model {m}"));
    }
    match ch.use_chat {
        Some(true) => p.push("let the assistant use Gemini (your messages and tool results would go to Google)".to_string()),
        Some(false) => p.push("stop the assistant using Gemini".to_string()),
        None => {}
    }
    match ch.use_stt {
        Some(true) => p.push("use Gemini for speech-to-text (your recordings would go to Google)".to_string()),
        Some(false) => p.push("stop using Gemini for speech-to-text".to_string()),
        None => {}
    }
    p.join(", ")
}

/// `key_present` says whether the key is stored (never its value).
pub fn entries(env: &Env, key_present: Arc<dyn Fn() -> bool + Send + Sync>) -> Vec<Entry> {
    let mut v = Vec::new();
    let (kp1, kp2) = (key_present.clone(), key_present);
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "gemini_status", "Show whether Google Gemini (experimental, optional, off by default) is on, the tier, the models, what uses it and whether you said yes to sending audio and text to Google. Never shows the key.", no_args(), move |_| {
            Ok(gemini::status_text(&e.dir, &e.user, kp1(), &e.settings))
        }));
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "tier": {"type": "string", "enum": ["free", "paid"], "description": "Only changes the notice and the advice when the limit is reached; asks the consent question again."},
            "enabled": {"type": "boolean", "description": "true: make Google Gemini available (nothing is sent until the person says yes in Settings); false: off, earlier choices put back."},
            "chat_model": {"type": "string", "description": "The Gemini chat model name."},
            "stt_model": {"type": "string", "description": "The Gemini speech-to-text model name."},
            "use_for_chat": {"type": "boolean", "description": "The assistant itself uses Gemini."},
            "use_for_speech": {"type": "boolean", "description": "Speech-to-text uses Gemini."}}});
        v.push(
            Entry::change(TAB, "gemini_set", "Change Google Gemini: the tier, on or off, the model names, and whether the assistant and speech-to-text use it. The person approves a card every time; the key and the consent are never set by this tool.", schema, |a| match parse(a) {
                Ok(c) => describe(&c),
                Err(x) => format!("gemini_set (invalid: {x})"),
            }, move |a| {
                let ch = parse(a)?;
                gemini::apply(&e.dir, &e.settings, &ch)?;
                e.audit("settings.gemini", &describe(&ch));
                Ok(format!("Done. {}", gemini::status_text(&e.dir, &e.user, kp2(), &e.settings)))
            })
            .admin()
            .with_preview(move |a| match parse(a) {
                Ok(ch) => {
                    let c = gemini::load(&e2.dir);
                    let tier = ch.tier.unwrap_or(c.tier);
                    let sends = ch.use_chat == Some(true) || ch.use_stt == Some(true) || ch.tier.is_some();
                    format!("{}\n{}", describe(&ch), if sends { gemini::consent_card(tier) } else { "Nothing is sent to Google by this change.".to_string() })
                }
                Err(x) => x,
            })
            .with_refusal(|a| parse(a).err()),
        );
    }
    {
        let e = env.clone();
        v.push(
            Entry::change(TAB, "gemini_consent_revoke", "Take back your yes to sending audio and text to Google. After this nothing is sent to Google until you say yes again in Settings.", no_args(), |_| "Take back the consent to send audio and text to Google".to_string(), move |_| {
                let had = gemini::revoke(&e.dir, &e.user)?;
                e.audit("settings.gemini", "consent taken back");
                Ok(if had { "Your yes is taken back. Nothing is sent to Google until you say yes again in Settings -> Models.".into() } else { "There was no yes on file. Nothing is sent to Google.".into() })
            }),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn status_is_free_changes_ask_every_time_and_the_assistant_has_no_way_to_consent_or_set_the_key() {
        let r = rig("gemini", true);
        let t = tools(&r, entries(&r.env, Arc::new(|| true)));
        assert_policy(&t, &["gemini_status"], &["gemini_set", "gemini_consent_revoke"]);
        assert!(TOOLS.iter().all(|n| !n.contains("consent_grant") && !n.contains("key_set") && !n.contains("consent_give")));
        for d in t.defs() {
            let text = format!("{} {}", d.spec.name, d.spec.schema);
            assert!(!text.contains("api_key") && !text.contains("\"key\""), "no argument carries the key: {text}");
        }
        let s = t.call("gemini_status", &json!({})).unwrap();
        assert!(s.contains("off by default") && s.contains("Key: stored") && s.contains("not said yes"), "{s}");
        assert!(hard(&"gemini_set".to_string()) && hard(&"gemini_consent_revoke".to_string()));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    fn hard(n: &String) -> bool {
        crate::agent::hard_ask(n, &json!({})).is_some()
    }

    #[test]
    fn set_validates_previews_what_would_be_sent_and_changes_the_settings() {
        let r = rig("gemini-set", true);
        let t = tools(&r, entries(&r.env, Arc::new(|| false)));
        assert!(t.refusal("gemini_set", &json!({})).is_some());
        assert!(t.refusal("gemini_set", &json!({"tier": "gold"})).unwrap().contains("free or paid"));
        assert!(t.refusal("gemini_set", &json!({"enabled": "yes"})).is_some());
        assert!(t.call("gemini_set", &json!({"chat_model": "a b"})).is_err());
        let p = t.preview("gemini_set", &json!({"enabled": true, "use_for_speech": true})).unwrap();
        assert!(p.contains("recordings would go to Google") && p.contains("Send audio and text to Google?"), "{p}");
        let none = t.preview("gemini_set", &json!({"enabled": false})).unwrap();
        assert!(none.contains("Nothing is sent to Google by this change"), "{none}");
        // using it before it is on is refused
        assert!(t.call("gemini_set", &json!({"use_for_chat": true})).unwrap_err().contains("turn Google Gemini on first"));
        let out = t.call("gemini_set", &json!({"enabled": true, "use_for_chat": true, "use_for_speech": true, "tier": "paid"})).unwrap();
        assert!(out.contains("on") && out.contains("paid") && out.contains("not said yes"), "{out}");
        {
            let g = r.env.settings.lock().unwrap();
            assert_eq!(g.get().chat.as_ref().map(|m| m.provider.as_str()), Some(gemini::PROVIDER_ID));
            assert!(matches!(g.get().voice.stt, Some(crate::voice::SpeechEngine::Gemini { tier: Tier::Paid, .. })));
        }
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("settings.gemini") && l.contains("paid")));
        assert!(!r.audits.lock().unwrap().iter().any(|l| l.contains("AIza")));
        // the person (not a tool) said yes; the assistant takes it back
        gemini::grant_on(&r.dir, "ada", "2026-11-13").unwrap();
        // the tier changed after the yes, so the status says it is asked again
        assert!(t.call("gemini_status", &json!({})).unwrap().contains("2026-11-13"));
        gemini::update(&r.dir, |c| {
            c.tier = Tier::Free;
            Ok(())
        })
        .unwrap();
        assert!(t.call("gemini_status", &json!({})).unwrap().contains("asked again"));
        gemini::update(&r.dir, |c| {
            c.tier = Tier::Paid;
            Ok(())
        })
        .unwrap();
        assert!(t.call("gemini_consent_revoke", &json!({})).unwrap().contains("taken back"));
        assert!(t.call("gemini_consent_revoke", &json!({})).unwrap().contains("no yes on file"));
        assert_eq!(gemini::consent_state(&r.dir, "ada"), gemini::ConsentState::Missing);
        t.call("gemini_set", &json!({"enabled": false})).unwrap();
        assert!(r.env.settings.lock().unwrap().get().chat.is_none(), "off puts the earlier choice back (there was none)");
        // not offered to people who are not administrators, except taking the consent back
        let user = rig("gemini-user", false);
        let ut = tools(&user, entries(&user.env, Arc::new(|| false)));
        let names: Vec<String> = ut.defs().into_iter().map(|d| d.spec.name).collect();
        assert!(names.contains(&"gemini_status".to_string()) && names.contains(&"gemini_consent_revoke".to_string()) && !names.contains(&"gemini_set".to_string()));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }
}

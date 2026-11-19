//! Chat tools for the Gemini voice assistant (EXPERIMENTAL, optional, off by default; docs/development/GEMINI-VOICE-DESIGN.md):
//! `gemini_voice_status` (free), `gemini_voice_set` (asks every time, administrators: on or off, the start key's name, the session
//! length, the quiet time, the daily cap, the spoken-reply cap, the model name) and `gemini_voice_stop` (free: closes the microphone
//! at once).
//!
//! The assistant can never give the consent to send the microphone to Google (the person answers the card on the Settings page) and can
//! never type the key: there is no tool for either, and no argument carries a key.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::geminilive::{self, LiveChange};
use crate::settingstools::{no_args, Entry, Env};
pub use iyakki_shared::settingstools_geminivoice::{PROMPT_RULE};

const TAB: &str = "Models";

pub const TOOLS: [&str; 3] = ["gemini_voice_status", "gemini_voice_set", "gemini_voice_stop"];


fn num(a: &Value, k: &str) -> Result<Option<u32>, String> {
    a.get(k).map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| format!("`{k}` must be a whole number"))).transpose()
}

fn parse(a: &Value) -> Result<LiveChange, String> {
    let enabled = a.get("enabled").map(|v| v.as_bool().ok_or("`enabled` must be true or false")).transpose()?;
    let text = |k: &str| a.get(k).map(|v| v.as_str().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).ok_or_else(|| format!("`{k}` must be text"))).transpose();
    let key = text("key")?.map(|k| geminilive::valid_shortcut(&k)).transpose()?;
    let wake_handoff = a.get("wake_handoff").map(|v| v.as_bool().ok_or("`wake_handoff` must be true or false")).transpose()?;
    let voice_out = a.get("voice_out").map(|v| v.as_str().and_then(geminilive::VoiceOut::parse).ok_or("`voice_out` must be local or gemini")).transpose()?;
    let ch = LiveChange { voice_out, wake_handoff, enabled, key, model: text("model")?, max_session_secs: num(a, "max_session_secs")?, idle_secs: num(a, "idle_secs")?, day_cap_secs: num(a, "day_cap_secs")?, spoken_cap_chars: num(a, "spoken_cap_chars")?, language: text("language")? };
    if ch.is_empty() {
        return Err("give at least one of `enabled`, `key`, `model`, `max_session_secs`, `idle_secs`, `day_cap_secs`, `spoken_cap_chars`, `wake_handoff`, `voice_out`, `language`".into());
    }
    Ok(ch)
}

fn describe(ch: &LiveChange) -> String {
    let mut p = Vec::new();
    match ch.enabled {
        Some(true) => p.push("make the Gemini voice assistant (EXPERIMENTAL) available (nothing is sent until you say yes in Settings and start a session)".to_string()),
        Some(false) => p.push("turn the Gemini voice assistant off".to_string()),
        None => {}
    }
    if let Some(k) = &ch.key {
        p.push(format!("start it with {k}"));
    }
    if let Some(m) = &ch.model {
        p.push(format!("use the live model {m}"));
    }
    if let Some(n) = ch.max_session_secs {
        p.push(format!("end a session after {n} seconds"));
    }
    if let Some(n) = ch.idle_secs {
        p.push(format!("close the microphone after {n} seconds of quiet"));
    }
    if let Some(n) = ch.day_cap_secs {
        p.push(format!("cap the microphone time sent to Google at {n} seconds a day"));
    }
    if let Some(n) = ch.spoken_cap_chars {
        p.push(format!("read at most {n} characters of a reply aloud"));
    }
    match ch.voice_out {
        Some(geminilive::VoiceOut::Gemini) => p.push("let Gemini speak with its own voice (its audio is streamed to this computer and played; echo cancellation is on while it speaks)".to_string()),
        Some(geminilive::VoiceOut::Local) => p.push("let the local voice read Gemini's text (the default)".to_string()),
        None => {}
    }
    if let Some(l) = &ch.language {
        match geminilive::valid_language(l) {
            Ok(c) if c.is_empty() => p.push("let Gemini follow the language it hears".to_string()),
            Ok(c) => p.push(format!("speak and answer in {} ({c})", geminilive::language_name(&c))),
            Err(_) => p.push("change the language".to_string()),
        }
    }
    match ch.wake_handoff {
        Some(true) => p.push("let the wake phrase (experimental, opt-in) open a Gemini voice session: nothing is sent before the phrase is heard on this computer".to_string()),
        Some(false) => p.push("stop the wake phrase opening a Gemini voice session".to_string()),
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
        v.push(Entry::read(TAB, "gemini_voice_status", "Show whether the Gemini voice assistant (EXPERIMENTAL, optional, off by default) is on, whether a session is open, the start key, the limits, today's microphone time and whether you said yes to sending your microphone to Google. Never shows the key.", no_args(), move |_| {
            Ok(geminilive::status_text(&e.dir, &e.user, kp1(), &iyakki_clock::today()))
        }));
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "enabled": {"type": "boolean", "description": "true: make the Gemini voice assistant (EXPERIMENTAL) available; nothing is sent until the person says yes in Settings and starts a session. false: off."},
            "key": {"type": "string", "description": "The name of the start shortcut, like Super+V (one or more of Super, Ctrl, Alt, Shift and then one letter, digit or F1 to F12)."},
            "model": {"type": "string", "description": "The Gemini live model name."},
            "max_session_secs": {"type": "integer", "description": "A session ends after this many seconds (30 to 900)."},
            "idle_secs": {"type": "integer", "description": "The microphone closes after this many quiet seconds (5 to 300)."},
            "day_cap_secs": {"type": "integer", "description": "The person's own cap of microphone seconds sent per day (60 to 36000)."},
            "language": {"type": "string", "description": "The language to speak and answer in: auto (default) or a code such as en-US, hi-IN, ta-IN, de-DE."},
            "spoken_cap_chars": {"type": "integer", "description": "The most characters of a reply read aloud (100 to 2000); the rest stays on the screen."},
            "voice_out": {"type": "string", "enum": ["local", "gemini"], "description": "Who speaks the answer: local (the local voice reads Gemini's text, the default) or gemini (Gemini's own voice, played through the shared audio path). Asks the person every time."},
            "wake_handoff": {"type": "boolean", "description": "true: the wake phrase (itself experimental, opt-in and local) opens a live session; nothing is sent before it is heard. Default false."}}});
        v.push(
            Entry::change(TAB, "gemini_voice_set", "Change the Gemini voice assistant (EXPERIMENTAL): on or off, the start key's name, the live model, the session length, the quiet time, the daily cap and the spoken-reply cap. The person approves a card every time; the key (the secret) and the consent are never set by this tool.", schema, |a| match parse(a) {
                Ok(c) => describe(&c),
                Err(x) => format!("gemini_voice_set (invalid: {x})"),
            }, move |a| {
                let ch = parse(a)?;
                geminilive::apply(&e.dir, &ch)?;
                e.audit("settings.gemini_voice", &describe(&ch));
                Ok(format!("Done. {}", geminilive::status_text(&e.dir, &e.user, kp2(), &iyakki_clock::today())))
            })
            .admin()
            .with_preview(move |a| match parse(a) {
                Ok(ch) => {
                    let tier = crate::gemini::load(&e2.dir).tier;
                    if ch.enabled == Some(true) {
                        format!("{}\n{}", describe(&ch), geminilive::consent_card_live(tier))
                    } else {
                        format!("{}\nNothing is sent to Google by this change.", describe(&ch))
                    }
                }
                Err(x) => x,
            })
            .with_refusal(|a| parse(a).err()),
        );
    }
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "gemini_voice_stop", "Close the Gemini voice assistant's microphone at once and end any open session (EXPERIMENTAL). Needs no approval; use it whenever the person wants the microphone off.", no_args(), move |_| {
            let was = geminilive::request_stop();
            e.audit("settings.gemini_voice", if was { "stopped an open session" } else { "stop asked, none was open" });
            Ok(if was { "The Gemini voice session is being closed: the microphone is shut and nothing more is sent.".into() } else { "No Gemini voice session was open. Nothing is being sent.".into() })
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn status_and_stop_are_free_set_asks_every_time_and_nothing_can_give_the_consent_or_the_key() {
        let r = rig("gvoice", true);
        let t = tools(&r, entries(&r.env, Arc::new(|| true)));
        assert_policy(&t, &["gemini_voice_status", "gemini_voice_stop"], &["gemini_voice_set"]);
        assert!(TOOLS.iter().all(|n| !n.contains("consent_grant") && !n.contains("key_set") && !n.contains("consent_give")));
        for d in t.defs() {
            let text = format!("{} {}", d.spec.name, d.spec.schema);
            assert!(!text.contains("api_key") && !text.contains("\"secret\"") && !text.contains("consent\""), "no argument carries the key or the consent: {text}");
            assert!(d.spec.description.contains("EXPERIMENTAL") || d.spec.name != "gemini_voice_set" && d.spec.name != "gemini_voice_status", "{}", d.spec.name);
        }
        let s = t.call("gemini_voice_status", &json!({})).unwrap();
        assert!(s.contains("EXPERIMENTAL") && s.contains("OFF") && s.contains("off by default") && s.contains("Key: stored") && s.contains("you have not said yes") && s.contains("Super+V"), "{s}");
        assert!(crate::agent::hard_ask("gemini_voice_set", &json!({})).is_some(), "hard-ask: a spoken yes never approves it");
        assert!(crate::agent::hard_ask("gemini_voice_status", &json!({})).is_none());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn set_validates_previews_the_consent_card_and_changes_the_file_but_never_the_consent() {
        let _g = crate::geminilive_tests::serial();
        let r = rig("gvoice-set", true);
        let t = tools(&r, entries(&r.env, Arc::new(|| false)));
        assert!(t.refusal("gemini_voice_set", &json!({})).is_some());
        assert!(t.refusal("gemini_voice_set", &json!({"key": "Esc"})).is_some());
        assert!(t.refusal("gemini_voice_set", &json!({"enabled": "yes"})).is_some());
        assert!(t.refusal("gemini_voice_set", &json!({"idle_secs": -1})).is_some());
        assert!(t.call("gemini_voice_set", &json!({"max_session_secs": 5})).unwrap_err().contains("between 30 and 900"));
        let p = t.preview("gemini_voice_set", &json!({"enabled": true})).unwrap();
        assert!(p.contains("microphone") && p.contains("results of the tools") && p.contains("EXPERIMENTAL"), "{p}");
        assert!(t.preview("gemini_voice_set", &json!({"enabled": false})).unwrap().contains("Nothing is sent to Google"));
        let out = t.call("gemini_voice_set", &json!({"enabled": true, "key": "ctrl+alt+v", "day_cap_secs": 600, "spoken_cap_chars": 300})).unwrap();
        assert!(out.contains("ON") && out.contains("Ctrl+Alt+V") && out.contains("300 characters") && out.contains("you have not said yes") && out.contains("Key: not stored"), "{out}");
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("settings.gemini_voice") && l.contains("Ctrl+Alt+V")));
        assert_eq!(crate::gemini::live_consent_state(&r.dir, "ada"), crate::gemini::ConsentState::Missing, "setting it on does not give the consent");
        // who speaks: the default is the local voice; Gemini's own voice is a change that asks, with its own words on the card
        assert!(out.contains("Voice out: local"), "{out}");
        assert!(t.refusal("gemini_voice_set", &json!({"voice_out": "robot"})).is_some());
        let p = t.preview("gemini_voice_set", &json!({"voice_out": "gemini"})).unwrap();
        assert!(p.contains("Gemini speak with its own voice") && p.contains("echo cancellation"), "{p}");
        let out = t.call("gemini_voice_set", &json!({"voice_out": "gemini"})).unwrap();
        assert!(out.contains("Voice out: gemini") && out.contains("Delay"), "{out}");
        assert!(crate::agent::hard_ask("gemini_voice_set", &json!({"voice_out": "gemini"})).is_some());
        // not offered to people who are not administrators, but they can read and stop
        let user = rig("gvoice-user", false);
        let ut = tools(&user, entries(&user.env, Arc::new(|| false)));
        let names: Vec<String> = ut.defs().into_iter().map(|d| d.spec.name).collect();
        assert!(names.contains(&"gemini_voice_status".to_string()) && names.contains(&"gemini_voice_stop".to_string()) && !names.contains(&"gemini_voice_set".to_string()));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }

    #[test]
    fn stop_says_whether_a_session_was_open() {
        let _g = crate::geminilive_tests::serial();
        let r = rig("gvoice-stop", false);
        let t = tools(&r, entries(&r.env, Arc::new(|| false)));
        let s = t.call("gemini_voice_stop", &json!({})).unwrap();
        assert!(s.contains("No Gemini voice session was open"), "{s}");
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("stop asked")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}

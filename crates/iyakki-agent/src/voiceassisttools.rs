//! The lead assistant's tools for My IYAKKI (the push-to-talk voice assistant, `voiceassist`): `voice_assistant_status` reads the
//! settings, `voice_assistant_set` changes them (asks every time, on the hard-ask list, so a spoken yes never approves it).
//! Muting only closes the microphone, so anyone may mute; turning the assistant on and the follow-up window are machine settings
//! for administrators.
//!
//! The wake phrase ("My IYAKKI", the in-house spotter in `crates/iyakki-wake`) has two chat-facing parts: `voice_wake_status` (free) says
//! whether it is opted in, whether a model file exists and what it was trained on, and plainly whether anything listens; the `wake`
//! option of `voice_assistant_set` (the same hard-ask tool: it asks every time, a tap or key approves, never a spoken yes) saves the
//! opt-in. In this version the spotter is built and tested but NOT connected to the microphone, so saving the opt-in starts no listening
//! and the status line says so (`WAKE_CONNECTED_TO_MICROPHONE`).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nMY IYAKKI (voice assistant): voice_assistant_status says whether the push-to-talk voice assistant is on, muted, and its follow-up window (Voice tile -> Assistant tab). voice_assistant_set (on, muted, wake, follow_up_seconds 0 to 20) changes it; voice_wake_status says whether the opt-in wake phrase \"My IYAKKI\" has a model and whether anything listens for it (in this version nothing does); it always asks the person, and only a tap or key approves it, never a spoken yes. The microphone is only open while the Voice tile shows it; nothing is recorded to disk.";

pub const STATUS_TOOL: &str = "voice_assistant_status";
pub const SET_TOOL: &str = "voice_assistant_set";
pub const WAKE_STATUS_TOOL: &str = "voice_wake_status";

/// Is the wake spotter fed from the microphone by anything in this build? No: the always-on listening is not enabled. When it is, this
/// becomes true in the change that wires it, with its tests (the indicator, the mute, the opt-in).
pub const WAKE_CONNECTED_TO_MICROPHONE: bool = false;

/// Where the trained weights live (under the data folder), made by `iyakki-wake-train`.
pub const WAKE_WEIGHTS_FILE: &str = "voice/my-iyakki-wake.iywk";

pub fn is_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL || n == WAKE_STATUS_TOOL
}

/// What is known about the model file.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelState {
    Missing,
    Bad(String),
    Found { how_trained: String, threshold: f32 },
}

/// The wake phrase status in plain words. `opted_in` is the setting `voice.my_iyakki_wake`.
pub fn wake_status_line(opted_in: bool, assistant_on: bool, muted: bool, model: &ModelState) -> String {
    let setting = if opted_in { "The wake phrase \"My IYAKKI\" is opted in" } else { "The wake phrase \"My IYAKKI\" is off (it is opt-in)" };
    let model = match model {
        ModelState::Missing => "No wake model file exists yet (record takes and train one: docs/development/WAKE-PHRASE.md).".to_string(),
        ModelState::Bad(e) => format!("The wake model file cannot be used: {e}."),
        ModelState::Found { how_trained, threshold } => format!("A wake model file is installed, {how_trained}; its suggested threshold is {threshold:.2}."),
    };
    let listening = if WAKE_CONNECTED_TO_MICROPHONE && opted_in && assistant_on && !muted {
        "The microphone is open to the local spotter (sound stays in memory and is never saved)."
    } else if opted_in && !WAKE_CONNECTED_TO_MICROPHONE {
        "Nothing is listening for it: this version has the spotter built and tested but not connected to the microphone, so use push to talk."
    } else if muted {
        "Nothing is listening: the microphone is muted."
    } else {
        "Nothing is listening for it."
    };
    format!("{setting}. {model} {listening}")
}

/// What the tools change (a fake in tests).
pub trait Backend: Send + Sync {
    fn status(&self) -> String;
    fn set_on(&self, on: bool) -> Result<(), String>;
    fn set_muted(&self, muted: bool) -> Result<(), String>;
    fn set_follow_ms(&self, ms: u32) -> Result<(), String>;
    /// Save the wake phrase opt-in.
    fn set_wake(&self, _on: bool) -> Result<(), String> {
        Err("the wake phrase setting is not available here".into())
    }
    fn wake_status(&self) -> String {
        wake_status_line(false, false, false, &ModelState::Missing)
    }
}

/// A validated change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub on: Option<bool>,
    pub muted: Option<bool>,
    pub follow_ms: Option<u32>,
    pub wake: Option<bool>,
}

pub fn parse(args: &Value) -> Result<Change, String> {
    let on = args.get("on").map(|v| v.as_bool().ok_or("`on` must be true or false")).transpose()?;
    let muted = args.get("muted").map(|v| v.as_bool().ok_or("`muted` must be true or false")).transpose()?;
    let wake = args.get("wake").map(|v| v.as_bool().ok_or("`wake` must be true or false")).transpose()?;
    let follow_ms = match args.get("follow_up_seconds") {
        None => None,
        Some(v) => {
            let s = v.as_f64().ok_or("`follow_up_seconds` must be a number")?;
            if !(0.0..=20.0).contains(&s) {
                return Err("`follow_up_seconds` must be between 0 and 20".into());
            }
            Some((s * 1000.0).round() as u32)
        }
    };
    if on.is_none() && muted.is_none() && follow_ms.is_none() && wake.is_none() {
        return Err("give at least one of `on`, `muted`, `wake`, `follow_up_seconds`".into());
    }
    Ok(Change { on, muted, follow_ms, wake })
}

pub struct VoiceAssistantTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl VoiceAssistantTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> VoiceAssistantTools {
        VoiceAssistantTools { inner, backend }
    }
}

impl Tools for VoiceAssistantTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Say whether My IYAKKI (the push-to-talk voice assistant) is on, whether its microphone is muted, and its follow-up window.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: WAKE_STATUS_TOOL.into(), description: "Say whether the My IYAKKI wake phrase is opted in, whether a wake model exists and what it was trained on, and whether anything is listening for it.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: SET_TOOL.into(),
                description: "Turn My IYAKKI on or off, mute or unmute its microphone, opt in or out of the wake phrase (`wake`), or set the follow-up window (seconds, 0 to 20). The person approves with a tap or key.".into(),
                schema: json!({"type": "object", "properties": {"on": {"type": "boolean"}, "muted": {"type": "boolean"}, "wake": {"type": "boolean"}, "follow_up_seconds": {"type": "number"}}}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.backend.status()),
            WAKE_STATUS_TOOL => Ok(self.backend.wake_status()),
            SET_TOOL => {
                let c = parse(args)?;
                if let Some(m) = c.muted {
                    self.backend.set_muted(m)?;
                }
                if let Some(o) = c.on {
                    self.backend.set_on(o)?;
                }
                if let Some(w) = c.wake {
                    self.backend.set_wake(w)?;
                }
                if let Some(f) = c.follow_ms {
                    self.backend.set_follow_ms(f)?;
                }
                Ok(self.backend.status())
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            SET_TOOL => match parse(args) {
                Ok(c) => {
                    let mut p = Vec::new();
                    match c.on {
                        Some(true) => p.push("turn My IYAKKI on (the microphone opens when you tap to talk)".to_string()),
                        Some(false) => p.push("turn My IYAKKI off".to_string()),
                        None => {}
                    }
                    match c.muted {
                        Some(true) => p.push("mute its microphone".to_string()),
                        Some(false) => p.push("unmute its microphone".to_string()),
                        None => {}
                    }
                    match c.wake {
                        Some(true) => p.push("opt in to the wake phrase \"My IYAKKI\" (a local spotter would listen for it; sound stays in memory; in this version it is not connected to the microphone)".to_string()),
                        Some(false) => p.push("turn the wake phrase off".to_string()),
                        None => {}
                    }
                    if let Some(f) = c.follow_ms {
                        p.push(format!("set the follow-up window to {} seconds", f as f64 / 1000.0));
                    }
                    p.join(", ")
                }
                Err(e) => format!("{name} (invalid: {e})"),
            },
            STATUS_TOOL => "Show the My IYAKKI settings".into(),
            WAKE_STATUS_TOOL => "Show the My IYAKKI wake phrase status".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL { parse(args).err() } else { self.inner.refusal(name, args) }
    }
    fn pre_approved(&self, name: &str) -> bool {
        !is_tool(name) && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        name != SET_TOOL && self.inner.approved_always(name)
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if is_tool(name) { None } else { self.inner.preview(name, args) }
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL { Some("it decides when this computer listens, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}

/// The status in plain words.
pub fn status_line(on: bool, muted: bool, follow_ms: u32) -> String {
    let follow = if follow_ms == 0 { "no follow-up window".to_string() } else { format!("follow-up window {} seconds", follow_ms as f64 / 1000.0) };
    format!(
        "My IYAKKI is {}{}; push to talk (tap the Assistant tab's button or press Space there), {follow}. The microphone is only open while the Voice tile shows it, and no sound is kept. See voice_wake_status for the wake phrase.",
        if on { "on" } else { "off" },
        if muted { " and muted" } else { "" }
    )
}

/// The real backend: this machine's voice settings. `audit(action, detail)` writes the audit line (never any speech).
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    /// The person may change machine settings (administrators); anyone may mute.
    pub can_change: bool,
}

impl RealBackend {
    fn admin(&self) -> Result<(), String> {
        if self.can_change { Ok(()) } else { Err("only an administrator can change this setting (Settings -> Voice)".into()) }
    }
    fn update(&self, f: impl FnOnce(&mut crate::voice::VoiceSettings)) -> Result<(), String> {
        self.svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| f(&mut s.voice)).map_err(|e| e.to_string())
    }
}

impl Backend for RealBackend {
    fn status(&self) -> String {
        let v = self.svc.voice_settings();
        status_line(v.my_on, v.my_muted, v.my_follow_ms)
    }
    fn set_on(&self, on: bool) -> Result<(), String> {
        self.admin()?;
        self.update(|v| v.my_on = on)?;
        (self.audit)("voice.assistant", if on { "on" } else { "off" });
        Ok(())
    }
    fn set_muted(&self, muted: bool) -> Result<(), String> {
        self.update(|v| v.my_muted = muted)?;
        (self.audit)("voice.assistant", if muted { "muted" } else { "unmuted" });
        Ok(())
    }
    fn set_follow_ms(&self, ms: u32) -> Result<(), String> {
        self.admin()?;
        self.update(|v| v.my_follow_ms = ms)?;
        (self.audit)("voice.assistant", "follow-up");
        Ok(())
    }
    fn set_wake(&self, on: bool) -> Result<(), String> {
        self.admin()?;
        if on && !self.svc.voice_settings().my_on {
            return Err("turn My IYAKKI on first; the wake phrase is an extra on top of it".into());
        }
        self.update(|v| v.my_wake = on)?;
        (self.audit)("voice.assistant", if on { "wake-on" } else { "wake-off" });
        Ok(())
    }
    fn wake_status(&self) -> String {
        let v = self.svc.voice_settings();
        let model = match std::fs::read(self.svc.dir().join(WAKE_WEIGHTS_FILE)) {
            Err(_) => ModelState::Missing,
            Ok(b) => match iyakki_wake::weights::load_bytes(&b) {
                Ok((_, h)) => ModelState::Found { how_trained: iyakki_wake::describe_weights(&h), threshold: h.threshold },
                Err(e) => ModelState::Bad(e),
            },
        };
        wake_status_line(v.my_wake, v.my_on, v.my_muted, &model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Base;
    impl Tools for Base {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, n: &str, _: &Value) -> Result<String, String> {
            Err(format!("unknown {n}"))
        }
        fn describe(&self, n: &str, _: &Value) -> String {
            n.to_string()
        }
    }
    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<String>>,
    }
    impl Backend for Fake {
        fn status(&self) -> String {
            "status".into()
        }
        fn set_on(&self, on: bool) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("on={on}"));
            Ok(())
        }
        fn set_muted(&self, m: bool) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("muted={m}"));
            Ok(())
        }
        fn set_follow_ms(&self, ms: u32) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("follow={ms}"));
            Ok(())
        }
        fn set_wake(&self, on: bool) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("wake={on}"));
            Ok(())
        }
    }

    fn tools() -> (VoiceAssistantTools, Arc<Fake>) {
        let f = Arc::new(Fake::default());
        (VoiceAssistantTools::new(Arc::new(Base), f.clone()), f)
    }

    #[test]
    fn status_is_free_and_set_asks_and_is_hard_ask() {
        let (t, _) = tools();
        let d = t.defs();
        let st = d.iter().find(|d| d.spec.name == STATUS_TOOL).unwrap();
        let se = d.iter().find(|d| d.spec.name == SET_TOOL).unwrap();
        assert_eq!(st.risk, Risk::Safe);
        assert_eq!(se.risk, Risk::Ask);
        assert!(crate::agent::hard_ask(SET_TOOL, &json!({"on": true})).is_some(), "Auto mode never runs it");
        assert!(crate::agent::hard_ask(STATUS_TOOL, &json!({})).is_none());
        assert!(crate::voiceassist::is_hard(SET_TOOL), "and a spoken yes never approves it");
        assert!(t.grant_blocker(SET_TOOL, &json!({})).is_some());
        assert!(!t.approved_always(SET_TOOL));
    }

    #[test]
    fn set_validates_and_applies() {
        let (t, f) = tools();
        assert!(t.refusal(SET_TOOL, &json!({})).is_some(), "nothing to change");
        assert!(t.refusal(SET_TOOL, &json!({"follow_up_seconds": 99})).is_some());
        assert!(t.refusal(SET_TOOL, &json!({"on": "yes"})).is_some());
        assert_eq!(t.call(SET_TOOL, &json!({"on": true, "muted": false, "follow_up_seconds": 5})).unwrap(), "status");
        assert_eq!(*f.log.lock().unwrap(), vec!["muted=false", "on=true", "follow=5000"]);
        assert!(t.describe(SET_TOOL, &json!({"on": true})).contains("microphone opens when you tap"));
        assert_eq!(t.call(STATUS_TOOL, &json!({})).unwrap(), "status");
    }

    #[test]
    fn the_wake_option_is_part_of_the_hard_ask_tool_and_the_wake_status_is_free() {
        let (t, f) = tools();
        let d = t.defs();
        let ws = d.iter().find(|d| d.spec.name == WAKE_STATUS_TOOL).unwrap();
        assert_eq!(ws.risk, Risk::Safe);
        assert!(crate::agent::hard_ask(WAKE_STATUS_TOOL, &json!({})).is_none());
        assert!(crate::agent::hard_ask(SET_TOOL, &json!({"wake": true})).is_some(), "turning the wake phrase on is a hard-ask");
        assert!(crate::voiceassist::is_hard(SET_TOOL));
        assert!(t.refusal(SET_TOOL, &json!({"wake": "yes"})).is_some());
        t.call(SET_TOOL, &json!({"wake": true})).unwrap();
        assert_eq!(*f.log.lock().unwrap(), vec!["wake=true"]);
        assert!(t.describe(SET_TOOL, &json!({"wake": true})).contains("not connected to the microphone"));
        assert_eq!(t.call(WAKE_STATUS_TOOL, &json!({})).unwrap(), wake_status_line(false, false, false, &ModelState::Missing));
        assert!(is_tool(WAKE_STATUS_TOOL));
    }

    #[test]
    fn the_wake_status_never_claims_to_listen_when_nothing_does() {
        assert!(!WAKE_CONNECTED_TO_MICROPHONE, "this version does not listen; flip this only with the wiring and its tests");
        let off = wake_status_line(false, true, false, &ModelState::Missing);
        assert!(off.contains("is off (it is opt-in)") && off.contains("No wake model file") && off.contains("Nothing is listening"));
        let on = wake_status_line(true, true, false, &ModelState::Found { how_trained: "trained on synthetic speech only (it has not heard a real voice)".into(), threshold: 0.85 });
        assert!(on.contains("opted in") && on.contains("synthetic speech only") && on.contains("not connected to the microphone"));
        assert!(!on.contains("microphone is open"));
        let bad = wake_status_line(true, true, true, &ModelState::Bad("damaged".into()));
        assert!(bad.contains("cannot be used: damaged"));
    }

    #[test]
    fn the_status_line_is_honest() {
        let s = status_line(false, true, 8000);
        assert!(s.contains("off and muted") && s.contains("8 seconds") && s.contains("See voice_wake_status"));
    }
}

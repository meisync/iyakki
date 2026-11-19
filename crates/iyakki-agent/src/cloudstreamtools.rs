//! The lead assistant's tools for My IYAKKI on a cloud machine (F-VOICE-38): `voice_cloud_stream_status` reads the switch,
//! `voice_cloud_stream_set` changes it (asks every time and is on the hard-ask list, so a spoken yes never approves it). Turning it ON only allows
//! the person's device to offer the stream: the consent card on the screen (`cloudstream::Gate`) still has to be pressed in every session.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nMY IYAKKI IN THE CLOUD: voice_cloud_stream_status says whether this device may send the person's microphone sound to a cloud machine so My IYAKKI there can hear them (off by default). voice_cloud_stream_set (on = true or false) switches it; it always asks, and a spoken yes never approves it. Even when it is on, a card on the screen asks again in every session and only a tap or key answers it; sound travels over a pinned TLS link, is never kept, and muting stops it at once.";

pub const STATUS_TOOL: &str = "voice_cloud_stream_status";
pub const SET_TOOL: &str = "voice_cloud_stream_set";

pub fn is_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL
}

/// The status in plain words.
pub fn status_line(on: bool, machine: &str) -> String {
    if on {
        format!("Sending your microphone sound to a cloud machine ({machine}) is ALLOWED: when My IYAKKI runs there, a card asks you in every session before any sound leaves this device (tap or key; saying yes does not count). It travels over a secure link pinned to that machine, nothing is kept, and Mute stops it at once.")
    } else {
        "Sending your microphone sound to a cloud machine is OFF (the default): no sound leaves this device for My IYAKKI.".into()
    }
}

pub trait Backend: Send + Sync {
    fn status(&self) -> String;
    fn set(&self, on: bool) -> Result<String, String>;
}

pub struct CloudStreamTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl CloudStreamTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> CloudStreamTools {
        CloudStreamTools { inner, backend }
    }
}

fn parse_on(args: &Value) -> Result<bool, String> {
    args.get("on").and_then(Value::as_bool).ok_or_else(|| "`on` must be true or false".to_string())
}

impl Tools for CloudStreamTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Say whether this device may send the person's microphone sound to a cloud machine for My IYAKKI (off by default).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: SET_TOOL.into(),
                description: "Allow or stop sending the person's microphone sound to a cloud machine for My IYAKKI (`on`). Allowing it still needs a tap or key on a card in every session. The person approves; a spoken yes never does.".into(),
                schema: json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }
    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.backend.status()),
            SET_TOOL => self.backend.set(parse_on(args)?),
            _ => self.inner.call(name, args),
        }
    }
    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            SET_TOOL => match parse_on(args) {
                Ok(true) => "allow this device to send your microphone sound to a cloud machine for My IYAKKI (a card still asks in every session; the sound leaves this device over a secure link and is never kept)".into(),
                Ok(false) => "stop sending your microphone sound to a cloud machine".into(),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            STATUS_TOOL => "Show whether microphone sound may go to a cloud machine".into(),
            _ => self.inner.describe(name, args),
        }
    }
    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => parse_on(args).err(),
            _ => self.inner.refusal(name, args),
        }
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
        if name == SET_TOOL { Some("it decides whether your voice leaves this device, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

impl crate::services::Services {
    /// Put the cloud stream tools on top of `inner` (the chat chain).
    pub fn wrap_cloud_stream_tools(self: &Arc<Self>, inner: Arc<dyn Tools>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, can_change: bool) -> Arc<dyn Tools> {
        Arc::new(CloudStreamTools::new(inner, Arc::new(RealBackend { svc: self.clone(), audit, can_change })))
    }

    /// The line of the chat's system prompt about it.
    pub fn cloud_stream_prompt_rule(&self) -> &'static str {
        PROMPT_RULE
    }
}

/// The real backend: this machine's voice settings. `audit` never gets any sound.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    pub can_change: bool,
}

impl Backend for RealBackend {
    fn status(&self) -> String {
        status_line(self.svc.voice_settings().my_cloud_stream, &crate::devname::label(self.svc.dir()))
    }
    fn set(&self, on: bool) -> Result<String, String> {
        if !self.can_change {
            return Err("only an administrator can change this setting (Settings -> Voice)".into());
        }
        self.svc.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_cloud_stream = on).map_err(|e| e.to_string())?;
        (self.audit)("voice.cloud_stream", if on { "allowed" } else { "off" });
        Ok(self.status())
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
    struct Fake(Mutex<Vec<bool>>);
    impl Backend for Fake {
        fn status(&self) -> String {
            status_line(self.0.lock().unwrap().last().copied().unwrap_or(false), "m1")
        }
        fn set(&self, on: bool) -> Result<String, String> {
            self.0.lock().unwrap().push(on);
            Ok(self.status())
        }
    }

    #[test]
    fn the_stream_switch_is_a_free_read_and_a_hard_ask_change_that_says_what_leaves_the_device() {
        let f = Arc::new(Fake::default());
        let t = CloudStreamTools::new(Arc::new(Base), f.clone());
        let d = t.defs();
        assert_eq!(d.iter().find(|x| x.spec.name == STATUS_TOOL).unwrap().risk, Risk::Safe);
        assert_eq!(d.iter().find(|x| x.spec.name == SET_TOOL).unwrap().risk, Risk::Ask);
        assert!(crate::agent::hard_ask(SET_TOOL, &json!({"on": true})).is_some(), "a spoken yes can never approve it");
        assert!(t.describe(SET_TOOL, &json!({"on": true})).contains("leaves this device"));
        assert!(t.refusal(SET_TOOL, &json!({"on": "yes"})).is_some() && t.refusal(SET_TOOL, &json!({})).is_some());
        assert!(!t.approved_always(SET_TOOL) && t.grant_blocker(SET_TOOL, &json!({})).is_some());
        assert!(t.call(STATUS_TOOL, &json!({})).unwrap().contains("OFF"));
        assert!(t.call(SET_TOOL, &json!({"on": true})).unwrap().contains("ALLOWED"));
        assert_eq!(*f.0.lock().unwrap(), vec![true]);
        assert!(PROMPT_RULE.contains("voice_cloud_stream_set") && PROMPT_RULE.contains("never approves"));
    }
}

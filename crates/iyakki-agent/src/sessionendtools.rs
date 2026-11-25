//! The assistant's read tool `session_end_report` (D-SESSION-END): what ends when a person signs out, when the sound changes owner and
//! when the screen locks, as the registry (`iyakki_shared::sessionend`) holds it. Free (it only reads); nothing is stopped by it.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const REPORT_TOOL: &str = "session_end_report";

/// Hooks only the shell can hold (its own registry, not the process-wide one).
pub const SHELL_OWN: &[(&str, &str)] = &[("wake-listener", "capture")];

pub struct SessionEndTools {
    inner: Arc<dyn Tools>,
}

impl SessionEndTools {
    pub fn new(inner: Arc<dyn Tools>) -> SessionEndTools {
        SessionEndTools { inner }
    }
}

/// The answer: the rule in a few lines and every registered hook with its kind.
pub fn report(reg: &iyakki_shared::sessionend::SessionEnd) -> String {
    let mut s = String::from("Signing out stops what the person started: sound (radio, the media player and its queue, readers, the voice assistant, a client's sound), capture (camera, screen and microphone, in the browser or on this machine, the wake listener) and their sessions (terminals, port-forwards, Linux windows, IDE task runs and other jobs, the sound sent to their browser). A change of who owns the sound ends the previous person's sound. A lock stops capture only; playback goes on. Before the machine sleeps capture and sound stop (terminals, forwards and jobs go on). Scheduled flows and alarms, installs and downloads, and a job marked to go on after sign-out (job_keep) continue. The kinds: sound, capture, process.\n");
    s.push_str(&reg.describe());
    for (n, k) in SHELL_OWN {
        s.push_str(&format!("\n- {n} ({k}, held by the screen)"));
    }
    s
}

impl Tools for SessionEndTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }

    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != REPORT_TOOL);
        v.push(ToolDef {
            spec: ToolSpec { name: REPORT_TOOL.into(), description: "List what ends when a person signs out, when the machine's sound changes owner, when the screen locks and before the machine sleeps (sound, capture and the person's sessions: terminals, port-forwards, Linux windows, jobs).".into(), schema: json!({"type": "object", "properties": {}}) },
            risk: Risk::Safe,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            REPORT_TOOL => Ok(report(iyakki_shared::sessionend::global())),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            REPORT_TOOL => "Read what ends with a session".into(),
            _ => self.inner.describe(name, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::sessionend::{Kind, SessionEnd};

    #[test]
    fn the_report_names_every_registered_hook_and_the_shell_held_one() {
        let reg = SessionEnd::new();
        reg.register("radio", Kind::Sound, |_| false);
        reg.register("camera-grant", Kind::Capture, |_| false);
        let r = report(&reg);
        assert!(r.contains("- radio (sound)") && r.contains("- camera-grant (capture)") && r.contains("wake-listener"), "{r}");
        assert!(r.contains("A lock stops capture only"));
        assert!(r.contains("Before the machine sleeps capture and sound stop") && r.contains("job_keep") && r.contains("terminals, port-forwards"), "{r}");
    }
}

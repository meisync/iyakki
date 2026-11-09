//! The lead assistant's tools for accessibility: `accessibility_status` reads every choice (free), `accessibility_set` changes some of
//! them and always asks (turning the screen reader on changes what the computer does with sound and keys; sticky and slow keys change
//! how the keyboard answers). The assistant cannot hear whether a voice works: it says what the status shows.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::accessibility::{Access, SLOW_CHOICES};
use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nACCESSIBILITY: accessibility_status shows the screen reader, high contrast, text size, large pointer, focus ring, reduced motion, sticky keys and slow keys. accessibility_set changes them and always asks the person first. Never claim a screen reader works because it is switched on: it also needs a reading voice (Settings -> Voice), and you cannot hear it. Super+Alt+S toggles the reader from any screen, including sign-in.";

pub const STATUS_TOOL: &str = "accessibility_status";
pub const SET_TOOL: &str = "accessibility_set";

pub fn is_access_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL
}

/// What the tools need (a fake in tests).
pub trait Backend: Send + Sync {
    fn access(&self) -> Access;
    fn save(&self, a: &Access) -> Result<(), String>;
    fn high_contrast(&self) -> bool;
    fn set_high_contrast(&self, on: bool) -> Result<(), String>;
    /// 0 Small .. 4 Huge, 1 Normal.
    fn text_level(&self) -> usize;
    /// Whether a reading voice is set up (the reader is silent without one).
    fn voice_ready(&self) -> bool;
    fn audit(&self, action: &str, detail: &str);
}

pub struct AccessTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

const SIZES: [&str; 5] = ["Small", "Normal", "Large", "Larger", "Huge"];

/// The change a call asks for: the new choices and the new contrast, or why it cannot be done.
fn plan(cur: &Access, hc: bool, args: &Value) -> Result<(Access, bool), String> {
    let mut a = cur.clone();
    let mut hc = hc;
    let mut any = false;
    let mut flag = |key: &str, slot: &mut bool| -> Result<(), String> {
        if let Some(v) = args.get(key) {
            *slot = v.as_bool().ok_or_else(|| format!("`{key}` must be true or false"))?;
            any = true;
        }
        Ok(())
    };
    flag("screen_reader", &mut a.screen_reader)?;
    flag("speak_notices", &mut a.speak_notices)?;
    flag("large_pointer", &mut a.large_pointer)?;
    flag("big_focus_ring", &mut a.big_ring)?;
    flag("reduce_motion", &mut a.reduce_motion)?;
    flag("sticky_keys", &mut a.sticky_keys)?;
    flag("high_contrast", &mut hc)?;
    if let Some(v) = args.get("slow_keys_ms") {
        let ms = v.as_u64().and_then(|m| u32::try_from(m).ok()).filter(|m| SLOW_CHOICES.contains(m)).ok_or_else(|| format!("`slow_keys_ms` must be one of {SLOW_CHOICES:?} (0 = off)"))?;
        a.slow_keys_ms = ms;
        any = true;
    }
    if !any {
        return Err("say what to change: screen_reader, speak_notices, large_pointer, big_focus_ring, reduce_motion, sticky_keys, slow_keys_ms or high_contrast".into());
    }
    Ok((a, hc))
}

impl AccessTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> AccessTools {
        AccessTools { inner, backend }
    }

    fn status(&self) -> String {
        let a = self.backend.access();
        let voice = if self.backend.voice_ready() { "a reading voice is set up" } else { "NO reading voice is set up, so the screen reader would be silent (Settings -> Voice)" };
        format!(
            "{}; high contrast {}; text size {} (Settings -> Display). {voice}. Super+Alt+S turns the screen reader on or off anywhere; Super+Alt+Right/Left walk the controls, Super+Alt+Enter presses one.",
            a.describe(),
            if self.backend.high_contrast() { "on" } else { "off" },
            SIZES[self.backend.text_level().min(4)]
        )
    }
}

impl Tools for AccessTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_access_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Show the accessibility choices: screen reader, high contrast, text size, large pointer, big focus ring, reduced motion, sticky keys, slow keys, and whether a reading voice exists.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: SET_TOOL.into(),
                description: "Change accessibility choices (any of them in one call). The person approves a card first.".into(),
                schema: json!({"type": "object", "properties": {
                    "screen_reader": {"type": "boolean"}, "speak_notices": {"type": "boolean"}, "high_contrast": {"type": "boolean"},
                    "large_pointer": {"type": "boolean"}, "big_focus_ring": {"type": "boolean"}, "reduce_motion": {"type": "boolean"},
                    "sticky_keys": {"type": "boolean"}, "slow_keys_ms": {"type": "integer", "enum": SLOW_CHOICES}
                }}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.status()),
            SET_TOOL => {
                let (a, hc) = plan(&self.backend.access(), self.backend.high_contrast(), args)?;
                self.backend.save(&a)?;
                if hc != self.backend.high_contrast() {
                    self.backend.set_high_contrast(hc)?;
                }
                self.backend.audit("settings.accessibility", &format!("{}; high contrast {}", a.describe(), if hc { "on" } else { "off" }));
                let warn = if a.screen_reader && !self.backend.voice_ready() { " There is no reading voice yet, so it stays silent until one is set up in Settings -> Voice." } else { "" };
                Ok(format!("Saved. It applies within a second. {}{warn}", self.status()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS_TOOL => "Show the accessibility choices".into(),
            SET_TOOL => match plan(&self.backend.access(), self.backend.high_contrast(), args) {
                Ok((a, hc)) => format!("Change accessibility: {}; high contrast {}", a.describe(), if hc { "on" } else { "off" }),
                Err(_) => "Change accessibility choices".into(),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => Some(match plan(&self.backend.access(), self.backend.high_contrast(), args) {
                Ok((a, hc)) => format!("Now: {}; high contrast {}.\nAfter: {}; high contrast {}.", self.backend.access().describe(), if self.backend.high_contrast() { "on" } else { "off" }, a.describe(), if hc { "on" } else { "off" }),
                Err(e) => format!("Cannot change it: {e}"),
            }),
            STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => plan(&self.backend.access(), self.backend.high_contrast(), args).err(),
            STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_access_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != SET_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL { Some("it changes how the keyboard and the screen answer, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

/// The real backend: the data folder, the voice settings and the audit log.
pub struct RealBackend {
    pub dir: std::path::PathBuf,
    pub user: String,
    pub voice_ready: Arc<dyn Fn() -> bool + Send + Sync>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn access(&self) -> Access {
        crate::accessibility::load(&self.dir, &self.user)
    }
    fn save(&self, a: &Access) -> Result<(), String> {
        crate::accessibility::save(&self.dir, &self.user, a)
    }
    fn high_contrast(&self) -> bool {
        crate::accessibility::high_contrast(&self.dir, &self.user)
    }
    fn set_high_contrast(&self, on: bool) -> Result<(), String> {
        crate::accessibility::set_high_contrast(&self.dir, &self.user, on)
    }
    fn text_level(&self) -> usize {
        crate::accessibility::text_level(&self.dir, &self.user)
    }
    fn voice_ready(&self) -> bool {
        (self.voice_ready)()
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

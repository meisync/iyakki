//! The lead assistant's tools for the browser's graphics mode (`webgraphics.rs`): `browser_graphics_get` reads (no card);
//! `browser_graphics_set` asks on a card, every time (hard ask: a wrong choice can freeze the computer when the browser starts), and
//! "Allow for this session" does not remember it. The same choices as Settings -> Browser -> Graphics: automatic, software, hardware,
//! and `try_hardware_again` (forget the remembered fall back for this graphics card).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::webgraphics::{Mode, Status};

pub const PROMPT_RULE: &str = "\nBROWSER GRAPHICS: browser_graphics_get shows how the browser engine draws (automatic, software or hardware) and why. browser_graphics_set changes it (automatic, software, hardware, or try_hardware_again after an automatic fall back) and asks the person every time. Use it when the computer froze right after the browser started, or the person asks for the safe mode. A change applies the next time the browser engine starts.";

pub const WRITE_TOOLS: [&str; 1] = ["browser_graphics_set"];
pub const READ_TOOL: &str = "browser_graphics_get";

pub fn is_graphics_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

/// What the tools need from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    fn status(&self) -> Status;
    fn set_mode(&self, m: Mode) -> Result<(), String>;
    fn try_hardware_again(&self) -> Result<(), String>;
}

pub struct GraphicsTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

/// The change a call asks for.
enum Change {
    Set(Mode),
    Retry,
}

impl GraphicsTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> GraphicsTools {
        GraphicsTools { inner, backend }
    }

    fn change(&self, args: &Value) -> Result<Change, String> {
        let w = args.get("mode").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
        if w == "try_hardware_again" {
            return Ok(Change::Retry);
        }
        Mode::from_id(&w).map(Change::Set).ok_or_else(|| "mode is `automatic`, `software`, `hardware` or `try_hardware_again`".to_string())
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let s = self.backend.status();
        if let Some(why) = &s.read_only {
            return Err(why.clone());
        }
        Ok(match self.change(args)? {
            Change::Set(Mode::Automatic) => format!("Set Browser graphics to Automatic (now: {}). The browser uses the graphics card, and draws in software by itself after a start on the card that ended with the computer stopping. Any remembered fall back is forgotten. Applies the next time the browser engine starts.", s.mode.label()),
            Change::Set(Mode::Software) => format!("Set Browser graphics to Software (safe) (now: {}). The browser always draws with the processor: slower, but it does not use the graphics card. Applies the next time the browser engine starts.", s.mode.label()),
            Change::Set(Mode::Hardware) => format!("Set Browser graphics to Hardware (now: {}). The browser always uses the graphics card and never falls back to software, even after a freeze. If the computer freezes when the browser starts, a power cycle and this setting back to Automatic or Software is the way out. Applies the next time the browser engine starts.", s.mode.label()),
            Change::Retry => {
                if !s.fallback {
                    return Err("no fall back to software is remembered for this graphics card".into());
                }
                "Try hardware again: forget the fall back to software for this graphics card. The next browser start uses the card; if the computer stops again, the browser falls back once more.".to_string()
            }
        })
    }

    fn get_text(&self) -> String {
        let s = self.backend.status();
        let mut t = format!("Browser graphics: {}. {}", s.line, s.detail);
        t.push_str(&format!("\nSetting: {}. Graphics card: {}.", s.mode.label(), if s.gpu.is_empty() { "none found" } else { &s.gpu }));
        if s.offer {
            t.push_str("\nThe last session in software ended cleanly: browser_graphics_set with try_hardware_again lets the person try the card once more (asks).");
        }
        if let Some(r) = &s.read_only {
            t.push_str(&format!("\nCannot be changed: {r}"));
        }
        t
    }
}

impl Tools for GraphicsTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_graphics_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show how the browser engine draws: Automatic (hardware, or software after a freeze), Software (safe) or Hardware, and why.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: "browser_graphics_set".into(),
                description: "Change how the browser engine draws. The person approves a card that says what changes. software is the safe mode (slower, no graphics card); hardware never falls back; automatic falls back by itself after a freeze; try_hardware_again forgets a remembered fall back.".into(),
                schema: json!({"type": "object", "properties": {"mode": {"type": "string", "enum": ["automatic", "software", "hardware", "try_hardware_again"]}}, "required": ["mode"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.get_text()),
            "browser_graphics_set" => {
                let card = self.card(args)?;
                match self.change(args)? {
                    Change::Set(m) => self.backend.set_mode(m)?,
                    Change::Retry => self.backend.try_hardware_again()?,
                }
                let _ = card;
                Ok(format!("Done. It applies the next time the browser engine starts.\n{}", self.get_text()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(args) {
                Ok(c) => c.split(". ").next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL { "Show the browser graphics mode".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(args).ok();
        }
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(args).err();
        }
        if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_graphics_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("changing the browser's graphics mode asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: the setting files in the data folder; every change is audited.
pub struct RealBackend {
    pub data: std::path::PathBuf,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn status(&self) -> Status {
        crate::webgraphics::status(&self.data)
    }
    fn set_mode(&self, m: Mode) -> Result<(), String> {
        crate::webgraphics::set_mode(&self.data, m)?;
        (self.audit)("settings.browser", &format!("Browser graphics: {} (chosen in chat)", m.label()));
        Ok(())
    }
    fn try_hardware_again(&self) -> Result<(), String> {
        crate::webgraphics::try_hardware_again(&self.data, &crate::webgraphics::status(&self.data).gpu)?;
        (self.audit)("settings.browser", "Browser graphics: trying hardware again (chosen in chat)");
        Ok(())
    }
}

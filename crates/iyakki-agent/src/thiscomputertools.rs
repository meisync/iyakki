//! The lead assistant's tools for the per-computer settings (`hwprofile`): `this_computer_status` is free; sharing a category between
//! computers, copying another computer's settings here and resetting this computer's settings ask every time.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::hwprofile::Category;

pub const PROMPT_RULE: &str = "\nTHIS COMPUTER: the install can run on several computers and keeps the settings that belong to a computer (sound, display, keyboard and pointer, microphone, GPU, power) apart from the person's. this_computer_status says which computer this is and what is kept per computer. this_computer_set_sharing (category, shared) shares a category between all computers or keeps it per computer; this_computer_copy_from (computer) takes another computer's settings here; this_computer_reset puts this computer's settings back to automatic. The three changes ask the person.";

pub const STATUS: &str = "this_computer_status";
pub const SET_SHARING: &str = "this_computer_set_sharing";
pub const COPY_FROM: &str = "this_computer_copy_from";
pub const RESET: &str = "this_computer_reset";

pub fn is_tool(n: &str) -> bool {
    [STATUS, SET_SHARING, COPY_FROM, RESET].contains(&n)
}

/// What the tools need from the install (a fake in tests).
pub trait Backend: Send + Sync {
    fn status(&self) -> String;
    fn set_sharing(&self, cat: Category, shared: bool) -> Result<String, String>;
    fn copy_from(&self, computer: &str) -> Result<String, String>;
    fn reset(&self) -> Result<String, String>;
}

pub struct ThisComputerTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn parse_sharing(args: &Value) -> Result<(Category, bool), String> {
    let w = args.get("category").and_then(Value::as_str).unwrap_or("");
    let cat = Category::parse(w).ok_or_else(|| format!("`{w}` is not a category; use one of: {}", Category::ALL.iter().map(|c| c.key()).collect::<Vec<_>>().join(", ")))?;
    let shared = args.get("shared").and_then(Value::as_bool).ok_or("`shared` must be true or false")?;
    Ok((cat, shared))
}

fn computer_arg(args: &Value) -> Result<String, String> {
    let w = args.get("computer").and_then(Value::as_str).unwrap_or("").trim();
    if w.is_empty() { Err("name the other computer (its label or id, see this_computer_status)".into()) } else { Ok(w.to_string()) }
}

impl ThisComputerTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ThisComputerTools {
        ThisComputerTools { inner, backend }
    }
}

impl Tools for ThisComputerTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_tool(&d.spec.name));
        let def = |name: &str, description: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: description.into(), schema }, risk, untrusted: false };
        let cats: Vec<&str> = Category::ALL.iter().map(|c| c.key()).collect();
        v.push(def(STATUS, "Say which computer this is, which other computers used this install, and which kinds of settings are kept per computer.", json!({"type": "object", "properties": {}}), Risk::Safe));
        v.push(def(SET_SHARING, "Share one kind of settings between all computers (shared=true) or keep it per computer (shared=false). The person approves.", json!({"type": "object", "properties": {"category": {"type": "string", "enum": cats}, "shared": {"type": "boolean"}}, "required": ["category", "shared"]}), Risk::Ask));
        v.push(def(COPY_FROM, "Take the per-computer settings of another computer (its label or id) over this computer's. The person approves.", json!({"type": "object", "properties": {"computer": {"type": "string"}}, "required": ["computer"]}), Risk::Ask));
        v.push(def(RESET, "Put this computer's per-computer settings back to automatic. The person approves.", json!({"type": "object", "properties": {}}), Risk::Ask));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS => Ok(self.backend.status()),
            SET_SHARING => {
                let (c, s) = parse_sharing(args)?;
                self.backend.set_sharing(c, s)
            }
            COPY_FROM => self.backend.copy_from(&computer_arg(args)?),
            RESET => self.backend.reset(),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS => "Show this computer and what is kept per computer".into(),
            SET_SHARING => match parse_sharing(args) {
                Ok((c, true)) => format!("Share {} between all computers", c.label()),
                Ok((c, false)) => format!("Keep {} per computer", c.label()),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            COPY_FROM => match computer_arg(args) {
                Ok(c) => format!("Copy the settings of {c} to this computer (this computer's own are replaced)"),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            RESET => "Put this computer's sound, display, keyboard, microphone, GPU and power settings back to automatic".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_SHARING => parse_sharing(args).err(),
            COPY_FROM => computer_arg(args).err(),
            _ => self.inner.refusal(name, args),
        }
    }
    fn pre_approved(&self, name: &str) -> bool {
        !is_tool(name) && self.inner.pre_approved(name)
    }
    fn approved_always(&self, name: &str) -> bool {
        ![SET_SHARING, COPY_FROM, RESET].contains(&name) && self.inner.approved_always(name)
    }
    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if is_tool(name) { None } else { self.inner.preview(name, args) }
    }
    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if [SET_SHARING, COPY_FROM, RESET].contains(&name) { Some("this changes the settings of the computer, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// The real backend: this install's settings.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
}

impl Backend for RealBackend {
    fn status(&self) -> String {
        match crate::hwprofile::computers_of(&self.svc) {
            Some(c) => crate::hwprofile::status_text(&c),
            None => "No computer profile is kept here (a cloud instance, or the profile could not be opened).".into(),
        }
    }
    fn set_sharing(&self, cat: Category, shared: bool) -> Result<String, String> {
        crate::hwprofile::set_sharing(&self.svc, cat, shared)
    }
    fn copy_from(&self, computer: &str) -> Result<String, String> {
        crate::hwprofile::copy_from(&self.svc, computer)
    }
    fn reset(&self) -> Result<String, String> {
        crate::hwprofile::reset(&self.svc)
    }
}

//! One small frame for the assistant's Settings tools (docs/development/CHAT-REACHABILITY.md). A page of Settings gets its chat tools as a
//! list of [`Entry`] values; this wrapper puts the list in front of the other tools. Reading is free. Every change asks the person each time
//! and never rides on a session grant. Every entry calls the same service code the Settings page calls (a `views::Views`, a
//! `screensaver::Config`, ...), over the person's data folder, and writes one audit line.
//!
//! The frame knows nothing about the pages; each page's entries live in `settingstools_<page>.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

/// Who is asking and where the data is: what every entry needs.
#[derive(Clone)]
pub struct Env {
    pub dir: PathBuf,
    pub user: String,
    /// An administrator (the page may be for administrators only).
    pub admin: bool,
    /// The machine settings the shell holds (the same store the Settings pages change; never open the file a second time).
    pub settings: Arc<std::sync::Mutex<crate::settings::SettingsStore>>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Env {
    pub fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

type CallFn = Box<dyn Fn(&Value) -> Result<String, String> + Send + Sync>;
type TextFn = Box<dyn Fn(&Value) -> String + Send + Sync>;

/// One tool of one Settings page.
pub struct Entry {
    /// The Settings page this belongs to (the `Tab` name, for the reachability gate).
    pub tab: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    /// `Risk::Safe` reads only; `Risk::Ask` changes something and asks every time.
    pub risk: Risk,
    /// Only listed for administrators.
    pub admin_only: bool,
    pub describe: TextFn,
    /// The card text shown before a change (None: the description line only).
    pub preview: Option<TextFn>,
    /// A reason the call cannot work, found before the person is asked.
    pub refusal: Option<Box<dyn Fn(&Value) -> Option<String> + Send + Sync>>,
    pub call: CallFn,
}

impl Entry {
    pub fn read(tab: &'static str, name: &'static str, description: &'static str, schema: Value, call: impl Fn(&Value) -> Result<String, String> + Send + Sync + 'static) -> Entry {
        Entry { tab, name, description, schema, risk: Risk::Safe, admin_only: false, describe: Box::new(move |_| description.split('.').next().unwrap_or(name).to_string()), preview: None, refusal: None, call: Box::new(call) }
    }

    pub fn change(tab: &'static str, name: &'static str, description: &'static str, schema: Value, describe: impl Fn(&Value) -> String + Send + Sync + 'static, call: impl Fn(&Value) -> Result<String, String> + Send + Sync + 'static) -> Entry {
        Entry { tab, name, description, schema, risk: Risk::Ask, admin_only: false, describe: Box::new(describe), preview: None, refusal: None, call: Box::new(call) }
    }

    pub fn admin(mut self) -> Entry {
        self.admin_only = true;
        self
    }

    pub fn with_preview(mut self, p: impl Fn(&Value) -> String + Send + Sync + 'static) -> Entry {
        self.preview = Some(Box::new(p));
        self
    }

    pub fn with_refusal(mut self, r: impl Fn(&Value) -> Option<String> + Send + Sync + 'static) -> Entry {
        self.refusal = Some(Box::new(r));
        self
    }
}

pub struct SettingsTools {
    inner: Arc<dyn Tools>,
    admin: bool,
    entries: Vec<Entry>,
}

impl SettingsTools {
    pub fn new(inner: Arc<dyn Tools>, admin: bool, entries: Vec<Entry>) -> SettingsTools {
        SettingsTools { inner, admin, entries }
    }

    fn mine(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name && (self.admin || !e.admin_only))
    }

    fn owns(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }
}

/// A string argument, trimmed; an error that names the missing field.
pub fn arg_str(a: &Value, key: &str) -> Result<String, String> {
    a.get(key).and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).ok_or_else(|| format!("say `{key}`"))
}

/// A yes/no argument.
pub fn arg_bool(a: &Value, key: &str) -> Result<bool, String> {
    a.get(key).and_then(Value::as_bool).ok_or_else(|| format!("say `{key}`: true or false"))
}

pub fn no_args() -> Value {
    json!({"type": "object", "properties": {}})
}

impl Tools for SettingsTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !self.owns(&d.spec.name));
        for e in self.entries.iter().filter(|e| self.admin || !e.admin_only) {
            v.push(ToolDef { spec: ToolSpec { name: e.name.into(), description: e.description.into(), schema: e.schema.clone() }, risk: e.risk, untrusted: false });
        }
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match self.mine(name) {
            Some(e) => (e.call)(args),
            None if self.owns(name) => Err("This is for administrators only.".into()),
            None => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match self.mine(name) {
            Some(e) => (e.describe)(args),
            None => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match self.mine(name) {
            Some(e) => e.preview.as_ref().map(|p| p(args)),
            None => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match self.mine(name) {
            Some(e) => e.refusal.as_ref().and_then(|r| r(args)),
            None => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !self.owns(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        match self.mine(name) {
            Some(e) => e.risk == Risk::Safe,
            None => self.inner.approved_always(name),
        }
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        match self.mine(name) {
            Some(e) if e.risk == Risk::Ask => Some("a Settings change asks every time".into()),
            Some(_) => None,
            None => self.inner.grant_blocker(name, args),
        }
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

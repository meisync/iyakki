//! The assistant's tools for the `native_pdf_output` switch (docs/PDF.md, F-PDF-14), the same switch Settings -> Models (key P, a provider)
//! and Settings -> Agents (key p, an external agent) flip: `pdf_output_status` lists every provider and agent with the switch (free);
//! `pdf_output_set` turns it on or off for one of them (asks every time, administrators only). On means "this model or agent writes PDF
//! files itself", so `pdf_create` is not offered to it; off (the default) means it is offered.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const PROMPT_RULE: &str = "\nPDF OUTPUT SWITCH: pdf_output_status lists which providers and agents are marked as writing PDF files themselves (then pdf_create is not offered to them); pdf_output_set changes one after the person approves (a provider id, or agent:claude, agent:copilot, agent:custom:<id>).";

pub const STATUS_TOOL: &str = "pdf_output_status";
pub const SET_TOOL: &str = "pdf_output_set";

pub fn is_pdf_output_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL
}

/// One provider or agent with the switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The id the tool takes: a provider id, or `agent:claude`, `agent:copilot`, `agent:custom:<id>`.
    pub target: String,
    pub label: String,
    pub agent: bool,
    pub native: bool,
}

pub trait Backend: Send + Sync {
    fn entries(&self) -> Vec<Entry>;
    fn set(&self, target: &str, native: bool) -> Result<(), String>;
}

pub struct PdfOutputTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl PdfOutputTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> PdfOutputTools {
        PdfOutputTools { inner, backend }
    }

    fn pick(&self, args: &Value) -> Result<(Entry, bool), String> {
        let t = args.get("target").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or("say which provider or agent: target")?;
        let native = args.get("native").and_then(Value::as_bool).ok_or("`native` must be true or false")?;
        let e = self.backend.entries().into_iter().find(|e| e.target == t).ok_or_else(|| format!("`{t}` is not a provider or agent here; pdf_output_status lists them"))?;
        Ok((e, native))
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let (e, native) = self.pick(args)?;
        let what = |n: bool| if n { "writes PDF files itself (pdf_create is not offered)" } else { "is offered pdf_create" };
        Ok(format!("Mark {} {}: it {} now, it {}.", if e.agent { "the agent" } else { "the provider" }, e.target, what(e.native), what(native)))
    }
}

impl Tools for PdfOutputTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_pdf_output_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "List the providers and external agents and whether each is marked as writing PDF files itself (then pdf_create is not offered to it).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: SET_TOOL.into(), description: "Mark a provider (its id) or agent (agent:claude, agent:copilot, agent:custom:<id>) as writing PDF files itself (native true: pdf_create is not offered) or not (false). Asks first.".into(), schema: json!({"type": "object", "properties": {"target": {"type": "string"}, "native": {"type": "boolean"}}, "required": ["target", "native"]}) }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => {
                let es = self.backend.entries();
                if es.is_empty() {
                    return Ok("No providers or agents.".into());
                }
                Ok(es.iter().map(|e| format!("{} ({}): {}", e.target, e.label, if e.native { "writes PDF files itself, pdf_create not offered" } else { "pdf_create offered" })).collect::<Vec<_>>().join("\n"))
            }
            SET_TOOL => {
                let (e, native) = self.pick(args)?;
                self.backend.set(&e.target, native)?;
                Ok(format!("{} is now {}.", e.target, if native { "marked as writing PDF files itself: pdf_create is not offered to it" } else { "offered pdf_create" }))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS_TOOL => "Look at which models and agents write PDF files themselves".into(),
            SET_TOOL => self.card(args).unwrap_or_else(|e| format!("{SET_TOOL} (invalid: {e})")),
            _ => self.inner.describe(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => self.card(args).err(),
            STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => self.card(args).ok(),
            STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
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

/// Is the model or agent answering marked as writing PDF files itself? `agent` is the external agent chosen for the chat, if any; otherwise
/// the provider of the chosen chat model counts.
pub fn is_native(settings: &crate::settings::Settings, agents: &crate::extagent::AgentsCfg, agent: Option<&str>) -> bool {
    match agent {
        Some(a) => agents.native_pdf_output(a),
        None => settings.chat.as_ref().and_then(|c| settings.provider(&c.provider)).is_some_and(|p| p.native_pdf_output),
    }
}

/// The real backend: the settings of the providers and the agents.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
}

impl Backend for RealBackend {
    fn entries(&self) -> Vec<Entry> {
        let (providers, agents) = {
            let st = self.svc.settings.lock().unwrap_or_else(|e| e.into_inner());
            (st.get().providers.clone(), st.get().agents.clone())
        };
        let mut v: Vec<Entry> = providers.iter().map(|p| Entry { target: p.id.clone(), label: p.kind.label().to_string(), agent: false, native: p.native_pdf_output }).collect();
        v.push(Entry { target: "agent:claude".into(), label: "Claude Code".into(), agent: true, native: agents.claude.native_pdf_output });
        v.push(Entry { target: "agent:copilot".into(), label: "GitHub Copilot".into(), agent: true, native: agents.copilot.native_pdf_output });
        v.extend(agents.custom.iter().map(|c| Entry { target: format!("agent:custom:{}", c.id), label: if c.name.is_empty() { c.id.clone() } else { c.name.clone() }, agent: true, native: c.native_pdf_output }));
        v
    }

    fn set(&self, target: &str, native: bool) -> Result<(), String> {
        let t = target.to_string();
        self.svc
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(move |s| match t.as_str() {
                "agent:claude" => s.agents.claude.native_pdf_output = native,
                "agent:copilot" => s.agents.copilot.native_pdf_output = native,
                other => {
                    if let Some(c) = other.strip_prefix("agent:custom:").and_then(|c| s.agents.custom.iter_mut().find(|x| x.id == c)) {
                        c.native_pdf_output = native;
                    } else if let Some(p) = s.providers.iter_mut().find(|p| p.id == other) {
                        p.native_pdf_output = native;
                    }
                }
            })
            .map_err(|e| e.to_string())
    }
}

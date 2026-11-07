//! Tool routing, per person (Settings -> Models -> Tool routing, key `r`; the machine-wide switch is key `T`, administrators).
//!
//! Tool routing is on by default: the model is offered the groups of tools the chat needs and loads the rest with `more_tools`.
//! The administrator can switch it off for the whole machine (`Settings::no_tool_routing`). A person can also switch it off for
//! themselves: that is this file, `tool-routing-<user>.json` in the data folder, and it **defaults to off** (no override: the machine's
//! setting applies). The two only ever add up towards "offer every tool": routing is off for a person when the machine's switch OR the
//! person's own switch says so; a person can never turn routing back on against the administrator's choice, and nobody's choice
//! changes another person's chat. Offering every tool does not widen what the assistant may do: every approval still applies.
//!
//! A damaged file, or one written by a newer version, is moved aside (`.damaged` / `.newer`) and never overwritten; the person then has
//! no override.
//!
//! The assistant can read the state (`tool_routing_get`, no card) and change the person's own switch (`tool_routing_set`, asks on a
//! card every time). The tool never touches the machine-wide switch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

const VERSION: u32 = 1;
pub const READ_TOOL: &str = "tool_routing_get";
pub const WRITE_TOOL: &str = "tool_routing_set";

pub const PROMPT_RULE: &str = "\nTOOL ROUTING: tool_routing_get shows whether the model is offered every tool on every request (more tokens per request) or only the groups the chat needs. tool_routing_set switches that for this person only and asks every time; you cannot change the machine-wide setting.";

pub fn is_routing_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
struct File {
    version: u32,
    /// The person turned routing off for themselves (offer every tool). Default false: no override.
    off: bool,
}

fn path(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    dir.join(format!("tool-routing-{who}.json"))
}

fn aside(p: &Path, why: &str) {
    let mut to = p.as_os_str().to_owned();
    to.push(format!(".{why}"));
    let mut to = PathBuf::from(to);
    let mut n = 1;
    while to.exists() {
        let mut t = p.as_os_str().to_owned();
        t.push(format!(".{why}.{n}"));
        to = PathBuf::from(t);
        n += 1;
    }
    let _ = std::fs::rename(p, to);
}

/// Whether this person turned routing off for themselves (false when they did not, or the file is not usable).
pub fn person_off(dir: &Path, user: &str) -> bool {
    let p = path(dir, user);
    let Ok(t) = std::fs::read_to_string(&p) else { return false };
    match serde_json::from_str::<File>(&t) {
        Ok(f) if f.version <= VERSION => f.off,
        Ok(_) => {
            aside(&p, "newer");
            false
        }
        Err(_) => {
            aside(&p, "damaged");
            false
        }
    }
}

/// Save the person's own switch (written whole, then renamed into place).
pub fn set_person_off(dir: &Path, user: &str, off: bool) -> Result<(), String> {
    let p = path(dir, user);
    // a file that cannot be read is moved aside by the read, so the write below never replaces it
    let _ = person_off(dir, user);
    let tmp = p.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(&File { version: VERSION, off }).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Whether the model starts with the groups the chat needs (routing on) for this person.
pub fn lazy_groups(dir: &Path, user: &str, machine_off: bool) -> bool {
    !(machine_off || person_off(dir, user))
}

/// One sentence on the state, for the page and the tool.
pub fn summary(dir: &Path, user: &str, machine_off: bool) -> String {
    let me = person_off(dir, user);
    match (machine_off, me) {
        (true, _) => "Tool routing is off for the whole machine (an administrator's setting): the model is offered every tool.".into(),
        (false, true) => "Tool routing is off for you: the model is offered every tool (more tokens per request). The machine's setting is on.".into(),
        (false, false) => "Tool routing is on: the model is offered the groups of tools the chat needs and loads the rest on request.".into(),
    }
}

pub struct ToolRoutingTools {
    inner: Arc<dyn Tools>,
    dir: PathBuf,
    user: String,
    /// Reads the machine-wide switch each time (an administrator can change it while a chat is open).
    machine_off: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ToolRoutingTools {
    pub fn new(inner: Arc<dyn Tools>, dir: PathBuf, user: &str, machine_off: Arc<dyn Fn() -> bool + Send + Sync>) -> ToolRoutingTools {
        ToolRoutingTools { inner, dir, user: user.into(), machine_off }
    }

    fn wanted(args: &Value) -> Result<bool, String> {
        match args.get("routing").and_then(Value::as_str) {
            Some("on") => Ok(false),
            Some("off") => Ok(true),
            _ => Err("routing must be \"on\" or \"off\"".into()),
        }
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let off = Self::wanted(args)?;
        Ok(if off {
            "Turn tool routing OFF for you: the model is offered every tool on every request (more tokens, slower with small models). Approvals are unchanged.".to_string()
        } else {
            let note = if (self.machine_off)() { " An administrator has switched it off for the whole machine, so it stays off." } else { "" };
            format!("Turn tool routing back ON for you: the model is offered only the tools the chat needs.{note}")
        })
    }
}

impl Tools for ToolRoutingTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_routing_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show whether tool routing is on for this person and for the machine.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        let schema = json!({"type": "object", "properties": {"routing": {"type": "string", "enum": ["on", "off"], "description": "off: offer the model every tool on every request. on: only the groups the chat needs (the default)."}}, "required": ["routing"]});
        v.push(ToolDef { spec: ToolSpec { name: WRITE_TOOL.into(), description: "Switch tool routing for this person only. The person approves a card every time. The machine-wide setting is not touched.".into(), schema }, risk: Risk::Ask, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(summary(&self.dir, &self.user, (self.machine_off)())),
            WRITE_TOOL => {
                let off = Self::wanted(args)?;
                set_person_off(&self.dir, &self.user, off)?;
                Ok(format!("Saved. {} It applies to the next chat.", summary(&self.dir, &self.user, (self.machine_off)())))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            READ_TOOL => "Show the tool routing state".into(),
            WRITE_TOOL => match self.card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).ok(),
            READ_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            WRITE_TOOL => self.card(args).err(),
            READ_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_routing_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !is_routing_tool(name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL {
            return Some("changing how tools are offered always asks".into());
        }
        self.inner.grant_blocker(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
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

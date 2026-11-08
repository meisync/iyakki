//! The lead assistant's read-only tool for system components (docs/development/COMPONENT-UPDATES-PLAN.md): `components_status` shows what
//! the pinned private stores offer for the device's channel, what is installed, and the plan in plain words (sizes, delta or whole file,
//! restart or not, anything the installed parts cannot support). It contacts only stores the person added and pinned, reads their signed
//! list (checked against the pinned key) and never downloads an artifact or applies anything: applying is a later step.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::store::compmanifest::{self as cm, Doc};
use crate::store::components::{self as comps, human, Installed, PinnedStore};

pub const PROMPT_RULE: &str = "\nSYSTEM COMPONENTS: components_status shows what the person's pinned private stores offer for the kernel, boot, base system, core and kernel layer, what is installed, and the plan (sizes, only-the-difference or whole file, whether a restart is needed, and anything the installed parts cannot support). It only reads the stores' signed lists; it never downloads or installs a component. Do not promise a component will be installed.";

pub const TOOL: &str = "components_status";

pub fn is_components_tool(n: &str) -> bool {
    n == TOOL
}

/// What the tool needs from the machine (a fake in tests).
pub trait Backend: Send + Sync {
    /// The private stores the person added with their key pinned.
    fn stores(&self) -> Vec<PinnedStore>;
    fn installed(&self) -> Installed;
    /// The store's list, checked against its pinned key. Slow (network).
    fn fetch(&self, store: &PinnedStore) -> Result<Doc, String>;
    /// The newest list serial this device saw from the store (older ones are refused).
    fn last_serial(&self, _store: &PinnedStore) -> Option<u64> {
        None
    }
}

pub struct ComponentsTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

impl ComponentsTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ComponentsTools {
        ComponentsTools { inner, backend }
    }

    fn status(&self, args: &Value) -> Result<String, String> {
        let channel = args.get("channel").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()).unwrap_or("stable");
        if !cm::valid_channel(channel) {
            return Err(format!("`{channel}` is not a channel name (for example stable, beta, nightly)"));
        }
        let stores = self.backend.stores();
        if stores.is_empty() {
            return Ok("No private store with a pinned key is added, so there is nothing to ask for system components. The person adds one in STORE, Settings, Catalogs (confirming its key fingerprint).".into());
        }
        let inst = self.backend.installed();
        let mut out = vec![if inst.versions.is_empty() { "Installed components: none on record yet (this device has not recorded component versions).".to_string() } else { format!("Installed components: {}.", inst.versions.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")) }];
        for s in &stores {
            match self.backend.fetch(s) {
                Err(e) => out.push(format!("{}: {e}", s.name)),
                Ok(doc) => {
                    let offered: Vec<String> = cm::COMPONENTS.iter().filter_map(|c| doc.latest(c, channel).map(|e| format!("{c} {} ({})", e.version, human(e.size)))).collect();
                    out.push(format!("{} offers in channel {channel}: {}.", s.name, if offered.is_empty() { "no system components".to_string() } else { offered.join(", ") }));
                    match comps::plan(&doc, &s.name, channel, &inst, self.backend.last_serial(s)) {
                        Ok(p) => out.push(comps::plan_text(&p)),
                        Err(e) => out.push(e),
                    }
                }
            }
        }
        Ok(out.join("\n"))
    }
}

impl Tools for ComponentsTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_components_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: TOOL.into(), description: "Show what the person's pinned private stores offer for the system components (kernel, boot, base, core, kernel layer) against what is installed, with the plan in plain words: sizes, difference or whole file, restart needed or not, and anything that must be updated first. Reads the stores' signed lists only; never downloads or installs.".into(), schema: json!({"type": "object", "properties": {"channel": {"type": "string", "description": "stable (default), beta or nightly"}}}) }, risk: Risk::Safe, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == TOOL { self.status(args) } else { self.inner.call(name, args) }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == TOOL { "Show the system component status".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_components_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        is_components_tool(name) || self.inner.approved_always(name)
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

/// The real backend: the stores the person pinned, the recorded versions and the system's egress.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
}

impl Backend for RealBackend {
    fn stores(&self) -> Vec<PinnedStore> {
        let cfg = crate::store::Store::for_services(&self.svc).config();
        cfg.sources.iter().filter(|s| s.enabled && !s.pin.is_empty() && s.kind == crate::store::catalog::SourceKind::Url).map(|s| PinnedStore { name: s.name.clone(), base: cm::base_of(&s.location), pin: s.pin.clone() }).collect()
    }
    fn installed(&self) -> Installed {
        Installed::load(self.svc.dir())
    }
    fn fetch(&self, store: &PinnedStore) -> Result<Doc, String> {
        comps::fetch_doc(self.svc.quick_upstream().as_ref(), store)
    }
}

//! The lead assistant's tools for system components (docs/development/COMPONENT-UPDATES-PLAN.md): `components_status` (read-only) shows
//! what the pinned private stores offer for the device's channel, what is installed, and the plan in plain words (sizes, delta or whole
//! file, restart or not, anything the installed parts cannot support). It contacts only stores the person added and pinned and reads
//! their signed list (checked against the pinned key); it never downloads an artifact or applies anything.
//!
//! `components_apply` installs the plan: a hard ask that never gets a session grant. The approval card IS the review (what changes, sizes,
//! restart needed or not, what is kept for going back); approving it makes the [`Approval`] that `compapply::apply_staged` requires, bound
//! to that exact plan, so a list that changed after the card was shown is not applied.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::store::compmanifest::{self as cm, Doc};
use crate::store::compapply::{self, Applied, Approval, Review};
use crate::store::components::{self as comps, human, Installed, PinnedStore};

pub const PROMPT_RULE: &str = "\nSYSTEM COMPONENTS: components_status shows what the person's pinned private stores offer for the kernel, boot, base system, core and kernel layer, what is installed, and the plan (sizes, only-the-difference or whole file, whether a restart is needed, and anything the installed parts cannot support). It only reads the stores' signed lists; it never downloads or installs a component. components_apply installs that plan after the person approves the card that shows it (what changes, sizes, restart needed, what is kept to go back); it always asks and the computer never restarts by itself. Do not promise a component will be installed before the person approved the card.";

pub const TOOL: &str = "components_status";
pub const APPLY_TOOL: &str = "components_apply";

pub fn is_components_tool(n: &str) -> bool {
    n == TOOL || n == APPLY_TOOL
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
    /// The plan and the card text for one store and channel (a read of its signed list; nothing is downloaded).
    fn review_for(&self, store: &PinnedStore, channel: &str) -> Result<(comps::Plan, Review), String> {
        let doc = self.fetch(store)?;
        let plan = comps::plan(&doc, &store.name, channel, &self.installed(), self.last_serial(store))?;
        if plan.steps.is_empty() {
            return Err(format!("{} has nothing newer for channel {channel}.", store.name));
        }
        let release = compapply::release_of(&plan, None)?;
        let rev = compapply::review(&plan, Some(&release));
        Ok((plan, rev))
    }
    /// Download, check and install the plan the person approved. Slow. The default cannot (a backend without a device).
    fn apply_approved(&self, _store: &PinnedStore, _channel: &str, _approval: &Approval) -> Result<Applied, String> {
        Err("applying system components is not available here".into())
    }
    /// Who approved (for the audit trail).
    fn approver(&self) -> String {
        "chat".into()
    }
}

pub struct ComponentsTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
    /// The digest of the plan the last card showed (the approval is only valid for that plan).
    shown: Mutex<Option<String>>,
}

impl ComponentsTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> ComponentsTools {
        ComponentsTools { inner, backend, shown: Mutex::new(None) }
    }

    /// Which store and channel a call means: the named store, else the only one.
    fn pick(&self, args: &Value) -> Result<(PinnedStore, String), String> {
        let channel = args.get("channel").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()).unwrap_or("stable").to_string();
        if !cm::valid_channel(&channel) {
            return Err(format!("`{channel}` is not a channel name (for example stable, beta, nightly)"));
        }
        let stores = self.backend.stores();
        let want = args.get("store").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty());
        match (want, stores.len()) {
            (_, 0) => Err("No private store with a pinned key is added, so there is nothing to install from. The person adds one in STORE, Settings, Catalogs (confirming its key fingerprint).".into()),
            (Some(n), _) => stores.into_iter().find(|s| s.name.eq_ignore_ascii_case(n)).map(|s| (s, channel)).ok_or_else(|| format!("no pinned store is named {n:?}")),
            (None, 1) => stores.into_iter().next().map(|s| (s, channel)).ok_or_else(|| "no store".to_string()),
            (None, _) => Err(format!("several stores are added ({}): say which with `store`", stores.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))),
        }
    }

    /// The card: the review of the plan as it is now, remembering its digest.
    fn card(&self, args: &Value) -> Result<String, String> {
        let (store, channel) = self.pick(args)?;
        let (_plan, rev) = self.backend.review_for(&store, &channel)?;
        *self.shown.lock().unwrap_or_else(|e| e.into_inner()) = Some(rev.digest.clone());
        Ok(rev.text)
    }

    fn apply(&self, args: &Value) -> Result<String, String> {
        let (store, channel) = self.pick(args)?;
        let (_plan, rev) = self.backend.review_for(&store, &channel)?;
        let shown = self.shown.lock().unwrap_or_else(|e| e.into_inner()).take();
        match shown {
            Some(d) if d == rev.digest => {}
            Some(_) => return Err("The store's list changed after the card was shown, so nothing was installed. Ask again to see the new plan.".into()),
            None => return Err("No plan was shown to the person, so nothing was installed.".into()),
        }
        let approval = Approval::given(&rev, &self.backend.approver());
        let done = self.backend.apply_approved(&store, &channel, &approval)?;
        Ok(done.text())
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
        v.push(ToolDef { spec: ToolSpec { name: APPLY_TOOL.into(), description: "Install the system components (kernel, boot, base, core, kernel layer) a pinned private store offers, as components_status planned them. Always asks: the card shows what changes, sizes, whether a restart is needed and what is kept to go back to. Downloads, checks every file against the store's signed list, installs through the update machinery (the previous version stays in the boot menu), and never restarts the computer by itself. Slow for a large part.".into(), schema: json!({"type": "object", "properties": {"store": {"type": "string", "description": "the store's name (needed only when several are added)"}, "channel": {"type": "string", "description": "stable (default), beta or nightly"}}}) }, risk: Risk::Ask, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOL.into(), description: "Show what the person's pinned private stores offer for the system components (kernel, boot, base, core, kernel layer) against what is installed, with the plan in plain words: sizes, difference or whole file, restart needed or not, and anything that must be updated first. Reads the stores' signed lists only; never downloads or installs.".into(), schema: json!({"type": "object", "properties": {"channel": {"type": "string", "description": "stable (default), beta or nightly"}}}) }, risk: Risk::Safe, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            TOOL => self.status(args),
            APPLY_TOOL => self.apply(args),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            TOOL => "Show the system component status".into(),
            APPLY_TOOL => match self.pick(args) {
                Ok((s, c)) => format!("Install the system components from {} (channel {c}); a restart is needed to use them", s.name),
                Err(_) => "Install system components from a store".into(),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            TOOL => None,
            APPLY_TOOL => Some(self.card(args).unwrap_or_else(|e| format!("Cannot install: {e}"))),
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            TOOL => None,
            APPLY_TOOL => self.pick(args).err(),
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_components_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name == TOOL || (name != APPLY_TOOL && self.inner.approved_always(name))
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == APPLY_TOOL { Some("installing system components changes the boot files and the programs the computer starts with, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
        crate::store::compinstalled::current(self.svc.dir(), &crate::store::compinstalled::Facts::real())
    }
    fn fetch(&self, store: &PinnedStore) -> Result<Doc, String> {
        comps::fetch_doc(self.svc.quick_upstream().as_ref(), store)
    }
    fn apply_approved(&self, store: &PinnedStore, channel: &str, approval: &Approval) -> Result<Applied, String> {
        let svc = compapply::Svc::real(self.svc.dir());
        let doc = self.fetch(store)?;
        let plan = comps::plan(&doc, &store.name, channel, &svc.installed(), self.last_serial(store))?;
        let st = iyakki_proxy::https::download_streamer();
        let prep = compapply::prepare(&svc, &plan, &store.base, st.as_ref(), None, &mut |_, _, _| {}, &|| false)?;
        compapply::apply_staged(&svc, &prep, Some(approval))
    }
    fn approver(&self) -> String {
        "assistant".into()
    }
}

impl RealBackend {
    /// For Settings: ask every pinned store how many parts are newer, remember the answer for the page (`components-last-check.json`).
    pub fn check_all(&self, channel: &str) -> Vec<compapply::StoreLine> {
        let now = crate::clock::now_unix();
        let lines: Vec<compapply::StoreLine> = self
            .stores()
            .iter()
            .map(|s| match self.fetch(s).and_then(|d| comps::plan(&d, &s.name, channel, &self.installed(), self.last_serial(s))) {
                Ok(p) => compapply::StoreLine { store: s.name.clone(), newer: p.steps.len(), checked_at: now, error: String::new() },
                Err(e) => compapply::StoreLine { store: s.name.clone(), newer: 0, checked_at: now, error: e.chars().take(120).collect() },
            })
            .collect();
        compapply::save_check(self.svc.dir(), &lines);
        lines
    }
}

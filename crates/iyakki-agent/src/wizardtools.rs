//! The assistant's tools for the wizard engine (F-CORE-24): `wizard_pending` lists what is waiting after an install or update and the wizards
//! of the installed apps; `wizard_start` opens one and shows its first step; `wizard_answer` hands in the values of a form; `wizard_continue` runs
//! the automatic steps (checks, migrations) and ASKS the person first with a card that lists them; `wizard_cancel` stops it. Reading and answering
//! change nothing outside the wizard; secret fields are never echoed. One run per person at a time.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use iyakki_core_api::wizard::WizardSpec;
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::wizardrun::{Env, Run, State, View};

pub const PENDING: &str = "wizard_pending";
pub const START: &str = "wizard_start";
pub const ANSWER: &str = "wizard_answer";
pub const CONTINUE: &str = "wizard_continue";
pub const CANCEL: &str = "wizard_cancel";

pub const PROMPT_RULE: &str = "\nWIZARDS: after an app is installed or updated it may bring a setup wizard. wizard_pending lists them; wizard_start opens one and shows the first step; ask the person each question in the step, then call wizard_answer with their values (never invent an answer, never ask for a secret in plain chat: tell the person to enter it in the app). wizard_continue runs the checks and data migrations and asks the person first. Say what the wizard reported; do not claim it finished until wizard_continue shows the summary.";

pub fn is_wizard_tool(n: &str) -> bool {
    matches!(n, PENDING | START | ANSWER | CONTINUE | CANCEL)
}

pub trait Backend: Send + Sync {
    /// `(app, version, spec)` of every installed app wizard.
    fn installed(&self) -> Vec<(String, String, WizardSpec)>;
    /// `(app, wizard id, trigger, title)` waiting after an install or update.
    fn pending(&self) -> Vec<(String, String, String, String)>;
    fn clear_pending(&self, app: &str, wizard: &str);
    fn env(&self) -> Arc<dyn Env>;
    /// The run in progress (one per person).
    fn run(&self) -> Arc<Mutex<Option<(String, Run)>>>;
    /// The line for the audit log.
    fn audit(&self, what: &str, detail: &str);
}

pub struct WizardTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
}

fn show(v: &View, log: &[String]) -> String {
    let mut out = vec![];
    match v {
        View::Form { title, fields, .. } => {
            out.push(format!("Step: {}", if title.is_empty() { "questions" } else { title }));
            for f in fields {
                let shown = match (&f.value, f.kind.as_str()) {
                    (_, "secret") => "(a secret: the person enters it in the app, not in the chat)".to_string(),
                    (Some(v), _) => format!("current: {v}"),
                    (None, _) => "no value yet".to_string(),
                };
                let choices = if f.choices.is_empty() { String::new() } else { format!(" choices: {}", f.choices.iter().map(|(v, l)| format!("{v} ({l})")).collect::<Vec<_>>().join(", ")) };
                out.push(format!("- {} [{}] {}{}{} - {}", f.id, f.kind, f.label, if f.required { " (needed)" } else { "" }, choices, shown));
                if !f.help.is_empty() {
                    out.push(format!("    {}", f.help));
                }
            }
            out.push("Ask the person, then call wizard_answer with {\"values\": {...}}.".into());
        }
        View::Working { title, .. } => out.push(format!("Next: {} (runs with wizard_continue, which asks the person first).", if title.is_empty() { "an automatic step" } else { title })),
        View::Summary { title, text } => {
            out.push(format!("{}\n{}", title, text));
            out.push("Done reading? wizard_continue finishes the wizard.".into());
        }
        View::Finished(State::Done) => out.push("The wizard is finished.".into()),
        View::Finished(State::Failed(e)) => out.push(format!("The wizard stopped: {e}")),
        View::Finished(State::Cancelled) => out.push("The wizard was cancelled.".into()),
        View::Finished(State::Running) => {}
    }
    if !log.is_empty() {
        out.push(format!("So far: {}", log.join("; ")));
    }
    out.join("\n")
}

impl WizardTools {
    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> WizardTools {
        WizardTools { inner, be }
    }

    fn pending(&self) -> String {
        let p = self.be.pending();
        let mut out = vec![];
        if p.is_empty() {
            out.push("Nothing is waiting.".to_string());
        } else {
            out.push("Waiting after an install or update:".into());
            out.extend(p.iter().map(|(a, w, t, title)| format!("- {a} / {w} ({t}){}", if title.is_empty() { String::new() } else { format!(": {title}") })));
        }
        let all = self.be.installed();
        if !all.is_empty() {
            out.push("Wizards of installed apps:".into());
            out.extend(all.iter().map(|(a, v, w)| format!("- {a} {v} / {} (runs on {}){}", w.id, w.trigger.on, if w.check_runnable().is_err() { " - needs a newer IYAKKI" } else { "" })));
        }
        out.join("\n")
    }

    fn start(&self, a: &Value) -> Result<String, String> {
        let app = a["app"].as_str().ok_or("give the app")?;
        let wiz = a["wizard"].as_str().ok_or("give the wizard id")?;
        let slot = self.be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.as_ref().is_some_and(|(_, r)| r.state() == &State::Running) {
            return Err("a wizard is already running; finish it or call wizard_cancel first".into());
        }
        let (_, ver, spec) = self.be.installed().into_iter().find(|(x, _, w)| x == app && w.id == wiz).ok_or("that app has no such wizard")?;
        let trigger = a["trigger"].as_str().map(str::to_string).unwrap_or_else(|| spec.trigger.on.clone());
        let from = self.be.pending().iter().find(|p| p.0 == app && p.1 == wiz).map(|_| ()).and(None::<String>);
        let installed_from = a["from_version"].as_str().map(str::to_string).or(from).or_else(|| (trigger == "on_update").then(|| ver.clone()));
        let run = Run::start(spec, &trigger, installed_from.as_deref())?;
        let text = show(&run.view(), run.log());
        *g = Some((app.to_string(), run));
        self.be.clear_pending(app, wiz);
        self.be.audit("wizard.start", &format!("{app}/{wiz} on {trigger}"));
        Ok(text)
    }

    fn answer(&self, a: &Value) -> Result<String, String> {
        let vals: BTreeMap<String, Value> = a["values"].as_object().ok_or("give {\"values\": {...}}")?.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let slot = self.be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open; call wizard_start")?;
        if let Err(errs) = run.submit(&vals) {
            return Err(errs.iter().map(|e| if e.field.is_empty() { e.message.clone() } else { format!("{}: {}", e.field, e.message) }).collect::<Vec<_>>().join("; "));
        }
        self.be.audit("wizard.answer", &format!("{app}/{} ({} value(s))", run.id(), vals.len()));
        Ok(show(&run.view(), run.log()))
    }

    fn upcoming(&self) -> Vec<String> {
        self.be.run().lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map(|(_, r)| r.upcoming()).unwrap_or_default()
    }

    fn cont(&self) -> Result<String, String> {
        let slot = self.be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open; call wizard_start")?;
        if matches!(run.view(), View::Summary { .. }) {
            run.finish();
            self.be.audit("wizard.finish", &format!("{app}/{}", run.id()));
            return Ok(show(&run.view(), run.log()));
        }
        let env = self.be.env();
        run.advance(env.as_ref());
        self.be.audit("wizard.run", &format!("{app}/{} -> {:?}", run.id(), run.state()));
        Ok(show(&run.view(), run.log()))
    }

    fn cancel(&self) -> Result<String, String> {
        let slot = self.be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open")?;
        run.cancel();
        self.be.audit("wizard.cancel", &format!("{app}/{}", run.id()));
        Ok("The wizard was cancelled.".into())
    }
}

// ------------------------------------------------------------------------------------------------ the same run, for a screen

/// A screen that shows the wizard (Settings, System update) works on the SAME run as the chat tools: the one run of the person in
/// [`Backend::run`]. A wizard started in the chat can be answered on the screen and the other way round (a secret is only ever typed on the screen).
pub mod screen {
    use super::*;

    /// What the screen has to show now: the app, the wizard's run view, and what the automatic steps ahead are.
    pub struct Shown {
        pub app: String,
        pub view: View,
        pub upcoming: Vec<String>,
        pub log: Vec<String>,
    }

    /// Open the wizard `wiz` of `app` (or pick up the one that is already running for this app). `from` is the version an update comes from.
    pub fn open(be: &dyn Backend, app: &str, wiz: &str, from: Option<&str>) -> Result<(), String> {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((a, r)) = g.as_ref().filter(|(_, r)| r.state() == &State::Running) {
            if a == app && r.id() == wiz {
                return Ok(());
            }
            return Err(format!("a wizard of {a} is already running; finish or cancel it first"));
        }
        let (_, ver, spec) = be.installed().into_iter().find(|(x, _, w)| x == app && w.id == wiz).ok_or("that app has no such wizard")?;
        let trigger = spec.trigger.on.clone();
        let installed_from = from.map(str::to_string).or_else(|| (trigger == "on_update").then_some(ver));
        let run = Run::start(spec, &trigger, installed_from.as_deref())?;
        *g = Some((app.to_string(), run));
        // it stays waiting until it is over: a cancelled or failed one can be started again
        be.audit("wizard.start", &format!("{app}/{wiz} on {trigger} (screen)"));
        Ok(())
    }

    /// The step the run is at, or `None` when no wizard is open.
    pub fn shown(be: &dyn Backend) -> Option<Shown> {
        let slot = be.run();
        let g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.as_ref().map(|(a, r)| Shown { app: a.clone(), view: r.view(), upcoming: r.upcoming(), log: r.log().to_vec() })
    }

    /// Hand in the answers of the form that is showing. The errors are one line per field, in the person's words.
    pub fn answer(be: &dyn Backend, values: &BTreeMap<String, Value>) -> Result<(), Vec<(String, String)>> {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((app, run)) = g.as_mut() else { return Err(vec![(String::new(), "no wizard is open".into())]) };
        run.submit(values).map_err(|e| e.into_iter().map(|x| (x.field, x.message)).collect::<Vec<_>>())?;
        be.audit("wizard.answer", &format!("{app}/{} ({} value(s), screen)", run.id(), values.len()));
        Ok(())
    }

    /// Run the automatic steps ahead (checks, migrations). The caller has asked the person first; the real work may take a while, so a
    /// screen calls this from a thread. Returns the state after it.
    pub fn run_steps(be: &dyn Backend) -> Result<State, String> {
        let env = be.env();
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open")?;
        run.advance(env.as_ref());
        be.audit("wizard.run", &format!("{app}/{} -> {:?} (screen)", run.id(), run.state()));
        if run.state() == &State::Done {
            be.clear_pending(app, run.id());
        }
        Ok(run.state().clone())
    }

    /// The person read the summary: the wizard is over.
    pub fn finish(be: &dyn Backend) -> Result<(), String> {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (app, run) = g.as_mut().ok_or("no wizard is open")?;
        run.finish();
        be.audit("wizard.finish", &format!("{app}/{} (screen)", run.id()));
        if run.state() == &State::Done {
            be.clear_pending(app, run.id());
        }
        Ok(())
    }

    pub fn cancel(be: &dyn Backend) {
        let slot = be.run();
        let mut g = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((app, run)) = g.as_mut() {
            run.cancel();
            be.audit("wizard.cancel", &format!("{app}/{} (screen)", run.id()));
        }
    }
}

impl Tools for WizardTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_wizard_tool(&d.spec.name));
        let t = |name: &str, d: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: d.into(), schema }, risk, untrusted: false };
        v.push(t(PENDING, "List the setup wizards waiting after an app was installed or updated, and the wizards of installed apps. Reads only.", json!({"type": "object", "properties": {}}), Risk::Safe));
        v.push(t(START, "Open a wizard of an installed app and show its first step. Nothing runs yet.", json!({"type": "object", "properties": {"app": {"type": "string"}, "wizard": {"type": "string"}, "trigger": {"type": "string", "enum": ["on_install", "on_update", "reconfigure", "on_remove"]}}, "required": ["app", "wizard"]}), Risk::Safe));
        v.push(t(ANSWER, "Hand the person's answers to the form step that is showing. Values are checked; nothing is kept when one is wrong. Never put a secret here.", json!({"type": "object", "properties": {"values": {"type": "object"}}, "required": ["values"]}), Risk::Safe));
        v.push(t(CONTINUE, "Run the automatic steps of the open wizard (checks and data migrations), or finish it at the summary. The person approves a card that lists the steps first.", json!({"type": "object", "properties": {}}), Risk::Ask));
        v.push(t(CANCEL, "Stop the open wizard. Answers already given are not applied.", json!({"type": "object", "properties": {}}), Risk::Safe));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            PENDING => Ok(self.pending()),
            START => self.start(args),
            ANSWER => self.answer(args),
            CONTINUE => self.cont(),
            CANCEL => self.cancel(),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            PENDING => "List setup wizards".into(),
            START => format!("Open the wizard {} of {}", args["wizard"].as_str().unwrap_or("?"), args["app"].as_str().unwrap_or("?")),
            ANSWER => "Hand in the answers of a wizard step".into(),
            CONTINUE => "Run the next steps of the wizard".into(),
            CANCEL => "Cancel the wizard".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == CONTINUE {
            let up = self.upcoming();
            return Some(if up.is_empty() { "Finish the wizard.".to_string() } else { format!("The wizard will now run:\n{}", up.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")) });
        }
        if is_wizard_tool(name) { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == CONTINUE && self.be.run().lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_none() {
            return Some("no wizard is open; call wizard_start first".into());
        }
        if is_wizard_tool(name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_wizard_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != CONTINUE && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == CONTINUE { Some("a wizard's checks and data migrations are different every time, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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

/// What the real wizard environment can do today: `net.reach` (a TCP connect with a short limit), `path.readable` and `path.writable` inside the
/// person's own home folder, and `capability.held` (the person's approval for the app). No migration is built in: a migration step fails with
/// a plain sentence until an app's migration runner exists (KI-780).
pub struct RealEnv {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
    pub app: String,
}

impl Env for RealEnv {
    fn check(&self, check: &str, param: &str) -> Result<(), String> {
        use std::net::ToSocketAddrs;
        match check {
            "net.reach" => {
                let hp = if param.contains(':') { param.to_string() } else { format!("{param}:443") };
                let addr = hp.to_socket_addrs().map_err(|_| format!("the name {param} could not be found"))?.next().ok_or_else(|| format!("{param} has no address"))?;
                std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3)).map(|_| ()).map_err(|e| format!("{param} did not answer: {e}"))
            }
            "path.readable" | "path.writable" => {
                let home = crate::scope::home_root(&self.user, self.svc.dir());
                let p = std::path::Path::new(param);
                let full = if p.is_absolute() { p.to_path_buf() } else { home.join(p) };
                if full.components().any(|c| matches!(c, std::path::Component::ParentDir)) || !full.starts_with(&home) {
                    return Err(format!("{param} is outside your own folder, which a wizard may not look at"));
                }
                let md = std::fs::metadata(&full).map_err(|_| format!("{param} is not there"))?;
                if check == "path.writable" && md.permissions().readonly() {
                    return Err(format!("{param} is read only"));
                }
                Ok(())
            }
            "capability.held" => {
                use iyakki_core_api::appcaps::ApprovalBook;
                let store = crate::store::Store::for_services(&self.svc);
                let key = store.records().into_iter().find(|(_, r)| r.name == self.app).map(|(k, _)| k).ok_or("that app is not installed")?;
                let a = crate::store::approvals::FileBook(&store).get(&key).ok_or_else(|| format!("{param} is not granted to the app"))?;
                if a.grants.iter().any(|g| g.name == param) && !a.revoked.iter().any(|g| g.name == param) { Ok(()) } else { Err(format!("{param} is not granted to the app")) }
            }
            other => Err(format!("this IYAKKI does not know the check {other}")),
        }
    }

    fn migrate(&self, reference: &str, _: Option<&str>, _: Option<&str>, _: &BTreeMap<String, Value>) -> Result<String, String> {
        Err(format!("this IYAKKI has no migration called {reference} yet; the app's data was not touched"))
    }
}

/// The real backend: the person's installed apps, their waiting list and one run at a time per person, kept while the program runs.
pub struct RealBackend {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

type RunSlot = Arc<Mutex<Option<(String, Run)>>>;

fn runs() -> &'static Mutex<std::collections::HashMap<String, RunSlot>> {
    static RUNS: std::sync::OnceLock<Mutex<std::collections::HashMap<String, RunSlot>>> = std::sync::OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

impl Backend for RealBackend {
    fn installed(&self) -> Vec<(String, String, WizardSpec)> {
        crate::store::wizardhook::installed(&crate::store::Store::for_services(&self.svc), &self.user)
    }
    fn pending(&self) -> Vec<(String, String, String, String)> {
        crate::store::wizardhook::pending(self.svc.dir(), &self.user).into_iter().map(|p| (p.app, p.wizard, p.trigger, p.title)).collect()
    }
    fn clear_pending(&self, app: &str, wizard: &str) {
        crate::store::wizardhook::clear(self.svc.dir(), &self.user, app, wizard);
    }
    fn env(&self) -> Arc<dyn Env> {
        let app = self.run().lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map(|(a, _)| a.clone()).unwrap_or_default();
        Arc::new(RealEnv { svc: self.svc.clone(), user: self.user.clone(), app })
    }
    fn run(&self) -> RunSlot {
        runs().lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(format!("{}|{}", self.svc.dir().display(), self.user)).or_default().clone()
    }
    fn audit(&self, what: &str, detail: &str) {
        (self.audit)(what, detail)
    }
}

#[cfg(test)]
#[path = "wizardtools_tests.rs"]
mod tests;

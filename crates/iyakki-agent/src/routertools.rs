//! The assistant's tools for the model router (F-ROUTER-07, chat half): `router_status`, `router_models`, `router_strategy_list` read only;
//! `router_set` (turn the router on or off, the privacy zone, the budget caps, a fixed planner, a correction of one model's profile),
//! `router_strategy_set` (which strategy decides), `router_strategy_add` and `router_strategy_remove` (the person's own strategy scripts)
//! change what decides which model sees the person's work, so each ALWAYS asks, even in Auto mode (`agent::hard_ask`), and each writes
//! an audit line without any content. A strategy script is shown in full on the card before it is added. Nothing here calls a model.

use std::path::PathBuf;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::providers::ProviderCfg;
use crate::router::{allowed, AcceptClaim, Budget, DefaultStrategy, Live, ModelPool, Plan, Router, RouterConfig, RunReport, Strategy, UsageSink, Verifier, DEFAULT_STRATEGY};
use crate::router_js::{valid_name, MAX_SOURCE_BYTES};
use crate::router_registry::{build, Cost, Profile, ProfileOverride, Role, Zone};
pub use iyakki_shared::routertools::{PROMPT_RULE};

pub const STATUS: &str = "router_status";
pub const MODELS: &str = "router_models";
pub const SET: &str = "router_set";
pub const STRATEGY_LIST: &str = "router_strategy_list";
pub const STRATEGY_SET: &str = "router_strategy_set";
pub const STRATEGY_ADD: &str = "router_strategy_add";
pub const STRATEGY_REMOVE: &str = "router_strategy_remove";
pub const RUN: &str = "router_run";
pub const CANCEL: &str = "router_cancel";


/// The press that approved a tool call, kept for the thread that carries the call out. The chat answers a card on the screen's thread,
/// where the person's press is the answer to the core's card too, but the tool runs on another thread that is not inside that press:
/// the core would refuse it for lack of a gesture. The chat takes the ticket of the press that approved ([`note_press`]); the router
/// redeems it, once and within the ticket's short life, when it asks the core ([`with_press`]). A call with no press behind it (an
/// automatic run, an old approval) gets the core's own refusal.
static PRESS: Mutex<Option<iyakki_core_api::guarded::GestureTicket>> = Mutex::new(None);

/// Keep the press being handled on this thread (if it is in one) for the next router call. Call it where an approval is answered.
pub fn note_press() {
    *PRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = iyakki_core_api::guarded::gesture_ticket();
}

/// Run `ask` with the kept press, which is spent by it.
pub fn with_press<R>(ask: impl FnOnce() -> R) -> R {
    let t = PRESS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    match t {
        Some(t) => t.redeem(ask),
        None => ask(),
    }
}

pub fn is_router_tool(n: &str) -> bool {
    matches!(n, STATUS | MODELS | SET | STRATEGY_LIST | STRATEGY_SET | STRATEGY_ADD | STRATEGY_REMOVE | RUN | CANCEL)
}

/// What the tools need from the system. The real one keeps files; tests use a fake in memory.
pub trait Backend: Send + Sync {
    fn providers(&self) -> Vec<ProviderCfg>;
    fn config(&self) -> RouterConfig;
    fn save_config(&self, c: &RouterConfig) -> Result<(), String>;
    /// The strategy scripts the person added: (name, bytes).
    fn strategies(&self) -> Vec<(String, usize)>;
    fn strategy_source(&self, name: &str) -> Option<String>;
    fn save_strategy(&self, name: &str, source: &str) -> Result<(), String>;
    fn remove_strategy(&self, name: &str) -> Result<(), String>;
    /// The summary of the latest run.
    fn last_run(&self) -> Option<String>;
    /// The line for the audit log (what, detail); never any message content.
    fn audit(&self, what: &str, detail: &str);
    /// The models a run may call. `None`: no model can be reached.
    fn pool(&self) -> Option<Arc<dyn ModelPool>>;
    /// Where the counts of a run go (the usage ledger).
    fn sink(&self) -> Option<Arc<dyn UsageSink>> {
        None
    }
    /// How a finished step is judged. The default accepts what the executor says; the IDE supplies a real check
    /// ([`crate::router::CheckedVerifier`]).
    fn verifier(&self) -> Arc<dyn Verifier> {
        Arc::new(AcceptClaim)
    }
    /// The tools an executor may use for a step that needs them (free, read-only ones; see [`ToolsForSteps`]). The default: none.
    fn step_tools(&self) -> Arc<dyn crate::router::StepTools> {
        Arc::new(crate::router::NoStepTools)
    }
    /// What an executor may take a photo or a sound with (the core's approvals apply to each use). The default: nothing.
    fn senses(&self) -> Arc<dyn crate::router::Senses> {
        Arc::new(crate::router::NoSenses)
    }
    /// How fast each model has answered so far, from the usage ledger: `provider/model` to (calls that worked, milliseconds in all). Default: none.
    fn speeds(&self) -> BTreeMap<String, (u64, u64)> {
        BTreeMap::new()
    }
    /// Tokens the router has used in the last 24 hours (from the usage ledger). Default: none counted.
    fn tokens_last_day(&self) -> u64 {
        0
    }
    /// Other members of the fleet a step may go to. The default: none.
    fn fleet_runner(&self) -> Option<Arc<dyn crate::router::FleetRunner>> {
        None
    }
    /// The core's permission to start a run (`router.use`, a card every time). `label` has numbers and the strategy, never the goal.
    fn permit_run(&self, _label: &str) -> Result<(), String> {
        Ok(())
    }
    /// The core's permission to change or add a strategy (`router.strategy.set`).
    fn permit_strategy(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    /// The one run of this person.
    fn slot(&self) -> Arc<RunSlot>;
    /// Keep the summary of the run that just ended.
    fn set_last_run(&self, text: &str);
}

/// The run of one person that is going on, if any (one at a time).
#[derive(Default)]
pub struct RunSlot {
    st: Mutex<Active>,
}

#[derive(Default)]
struct Active {
    cancel: Option<Arc<AtomicBool>>,
    live: Option<Arc<Mutex<Live>>>,
    done: Option<Arc<AtomicBool>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RunSlot {
    pub fn running(&self) -> bool {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).done.as_ref().is_some_and(|d| !d.load(Ordering::SeqCst))
    }

    /// Wait for the run to end (tests, and a screen that wants the result).
    pub fn join(&self) {
        let h = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).handle.take();
        if let Some(h) = h {
            let _ = h.join();
        }
    }

    pub fn cancel(&self) -> bool {
        let g = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match (&g.cancel, self.running_locked(&g)) {
            (Some(c), true) => {
                c.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    fn running_locked(&self, g: &Active) -> bool {
        g.done.as_ref().is_some_and(|d| !d.load(Ordering::SeqCst))
    }

    /// The plan of the run in progress, one row per step (for the IDE's panel); empty when nothing runs or the plan is not made yet.
    pub fn plan_rows(&self) -> Vec<crate::router::PlanRow> {
        let g = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.running_locked(&g) {
            return vec![];
        }
        g.live.as_ref().map(|l| l.lock().unwrap_or_else(std::sync::PoisonError::into_inner).plan.clone()).unwrap_or_default()
    }

    /// The progress of the run in progress, as plain lines.
    pub fn progress(&self) -> Option<String> {
        let g = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.running_locked(&g) {
            return None;
        }
        let l = g.live.as_ref()?.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let mut out = vec![if l.steps == 0 { format!("A run is in progress: the plan is being made ({} tokens, {} calls so far).", l.tokens, l.calls) } else { format!("A run is in progress: {} of {} steps done, {} waiting for a person, {} tokens, {} calls.", l.done, l.steps, l.waiting, l.tokens, l.calls) }];
        out.extend(crate::router::plan_lines(&l.plan).into_iter().map(|x| format!("  {x}")));
        out.extend(l.log.iter().rev().take(5).rev().map(|x| format!("- {x}")));
        Some(out.join("\n"))
    }
}

/// Start a run on its own thread. The core's permission is asked first. Returns what the person is told.
pub fn start_run(be: &Arc<dyn Backend>, goal: &str, plan: Option<Plan>) -> Result<String, String> {
    start_run_in(be, goal, plan, "")
}

/// The same for a place (`workspace:NAME` or `fleet:NAME`): the strategy chosen for that place runs it.
pub fn start_run_in(be: &Arc<dyn Backend>, goal: &str, plan: Option<Plan>, scope: &str) -> Result<String, String> {
    if !scope_ok(scope) {
        return Err("the place is workspace:NAME or fleet:NAME".into());
    }
    let mut cfg = be.config();
    if !cfg.enabled {
        return Err("the router is off: turn it on with router_set first".into());
    }
    if cfg.daily_tokens > 0 {
        let used = be.tokens_last_day();
        if used >= cfg.daily_tokens {
            return Err(format!("the router has used its {} tokens of the last 24 hours (you set that limit with router_set max_tokens_per_day); it can run again when some of that falls out of the day", cfg.daily_tokens));
        }
        // a run gets what is left of the day if that is less than a run's own limit
        cfg.budget.max_tokens = cfg.budget.max_tokens.min(cfg.daily_tokens - used).max(1);
    }
    let profiles = profiles_of(be.as_ref());
    let models = allowed(&profiles, cfg.max_zone);
    if !models.iter().any(|m| m.role.can_plan()) || !models.iter().any(|m| m.role.can_execute()) {
        return Err("a run needs one model that can plan and one that can run steps, both enabled and inside the zone you allow (router_models)".into());
    }
    let pool = be.pool().ok_or("no model can be reached")?;
    let slot = be.slot();
    if slot.running() {
        return Err("a run is already going on; router_cancel stops it".into());
    }
    let label = format!("{} steps, strategy {}, up to {} tokens", plan.as_ref().map_or("planned".to_string(), |p| p.steps.len().to_string()), cfg.strategy, cfg.budget.max_tokens);
    be.permit_run(&label)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let live = Arc::new(Mutex::new(Live::default()));
    let done = Arc::new(AtomicBool::new(false));
    let cfg2 = cfg.clone();
    let scope2 = scope.to_string();
    let (be2, goal2, cancel2, live2, done2) = (be.clone(), goal.to_string(), cancel.clone(), live.clone(), done.clone());
    let h = std::thread::Builder::new().name("iyakki-router-run".into()).spawn(move || {
        let audit_be = be2.clone();
        let audit: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(move |w, d| audit_be.audit(w, d));
        let mut r = Router::new(cfg2, &profiles, pool, strategy_for(be2.as_ref(), &scope2), audit).with_cancel(cancel2).with_live(live2).with_verifier(be2.verifier());
        if let Some(s) = be2.sink() {
            r = r.with_sink(s);
        }
        r = r.with_tools(be2.step_tools()).with_senses(be2.senses()).with_speeds(be2.speeds().into_iter().filter(|(_, (n, _))| *n > 0).map(|(k, (n, ms))| (k, ms / n)).collect());
        if let Some(f) = be2.fleet_runner() {
            r = r.with_fleet(f);
        }
        let rep = match plan {
            Some(p) => r.run_plan(&goal2, p, None),
            None => r.run(&goal2),
        };
        be2.set_last_run(&rep.summary());
        done2.store(true, Ordering::SeqCst);
    }).map_err(|e| format!("could not start the run: {e}"))?;
    *slot.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Active { cancel: Some(cancel), live: Some(live), done: Some(done), handle: Some(h) };
    be.audit("router.run.permit", &label);
    Ok(format!("The run has started ({label}). router_status shows how it goes; router_cancel stops it."))
}

/// The strategy the person chose, or the built-in one when it is missing.
pub fn strategy_of(be: &dyn Backend) -> Arc<dyn Strategy> {
    strategy_for(be, "")
}

/// The strategy for a place (`workspace:NAME`, `fleet:NAME`; empty: the computer's own): the one chosen for that place, else the computer's.
pub fn strategy_for(be: &dyn Backend, scope: &str) -> Arc<dyn Strategy> {
    let cfg = be.config();
    let name = cfg.scoped.get(scope).cloned().unwrap_or_else(|| cfg.strategy.clone());
    if name != DEFAULT_STRATEGY {
        if let Some(s) = be.strategy_source(&name).and_then(|src| crate::router_wasm::make(&name, &src).ok()) {
            return s;
        }
    }
    Arc::new(DefaultStrategy)
}

/// The text of a strategy to add: the `source` given, or the sample of the store named in `sample` (`first-model`).
fn strategy_source_arg(a: &Value) -> Result<String, String> {
    match a.get("sample").and_then(Value::as_str) {
        Some("first-model") => Ok(crate::router_wasm::SAMPLE_FIRST_MODEL.to_string()),
        Some(other) => Err(format!("there is no sample called {other} (the sample is first-model)")),
        None => a["source"].as_str().map(str::to_string).ok_or_else(|| "give the script text in source, or sample: first-model".to_string()),
    }
}

/// A place a strategy can be chosen for: `workspace:` or `fleet:` and a plain name.
pub fn scope_ok(s: &str) -> bool {
    s.is_empty() || s.split_once(':').is_some_and(|(k, n)| matches!(k, "workspace" | "fleet") && !n.is_empty() && n.len() <= 60 && n.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')))
}

pub fn profiles_of(be: &dyn Backend) -> Vec<Profile> {
    build(&be.providers(), &be.config().overrides)
}

/// No tools underneath: for a screen that uses the same code as the chat tools.
struct NoInner;

impl Tools for NoInner {
    fn defs(&self) -> Vec<ToolDef> {
        vec![]
    }
    fn call(&self, name: &str, _: &Value) -> Result<String, String> {
        Err(format!("unknown tool {name}"))
    }
    fn describe(&self, name: &str, _: &Value) -> String {
        name.to_string()
    }
}

pub struct RouterTools {
    inner: Arc<dyn Tools>,
    be: Arc<dyn Backend>,
    /// The place this chat belongs to (`workspace:NAME` in the IDE): used when a call names none.
    scope: String,
}

fn flag(a: &Value, k: &str) -> Result<Option<bool>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => Err(format!("{k} is true or false")),
    }
}

fn number(a: &Value, k: &str) -> Result<Option<u64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("{k} is a whole number")),
    }
}

/// What `router_set` would change: the new settings and one line per change. Nothing is saved.
pub fn apply_set(cfg: &RouterConfig, profiles: &[Profile], a: &Value) -> Result<(RouterConfig, Vec<String>), String> {
    let mut c = cfg.clone();
    let mut lines = vec![];
    if let Some(on) = flag(a, "enabled")? {
        c.enabled = on;
        lines.push(format!("The router is {}.", if on { "turned on" } else { "turned off" }));
    }
    if let Some(z) = a.get("max_zone").and_then(Value::as_str) {
        c.max_zone = Zone::parse(z).ok_or("max_zone is computer, network or internet")?;
        lines.push(format!("Models may run on {} or closer.", c.max_zone.label()));
    }
    let mut b: Budget = c.budget;
    if let Some(n) = number(a, "max_tokens")? {
        b.max_tokens = n;
    }
    if let Some(n) = number(a, "max_calls")? {
        b.max_calls = u32::try_from(n).map_err(|_| "max_calls is too large")?;
    }
    if let Some(n) = number(a, "max_attempts")? {
        b.max_attempts = u32::try_from(n).map_err(|_| "max_attempts is too large")?;
    }
    if let Some(n) = number(a, "max_escalations")? {
        b.max_escalations = u32::try_from(n).map_err(|_| "max_escalations is too large")?;
    }
    if b != c.budget {
        b.validate()?;
        c.budget = b;
        lines.push(format!("A run may use {} tokens and {} calls; a step gets {} tries and {} rounds with the planner.", b.max_tokens, b.max_calls, b.max_attempts, b.max_escalations));
    }
    if let Some(n) = number(a, "parallel")? {
        let n = u32::try_from(n).map_err(|_| "parallel is too large")?;
        if !(1..=crate::router::HARD_PARALLEL).contains(&n) {
            return Err(format!("parallel is 1 (one step at a time) to {}", crate::router::HARD_PARALLEL));
        }
        c.parallel = n;
        lines.push(if n == 1 { "Steps run one at a time.".to_string() } else { format!("Up to {n} steps that are ready at the same time run side by side.") });
    }
    if let Some(n) = number(a, "max_tokens_per_day")? {
        if n > crate::router::HARD_TOKENS * 100 {
            return Err("max_tokens_per_day is too large".into());
        }
        c.daily_tokens = n;
        lines.push(if n == 0 { "No daily token limit for the router.".to_string() } else { format!("All router runs together may use {n} tokens in 24 hours; a run gets what is left of that.") });
    }
    if let Some(p) = a.get("planner").and_then(Value::as_str) {
        if p.is_empty() || p == "auto" {
            c.planner = None;
            lines.push("The strategy picks the planner.".into());
        } else {
            if !profiles.iter().any(|m| m.id == p && m.role.can_plan()) {
                return Err(format!("{p} is not a model that can plan (router_models lists them)"));
            }
            c.planner = Some(p.to_string());
            lines.push(format!("{p} is the planner."));
        }
    }
    if let Some(m) = a.get("model").filter(|m| !m.is_null()) {
        let id = m.get("id").and_then(Value::as_str).ok_or("model needs an id")?;
        if !profiles.iter().any(|p| p.id == id) {
            return Err(format!("{id} is not a known model (router_models lists them)"));
        }
        let mut o = if m.get("reset").and_then(Value::as_bool) == Some(true) { ProfileOverride::default() } else { c.overrides.get(id).cloned().unwrap_or_default() };
        if let Some(r) = m.get("role").and_then(Value::as_str) {
            o.role = Some(Role::parse(r).ok_or("role is planner, executor or both")?);
        }
        if let Some(r) = m.get("cost").and_then(Value::as_str) {
            o.cost = Some(Cost::parse(r).ok_or("cost is free, low, medium or high")?);
        }
        if let Some(n) = number(m, "context")? {
            o.context = Some(u32::try_from(n).ok().filter(|n| (512..=10_000_000).contains(n)).ok_or("context is 512 to 10000000 tokens")?);
        }
        if let Some(z) = m.get("zone").and_then(Value::as_str) {
            o.zone = Some(Zone::parse(z).ok_or("zone is computer, network or internet")?);
        }
        for (k, slot) in [("tools", &mut o.tools), ("vision", &mut o.vision), ("audio", &mut o.audio), ("enabled", &mut o.enabled)] {
            if let Some(v) = flag(m, k)? {
                *slot = Some(v);
            }
        }
        if o.is_empty() {
            c.overrides.remove(id);
            lines.push(format!("{id} goes back to the hints from its provider."));
        } else {
            c.overrides.insert(id.to_string(), o);
            lines.push(format!("Your corrections to {id} are saved."));
        }
    }
    if lines.is_empty() {
        return Err("nothing to change: give enabled, max_zone, a budget limit, planner or model".into());
    }
    Ok((c, lines))
}

/// Calls that worked and the milliseconds they took, per model, from ledger lines (a line counts only when it has times).
pub fn speeds_of(entries: &[crate::usage::Entry]) -> BTreeMap<String, (u64, u64)> {
    let mut m: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.ms > 0 && e.calls > e.errors && matches!(e.cat.as_str(), "" | "llm" | "assistant" | "router")) {
        let x = m.entry(format!("{}/{}", e.provider, e.model)).or_default();
        x.0 += e.calls - e.errors;
        x.1 += e.ms;
    }
    m
}

fn profile_line(p: &Profile) -> String {
    let mut abilities = vec![];
    if p.caps.tools {
        abilities.push("tools");
    }
    if p.caps.vision {
        abilities.push("vision");
    }
    if p.caps.audio {
        abilities.push("audio");
    }
    format!("- {} : {}, cost {}, runs on {}, context {}, {}{}", p.id, p.role.label(), p.cost.label(), p.zone.label(), p.caps.context, if abilities.is_empty() { "text only".to_string() } else { abilities.join("+") }, if p.enabled { "" } else { " (off)" })
}

impl RouterTools {
    /// The router's changes without the chat: a screen calls the same code, so a change means the same on both.
    pub fn standalone(be: Arc<dyn Backend>) -> RouterTools {
        RouterTools { inner: Arc::new(NoInner), be, scope: String::new() }
    }

    pub fn new(inner: Arc<dyn Tools>, be: Arc<dyn Backend>) -> RouterTools {
        RouterTools { inner, be, scope: String::new() }
    }

    /// The place this tool set works for: a run started here uses the strategy chosen for it.
    pub fn with_scope(mut self, scope: &str) -> RouterTools {
        if scope_ok(scope) {
            self.scope = scope.to_string();
        }
        self
    }

    pub fn status(&self) -> String {
        let c = self.be.config();
        let all = profiles_of(self.be.as_ref());
        let ok = allowed(&all, c.max_zone);
        let mut out = vec![
            format!("The router is {}. Strategy: {}. Models may run on {} or closer.", if c.enabled { "on" } else { "off" }, c.strategy, c.max_zone.label()),
            format!("{} models known, {} usable ({} can plan, {} can run steps).", all.len(), ok.len(), ok.iter().filter(|m| m.role.can_plan()).count(), ok.iter().filter(|m| m.role.can_execute()).count()),
            format!("Budget per run: {} tokens, {} calls; per step {} tries and {} rounds with the planner.", c.budget.max_tokens, c.budget.max_calls, c.budget.max_attempts, c.budget.max_escalations),
        ];
        out.push(c.planner.as_ref().map_or("Planner: chosen by the strategy.".to_string(), |p| format!("Planner: {p} (fixed by you).")));
        if let Some(p) = self.be.slot().progress() {
            out.push(p);
        }
        out.push(match self.be.last_run() {
            Some(s) => format!("Latest run:\n{s}"),
            None => "No run yet.".into(),
        });
        out.join("\n")
    }

    pub fn models(&self) -> String {
        let c = self.be.config();
        let all = profiles_of(self.be.as_ref());
        if all.is_empty() {
            return "No models yet: add a provider in Settings, Models.".into();
        }
        let mut out = vec!["Roles, costs and context sizes are hints from the provider unless you corrected them with router_set.".to_string()];
        let speeds = self.be.speeds();
        out.extend(all.iter().map(|p| {
            let speed = speeds.get(&p.id).filter(|(n, _)| *n > 0).map(|(n, ms)| format!(", answers in about {} ms ({} calls seen)", ms / n, n)).unwrap_or_default();
            format!("{}{}{}", profile_line(p), speed, if p.zone > c.max_zone { " (outside the zone you allow)" } else { "" })
        }));
        if let Some(f) = self.be.fleet_runner() {
            let m = f.members();
            if !m.is_empty() {
                out.push("Models of fleet members (reached only by sending a step to the member, which runs it under its own rules):".into());
                let rated = f.member_models();
                out.extend(m.iter().map(|x| {
                    let own = rated.iter().find(|(n, _)| *n == x.name).map(|(_, l)| l.as_slice()).unwrap_or(&[]);
                    if own.is_empty() {
                        format!("- {} : {}{}{}", x.name, if x.models.is_empty() { "no model names shared".to_string() } else { x.models.join(", ") }, if x.vision { " (sees pictures)" } else { "" }, if x.audio { " (hears)" } else { "" })
                    } else {
                        format!("- {} (its own router rates them):\n{}", x.name, own.iter().map(|p| format!("    {}", profile_line(p))).collect::<Vec<_>>().join("\n"))
                    }
                }));
            }
        }
        out.join("\n")
    }

    pub fn set(&self, a: &Value) -> Result<String, String> {
        let cfg = self.be.config();
        let profiles = profiles_of(self.be.as_ref());
        let (new, lines) = apply_set(&cfg, &profiles, a)?;
        self.be.save_config(&new)?;
        self.be.audit("router.set", &format!("{} change(s)", lines.len()));
        Ok(lines.join("\n"))
    }

    pub fn plan_arg(a: &Value) -> Result<Option<Plan>, String> {
        match a.get("plan").filter(|p| !p.is_null()) {
            None => Ok(None),
            Some(p) => {
                let plan: Plan = serde_json::from_value(p.clone()).map_err(|e| format!("the plan is not in the agreed form: {e}"))?;
                plan.validate()?;
                Ok(Some(plan))
            }
        }
    }

    pub fn run(&self, a: &Value) -> Result<String, String> {
        let goal = a["goal"].as_str().map(str::trim).filter(|g| !g.is_empty()).ok_or("give the goal")?;
        let plan = Self::plan_arg(a)?;
        let scope = a.get("scope").and_then(Value::as_str).unwrap_or(&self.scope);
        start_run_in(&self.be, goal, plan, scope)
    }

    pub fn cancel(&self) -> Result<String, String> {
        if self.be.slot().cancel() {
            self.be.audit("router.cancel", "requested");
            Ok("The run will stop after the model call that is running.".into())
        } else {
            Ok("No run is going on.".into())
        }
    }

    fn run_card(&self, a: &Value) -> String {
        let c = self.be.config();
        let all = profiles_of(self.be.as_ref());
        let ok = allowed(&all, c.max_zone);
        let mut out = vec![format!("Goal: {}", a["goal"].as_str().unwrap_or("?"))];
        out.push(format!("Strategy {}; models inside {} or closer: {}.", c.strategy, c.max_zone.label(), if ok.is_empty() { "none".to_string() } else { ok.iter().map(|m| format!("{} ({}, {})", m.id, m.role.label(), m.zone.label())).collect::<Vec<_>>().join(", ") }));
        out.push(format!("At most {} tokens and {} calls; {} tries and {} rounds with the planner per step. The goal and what the steps produce are sent to those models.", c.budget.max_tokens, c.budget.max_calls, c.budget.max_attempts, c.budget.max_escalations));
        match Self::plan_arg(a) {
            Ok(Some(p)) => {
                out.push("The plan:".into());
                out.extend(p.steps.iter().map(|s| format!("- {}{}: {}{}", s.id, if s.after.is_empty() { String::new() } else { format!(" (after {})", s.after.join(", ")) }, s.task, if s.check.is_empty() { String::new() } else { format!(" [check: {}]", s.check) })));
            }
            Ok(None) => out.push("The planner will write the plan during the run; router_status shows it as it goes.".into()),
            Err(e) => out.push(format!("This will be refused: {e}")),
        }
        out.join("\n")
    }

    pub fn strategy_list(&self) -> String {
        let c = self.be.config();
        let mut out = vec![format!("- {DEFAULT_STRATEGY} (built in): cheapest model that can do the step; asks the planner on a block or after a second failure{}", if c.strategy == DEFAULT_STRATEGY { " [in use]" } else { "" })];
        for (n, size) in self.be.strategies() {
            out.push(format!("- {n} (your script, {size} bytes){}", if c.strategy == n { " [in use]" } else { "" }));
        }
        out.join("\n")
    }

    pub fn strategy_set(&self, a: &Value) -> Result<String, String> {
        let name = a["name"].as_str().ok_or("give the name")?;
        if name != DEFAULT_STRATEGY && !self.be.strategies().iter().any(|(n, _)| n == name) {
            return Err(format!("there is no strategy called {name} (router_strategy_list)"));
        }
        let scope = a.get("scope").and_then(Value::as_str).unwrap_or("");
        if !scope_ok(scope) {
            return Err("scope is workspace:NAME or fleet:NAME (leave it out for the whole computer)".into());
        }
        self.be.permit_strategy(name)?;
        let mut c = self.be.config();
        if scope.is_empty() {
            c.strategy = name.to_string();
        } else {
            c.scoped.insert(scope.to_string(), name.to_string());
        }
        self.be.save_config(&c)?;
        self.be.audit("router.strategy.set", &format!("name={name}{}", if scope.is_empty() { String::new() } else { format!(" scope={scope}") }));
        Ok(if scope.is_empty() { format!("The strategy is now {name}.") } else { format!("The strategy for {scope} is now {name}; other places keep the computer's own.") })
    }

    pub fn strategy_add(&self, a: &Value) -> Result<String, String> {
        let name = a["name"].as_str().ok_or("give the name")?;
        let source = strategy_source_arg(a)?;
        let source = source.as_str();
        let js = crate::router_wasm::make(name, source)?;
        crate::router::try_strategy(js.as_ref()).map_err(|e| format!("the script did not pass the trial run: {e}"))?;
        self.be.permit_strategy(name)?;
        self.be.save_strategy(name, source)?;
        self.be.audit("router.strategy.add", &format!("name={name} bytes={}", source.len()));
        Ok(format!("The strategy {name} is added. router_strategy_set uses it."))
    }

    pub fn strategy_remove(&self, a: &Value) -> Result<String, String> {
        let name = a["name"].as_str().ok_or("give the name")?;
        if !self.be.strategies().iter().any(|(n, _)| n == name) {
            return Err(format!("there is no strategy called {name}"));
        }
        self.be.permit_strategy(name)?;
        self.be.remove_strategy(name)?;
        let mut c = self.be.config();
        if c.strategy == name {
            c.strategy = DEFAULT_STRATEGY.into();
            self.be.save_config(&c)?;
        }
        self.be.audit("router.strategy.remove", &format!("name={name}"));
        Ok(format!("The strategy {name} is removed."))
    }
}

impl Tools for RouterTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }

    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_router_tool(&d.spec.name));
        let t = |name: &str, d: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: d.into(), schema }, risk, untrusted: false };
        let none = json!({"type": "object", "properties": {}});
        v.push(t(STATUS, "Say whether the model router is on, its strategy, its limits and what the latest run used. Reads only.", none.clone(), Risk::Safe));
        v.push(t(MODELS, "List the models the router can use with role (planner, executor, both), abilities, cost class, where each runs and context size. Reads only.", none.clone(), Risk::Safe));
        v.push(t(STRATEGY_LIST, "List the strategies: the built-in one and the person's own scripts. Reads only.", none, Risk::Safe));
        v.push(t(SET, "Change the router: turn it on or off, the farthest place a model may run (privacy zone), the budget limits of a run, a fixed planner, or the corrections of one model (role, cost, where it runs, context, abilities, on or off). Always asks the person.", json!({"type": "object", "properties": {"enabled": {"type": "boolean"}, "max_zone": {"type": "string", "enum": ["computer", "network", "internet"]}, "max_tokens": {"type": "integer"}, "max_calls": {"type": "integer"}, "max_attempts": {"type": "integer"}, "max_escalations": {"type": "integer"}, "parallel": {"type": "integer", "description": "steps that may run at the same time, 1 to 4; 1 is one at a time"}, "max_tokens_per_day": {"type": "integer", "description": "tokens all runs together may use in 24 hours, 0 for no limit"}, "planner": {"type": "string", "description": "a model id from router_models, or auto"}, "model": {"type": "object", "properties": {"id": {"type": "string"}, "role": {"type": "string", "enum": ["planner", "executor", "both"]}, "cost": {"type": "string", "enum": ["free", "low", "medium", "high"]}, "context": {"type": "integer"}, "tools": {"type": "boolean"}, "vision": {"type": "boolean"}, "audio": {"type": "boolean"}, "enabled": {"type": "boolean"}, "zone": {"type": "string", "enum": ["computer", "network", "internet"]}, "reset": {"type": "boolean"}}, "required": ["id"]}}}), Risk::Ask));
        v.push(t(STRATEGY_SET, "Choose the strategy that decides which model takes each step: `default` or the name of the person's own script. Always asks the person.", json!({"type": "object", "properties": {"name": {"type": "string"}, "scope": {"type": "string", "description": "workspace:NAME or fleet:NAME to choose it for that place only; leave out for the whole computer"}}, "required": ["name"]}), Risk::Ask));
        v.push(t(STRATEGY_ADD, "Add the person's own strategy (a JavaScript script defining decide(input), or a WebAssembly module exporting decide that reads the input from stdin and logs the decision; either runs in a sandbox with no file, network or program access). The person must have asked for it. The script is shown in full on the approval card.", json!({"type": "object", "properties": {"name": {"type": "string"}, "source": {"type": "string", "description": "JavaScript text, WebAssembly module text (WAT) or wasm-hex: and a binary module in hexadecimal"}, "sample": {"type": "string", "enum": ["first-model"], "description": "a sample from the store instead of source"}}, "required": ["name"]}), Risk::Ask));
        v.push(t(RUN, "Start a model router run: a bigger model plans the goal into steps and lighter models do them, asking the bigger one when stuck. Spends tokens and sends the goal to the models, so it always asks the person; the card shows the models, the limits and the plan. Optional plan: {steps: [{id, task, after: [ids], needs: {tools, vision, audio}, check}]}. Runs in the background; router_status shows it.", json!({"type": "object", "properties": {"goal": {"type": "string"}, "plan": {"type": "object"}}, "required": ["goal"]}), Risk::Ask));
        v.push(t(CANCEL, "Stop the router run that is going on, after the model call that is running. Free.", json!({"type": "object", "properties": {}}), Risk::Safe));
        v.push(t(STRATEGY_REMOVE, "Remove one of the person's strategy scripts. Always asks the person.", json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}), Risk::Ask));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS => Ok(self.status()),
            MODELS => Ok(self.models()),
            SET => self.set(args),
            STRATEGY_LIST => Ok(self.strategy_list()),
            STRATEGY_SET => self.strategy_set(args),
            STRATEGY_ADD => self.strategy_add(args),
            STRATEGY_REMOVE => self.strategy_remove(args),
            RUN => self.run(args),
            CANCEL => self.cancel(),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS => "Show the model router".into(),
            MODELS => "List the models of the router".into(),
            STRATEGY_LIST => "List the router strategies".into(),
            SET => "Change the model router".into(),
            STRATEGY_SET => format!("Use the router strategy {}", args["name"].as_str().unwrap_or("?")),
            STRATEGY_ADD => format!("Add the router strategy script {}", args["name"].as_str().unwrap_or("?")),
            STRATEGY_REMOVE => format!("Remove the router strategy {}", args["name"].as_str().unwrap_or("?")),
            RUN => "Start a model router run".into(),
            CANCEL => "Stop the router run".into(),
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET => Some(match apply_set(&self.be.config(), &profiles_of(self.be.as_ref()), args) {
                Ok((_, lines)) => lines.join("\n"),
                Err(e) => format!("This will be refused: {e}"),
            }),
            RUN => Some(self.run_card(args)),
            STRATEGY_ADD => {
                let src = strategy_source_arg(args).unwrap_or_default();
                let kind = if crate::router_wasm::is_wasm_source(&src) { "WebAssembly module" } else { "script" };
                Some(format!("The {kind} ({} bytes, at most {MAX_SOURCE_BYTES}) runs in a sandbox with no file, network or program access, limited in time and memory. It decides which model sees each step of your work. Its text:\n{}", src.len(), src))
            }
            _ if is_router_tool(name) => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == STRATEGY_ADD && !valid_name(args["name"].as_str().unwrap_or("")) {
            return Some("a strategy name is lowercase letters, digits, - and _, starting with a letter, at most 32, and not `default`".into());
        }
        if name == RUN {
            if args["goal"].as_str().is_none_or(|g| g.trim().is_empty()) {
                return Some("give the goal".into());
            }
            if let Err(e) = Self::plan_arg(args) {
                return Some(e);
            }
        }
        if is_router_tool(name) { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_router_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !matches!(name, SET | STRATEGY_SET | STRATEGY_ADD | STRATEGY_REMOVE | RUN) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if matches!(name, SET | STRATEGY_SET | STRATEGY_ADD | STRATEGY_REMOVE | RUN) { Some("what decides which model sees your work is changed or started only with your answer each time".into()) } else { self.inner.grant_blocker(name, args) }
    }
}

// ------------------------------------------------------------------------------------------------ the real backend

/// One run slot per router folder for the whole process: the chat, the Settings page and every chat session of a person see the same run.
fn slot_for(dir: &std::path::Path) -> Arc<RunSlot> {
    static SLOTS: Mutex<Vec<(PathBuf, Arc<RunSlot>)>> = Mutex::new(Vec::new());
    let mut g = SLOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, s)) = g.iter().find(|(d, _)| d == dir) {
        return s.clone();
    }
    let s = Arc::new(RunSlot::default());
    g.push((dir.to_path_buf(), s.clone()));
    s
}

/// The account's tools as an executor's tools (F-ROUTER-10): only FREE tools (those that never ask), none on the hard-ask list, none of the router's
/// own (a step cannot start a run or change the router), and a call goes through the same wrappers, refusals and grants as in the chat. A tool that
/// would ask the person is NOT offered: an executor runs unattended, so there is nobody to press Allow, and the step is handed to a person
/// (the executor answers `blocked`) when it needs one. Results are marked as outside data.
pub struct ToolsForSteps {
    inner: Arc<dyn Tools>,
    /// The core's card for one call that asks (`Router::ask_step_tool`). Without one a tool that asks is not offered (the old rule).
    asker: Option<Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>>,
}

impl ToolsForSteps {
    pub fn new(inner: Arc<dyn Tools>) -> ToolsForSteps {
        ToolsForSteps { inner, asker: None }
    }

    /// Let a step use a tool that asks (F-ROUTER-40): each call is put to the person as a card through the core and the step waits for the
    /// answer; a denied or unanswered card comes back to the executor as words. Destructive calls ask just as in the chat.
    pub fn with_asker(mut self, f: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>) -> ToolsForSteps {
        self.asker = Some(f);
        self
    }

    fn askable(&self, name: &str) -> bool {
        self.asker.is_some() && !is_router_tool(name) && !name.starts_with("router_") && !name.starts_with("fleet_")
    }

    fn offered(&self, name: &str, risk: Risk) -> bool {
        risk == Risk::Safe && !is_router_tool(name) && !name.starts_with("router_") && !name.starts_with("fleet_") && crate::agent::hard_ask(name, &json!({})).is_none()
    }
}

impl crate::router::StepTools for ToolsForSteps {
    fn list(&self) -> Vec<(String, String)> {
        self.inner
            .defs()
            .into_iter()
            .filter_map(|d| {
                let one = d.spec.description.split(". ").next().unwrap_or("").to_string();
                if self.offered(&d.spec.name, d.risk) {
                    Some((d.spec.name.clone(), one))
                } else if self.askable(&d.spec.name) {
                    Some((d.spec.name.clone(), format!("{one} (the person is asked first)")))
                } else {
                    None
                }
            })
            .collect()
    }
    fn call(&self, _step: &str, name: &str, args: &Value) -> Result<String, String> {
        let def = self.inner.defs().into_iter().find(|d| d.spec.name == name).ok_or("that tool does not exist")?;
        let free = self.offered(name, def.risk);
        if !free && !self.askable(name) {
            return Err("that tool needs the person to approve it, so a step cannot use it".into());
        }
        if let Some(why) = self.inner.refusal(name, args) {
            return Err(why);
        }
        let asks = crate::agent::hard_ask(name, args).is_some() || !free;
        if asks {
            let Some(ask) = &self.asker else { return Err("that call needs the person to approve it, so a step cannot use it".into()) };
            // the card names the tool and what the call would do; the step is held here until the person answers
            ask(&format!("step tool {name}: {}", crate::fleet::clean(&self.inner.describe(name, args), 160)))?;
        }
        self.inner.call(name, args)
    }
}

impl crate::services::Services {
    /// The plan of the run in progress of this person, one row per step: (id, title, after, status, model); empty when nothing runs. For the IDE's ROUTER panel.
    pub fn router_plan_rows(&self, user: &str) -> Vec<(String, String, Vec<String>, &'static str, Option<String>)> {
        slot_for(&router_dir(self.dir(), user)).plan_rows().into_iter().map(|r| (r.id, r.title, r.after, r.status, r.model)).collect()
    }

    /// The tools an executor of the router may use, over the tool chain `tools` (see [`ToolsForSteps`]).
    pub fn router_step_tools(&self, tools: Arc<dyn Tools>, gate: Option<Arc<dyn iyakki_core_api::router::Router>>) -> Arc<dyn crate::router::StepTools> {
        let t = ToolsForSteps::new(tools);
        Arc::new(match gate {
            Some(g) => t.with_asker(Arc::new(move |label| g.ask_step_tool(label))),
            None => t,
        })
    }
}

fn router_dir(data: &std::path::Path, user: &str) -> PathBuf {
    let safe: String = user.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    data.join("router").join(if safe.is_empty() { "_".into() } else { safe })
}

/// Files under one folder of the person: `config.json`, `strategies/<name>.js`, `last-run.txt`.
pub struct FileBackend {
    pub dir: PathBuf,
    pub providers: Arc<dyn Fn() -> Vec<ProviderCfg> + Send + Sync>,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
    last: Mutex<Option<String>>,
    slot: Arc<RunSlot>,
    /// The core's door (`cx.os.router()`): the permission and the access log. Without one only the callback `audit` is used.
    gate: Option<Arc<dyn iyakki_core_api::router::Router>>,
    pool: Option<Arc<dyn ModelPool>>,
    sink: Option<Arc<dyn UsageSink>>,
    verifier: Option<Arc<dyn Verifier>>,
    senses: Option<Arc<dyn crate::router::Senses>>,
    step_tools: Option<Arc<dyn crate::router::StepTools>>,
    fleet: Option<Arc<dyn crate::router::FleetRunner>>,
    usage: Option<Arc<dyn Fn() -> Vec<crate::usage::Entry> + Send + Sync>>,
}

fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("could not make the folder: {e}"))?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("could not save: {e}"))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("could not save: {e}"))?;
    f.sync_all().map_err(|e| format!("could not save: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save: {e}"))
}

impl FileBackend {
    pub fn new(dir: PathBuf, providers: Arc<dyn Fn() -> Vec<ProviderCfg> + Send + Sync>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> FileBackend {
        FileBackend { slot: slot_for(&dir), dir, providers, audit, last: Mutex::new(None), gate: None, pool: None, sink: None, verifier: None, senses: None, step_tools: None, fleet: None, usage: None }
    }

    /// The core's router door: runs and strategy changes ask it, and every audit line is also written to the access log through it.
    pub fn with_gate(mut self, g: Arc<dyn iyakki_core_api::router::Router>) -> FileBackend {
        self.gate = Some(g);
        self
    }

    pub fn with_pool(mut self, p: Arc<dyn ModelPool>) -> FileBackend {
        self.pool = Some(p);
        self
    }

    pub fn with_sink(mut self, s: Arc<dyn UsageSink>) -> FileBackend {
        self.sink = Some(s);
        self
    }

    pub fn with_verifier(mut self, v: Arc<dyn Verifier>) -> FileBackend {
        self.verifier = Some(v);
        self
    }

    /// The tools an executor may use (F-ROUTER-10), usually a [`ToolsForSteps`] over the account's tool chain.
    pub fn with_step_tools(mut self, t: Arc<dyn crate::router::StepTools>) -> FileBackend {
        self.step_tools = Some(t);
        self
    }

    pub fn with_senses(mut self, s: Arc<dyn crate::router::Senses>) -> FileBackend {
        self.senses = Some(s);
        self
    }

    pub fn with_fleet(mut self, f: Option<Arc<dyn crate::router::FleetRunner>>) -> FileBackend {
        self.fleet = f;
        self
    }

    /// The person's router folder inside the services folder.
    pub fn for_services(svc: Arc<crate::services::Services>, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> FileBackend {
        let dir = router_dir(svc.dir(), user);
        let s2 = svc.clone();
        let (s3, u) = (svc.clone(), user.to_string());
        FileBackend::new(dir, Arc::new(move || s2.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().providers.clone()), audit).with_usage(Arc::new(move || s3.usage.entries(Some(&u))))
    }

    /// Where the speed figures come from (the usage ledger of the person).
    pub fn with_usage(mut self, f: Arc<dyn Fn() -> Vec<crate::usage::Entry> + Send + Sync>) -> FileBackend {
        self.usage = Some(f);
        self
    }

    /// Keep the summary of a run for `router_status`.
    pub fn record_run(&self, r: &RunReport) {
        let s = r.summary();
        let _ = write_private(&self.dir.join("last-run.txt"), &s);
        *self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(s);
    }

    fn strategy_path(&self, name: &str) -> Option<PathBuf> {
        valid_name(name).then(|| {
            let d = self.dir.join("strategies");
            if d.join(format!("{name}.wat")).is_file() { d.join(format!("{name}.wat")) } else { d.join(format!("{name}.js")) }
        })
    }
}

impl Backend for FileBackend {
    fn providers(&self) -> Vec<ProviderCfg> {
        (self.providers)()
    }
    fn config(&self) -> RouterConfig {
        std::fs::read_to_string(self.dir.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save_config(&self, c: &RouterConfig) -> Result<(), String> {
        write_private(&self.dir.join("config.json"), &serde_json::to_string_pretty(c).map_err(|e| e.to_string())?)
    }
    fn strategies(&self) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = std::fs::read_dir(self.dir.join("strategies")).into_iter().flatten().flatten().filter_map(|e| {
            let p = e.path();
            let name = p.file_stem()?.to_str()?.to_string();
            (matches!(p.extension()?.to_str()?, "js" | "wat") && valid_name(&name)).then(|| (name, e.metadata().map_or(0, |m| m.len() as usize)))
        }).collect();
        v.sort();
        v
    }
    fn strategy_source(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.strategy_path(name)?).ok()
    }
    fn save_strategy(&self, name: &str, source: &str) -> Result<(), String> {
        if !valid_name(name) {
            return Err("bad strategy name".into());
        }
        let d = self.dir.join("strategies");
        // a module is kept as `.wat`, a script as `.js`; saving one kind removes the other of the same name
        let (keep, drop) = if crate::router_wasm::is_wasm_source(source) { ("wat", "js") } else { ("js", "wat") };
        let _ = std::fs::remove_file(d.join(format!("{name}.{drop}")));
        write_private(&d.join(format!("{name}.{keep}")), source)
    }
    fn remove_strategy(&self, name: &str) -> Result<(), String> {
        std::fs::remove_file(self.strategy_path(name).ok_or("bad strategy name")?).map_err(|e| format!("could not remove it: {e}"))
    }
    fn last_run(&self) -> Option<String> {
        self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().or_else(|| std::fs::read_to_string(self.dir.join("last-run.txt")).ok())
    }
    fn audit(&self, what: &str, detail: &str) {
        (self.audit)(what, detail);
        if let Some(g) = &self.gate {
            g.record(what, detail);
        }
    }
    fn pool(&self) -> Option<Arc<dyn ModelPool>> {
        self.pool.clone()
    }
    fn sink(&self) -> Option<Arc<dyn UsageSink>> {
        self.sink.clone()
    }
    fn verifier(&self) -> Arc<dyn Verifier> {
        self.verifier.clone().unwrap_or_else(|| Arc::new(AcceptClaim))
    }
    fn senses(&self) -> Arc<dyn crate::router::Senses> {
        self.senses.clone().unwrap_or_else(|| Arc::new(crate::router::NoSenses))
    }
    fn step_tools(&self) -> Arc<dyn crate::router::StepTools> {
        self.step_tools.clone().unwrap_or_else(|| Arc::new(crate::router::NoStepTools))
    }
    fn fleet_runner(&self) -> Option<Arc<dyn crate::router::FleetRunner>> {
        self.fleet.clone()
    }
    fn speeds(&self) -> BTreeMap<String, (u64, u64)> {
        self.usage.as_ref().map(|f| speeds_of(&f())).unwrap_or_default()
    }
    fn tokens_last_day(&self) -> u64 {
        let now_h = crate::clock::now_unix() / 3600;
        self.usage.as_ref().map(|f| f().iter().filter(|e| e.src == crate::router::SRC_ROUTER && e.h >= now_h - 23).map(|e| e.input + e.output).sum()).unwrap_or(0)
    }
    fn permit_run(&self, label: &str) -> Result<(), String> {
        self.gate.as_ref().map_or(Ok(()), |g| with_press(|| g.begin_run(label)))
    }
    fn permit_strategy(&self, name: &str) -> Result<(), String> {
        self.gate.as_ref().map_or(Ok(()), |g| with_press(|| g.allow_strategy(name)))
    }
    fn slot(&self) -> Arc<RunSlot> {
        self.slot.clone()
    }
    fn set_last_run(&self, text: &str) {
        let _ = write_private(&self.dir.join("last-run.txt"), text);
        *self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text.to_string());
    }
}

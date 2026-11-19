//! Settings -> Model router (experimental): a bigger model plans, lighter models do the steps, and the router asks the bigger one when a
//! step gets stuck (F-ROUTER-07; `iyakki_agent::router*`, docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md).
//!
//! The page and the assistant's `router_*` tools are one thing: every change here calls the same code as the tool (`RouterTools`), after the
//! same kind of question: a card that says what will change, answered with y (a hard ask, nothing changes without it). Strategy changes and
//! runs also ask the core (`router.strategy.set`, `router.use`), which holds the same press as a hard ask. A strategy script is added
//! from a file in the person's home; its whole text is shown first and it is tried on a sample before it is kept.
//!
//! Keys: Up/Down choose a model; o router on/off; r role; c cost; z where the model runs; e use the model or not; u back to the hints;
//! m the farthest place a model may run; b the budget limits; s choose the strategy; a add a script; x remove a script; k cancel the run.

use iyakki_shared::router::{Budget, DEFAULT_STRATEGY};
use iyakki_shared::router_registry::{Cost, Profile, Role, Zone};
use iyakki_agent_api::{RouterPanel as RouterHandle};
use serde_json::{json, Value};

use super::*;

/// What a confirmed card does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RouterAct {
    /// `router_set` with these arguments (as JSON text).
    Set(String),
    Strategy(String),
    Remove(String),
    /// A script, by name and text, that was shown in full and is now added.
    Add(String, String),
}

#[derive(Default)]
pub(super) struct RouterPanel {
    /// A seam for tests: the backend (the real one is made from the services and `cx.os.router()`).
    pub(super) be: Option<Arc<dyn RouterHandle>>,
    /// A script being looked at before it is added: (name, text), and the line it is scrolled to.
    pub(super) review: Option<(String, String)>,
    pub(super) scroll: usize,
}

/// The longest script the page reads from a file: the router's own limit.
const MAX_SCRIPT: usize = iyakki_shared::router_js::MAX_SOURCE_BYTES;

fn next_role(r: Role) -> Role {
    match r {
        Role::Planner => Role::Executor,
        Role::Executor => Role::Both,
        Role::Both => Role::Planner,
    }
}

fn next_cost(c: Cost) -> Cost {
    match c {
        Cost::Free => Cost::Low,
        Cost::Low => Cost::Medium,
        Cost::Medium => Cost::High,
        Cost::High => Cost::Free,
    }
}

fn next_zone(z: Zone) -> Zone {
    match z {
        Zone::Computer => Zone::Network,
        Zone::Network => Zone::Internet,
        Zone::Internet => Zone::Computer,
    }
}

fn zone_word(z: Zone) -> &'static str {
    match z {
        Zone::Computer => "computer",
        Zone::Network => "network",
        Zone::Internet => "internet",
    }
}

impl SettingsApp {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn with_router_backend(mut self, be: Arc<dyn RouterHandle>) -> SettingsApp {
        self.router.be = Some(be);
        self
    }

    pub(super) fn router_be(&self, cx: &AppCx) -> Arc<dyn RouterHandle> {
        if let Some(b) = &self.router.be {
            return b.clone();
        }
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        cx.services.router_panel(&cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)), cx.os.router())
    }

    pub(super) fn router_models(&self, cx: &AppCx) -> Vec<Profile> {
        self.router_be(cx).profiles()
    }

    pub(super) fn router_len(&self, cx: &AppCx) -> usize {
        self.router_models(cx).len()
    }

    fn router_ask(&mut self, args: Value, text: String) {
        self.modal = Some(Modal::Confirm(ConfirmKind::Router(RouterAct::Set(args.to_string())), text));
    }

    pub(super) fn handle_router(&mut self, k: &Key, cx: &AppCx) {
        if let Some((name, src)) = self.router.review.clone() {
            let last = src.lines().count().saturating_sub(1);
            match k {
                Key::Esc | Key::Char('n') => self.router.review = None,
                Key::Up => self.router.scroll = self.router.scroll.saturating_sub(1),
                Key::Down => self.router.scroll = (self.router.scroll + 1).min(last),
                Key::PageUp => self.router.scroll = self.router.scroll.saturating_sub(10),
                Key::PageDown => self.router.scroll = (self.router.scroll + 10).min(last),
                Key::Home => self.router.scroll = 0,
                Key::End => self.router.scroll = last,
                Key::Char('y') => {
                    self.router.review = None;
                    self.modal = Some(Modal::Confirm(ConfirmKind::Router(RouterAct::Add(name.clone(), src)), format!("Add the strategy script {name}? It is tried on a sample first.")));
                }
                _ => {}
            }
            return;
        }
        if matches!(k, Key::Char('k')) {
            let be = self.router_be(cx);
            return match be.cancel() {
                Ok(m) => self.ok(m),
                Err(e) => self.fail(e),
            };
        }
        if !self.admin(cx) {
            return;
        }
        let be = self.router_be(cx);
        let cfg = be.config();
        let models = be.profiles();
        let len = models.len();
        let sel = self.sel[self.tab].min(len.saturating_sub(1));
        let cur = models.get(sel).cloned();
        let need = |s: &mut SettingsApp, f: &dyn Fn(&mut SettingsApp, &Profile)| match &cur {
            Some(p) => f(s, p),
            None => s.fail("There are no models yet: add a provider in Settings, Models."),
        };
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('o') => {
                let on = !cfg.enabled;
                self.router_ask(json!({"enabled": on}), format!("Turn the model router {}?", if on { "on" } else { "off" }));
            }
            Key::Char('m') => {
                let z = next_zone(cfg.max_zone);
                self.router_ask(json!({"max_zone": zone_word(z)}), format!("Let models run on {} or closer?", z.label()));
            }
            Key::Char('r') => need(self, &|s, p| {
                let r = next_role(p.role);
                s.router_ask(json!({"model": {"id": p.id, "role": r.label()}}), format!("{}: use it as {}?", p.id, r.label()));
            }),
            Key::Char('c') => need(self, &|s, p| {
                let c = next_cost(p.cost);
                s.router_ask(json!({"model": {"id": p.id, "cost": c.label()}}), format!("{}: count its cost as {}?", p.id, c.label()));
            }),
            Key::Char('z') => need(self, &|s, p| {
                let z = next_zone(p.zone);
                s.router_ask(json!({"model": {"id": p.id, "zone": zone_word(z)}}), format!("{}: it runs on {}?", p.id, z.label()));
            }),
            Key::Char('e') => need(self, &|s, p| {
                let on = !p.enabled;
                s.router_ask(json!({"model": {"id": p.id, "enabled": on}}), format!("{}: {} for the router?", p.id, if on { "use it" } else { "stop using it" }));
            }),
            Key::Char('u') => need(self, &|s, p| s.router_ask(json!({"model": {"id": p.id, "reset": true}}), format!("{}: go back to the hints from its provider?", p.id))),
            Key::Char('b') => {
                let b = cfg.budget;
                self.open_form(
                    FormKind::RouterBudget,
                    Form::new("Budget limits of a run")
                        .text("Tokens", &b.max_tokens.to_string(), "all models together")
                        .text("Model calls", &b.max_calls.to_string(), "all models together")
                        .text("Tries per step", &b.max_attempts.to_string(), "all executors")
                        .text("Rounds with the planner per step", &b.max_escalations.to_string(), "0 to 5"),
                );
            }
            Key::Char('s') => {
                let mut items = vec![DEFAULT_STRATEGY.to_string()];
                items.extend(be.strategies().into_iter().map(|(n, _)| n));
                self.modal = Some(Modal::Pick(PickKind::RouterStrategy, Picker::new("Which strategy decides?", items)));
            }
            Key::Char('a') => self.open_form(FormKind::RouterScript, Form::new("Add a strategy script").text("Name", "", "lowercase letters, digits, - and _").text("File in your home folder", "", "a JavaScript file that defines decide(input)")),
            Key::Char('x') => {
                let items: Vec<String> = be.strategies().into_iter().map(|(n, _)| n).collect();
                if items.is_empty() {
                    self.fail("There are no scripts of yours to remove.");
                } else {
                    self.modal = Some(Modal::Pick(PickKind::RouterRemove, Picker::new("Remove which script?", items)));
                }
            }
            _ => {}
        }
    }

    pub(super) fn router_pick(&mut self, cx: &AppCx, kind: PickKind, item: &str) {
        let _ = cx;
        match kind {
            PickKind::RouterStrategy => self.modal = Some(Modal::Confirm(ConfirmKind::Router(RouterAct::Strategy(item.to_string())), format!("Use the strategy {item} to decide which model takes each step?"))),
            _ => self.modal = Some(Modal::Confirm(ConfirmKind::Router(RouterAct::Remove(item.to_string())), format!("Remove the strategy script {item}?"))),
        }
    }

    pub(super) fn router_confirm(&mut self, cx: &AppCx, act: RouterAct) {
        if !self.admin(cx) {
            return;
        }
        let t = self.router_be(cx);
        let r = match act {
            RouterAct::Set(args) => serde_json::from_str::<Value>(&args).map_err(|e| e.to_string()).and_then(|a| t.set(&a)),
            RouterAct::Strategy(n) => t.strategy_set(&json!({"name": n})),
            RouterAct::Remove(n) => t.strategy_remove(&json!({"name": n})),
            RouterAct::Add(n, s) => t.strategy_add(&json!({"name": n, "source": s})),
        };
        match r {
            Ok(m) => self.ok(m),
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn router_budget_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        let num = |i: usize, what: &str| form.value(i).trim().parse::<u64>().map_err(|_| format!("{what}: a whole number"));
        let b = Budget { max_tokens: num(0, "Tokens")?, max_calls: u32::try_from(num(1, "Model calls")?).map_err(|_| "Model calls: too large")?, max_attempts: u32::try_from(num(2, "Tries per step")?).map_err(|_| "Tries per step: too large")?, max_escalations: u32::try_from(num(3, "Rounds with the planner")?).map_err(|_| "Rounds: too large")? };
        b.validate()?;
        let args = json!({"max_tokens": b.max_tokens, "max_calls": b.max_calls, "max_attempts": b.max_attempts, "max_escalations": b.max_escalations});
        let cfg = self.router_be(cx).config();
        if cfg.budget == b {
            return Err("Nothing changed.".into());
        }
        self.router_ask(args, format!("Limit a run to {} tokens and {} calls, {} tries and {} rounds with the planner per step?", b.max_tokens, b.max_calls, b.max_attempts, b.max_escalations));
        Ok(String::new())
    }

    /// The script file named in the form is read from the person's home folder and shown in full before anything is kept.
    pub(super) fn router_script_submit(&mut self, cx: &AppCx, form: &Form) -> Result<String, String> {
        let name = form.value(0).trim().to_string();
        if !cx.agent.router_js_valid_name(&name) {
            return Err("A name is lowercase letters, digits, - and _, starting with a letter, at most 32, and not `default`.".into());
        }
        let rel = form.value(1).trim().to_string();
        let home = cx.core.file_scope().home_root_for_writes(&cx.session.user, cx.data_dir());
        let p = std::path::Path::new(&rel);
        let full = if p.is_absolute() { p.to_path_buf() } else { home.join(p) };
        if full.components().any(|c| matches!(c, std::path::Component::ParentDir)) || !full.starts_with(&home) {
            return Err("The file must be inside your own folder.".into());
        }
        let meta = std::fs::metadata(&full).map_err(|_| "That file is not there.".to_string())?;
        if !meta.is_file() || meta.len() as usize > MAX_SCRIPT {
            return Err(format!("It must be a file of at most {MAX_SCRIPT} bytes."));
        }
        let src = std::fs::read_to_string(&full).map_err(|_| "That file is not text.".to_string())?;
        self.router.review = Some((name, src));
        self.router.scroll = 0;
        Ok(String::new())
    }

    pub(super) fn router_buttons(&self) -> Vec<Btn> {
        if self.router.review.is_some() {
            return vec![("Add this script", ACCENT, 'y'), ("Back", TEXT, 'n')];
        }
        let (acc, mu) = (ACCENT, TEXT);
        vec![("Router on/off", acc, 'o'), ("Role", mu, 'r'), ("Cost", mu, 'c'), ("Where it runs", mu, 'z'), ("Use model on/off", mu, 'e'), ("Reset model", mu, 'u'), ("Farthest place", mu, 'm'), ("Budget", acc, 'b'), ("Strategy", acc, 's'), ("Add script", mu, 'a'), ("Remove script", ERR, 'x'), ("Cancel run", ERR, 'k')]
    }

    pub(super) fn render_router(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mut y = a.y;
        if let Some((name, src)) = self.router.review.clone() {
            let head = format!("The script {name} ({} bytes). It runs in a sandbox with no file, network or program access, and decides which model sees each step of your work.", src.len());
            for w in g.wrap(13.0 * s, &head, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, crate::fx::AMBER, &w, Face::Sans);
                y += 18.0 * s;
            }
            y += 6.0 * s;
            let lh = 16.0 * s;
            for (i, l) in src.lines().enumerate().skip(self.router.scroll) {
                let _ = i;
                for w in g.wrap(12.0 * s, if l.is_empty() { " " } else { l }, Face::Mono, a.w) {
                    g.text(a.x, y, 12.0 * s, TEXT, &w, Face::Mono);
                    y += lh;
                }
            }
            return;
        }
        let be = self.router_be(cx);
        let c = be.config();
        let intro = "Experimental. A bigger model plans a goal into small steps, lighter models do them, and a stuck one asks the bigger one. Runs start in the chat (router_run), where the plan is shown on the card; this page sets the rules.";
        for w in g.wrap(13.0 * s, intro, Face::Sans, a.w) {
            g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
            y += 18.0 * s;
        }
        y += 8.0 * s;
        let label_w = (150.0 * s).min(a.w * 0.35);
        let rows = [
            ("Router", if c.enabled { "on".to_string() } else { "off".to_string() }, if c.enabled { OK } else { TEXT }),
            ("Strategy", c.strategy.clone(), TEXT),
            ("Farthest place", format!("{} or closer", c.max_zone.label()), TEXT),
            ("Budget of a run", format!("{} tokens, {} calls; {} tries and {} planner rounds per step", c.budget.max_tokens, c.budget.max_calls, c.budget.max_attempts, c.budget.max_escalations), TEXT),
            ("Planner", c.planner.clone().unwrap_or_else(|| "chosen by the strategy".into()), TEXT),
        ];
        for (label, value, col) in rows {
            g.text(a.x, y, 14.0 * s, MUTED, label, Face::Sans);
            let lines = g.wrap(14.0 * s, &value, Face::Bold, a.w - label_w);
            for (i, l) in lines.iter().enumerate() {
                g.text(a.x + label_w, y + i as f32 * 20.0 * s, 14.0 * s, col, l, Face::Bold);
            }
            y += (lines.len().max(1) as f32) * 20.0 * s + 4.0 * s;
        }
        y += 6.0 * s;
        // the run in progress (steps, tokens, calls), or the last one
        let (head, body) = match be.progress() {
            Some(p) => ("A run is going on. Cancel run stops it after the model call that is running.", p),
            None => ("The latest run", be.last_run().unwrap_or_else(|| "No run yet. Ask the assistant to start one.".into())),
        };
        g.text(a.x, y, 15.0 * s, TEXT, head, Face::Bold);
        y += 22.0 * s;
        for l in body.lines().take(8) {
            for w in g.wrap(12.0 * s, l, Face::Sans, a.w) {
                g.text(a.x, y, 12.0 * s, MUTED, &w, Face::Sans);
                y += 16.0 * s;
            }
        }
        y += 8.0 * s;
        g.text(a.x, y, 15.0 * s, TEXT, "Models", Face::Bold);
        y += 24.0 * s;
        let models = be.profiles();
        if models.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, "No models yet: add a provider in Settings, Models.", Face::Sans);
            y += 22.0 * s;
        }
        let sel = self.sel[self.tab].min(models.len().saturating_sub(1));
        let rh = 60.0 * s;
        for (i, m) in models.iter().enumerate().take(60) {
            let outside = m.zone > c.max_zone;
            let mut abilities = vec![];
            for (on, w) in [(m.caps.tools, "tools"), (m.caps.vision, "vision"), (m.caps.audio, "audio")] {
                if on {
                    abilities.push(w);
                }
            }
            let sub = format!("{} · cost {} · {} · {}", m.role.label(), m.cost.label(), m.zone.label(), if abilities.is_empty() { "text only".to_string() } else { abilities.join("+") });
            let chip: (&str, [u8; 4]) = if !m.enabled { ("off", DIM) } else if outside { ("too far", crate::fx::AMBER) } else { ("usable", OK) };
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &m.id, &sub, &[chip]);
            y += rh;
        }
        y += 6.0 * s;
        g.text(a.x, y, 15.0 * s, TEXT, "Strategies", Face::Bold);
        y += 24.0 * s;
        let mut lines = vec![format!("{DEFAULT_STRATEGY} (built in): the cheapest model that can do the step; asks the planner on a block or the second failure{}", if c.strategy == DEFAULT_STRATEGY { "  [in use]" } else { "" })];
        for (n, size) in be.strategies() {
            lines.push(format!("{n} (your script, {size} bytes){}", if c.strategy == n { "  [in use]" } else { "" }));
        }
        for l in lines {
            for w in g.wrap(13.0 * s, &l, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
                y += 18.0 * s;
            }
        }
    }
}

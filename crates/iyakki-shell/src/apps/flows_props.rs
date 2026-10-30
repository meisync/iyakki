//! The properties panel of the FLOWS designer: a form built from the selected step's kind (its schema), the settings every step has
//! (name, time limit, retry, where it runs, who may fix it, how it follows new versions) and the condition of a selected connection.
//! Text and numbers commit on Enter or when the focus moves; choices commit at once; JSON, lists and long text commit with Ctrl+Enter.

use iyakki_flow::kinds::{FieldKind, Registry};
use iyakki_flow::model::*;
use serde_json::{json, Value};

use super::dash_query::QueryBox;
use super::flows::*;
use super::flows_view::GfxFit;
use super::flows_canvas::unique_among;
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PKind {
    Id,
    Text,
    Number,
    Choice(Vec<String>),
    Json,
    Multi,
    List,
    // the settings every step has
    Name,
    Timeout,
    Retry,
    Runner,
    Heal,
    Track,
    OnError,
    // a connection
    When,
    // the flow's own settings (the key says which)
    Setting,
}

pub(super) struct PField {
    pub key: String,
    pub label: String,
    pub help: String,
    pub kind: PKind,
    pub required: bool,
    pub tf: TextField,
    pub qb: Option<QueryBox>,
    pub sel: usize,
    pub err: Option<String>,
    /// The text as it was when it was loaded, to see whether it was edited.
    pub orig: String,
}

#[derive(Default)]
pub struct Props {
    pub(super) key: String,
    pub(super) fields: Vec<PField>,
    pub(super) focus: usize,
    pub scroll: f32,
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn mk(key: &str, label: &str, help: &str, kind: PKind, required: bool, init: &str, sel: usize) -> PField {
    let multi = matches!(kind, PKind::Json | PKind::Multi | PKind::List | PKind::Retry);
    let mut tf = TextField::new("", false);
    tf.max = 4096;
    let mut qb = None;
    if multi {
        let mut q = QueryBox::new(area::PROP_QB);
        q.set_text(init);
        qb = Some(q);
    } else {
        tf.set(init);
    }
    PField { key: key.into(), label: label.into(), help: help.into(), kind, required, tf, qb, sel, err: None, orig: init.into() }
}

pub(super) const RUNNERS: [&str; 4] = ["inherit", "local", "sandbox (k3s)", "remote"];
pub(super) const HEALS: [&str; 5] = ["inherit", "off", "suggest", "auto-fix simple", "always ask"];
pub(super) const TRACKS: [&str; 3] = ["pinned", "latest-published", "compatible"];
pub(super) const ONERR: [&str; 2] = ["fail the run", "continue"];

impl Props {
    pub(super) fn for_node(n: &Node, reg: &Registry) -> Props {
        let mut fields = vec![mk("@id", "Step id", "letters, digits, - and _; references to it are renamed too", PKind::Id, true, &n.id, 0)];
        fields.push(mk("@name", "Name", "shown on the card", PKind::Name, false, &n.name, 0));
        if let Some(k) = reg.get(&n.kind) {
            for f in k.schema() {
                let v = n.config.get(f.key);
                let (kind, init, sel) = match &f.kind {
                    FieldKind::Text | FieldKind::Secret => (PKind::Text, text_of(v), 0),
                    FieldKind::Number => (PKind::Number, text_of(v), 0),
                    FieldKind::Bool => {
                        let on = v.and_then(Value::as_bool).or_else(|| f.default.as_ref().and_then(Value::as_bool)).unwrap_or(false);
                        (PKind::Choice(vec!["yes".into(), "no".into()]), String::new(), if on { 0 } else { 1 })
                    }
                    FieldKind::Choice(opts) => {
                        let cur = text_of(v.or(f.default.as_ref()));
                        let sel = opts.iter().position(|o| *o == cur).unwrap_or(0);
                        (PKind::Choice(opts.iter().map(|o| o.to_string()).collect()), String::new(), sel)
                    }
                    FieldKind::Json => (PKind::Json, v.map(|x| serde_json::to_string_pretty(x).unwrap_or_default()).unwrap_or_default(), 0),
                    FieldKind::Multiline => (PKind::Multi, text_of(v), 0),
                    FieldKind::List => (PKind::List, match v {
                        Some(Value::Array(a)) => a.iter().map(|x| text_of(Some(x))).collect::<Vec<_>>().join("\n"),
                        other => text_of(other),
                    }, 0),
                };
                fields.push(mk(f.key, f.label, f.help, kind, f.required, &init, sel));
            }
        }
        fields.push(mk("@timeout", "Time limit (ms)", "one attempt may take this long; empty uses the flow's", PKind::Timeout, false, &n.timeout_ms.map(|t| t.to_string()).unwrap_or_default(), 0));
        fields.push(mk("@retry", "Retry", "{\"attempts\":3,\"backoff\":{\"kind\":\"exponential\",\"base_ms\":200,\"factor\":2,\"max_ms\":5000,\"jitter\":0.2}} or {\"kind\":\"list\",\"steps\":[{\"timeout_ms\":700},{\"timeout_ms\":400,\"delay_ms\":200}]}; empty uses the flow's", PKind::Retry, false, &n.retry.as_ref().map(|r| serde_json::to_string_pretty(r).unwrap_or_default()).unwrap_or_default(), 0));
        fields.push(mk("@runner", "Runs", "sandbox runs the command in a Kubernetes Job", PKind::Runner, false, "", match n.runner { None => 0, Some(RunnerKind::Local) => 1, Some(RunnerKind::K3sJob) => 2, Some(RunnerKind::Remote) => 3 }));
        fields.push(mk("@heal", "Who fixes it", "what the assistant may do when this step fails for good", PKind::Heal, false, "", match n.heal.as_ref().map(|h| h.mode) { None => 0, Some(HealMode::Off) => 1, Some(HealMode::Suggest) => 2, Some(HealMode::AutoFixSimple) => 3, Some(HealMode::AlwaysAsk) => 4 }));
        fields.push(mk("@track", "Follows", "pinned never changes under a run; latest takes a newer published version at the step's start; compatible only if the ports and outputs stay", PKind::Track, false, "", match n.track { Track::Pinned => 0, Track::LatestPublished => 1, Track::Compatible => 2 }));
        fields.push(mk("@onerror", "If it fails", "continue goes on with an empty result instead of failing the run", PKind::OnError, false, "", usize::from(n.on_error == OnError::Continue)));
        Props { key: format!("node:{}", n.id), fields, focus: 1, scroll: 0.0 }
    }

    /// The flow's own settings, shown when no step is selected.
    pub(super) fn for_flow(f: &Flow, vision_line: &str) -> Props {
        let d = &f.defaults;
        let overlap = f.triggers.iter().find(|t| matches!(t.kind, TriggerKind::Cron { .. } | TriggerKind::Interval { .. })).map_or(Overlap::Skip, |t| t.overlap);
        let heal = match d.heal.mode { HealMode::Off => 0, HealMode::Suggest => 1, HealMode::AutoFixSimple => 2, HealMode::AlwaysAsk => 3 };
        let ch = |v: &[&str]| PKind::Choice(v.iter().map(|x| x.to_string()).collect());
        let fields = vec![
            mk("#conc", "Runs at the same time", "How many runs of this flow may be going at once. 1 is the safe default: a flow that sends, pays or deletes should not overlap itself. Raise it only for flows that are safe to repeat in parallel.", PKind::Setting, true, &d.max_concurrent_runs.to_string(), 0),
            mk("#overlap", "A timed run is due while one is going", "skip: that tick is dropped. queue: it waits for its turn. allow: it starts anyway (still limited by the number above). Applies to this flow's cron and interval triggers.", ch(&["skip", "queue", "allow"]), false, "", match overlap { Overlap::Skip => 0, Overlap::Queue => 1, Overlap::Allow => 2 }),
            mk("#par", "Steps at the same time (1 to 32)", "How many steps of one run may run side by side.", PKind::Setting, true, &d.max_parallel.to_string(), 0),
            mk("#timeout", "Default time limit (ms)", "A step without its own limit gets this one per attempt.", PKind::Setting, true, &d.timeout_ms.to_string(), 0),
            mk("#heal", "Who fixes failures", "The assistant's default for steps that do not say. The assistant itself needs your approval to widen this.", ch(&["off", "suggest", "auto-fix simple", "always ask"]), false, "", heal),
            mk("#ns", "Sandbox namespace", "Where sandbox Jobs run. Empty: your own flows namespace (flows-<you>). workspace:<name> uses that workspace's <name>-dev namespace; the cluster tools check it the same way they do for a teardown.", PKind::Setting, false, &d.sandbox.namespace, 0),
            mk("#image", "Sandbox image", "The container image sandbox steps use unless a step names its own.", PKind::Setting, false, &d.sandbox.image, 0),
            mk("#btrust", "Browser steps: ask at each run", "ask: each run asks once, listing the sites its browser steps will open. trusted: no question (the account's host list and the limits below still apply; scheduled runs need this). Only you can set it, never the assistant.", ch(&["ask", "trusted"]), false, "", usize::from(d.browser.trusted)),
            mk("#bprofile", "Browser profile", "isolated: a fresh private browser that is thrown away at the end of the run. my profile: your own browser profile with your sign-ins; always asked at every run, trusted or not.", ch(&["isolated", "my profile"]), false, "", usize::from(d.browser.profile == "person")),
            mk("#camallow", "Camera allowed", &format!("{}\n{vision_line}", iyakki_flow::kinds::camera::ALLOW_TEXT), ch(&["no", "yes, I allow it"]), false, "", usize::from(d.camera.allowed)),
            mk("#camunatt", "Camera also when nobody is there", "no: the camera step runs only when you start the flow (by hand or from the chat). yes: also for runs a schedule or an event starts, when nobody can see the camera-in-use sign. Only matters when Camera allowed is on.", ch(&["no", "yes"]), false, "", usize::from(d.camera.unattended)),
            mk("#assist", "Assist: an agent that fixes and upgrades this flow", "off: it does nothing by itself. copilot: after a failed run the configured agent drafts a fix and shows it in a strip on this flow; you approve and publish it. autopilot: it drafts and tests by itself, and publishes only if the switch below is on and every check passes. Takes effect when you publish. Only you can change this; the assistant cannot.", ch(&["off", "copilot", "autopilot"]), false, "", match d.assist.mode { AssistMode::Off => 0, AssistMode::Copilot => 1, AssistMode::Autopilot => 2 }),
            mk("#assistauto", "Assist: autopilot may publish a tested fix", "no: autopilot drafts and tests, and asks you before it publishes. yes: it publishes a small fix of the failing step by itself when the checks pass (the draft passes validation and a read-only trial, nothing protected is touched, no new host or effect), tells you, and rolls it back when its first runs fail.", ch(&["no, ask me", "yes, within the limits"]), false, "", usize::from(d.assist.auto_publish)),
            mk("#assistfix", "Assist: fixes after a failed run", "The agent may draft a fix of the failing step.", ch(&["no", "yes"]), false, "", usize::from(d.assist.allow_fix)),
            mk("#assistup", "Assist: upgrades", "The agent may suggest newer versions of shared steps (never published by itself).", ch(&["no", "yes"]), false, "", usize::from(d.assist.allow_upgrade)),
            mk("#assistpub", "Assist: automatic publishes per day (0 to 20)", "Most versions autopilot may publish for this flow in 24 hours. The account has a limit too.", PKind::Setting, true, &d.assist.max_publish_per_day.to_string(), 0),
            mk("#assistcents", "Assist: most it may spend per day, in cents (0 to 10000)", "What the model calls for this flow may cost in 24 hours; at the limit the agent stops asking the model.", PKind::Setting, true, &d.assist.max_cents_per_day.to_string(), 0),
            mk("#assistdiff", "Assist: most steps an automatic publish may touch (1 to 10)", "A bigger change is shown to you instead of being published.", PKind::Setting, true, &d.assist.max_diff_nodes.to_string(), 0),
            mk("#camblock", "Camera permission requests", "ask: when a step stops for want of the camera permission you get a popup and an INBOX entry with the choices. blocked: this flow stops asking (you chose Block); switch back to ask to be asked again.", ch(&["ask", "blocked"]), false, "", usize::from(d.camera.blocked)),
            mk("#bactions", "Browser actions per run (0: account limit)", "Most browser steps (and waits) one run may do; at most 1000.", PKind::Setting, false, &d.browser.max_actions.to_string(), 0),
            mk("#bminutes", "Browser minutes per run (0: account limit)", "Longest one run may keep its browser session open; at most 180.", PKind::Setting, false, &d.browser.max_minutes.to_string(), 0),
        ];
        Props { key: "flow".into(), fields, focus: 0, scroll: 0.0 }
    }

    pub(super) fn for_edge(i: usize, e: &Edge) -> Props {
        let fields = vec![mk("@when", "Take this connection only when", "an expression, for example nodes.a.output.status == 200; empty means always", PKind::When, false, &e.when, 0)];
        Props { key: format!("edge:{i}"), fields, focus: 0, scroll: 0.0 }
    }

    pub(super) fn choices(&self, i: usize) -> Vec<String> {
        match self.fields.get(i).map(|f| &f.kind) {
            Some(PKind::Choice(o)) => o.clone(),
            Some(PKind::Runner) => RUNNERS.iter().map(|s| s.to_string()).collect(),
            Some(PKind::Heal) => HEALS.iter().map(|s| s.to_string()).collect(),
            Some(PKind::Track) => TRACKS.iter().map(|s| s.to_string()).collect(),
            Some(PKind::OnError) => ONERR.iter().map(|s| s.to_string()).collect(),
            _ => vec![],
        }
    }

    pub(super) fn is_choice(&self, i: usize) -> bool {
        matches!(self.fields.get(i).map(|f| &f.kind), Some(PKind::Choice(_) | PKind::Runner | PKind::Heal | PKind::Track | PKind::OnError))
    }

    pub(super) fn is_multi(&self, i: usize) -> bool {
        self.fields.get(i).is_some_and(|f| f.qb.is_some())
    }

    fn current_text(&self, i: usize) -> String {
        match self.fields.get(i) {
            Some(f) => f.qb.as_ref().map(|q| q.text()).unwrap_or_else(|| f.tf.value.clone()),
            None => String::new(),
        }
    }

    pub(super) fn edited(&self, i: usize) -> bool {
        self.fields.get(i).is_some_and(|f| !self.is_choice(i) && self.current_text(i) != f.orig)
    }
}

pub(super) fn rename_node(f: &mut Flow, old: &str, new: &str) {
    for n in &mut f.nodes {
        if n.id == old {
            n.id = new.into();
        }
        rename_in_value(&mut n.config, old, new);
    }
    for e in &mut f.edges {
        if e.from == old {
            e.from = new.into();
        }
        if e.to == old {
            e.to = new.into();
        }
        e.when = e.when.replace(&format!("nodes.{old}."), &format!("nodes.{new}."));
    }
}

fn rename_in_value(v: &mut Value, old: &str, new: &str) {
    match v {
        Value::String(s) if s.contains(&format!("nodes.{old}.")) => *s = s.replace(&format!("nodes.{old}."), &format!("nodes.{new}.")),
        Value::Array(a) => a.iter_mut().for_each(|x| rename_in_value(x, old, new)),
        Value::Object(o) => o.values_mut().for_each(|x| rename_in_value(x, old, new)),
        _ => {}
    }
}

impl FlowsApp {
    /// Build the form for what is selected, when it is not built yet.
    pub(super) fn sync_props(&mut self) {
        let want = if let Some(i) = self.sel_edge {
            format!("edge:{i}")
        } else if self.sel.len() == 1 {
            format!("node:{}", self.sel.iter().next().cloned().unwrap_or_default())
        } else if self.cur.is_some() && self.tab == Tab::Design {
            "flow".to_string()
        } else {
            String::new()
        };
        if want == self.props.key {
            return;
        }
        self.props = match (&self.sel_edge, self.sel.len()) {
            (Some(i), _) => self.flow.edges.get(*i).map(|e| Props::for_edge(*i, e)).unwrap_or_default(),
            (None, 1) => self.sel.iter().next().and_then(|id| self.flow.node(id)).map(|n| Props::for_node(n, &self.reg)).unwrap_or_default(),
            _ if want == "flow" => {
                // where a camera picture would go is said from the provider's real address, never guessed
                let line = match self.env.svc.vision_model(None) {
                    Ok(p) => p.line(),
                    Err(e) => format!("No vision model is available yet: {}", e.message()),
                };
                Props::for_flow(&self.flow, &line)
            }
            _ => Props::default(),
        };
    }

    pub(super) fn props_focus(&mut self, i: usize, x: f32) {
        if i != self.props.focus {
            self.commit_props();
        }
        if i < self.props.fields.len() {
            self.props.focus = i;
            if !self.props.is_multi(i) && !self.props.is_choice(i) {
                self.props.fields[i].tf.click(x);
            }
        }
        self.focus = Focus::Props;
    }

    pub(super) fn props_area(&mut self, i: usize, x: f32, y: f32) {
        if i != self.props.focus {
            self.commit_props();
        }
        if i < self.props.fields.len() {
            self.props.focus = i;
            if let Some(q) = self.props.fields[i].qb.as_mut() {
                q.click(x, y, crate::ui::shift_held());
            }
        }
        self.focus = Focus::Props;
    }

    pub(super) fn props_event(&mut self, ev: &Event) {
        let i = self.props.focus;
        if i >= self.props.fields.len() || self.props.is_choice(i) {
            return;
        }
        let f = &mut self.props.fields[i];
        match (&mut f.qb, ev) {
            (Some(q), _) => {
                q.handle(ev);
            }
            (None, Event::Key(k)) => {
                f.tf.handle(k);
            }
            (None, Event::Paste(t)) => {
                f.tf.paste(t);
            }
            _ => {}
        }
        f.err = None;
    }

    /// Keys while the form has the keyboard.
    pub(super) fn props_key(&mut self, k: &Key) {
        let n = self.props.fields.len();
        let i = self.props.focus;
        if n == 0 {
            self.focus = Focus::Canvas;
            return;
        }
        let multi = self.props.is_multi(i);
        match k {
            Key::Esc => {
                // put the field back as it was, and give the keyboard back to the canvas
                if let Some(f) = self.props.fields.get_mut(i) {
                    let orig = f.orig.clone();
                    match &mut f.qb {
                        Some(q) => q.set_text(&orig),
                        None => f.tf.set(&orig),
                    }
                    f.err = None;
                }
                self.focus = Focus::Canvas;
            }
            Key::Ctrl('s') | Key::Ctrl('e') => self.commit_props(),
            Key::Enter if !multi => {
                self.commit_props();
                // a value that was refused stays under the caret
                if self.props.fields.get(i).is_none_or(|f| f.err.is_none()) {
                    self.props.focus = (i + 1).min(n - 1);
                }
            }
            Key::Tab | Key::Down if !multi || matches!(k, Key::Tab) => {
                self.commit_props();
                self.props.focus = (i + 1) % n;
            }
            Key::BackTab | Key::Up if !multi || matches!(k, Key::BackTab) => {
                self.commit_props();
                self.props.focus = (i + n - 1) % n;
            }
            Key::Left | Key::Right if self.props.is_choice(i) => {
                let len = self.props.choices(i).len().max(1);
                let cur = self.props.fields[i].sel;
                let next = if *k == Key::Right { (cur + 1) % len } else { (cur + len - 1) % len };
                self.choose(i, next);
            }
            other => self.props_event(&Event::Key(other.clone())),
        }
    }

    /// A choice picked: commit at once.
    pub(super) fn choose(&mut self, i: usize, opt: usize) {
        if i >= self.props.fields.len() || opt >= self.props.choices(i).len() {
            return;
        }
        self.props.fields[i].sel = opt;
        self.props.focus = i;
        self.focus = Focus::Props;
        self.commit_field(i);
    }

    pub(super) fn commit_props(&mut self) {
        let i = self.props.focus;
        if self.props.edited(i) {
            self.commit_field(i);
        }
    }

    /// Put a field's value into the flow (an undoable edit), or say what is wrong with it.
    pub(super) fn commit_field(&mut self, i: usize) {
        let Some(f) = self.props.fields.get(i) else { return };
        let text = self.props.current_text(i);
        let (key, kind, sel, required) = (f.key.clone(), f.kind.clone(), f.sel, f.required);
        let target = self.props.key.clone();
        let fail = |this: &mut FlowsApp, e: String| {
            if let Some(f) = this.props.fields.get_mut(i) {
                f.err = Some(e);
            }
            this.redraw = true;
        };
        if let Some(i_edge) = target.strip_prefix("edge:").and_then(|s| s.parse::<usize>().ok()) {
            if kind == PKind::When {
                let t = text.trim().to_string();
                if !t.is_empty() {
                    if let Err(e) = iyakki_flow::expr::parse(&t) {
                        return fail(self, e.to_string());
                    }
                }
                self.edit("changed a condition", |f| {
                    if let Some(e) = f.edges.get_mut(i_edge) {
                        e.when = t.clone();
                    }
                });
                if let Some(f) = self.props.fields.get_mut(i) {
                    f.orig = text;
                }
            }
            return;
        }
        if target == "flow" {
            let t = text.trim().to_string();
            let num = |lo: u64, hi: u64, what: &str| t.parse::<u64>().ok().filter(|v| (lo..=hi).contains(v)).ok_or(format!("{what} is a number from {lo} to {hi}"));
            let res: Result<Box<dyn FnOnce(&mut Flow)>, String> = match key.as_str() {
                "#conc" => num(1, 64, "runs at the same time").map(|v| Box::new(move |f: &mut Flow| f.defaults.max_concurrent_runs = v as usize) as Box<dyn FnOnce(&mut Flow)>),
                "#par" => num(1, 32, "steps at the same time").map(|v| Box::new(move |f: &mut Flow| f.defaults.max_parallel = v as usize) as Box<dyn FnOnce(&mut Flow)>),
                "#timeout" => num(100, 86_400_000, "the time limit").map(|v| Box::new(move |f: &mut Flow| f.defaults.timeout_ms = v) as Box<dyn FnOnce(&mut Flow)>),
                "#overlap" => Ok(Box::new(move |f: &mut Flow| {
                    for tr in f.triggers.iter_mut().filter(|t| matches!(t.kind, TriggerKind::Cron { .. } | TriggerKind::Interval { .. })) {
                        tr.overlap = [Overlap::Skip, Overlap::Queue, Overlap::Allow][sel.min(2)];
                    }
                })),
                "#heal" => Ok(Box::new(move |f: &mut Flow| f.defaults.heal.mode = [HealMode::Off, HealMode::Suggest, HealMode::AutoFixSimple, HealMode::AlwaysAsk][sel.min(3)])),
                "#ns" => {
                    let ok = t.is_empty() || t.strip_prefix("workspace:").is_some_and(|w| !w.trim().is_empty() && w.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
                    if ok { Ok(Box::new(move |f: &mut Flow| f.defaults.sandbox.namespace = t) as Box<dyn FnOnce(&mut Flow)>) } else { Err("empty, or workspace:<name>".to_string()) }
                }
                "#image" => Ok(Box::new(move |f: &mut Flow| f.defaults.sandbox.image = t)),
                "#btrust" => Ok(Box::new(move |f: &mut Flow| f.defaults.browser.trusted = sel == 1)),
                "#bprofile" => Ok(Box::new(move |f: &mut Flow| f.defaults.browser.profile = if sel == 1 { "person".to_string() } else { String::new() })),
                "#camallow" => Ok(Box::new(move |f: &mut Flow| f.defaults.camera.allowed = sel == 1)),
                "#camblock" => Ok(Box::new(move |f: &mut Flow| f.defaults.camera.blocked = sel == 1)),
                "#camunatt" => Ok(Box::new(move |f: &mut Flow| f.defaults.camera.unattended = sel == 1)),
                "#assist" => Ok(Box::new(move |f: &mut Flow| f.defaults.assist.mode = [AssistMode::Off, AssistMode::Copilot, AssistMode::Autopilot][sel.min(2)])),
                "#assistauto" => Ok(Box::new(move |f: &mut Flow| f.defaults.assist.auto_publish = sel == 1)),
                "#assistfix" => Ok(Box::new(move |f: &mut Flow| f.defaults.assist.allow_fix = sel == 1)),
                "#assistup" => Ok(Box::new(move |f: &mut Flow| f.defaults.assist.allow_upgrade = sel == 1)),
                "#assistpub" => num(0, 20, "automatic publishes").map(|v| Box::new(move |f: &mut Flow| f.defaults.assist.max_publish_per_day = v as u32) as Box<dyn FnOnce(&mut Flow)>),
                "#assistcents" => num(0, 10_000, "cents").map(|v| Box::new(move |f: &mut Flow| f.defaults.assist.max_cents_per_day = v) as Box<dyn FnOnce(&mut Flow)>),
                "#assistdiff" => num(1, 10, "steps").map(|v| Box::new(move |f: &mut Flow| f.defaults.assist.max_diff_nodes = v as u32) as Box<dyn FnOnce(&mut Flow)>),
                "#bactions" => num(0, 1000, "browser actions").map(|v| Box::new(move |f: &mut Flow| f.defaults.browser.max_actions = v as u32) as Box<dyn FnOnce(&mut Flow)>),
                "#bminutes" => num(0, 180, "browser minutes").map(|v| Box::new(move |f: &mut Flow| f.defaults.browser.max_minutes = v as u32) as Box<dyn FnOnce(&mut Flow)>),
                _ => return,
            };
            match res {
                Err(e) => fail(self, e),
                Ok(change) => {
                    self.edit("changed the flow's settings", change);
                    if let Some(f) = self.props.fields.get_mut(i) {
                        f.orig = text;
                        f.err = None;
                    }
                }
            }
            return;
        }
        let Some(nid) = target.strip_prefix("node:").map(str::to_string) else { return };
        let mut err: Option<String> = None;
        let mut new_id: Option<String> = None;
        let choice_name = self.props.choices(i).get(sel).cloned().unwrap_or_default();
        let parsed: Result<Box<dyn FnOnce(&mut Node)>, String> = match kind {
            PKind::Id => {
                let t = text.trim().to_string();
                if !valid_id(&t) {
                    Err("a step id is letters, digits, - and _".into())
                } else if t != nid && self.flow.node(&t).is_some() {
                    Err(format!("there is already a step `{t}`"))
                } else {
                    new_id = Some(t);
                    Ok(Box::new(|_| {}))
                }
            }
            PKind::Name => {
                let t = text.trim().to_string();
                Ok(Box::new(move |n| n.name = t))
            }
            PKind::Text | PKind::Multi => {
                let t = if kind == PKind::Multi { text.clone() } else { text.trim().to_string() };
                if t.is_empty() && required {
                    Err("this one is needed".into())
                } else {
                    Ok(Box::new(move |n| set_cfg(n, &key, if t.is_empty() { None } else { Some(Value::String(t)) })))
                }
            }
            PKind::Number => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    Ok(Box::new(move |n| set_cfg(n, &key, None)))
                } else {
                    match t.parse::<u64>() {
                        Ok(v) => Ok(Box::new(move |n| set_cfg(n, &key, Some(json!(v))))),
                        Err(_) => Err("a whole number".into()),
                    }
                }
            }
            PKind::Choice(opts) => {
                let boolean = opts == vec!["yes".to_string(), "no".to_string()];
                let v = if boolean { json!(sel == 0) } else { json!(choice_name) };
                Ok(Box::new(move |n| set_cfg(n, &key, Some(v))))
            }
            PKind::Json => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    if required { Err("this one is needed".into()) } else { Ok(Box::new(move |n| set_cfg(n, &key, None))) }
                } else {
                    match serde_json::from_str::<Value>(&t) {
                        Ok(v) => Ok(Box::new(move |n| set_cfg(n, &key, Some(v)))),
                        Err(e) => Err(format!("not valid JSON: {e}")),
                    }
                }
            }
            PKind::List => {
                let lines: Vec<Value> = text.lines().map(str::trim_end).filter(|l| !l.is_empty()).map(|l| json!(l)).collect();
                if lines.is_empty() && required {
                    Err("this one is needed".into())
                } else {
                    Ok(Box::new(move |n| set_cfg(n, &key, if lines.is_empty() { None } else { Some(Value::Array(lines)) })))
                }
            }
            PKind::Timeout => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    Ok(Box::new(|n| n.timeout_ms = None))
                } else {
                    match t.parse::<u64>() {
                        Ok(v) if v > 0 => Ok(Box::new(move |n| n.timeout_ms = Some(v))),
                        _ => Err("a number of milliseconds, more than 0".into()),
                    }
                }
            }
            PKind::Retry => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    Ok(Box::new(|n| n.retry = None))
                } else {
                    match serde_json::from_str::<iyakki_flow::RetryPolicy>(&t) {
                        Ok(p) if p.validate().is_empty() => Ok(Box::new(move |n| n.retry = Some(p))),
                        Ok(p) => Err(p.validate().join("; ")),
                        Err(e) => Err(format!("not a retry policy: {e}")),
                    }
                }
            }
            PKind::Runner => Ok(Box::new(move |n| {
                n.runner = match sel {
                    1 => Some(RunnerKind::Local),
                    2 => Some(RunnerKind::K3sJob),
                    3 => Some(RunnerKind::Remote),
                    _ => None,
                }
            })),
            PKind::Heal => Ok(Box::new(move |n| {
                n.heal = match sel {
                    0 => None,
                    s => {
                        let mode = [HealMode::Off, HealMode::Off, HealMode::Suggest, HealMode::AutoFixSimple, HealMode::AlwaysAsk][s.min(4)];
                        let mut p = n.heal.clone().unwrap_or_default();
                        p.mode = mode;
                        Some(p)
                    }
                }
            })),
            PKind::Track => Ok(Box::new(move |n| n.track = [Track::Pinned, Track::LatestPublished, Track::Compatible][sel.min(2)])),
            PKind::OnError => Ok(Box::new(move |n| n.on_error = if sel == 1 { OnError::Continue } else { OnError::Fail })),
            PKind::When | PKind::Setting => Ok(Box::new(|_| {})),
        };
        match parsed {
            Err(e) => {
                err = Some(e);
            }
            Ok(change) => {
                let nid2 = nid.clone();
                self.edit(&format!("changed `{nid}`"), |f| {
                    if let Some(n) = f.node_mut(&nid2) {
                        change(n);
                    }
                    if let Some(newid) = &new_id {
                        if *newid != nid2 {
                            rename_node(f, &nid2, newid);
                        }
                    }
                });
                if let Some(newid) = new_id {
                    if newid != nid {
                        self.sel = [newid.clone()].into_iter().collect();
                        self.props.key = format!("node:{newid}");
                        for f in self.props.fields.iter_mut() {
                            f.err = None;
                        }
                    }
                }
                if let Some(f) = self.props.fields.get_mut(i) {
                    f.orig = text;
                    f.err = None;
                }
                // a changed step may be a different shape (a switch's cases are its ports): rebuild the form next time
                let keep = self.props.focus;
                if let Some(n) = self.flow.node(self.sel.iter().next().map(String::as_str).unwrap_or("")) {
                    if self.props.fields.len() != Props::for_node(n, &self.reg).fields.len() {
                        self.props = Props::for_node(n, &self.reg);
                        self.props.focus = keep.min(self.props.fields.len().saturating_sub(1));
                    }
                }
            }
        }
        if let Some(e) = err {
            fail(self, e);
        }
        let _ = unique_among;
    }

    /// Draw the panel into `r`; registers the clicks of its fields.
    pub(super) fn draw_props(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, ACCENT);
        let pad = 12.0 * s;
        let inner = R::new(r.x + pad, r.y + 8.0 * s, r.w - 2.0 * pad, r.h - 16.0 * s);
        let focused = self.focus == Focus::Props;
        let blink = super::blink(tick);
        // header
        let mut y = inner.y;
        let head = if self.props.key == "flow" {
            Some(("Flow settings".to_string(), "this flow".to_string(), "How runs of this flow are limited, where its sandbox steps run and who fixes failures.".to_string()))
        } else { None };
        let head = if head.is_some() { head } else { match self.props.key.split_once(':') {
            Some(("node", id)) => self.flow.node(id).map(|n| {
                let k = self.reg.get(&n.kind);
                (n.title().to_string(), k.map(|k| k.label().to_string()).unwrap_or_default(), k.map(|k| k.blurb().to_string()).unwrap_or_default())
            }),
            Some(("edge", i)) => i.parse::<usize>().ok().and_then(|i| self.flow.edges.get(i)).map(|e| (format!("{} → {}", e.from, e.to), format!("connection · output {}", e.port), "A connection carries the result of one step to the next.".into())),
            _ => None,
        } };
        let Some((title, kind, blurb)) = head else {
            g.text(inner.x, inner.y + 6.0 * s, 13.0 * s, DIM, "Select a step or a connection.", Face::Sans);
            g.text(inner.x, inner.y + 26.0 * s, 11.5 * s, DIM, "Its settings appear here.", Face::Sans);
            return;
        };
        let t = g.fit(15.0 * s, &title, Face::Bold, inner.w);
        g.text(inner.x, y, 15.0 * s, TEXT, &t, Face::Bold);
        y += 20.0 * s;
        g.text_fit(inner.x, y, 11.5 * s, CYAN, &kind, Face::Bold, inner.w);
        y += 16.0 * s;
        for l in g.wrap(11.0 * s, &blurb, Face::Sans, inner.w).into_iter().take(2) {
            g.text(inner.x, y, 11.0 * s, DIM, &l, Face::Sans);
            y += 14.0 * s;
        }
        // the flow's settings pane lists every problem and warning in full (the badge on a step only counts them)
        if self.props.key == "flow" {
            for i in self.issues.iter().take(8) {
                let c = if i.level == iyakki_flow::kinds::Level::Error { ERR } else { AMBER };
                let text = match &i.node {
                    Some(n) => format!("{n}: {}", i.msg),
                    None => i.msg.clone(),
                };
                for l in g.wrap(11.0 * s, &text, Face::Sans, inner.w - 10.0 * s).into_iter().take(4) {
                    g.circle(inner.x + 3.0 * s, y + 7.0 * s, 2.5 * s, c);
                    g.text(inner.x + 10.0 * s, y, 11.0 * s, c, &l, Face::Sans);
                    y += 14.0 * s;
                }
            }
        }
        // problems of this step, and the agent's change
        if let Some(id) = self.props.key.strip_prefix("node:") {
            let id = id.to_string();
            for i in self.issues.iter().filter(|i| i.node.as_deref() == Some(id.as_str())).take(3) {
                let c = if i.level == iyakki_flow::kinds::Level::Error { ERR } else { AMBER };
                for l in g.wrap(11.0 * s, &i.msg, Face::Sans, inner.w - 10.0 * s).into_iter().take(5) {
                    g.circle(inner.x + 3.0 * s, y + 7.0 * s, 2.5 * s, c);
                    g.text(inner.x + 10.0 * s, y, 11.0 * s, c, &l, Face::Sans);
                    y += 14.0 * s;
                }
            }
            if let Some(c) = self.flow.node(&id).and_then(|n| n.modified_by.clone()).filter(Change::by_agent) {
                let line = format!("The assistant changed this: {}", c.summary);
                for l in g.wrap(11.0 * s, &line, Face::Sans, inner.w).into_iter().take(2) {
                    g.text(inner.x, y, 11.0 * s, MAGENTA, &l, Face::Sans);
                    y += 14.0 * s;
                }
            }
            if let Some(u) = self.updates.iter().find(|u| u.node == id) {
                let line = format!("{} scheduled and {} running runs use an older version of this step.", u.scheduled.len(), u.running.len());
                for l in g.wrap(11.0 * s, &line, Face::Sans, inner.w).into_iter().take(2) {
                    g.text(inner.x, y, 11.0 * s, AMBER, &l, Face::Sans);
                    y += 14.0 * s;
                }
            }
        }
        // buttons
        let mut bx = inner.x;
        if self.props.key.starts_with("node:") {
            bx += g.button(bx, y, 11.5 * s, if self.testing.is_some() { "Testing..." } else { "Test this step" }, CYAN, Event::Tab(id::TEST)) + 6.0 * s;
        }
        if self.props.key != "flow" {
            g.button(bx, y, 11.5 * s, "Delete", ERR, Event::Tab(id::DELETE));
        }
        y += 32.0 * s;
        // the fields, scrolled
        let body = R::new(inner.x, y, inner.w, (inner.y + inner.h - y).max(0.0));
        let mut cy = body.y - self.props.scroll;
        let mut total = 0.0;
        let focus = self.props.focus;
        let n = self.props.fields.len();
        for i in 0..n {
            let (label, help, err, multi, is_choice, req, kind) = {
                let f = &self.props.fields[i];
                (f.label.clone(), f.help.clone(), f.err.clone(), f.qb.is_some(), self.props.is_choice(i), f.required, f.kind.clone())
            };
            let h_label = 15.0 * s;
            let h_ctl = if multi { 92.0 * s } else if is_choice { choice_height(g, &self.props.choices(i), body.w, s) } else { 32.0 * s };
            let on = i == focus && focused;
            let h_help = if i == focus && !help.is_empty() { g.wrap(10.5 * s, &help, Face::Sans, body.w).len().min(4) as f32 * 13.0 * s } else { 0.0 };
            let h_err = if err.is_some() { 14.0 * s } else { 0.0 };
            let block = h_label + h_ctl + h_help + h_err + 8.0 * s;
            if cy + block >= body.y && cy <= body.y + body.h {
                g.clipped(body, |g| {
                    g.text(body.x, cy, 11.5 * s, if on { CYAN } else { MUTED }, &format!("{}{}", label, if req { " *" } else { "" }), Face::Bold);
                    let ctl = R::new(body.x, cy + h_label, body.w, h_ctl);
                    if is_choice {
                        let opts = self.props.choices(i);
                        let sel = self.props.fields[i].sel;
                        let (mut x, mut yy) = (ctl.x, ctl.y);
                        for (o, name) in opts.iter().enumerate() {
                            let size = 11.5 * s;
                            let w = g.text_width(size, name, Face::Bold) + 16.0 * s;
                            if x + w > ctl.x + ctl.w && x > ctl.x {
                                x = ctl.x;
                                yy += 28.0 * s;
                            }
                            let rr = R::new(x, yy, w, 24.0 * s);
                            let active = o == sel;
                            g.rrect(rr.x, rr.y, rr.w, rr.h, 12.0 * s, if active { with_alpha(ACCENT, 70) } else { fx::WELL });
                            g.rrect_outline(rr.x, rr.y, rr.w, rr.h, 12.0 * s, 1.2, if active { ACCENT } else { with_alpha(MUTED, 90) });
                            g.text(rr.x + 8.0 * s, rr.y + 4.0 * s, size, if active { TEXT } else { MUTED }, name, Face::Bold);
                            g.hit(rr, Event::Tab(id::P_CHOICE + i * id::P_CHOICE_STRIDE + o), name);
                            x += w + 5.0 * s;
                        }
                    } else if multi {
                        let ph = if kind == PKind::Retry { "empty: use the flow's retry" } else { "" };
                        if let Some(q) = self.props.fields[i].qb.as_mut() {
                            q.area = area::PROP_QB + i;
                            super::with_caret(on, || q.draw(g, ctl, s, on, tick, ph));
                        }
                    } else {
                        let f = &self.props.fields[i];
                        f.tf.render(g, ctl, on, s, blink, Face::Sans, fid::PROP + i);
                    }
                    let mut hy = ctl.y + h_ctl + 2.0 * s;
                    if h_help > 0.0 {
                        for l in g.wrap(10.5 * s, &help, Face::Sans, body.w).into_iter().take(4) {
                            g.text(body.x, hy, 10.5 * s, DIM, &l, Face::Sans);
                            hy += 13.0 * s;
                        }
                    }
                    if let Some(e) = &err {
                        g.text(body.x, hy, 11.0 * s, ERR, &g_fit_static(e, 90), Face::Sans);
                    }
                });
            }
            cy += block;
            total += block;
        }
        if total > body.h {
            self.props.scroll = self.props.scroll.min(total - body.h);
        } else {
            self.props.scroll = 0.0;
        }
        let _ = OK;
    }
}

/// How tall the row(s) of option buttons are when they wrap in `width` (the same flow as the drawing code).
fn choice_height(g: &mut Gfx, opts: &[String], width: f32, s: f32) -> f32 {
    let (mut x, mut rows) = (0.0f32, 1usize);
    for name in opts {
        let w = g.text_width(11.5 * s, name, Face::Bold) + 16.0 * s;
        if x + w > width && x > 0.0 {
            x = 0.0;
            rows += 1;
        }
        x += w + 5.0 * s;
    }
    24.0 * s + (rows - 1) as f32 * 28.0 * s + 6.0 * s
}

fn g_fit_static(t: &str, n: usize) -> String {
    let s: String = t.chars().take(n).collect();
    if t.chars().count() > n { format!("{s}...") } else { s }
}

/// Set (or remove) a key of a step's configuration.
pub(super) fn set_cfg(n: &mut Node, key: &str, v: Option<Value>) {
    if !n.config.is_object() {
        n.config = json!({});
    }
    if let Some(o) = n.config.as_object_mut() {
        match v {
            Some(v) => {
                o.insert(key.to_string(), v);
            }
            None => {
                o.remove(key);
            }
        }
    }
}

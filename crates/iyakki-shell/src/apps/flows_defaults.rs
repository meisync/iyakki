//! FLOWS, the Assist and messages page (F-FLOW-32, F-FLOW-33, F-FLOW-34, F-MSG-04): one dialog on the flows list (key G) with four
//! pages. ASSIST: the account's defaults (the kill switch, the daily limit of automatic publishes, the Matrix notice and its room).
//! AGENT: the named agent profile (models, local first, fallback, each model's time limit). MESSAGES: the outside-message policy
//! (quiet hours, escalation and its room), the per-flow switch of every flow, and the messages waiting for an answer. ACTIVITY: what
//! the agent did for one flow (the same lines and counters as `iyakki dev flows assist` and the chat tool `flow_assist_activity`)
//! and a whole-flow trial that runs only read-only steps and refuses every step with effects.
//!
//! Only the person changes these here; each change is audited like the chat tools do (`flow.assist.settings`,
//! `flow.outside_messages`, `flow.outside_messages.policy`). Nothing on this page shows an input value or a secret.

use iyakki_agent::flows::channel_policy::Policy;
use iyakki_flow::assist_activity;
use iyakki_flow::assist_profile::{ProfileModel, MAX_MODELS, MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS};
use iyakki_flow::assist_run::WholeTrial;

use super::flows::*;
use super::flows_view::GfxFit;
use crate::safeclamp::SafeClamp;
use crate::fx::{self, AMBER, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DefTab {
    Assist,
    Agent,
    Messages,
    Activity,
}

impl DefTab {
    const ALL: [DefTab; 4] = [DefTab::Assist, DefTab::Agent, DefTab::Messages, DefTab::Activity];
    fn label(self) -> &'static str {
        match self {
            DefTab::Assist => "Defaults (1)",
            DefTab::Agent => "Agent (2)",
            DefTab::Messages => "Messages (3)",
            DefTab::Activity => "Activity (4)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RowKey {
    Enabled,
    Cap,
    Matrix,
    MatrixRoom,
    LocalFirst,
    Fallback,
    ModelLocal(usize),
    ModelTime(usize),
    ModelDelete(usize),
    ModelAdd,
    QuietOn,
    QuietStart,
    QuietEnd,
    EscAfter,
    EscRoom,
    FlowSwitch(String),
    Ack(String),
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowKind {
    Toggle,
    Step,
    Text,
    Action,
    Info,
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub label: String,
    pub value: String,
    pub hint: String,
    pub kind: RowKind,
    pub key: RowKey,
}

pub(super) struct DefView {
    pub tab: DefTab,
    pub sel: usize,
    pub rows: Vec<Row>,
    pub edit: Option<(RowKey, TextField)>,
    pub note: String,
    pub note_ok: bool,
    /// Index into the flows list shown on the Activity page.
    pub flow_i: usize,
    pub lines: Vec<(String, [u8; 4])>,
    pub scroll: usize,
    /// A whole-flow trial is running (its answer arrives as `Msg::Trial`).
    pub trying: bool,
}

fn hm(m: u32) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn row(label: &str, value: impl Into<String>, hint: &str, kind: RowKind, key: RowKey) -> Row {
    Row { label: label.into(), value: value.into(), hint: hint.into(), kind, key }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

impl FlowsApp {
    pub(super) fn open_defaults(&mut self, tab: DefTab) {
        let flow_i = self.cur.as_ref().and_then(|c| self.flows.iter().position(|f| &f.id == c)).unwrap_or(self.list_sel.min(self.flows.len().saturating_sub(1)));
        let mut v = DefView { tab, sel: 0, rows: vec![], edit: None, note: String::new(), note_ok: true, flow_i, lines: vec![], scroll: 0, trying: false };
        self.def_refresh(&mut v);
        self.modal = Some(Modal::Defaults(Box::new(v)));
    }

    /// Read the settings again into the rows (and the Activity lines).
    pub(super) fn def_refresh(&self, v: &mut DefView) {
        v.rows = self.def_rows(v.tab);
        v.sel = v.sel.min(v.rows.len().saturating_sub(1));
        if v.tab == DefTab::Activity {
            let keep = v.lines.iter().take_while(|l| l.0.starts_with('~')).count();
            let trial: Vec<(String, [u8; 4])> = v.lines.iter().take(keep).cloned().collect();
            v.lines = trial;
            v.lines.extend(self.def_activity(v.flow_i));
        }
    }

    fn def_rows(&self, tab: DefTab) -> Vec<Row> {
        let book = self.env.assist_store();
        let g = book.global();
        let mut r = vec![];
        match tab {
            DefTab::Assist => {
                r.push(row("Assist for every flow", if g.enabled { "ON" } else { "STOPPED" }, "STOPPED: no suggestions and no autopilot anywhere, until you switch it on (Enter).", RowKind::Toggle, RowKey::Enabled));
                r.push(row("Automatic publishes per day, all flows (0 to 20)", g.max_publish_per_day.to_string(), "Left and Right change it. Each flow has its own limit too, in its settings.", RowKind::Step, RowKey::Cap));
                r.push(row("Tell me by Matrix about autopilot", yes_no(g.matrix), "Every autopilot notice also goes to your Matrix room; the INBOX note is always made.", RowKind::Toggle, RowKey::Matrix));
                r.push(row("Matrix room for those notices", if g.matrix_room.is_empty() { "the main room".to_string() } else { g.matrix_room.clone() }, "Enter edits the saved room name; empty means the main room.", RowKind::Text, RowKey::MatrixRoom));
                r.push(row("Each flow's own setting", "in the flow's settings", "Off, copilot or autopilot is set per flow, only by you; the assistant cannot change it.", RowKind::Info, RowKey::Info));
            }
            DefTab::Agent => {
                r.push(row("Agent profile", g.profile.name.clone(), "The models that fix flows, in the order they are asked.", RowKind::Info, RowKey::Info));
                r.push(row("Ask local models first", yes_no(g.profile.local_first), "Models on this computer or network cost nothing and send nothing away.", RowKind::Toggle, RowKey::LocalFirst));
                r.push(row("Try the next model when one fails", yes_no(g.profile.fallback), "Off: one try, with the first model only.", RowKind::Toggle, RowKey::Fallback));
                for (i, m) in g.profile.models.iter().enumerate() {
                    r.push(row(&format!("Model {}", i + 1), format!("{} ({})", m.name, if m.local { "local" } else { "cloud" }), "Enter: local or cloud.", RowKind::Toggle, RowKey::ModelLocal(i)));
                    r.push(row(&format!("  time limit of model {}", i + 1), format!("{} s", m.timeout_secs), "Left and Right change it by 30 seconds.", RowKind::Step, RowKey::ModelTime(i)));
                    r.push(row(&format!("  remove model {}", i + 1), "remove", "Enter removes it from the profile.", RowKind::Action, RowKey::ModelDelete(i)));
                }
                if g.profile.models.is_empty() {
                    r.push(row("Models", "none: the assistant's own model, one try", "Add one to choose; names are provider/model.", RowKind::Info, RowKey::Info));
                }
                r.push(row("Add a model", "name it", "Enter, then type provider/model and press Enter.", RowKind::Text, RowKey::ModelAdd));
            }
            DefTab::Messages => {
                let p = self.env.host.msg_policy.policy();
                r.push(row("Quiet hours", match p.quiet { Some((s, e)) => format!("{} to {}", hm(s), hm(e)), None => "off".into() }, "A message in these hours waits and is sent when they end; an error always goes through; the INBOX note is made at once.", RowKind::Toggle, RowKey::QuietOn));
                if let Some((s, e)) = p.quiet {
                    r.push(row("  quiet hours start", hm(s), "Left and Right: 30 minutes.", RowKind::Step, RowKey::QuietStart));
                    r.push(row("  quiet hours end", hm(e), "Left and Right: 30 minutes.", RowKind::Step, RowKey::QuietEnd));
                }
                r.push(row("Ask again when nobody answers", match p.escalate_after_secs { Some(n) => format!("after {} min", n / 60), None => "off".into() }, "A message that needs you and is not acknowledged in time is sent once more, to the room below. Left and Right: off, 15, 30, 60, 120, 480 minutes.", RowKind::Step, RowKey::EscAfter));
                r.push(row("  room for the second message", if p.escalate_room.is_empty() { "the main room".to_string() } else { p.escalate_room.clone() }, "Enter edits the saved room name.", RowKind::Text, RowKey::EscRoom));
                r.push(row("Waiting", format!("{} held for quiet hours, {} waiting for an answer", self.env.host.msg_policy.held().len(), self.env.host.msg_policy.watching().len()), "Held messages leave by themselves when the quiet hours end.", RowKind::Info, RowKey::Info));
                for w in self.env.host.msg_policy.watching() {
                    r.push(row(&format!("  answered: {}", w.flow), "I saw it", "Enter stops the second message for this flow.", RowKind::Action, RowKey::Ack(w.flow)));
                }
                for f in &self.flows {
                    let on = self.env.host.chan.allowed(&f.id);
                    r.push(row(&format!("Outside messages: {}", f.name), if on { "may send" } else { "off" }, "One message per five minutes at most. An imported flow or a Store template starts off.", RowKind::Toggle, RowKey::FlowSwitch(f.id.clone())));
                }
            }
            DefTab::Activity => {
                r.push(row("Try the whole flow without effects", if self.flows.is_empty() { "no flow" } else { "run" }, "Runs the read-only steps once with the input of the newest run; every step with effects (a message, a write, the camera, a browser) is refused. Enter, or t.", RowKind::Action, RowKey::Info));
            }
        }
        r
    }

    /// The Activity lines of the chosen flow: its state, its counters and what the agent did, newest first.
    fn def_activity(&self, flow_i: usize) -> Vec<(String, [u8; 4])> {
        let Some(f) = self.flows.get(flow_i) else { return vec![("No flow yet.".into(), MUTED)] };
        let book = self.env.assist_store();
        let now = self.now();
        let ro = assist_activity::readout(&book, &f.id, now, 30);
        let n = |k: &str| ro["counters"][k].as_u64().unwrap_or(0);
        let mode = self.env.store.get_draft(&f.id).map_or("off".to_string(), |d| format!("{:?}", d.defaults.assist.mode).to_ascii_lowercase());
        let mut out = vec![(format!("{} · assist {}{}{}", f.name, mode, if ro["paused"] == true { " · PAUSED" } else { "" }, if ro["cooling"] == true { " · cooling down" } else { "" }), TEXT)];
        out.push((format!("{} waiting · {} approved · {} published by itself · {} rejected · {} rolled back", ro["waiting"].as_array().map_or(0, Vec::len), n("approved"), n("auto_published"), n("rejected"), n("rolled_back")), MUTED));
        out.push((format!("today: {} publishes · {} drafts · {}.{:02} spent", n("publishes_today"), n("drafts_today"), n("cents_today") / 100, n("cents_today") % 100), MUTED));
        if !ro["watch"].is_null() {
            out.push((format!("watching version {} (the one before: {})", ro["watch"]["version"], ro["watch"]["previous"]), AMBER));
        }
        out.push((String::new(), DIM));
        let lines: Vec<String> = ro["last"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
        if lines.is_empty() {
            out.push(("Nothing yet: the agent has not done anything for this flow.".into(), DIM));
        }
        out.extend(lines.into_iter().map(|l| (l, TEXT)));
        out
    }

    /// Start the whole-flow trial of the Activity page's flow on a worker thread.
    fn def_trial(&mut self, v: &mut DefView) {
        let Some(f) = self.flows.get(v.flow_i).map(|f| f.id.clone()) else {
            v.note = "There is no flow to try.".into();
            v.note_ok = false;
            return;
        };
        if v.trying {
            return;
        }
        v.trying = true;
        v.note = "Trying the read-only steps... nothing is sent or written.".into();
        v.note_ok = true;
        let (env, tx) = (self.env.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let r = env.trial_whole(&f);
            let _ = tx.send(Msg::Trial(f, r));
        });
    }

    /// The answer of a whole-flow trial: shown on the Activity page when it is open, else in the status line.
    pub(super) fn def_trial_done(&mut self, flow: &str, r: Result<WholeTrial, String>) {
        let mut lines: Vec<(String, [u8; 4])> = vec![];
        let (note, ok) = match &r {
            Ok(w) => {
                lines.push((format!("~ trial of {flow}: {}", w.summary()), if w.failed.is_empty() { OK } else { ERR }));
                for t in &w.tried {
                    lines.push((format!("~   tried {t}"), OK));
                }
                for (n, why) in &w.refused {
                    lines.push((format!("~   not run {n}: {why}"), AMBER));
                }
                for (n, e) in &w.failed {
                    lines.push((format!("~   failed {n}: {e}"), ERR));
                }
                (format!("Trial done: {}.", w.summary()), w.failed.is_empty())
            }
            Err(e) => {
                lines.push((format!("~ trial of {flow}: {e}"), ERR));
                (e.clone(), false)
            }
        };
        if let Some(Modal::Defaults(mut v)) = self.modal.take() {
            v.trying = false;
            v.lines.retain(|l| !l.0.starts_with('~'));
            for (i, l) in lines.into_iter().enumerate() {
                v.lines.insert(i, l);
            }
            v.note = note;
            v.note_ok = ok;
            self.modal = Some(Modal::Defaults(v));
        } else {
            self.say(note, ok);
        }
    }

    fn def_audit(&self, action: &str, detail: &str) {
        iyakki_flow::host::Host::audit(self.env.host.as_ref(), action, &format!("{detail} (by {})", self.env.who()));
    }

    fn def_profile(&self, v: &mut DefView, f: impl FnOnce(&mut iyakki_flow::assist_profile::AgentProfile)) {
        let book = self.env.assist_store();
        let mut p = book.global().profile;
        f(&mut p);
        match book.set_profile(p) {
            Ok(p) => {
                self.def_audit("flow.assist.settings", &format!("profile {}", p.summary()));
                v.note = "Saved.".into();
                v.note_ok = true;
            }
            Err(e) => {
                v.note = e;
                v.note_ok = false;
            }
        }
    }

    fn def_policy(&self, v: &mut DefView, f: impl FnOnce(&mut Policy)) {
        let book = &self.env.host.msg_policy;
        let mut p = book.policy();
        f(&mut p);
        match book.set_policy(p) {
            Ok(p) => {
                self.def_audit("flow.outside_messages.policy", &p.summary());
                v.note = "Saved.".into();
                v.note_ok = true;
            }
            Err(e) => {
                v.note = e;
                v.note_ok = false;
            }
        }
    }

    /// Change the selected row: `dir` is -1 or +1 for a step, 0 for Enter.
    fn def_apply(&mut self, v: &mut DefView, dir: i32) {
        let Some(r) = v.rows.get(v.sel).cloned() else { return };
        let book = self.env.assist_store();
        let g = book.global();
        match (r.key.clone(), dir) {
            (RowKey::Enabled, 0) => {
                self.assist_toggle_kill();
                v.note = self.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
                v.note_ok = true;
            }
            (RowKey::Cap, d) if d != 0 => {
                let n = (i64::from(g.max_publish_per_day) + i64::from(d)).clamp(0, 20) as u32;
                match book.set_global_cap(n) {
                    Ok(()) => {
                        self.def_audit("flow.assist.settings", &format!("automatic publishes per day {n}"));
                        v.note = "Saved.".into();
                        v.note_ok = true;
                    }
                    Err(e) => {
                        v.note = e;
                        v.note_ok = false;
                    }
                }
            }
            (RowKey::Matrix, 0) => match book.set_matrix(!g.matrix, &g.matrix_room) {
                Ok(()) => {
                    self.def_audit("flow.assist.settings", &format!("matrix {}", !g.matrix));
                    v.note = "Saved.".into();
                    v.note_ok = true;
                }
                Err(e) => {
                    v.note = e;
                    v.note_ok = false;
                }
            },
            (RowKey::MatrixRoom, 0) => self.def_edit(v, RowKey::MatrixRoom, &g.matrix_room, "saved room name"),
            (RowKey::LocalFirst, 0) => self.def_profile(v, |p| p.local_first = !p.local_first),
            (RowKey::Fallback, 0) => self.def_profile(v, |p| p.fallback = !p.fallback),
            (RowKey::ModelLocal(i), 0) => self.def_profile(v, |p| {
                if let Some(m) = p.models.get_mut(i) {
                    m.local = !m.local;
                }
            }),
            (RowKey::ModelTime(i), d) if d != 0 => self.def_profile(v, |p| {
                if let Some(m) = p.models.get_mut(i) {
                    m.timeout_secs = (m.timeout_secs as i64 + 30 * i64::from(d)).clamp_safe(MIN_TIMEOUT_SECS as i64, MAX_TIMEOUT_SECS as i64) as u64;
                }
            }),
            (RowKey::ModelDelete(i), 0) => self.def_profile(v, |p| {
                if i < p.models.len() {
                    p.models.remove(i);
                }
            }),
            (RowKey::ModelAdd, 0) => {
                if g.profile.models.len() >= MAX_MODELS {
                    v.note = format!("A profile lists at most {MAX_MODELS} models.");
                    v.note_ok = false;
                } else {
                    self.def_edit(v, RowKey::ModelAdd, "", "provider/model");
                }
            }
            (RowKey::QuietOn, 0) => {
                let off = self.env.host.msg_policy.policy().quiet.is_none();
                let offset = iyakki_agent::clock::Zone::system().local(iyakki_agent::clock::now_unix()).offset / 60;
                self.def_policy(v, |p| {
                    if off {
                        p.quiet = Some((22 * 60, 7 * 60));
                        p.offset_min = offset;
                    } else {
                        p.quiet = None;
                    }
                });
            }
            (RowKey::QuietStart, d) if d != 0 => self.def_policy(v, |p| {
                if let Some((s, e)) = p.quiet {
                    p.quiet = Some(((s as i32 + 30 * d).rem_euclid(1440) as u32, e));
                }
            }),
            (RowKey::QuietEnd, d) if d != 0 => self.def_policy(v, |p| {
                if let Some((s, e)) = p.quiet {
                    p.quiet = Some((s, (e as i32 + 30 * d).rem_euclid(1440) as u32));
                }
            }),
            (RowKey::EscAfter, d) if d != 0 => {
                const STEPS: [u64; 6] = [0, 15, 30, 60, 120, 480];
                let now = self.env.host.msg_policy.policy().escalate_after_secs.map_or(0, |s| s / 60);
                let i = STEPS.iter().position(|m| *m >= now).unwrap_or(0) as i32;
                let n = STEPS[(i + d).clamp(0, 5) as usize];
                self.def_policy(v, |p| p.escalate_after_secs = if n == 0 { None } else { Some(n * 60) });
            }
            (RowKey::EscRoom, 0) => {
                let room = self.env.host.msg_policy.policy().escalate_room;
                self.def_edit(v, RowKey::EscRoom, &room, "saved room name");
            }
            (RowKey::FlowSwitch(id), 0) => {
                let on = !self.env.host.chan.allowed(&id);
                match self.env.host.chan.set(&id, on) {
                    Ok(()) => {
                        self.def_audit("flow.outside_messages", &format!("{id} {}", if on { "on" } else { "off" }));
                        v.note = if on { "This flow may send outside messages (one per five minutes at most).".into() } else { "This flow may not send outside messages.".into() };
                        v.note_ok = true;
                    }
                    Err(e) => {
                        v.note = e;
                        v.note_ok = false;
                    }
                }
            }
            (RowKey::Ack(flow), 0) => {
                let n = self.env.host.msg_policy.ack(&flow);
                self.def_audit("flow.outside_messages.ack", &format!("{flow}: {n} acknowledged"));
                v.note = format!("{n} message(s) of this flow will not be asked about again.");
                v.note_ok = true;
            }
            (RowKey::Info, 0) if v.tab == DefTab::Activity => self.def_trial(v),
            _ => {}
        }
        self.def_refresh(v);
    }

    fn def_edit(&self, v: &mut DefView, key: RowKey, now: &str, hint: &str) {
        let mut t = TextField::new(hint, false);
        t.set(now);
        t.max = 80;
        v.edit = Some((key, t));
    }

    fn def_commit(&mut self, v: &mut DefView) {
        let Some((key, t)) = v.edit.take() else { return };
        let text = t.value.trim().to_string();
        let book = self.env.assist_store();
        match key {
            RowKey::MatrixRoom => {
                let g = book.global();
                match book.set_matrix(g.matrix, &text) {
                    Ok(()) => {
                        self.def_audit("flow.assist.settings", "matrix room");
                        v.note = "Saved.".into();
                        v.note_ok = true;
                    }
                    Err(e) => {
                        v.note = e;
                        v.note_ok = false;
                    }
                }
            }
            RowKey::EscRoom => self.def_policy(v, |p| p.escalate_room = text),
            RowKey::ModelAdd => {
                if text.is_empty() {
                    v.note = "A model needs a name.".into();
                    v.note_ok = false;
                } else {
                    let local = text.starts_with("local/") || text.starts_with("lemonade/") || text.starts_with("ollama/");
                    self.def_profile(v, |p| p.models.push(ProfileModel { name: text, local, timeout_secs: 120 }));
                }
            }
            _ => {}
        }
        self.def_refresh(v);
    }

    /// Keys and clicks of the dialog; the dialog stays until Esc.
    pub(super) fn defaults_event(&mut self, mut v: Box<DefView>, ev: &Event) {
        let mut keep = true;
        if v.edit.is_some() {
            match ev {
                Event::Key(Key::Esc) => v.edit = None,
                Event::Key(Key::Enter) => self.def_commit(&mut v),
                Event::Key(k) => {
                    if let Some((_, t)) = v.edit.as_mut() {
                        t.handle(k);
                    }
                }
                Event::Paste(p) => {
                    if let Some((_, t)) = v.edit.as_mut() {
                        t.paste(p);
                    }
                }
                _ => {}
            }
            self.modal = Some(Modal::Defaults(v));
            return;
        }
        let tab_to = |v: &mut DefView, t: DefTab, me: &FlowsApp| {
            v.tab = t;
            v.sel = 0;
            v.scroll = 0;
            v.note.clear();
            me.def_refresh(v);
        };
        match ev {
            Event::Key(Key::Esc) => keep = false,
            Event::Key(Key::Char(c @ '1'..='4')) => {
                let t = DefTab::ALL[(*c as usize) - ('1' as usize)];
                tab_to(&mut v, t, self);
            }
            Event::Key(Key::Tab) => {
                let i = DefTab::ALL.iter().position(|t| *t == v.tab).unwrap_or(0);
                tab_to(&mut v, DefTab::ALL[(i + 1) % 4], self);
            }
            Event::Key(Key::BackTab) => {
                let i = DefTab::ALL.iter().position(|t| *t == v.tab).unwrap_or(0);
                tab_to(&mut v, DefTab::ALL[(i + 3) % 4], self);
            }
            Event::Tab(i) if *i < 4 => tab_to(&mut v, DefTab::ALL[*i], self),
            Event::Key(Key::Up) | Event::Scroll(1..) => {
                if v.tab == DefTab::Activity && v.sel == 0 {
                    v.scroll = v.scroll.saturating_sub(1);
                } else {
                    v.sel = v.sel.saturating_sub(1);
                }
            }
            Event::Key(Key::Down) | Event::Scroll(..=-1) => {
                if v.tab == DefTab::Activity {
                    v.scroll = (v.scroll + 1).min(v.lines.len().saturating_sub(1));
                } else {
                    v.sel = (v.sel + 1).min(v.rows.len().saturating_sub(1));
                }
            }
            Event::Row(i) => {
                if v.sel == *i {
                    self.def_apply(&mut v, 0);
                } else {
                    v.sel = (*i).min(v.rows.len().saturating_sub(1));
                }
            }
            Event::Key(Key::Left) if v.tab == DefTab::Activity => {
                if v.flow_i > 0 {
                    v.flow_i -= 1;
                    v.scroll = 0;
                    v.lines.retain(|l| !l.0.starts_with('~'));
                    self.def_refresh(&mut v);
                }
            }
            Event::Key(Key::Right) if v.tab == DefTab::Activity => {
                if v.flow_i + 1 < self.flows.len() {
                    v.flow_i += 1;
                    v.scroll = 0;
                    v.lines.retain(|l| !l.0.starts_with('~'));
                    self.def_refresh(&mut v);
                }
            }
            Event::Key(Key::Left) => self.def_apply(&mut v, -1),
            Event::Key(Key::Right) => self.def_apply(&mut v, 1),
            Event::Key(Key::Enter | Key::Char(' ')) => self.def_apply(&mut v, 0),
            Event::Key(Key::Char('t')) if v.tab == DefTab::Activity => {
                self.def_trial(&mut v);
            }
            _ => {}
        }
        if keep {
            self.modal = Some(Modal::Defaults(v));
        }
    }

    pub(super) fn draw_defaults(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(Modal::Defaults(v)) = self.modal.take() else { return };
        let p = fx::modal(g, area, s, 860.0 * s, 580.0 * s, MAGENTA, false);
        let y0 = fx::modal_title(g, p, s, "Assist and messages", MAGENTA);
        let x = p.x + 22.0 * s;
        let w = p.w - 44.0 * s;
        // the pages, as buttons in a row that wraps
        let size = 11.5 * s;
        let (mut bx, mut by) = (x, y0);
        for (i, t) in DefTab::ALL.iter().enumerate() {
            let label = t.label();
            let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
            if bx > x && bx + bw > x + w {
                bx = x;
                by += 28.0 * s;
            }
            g.button(bx, by, size, label, if *t == v.tab { MAGENTA } else { MUTED }, Event::Tab(i));
            bx += bw + 6.0 * s;
        }
        let top = by + 36.0 * s;
        let foot = p.y + p.h - 50.0 * s;
        let note_y = foot - 18.0 * s;
        let body = R::new(x, top, w, (note_y - top - 4.0 * s).max(30.0 * s));
        if v.tab == DefTab::Activity {
            let name = self.flows.get(v.flow_i).map_or("no flow".to_string(), |f| f.name.clone());
            g.text_fit(x, top - 2.0 * s, 11.5 * s, CYAN, &format!("Flow: {name}   (Left and Right change the flow, Up and Down scroll)"), Face::Bold, w);
            // the one action row, then the lines
            let rr = R::new(x, top + 16.0 * s, w, 30.0 * s);
            if let Some(r) = v.rows.first() {
                fx::list_row(g, rr, s, v.sel == 0, g.hovered(rr), MAGENTA);
                g.text_fit(rr.x + 8.0 * s, rr.y + 8.0 * s, 12.0 * s, TEXT, &if v.trying { "Trying... (nothing is sent)".to_string() } else { format!("{}: {}", r.label, r.value) }, Face::Bold, rr.w - 16.0 * s);
                g.hit(rr, Event::Row(0), &r.label);
            }
            let line = 14.5 * s;
            let mut y = rr.y + rr.h + 8.0 * s;
            for (l, c) in v.lines.iter().skip(v.scroll) {
                if y + line > body.y + body.h {
                    break;
                }
                g.text_fit(x, y, 11.0 * s, *c, l.trim_start_matches('~').trim_start(), Face::Mono, w);
                y += line;
            }
        } else {
            let row_h = 38.0 * s;
            let vis = ((body.h / row_h).floor() as usize).max(1);
            let first = v.sel.saturating_sub(vis - 1);
            for (i, r) in v.rows.iter().enumerate().skip(first).take(vis) {
                let rr = R::new(x, body.y + (i - first) as f32 * row_h, w, row_h - 4.0 * s);
                let selected = i == v.sel;
                fx::list_row(g, rr, s, selected, g.hovered(rr), MAGENTA);
                let c = match (r.kind, r.value.as_str()) {
                    (_, "STOPPED") => ERR,
                    (RowKind::Info, _) => DIM,
                    _ => TEXT,
                };
                g.text_fit(rr.x + 8.0 * s, rr.y + 3.0 * s, 12.0 * s, c, &r.label, Face::Bold, rr.w * 0.58);
                let editing = v.edit.as_ref().filter(|(k, _)| *k == r.key && selected);
                if let Some((_, t)) = editing {
                    let fr = R::new(rr.x + rr.w * 0.6, rr.y + 2.0 * s, rr.w * 0.4 - 8.0 * s, rr.h - 4.0 * s);
                    t.render(g, fr, true, s, false, Face::Sans, 950);
                } else {
                    let val = match r.kind {
                        RowKind::Step if selected => format!("<  {}  >", r.value),
                        _ => r.value.clone(),
                    };
                    let vc = if r.value == "off" || r.value == "no" { MUTED } else if r.value == "STOPPED" { ERR } else { CYAN };
                    g.text_fit(rr.x + rr.w * 0.6, rr.y + 3.0 * s, 12.0 * s, vc, &val, Face::Sans, rr.w * 0.4 - 8.0 * s);
                }
                g.text_fit(rr.x + 8.0 * s, rr.y + 19.0 * s, 10.0 * s, DIM, &r.hint, Face::Sans, rr.w - 16.0 * s);
                g.hit(rr, Event::Row(i), &r.label);
            }
        }
        if !v.note.is_empty() {
            g.text_fit(x, note_y, 11.5 * s, if v.note_ok { OK } else { ERR }, &v.note, Face::Sans, w);
        }
        let mut bx = x;
        for (label, key, c) in [("Change (Enter)", Key::Enter, MAGENTA), ("Next page (Tab)", Key::Tab, MUTED), ("Close (Esc)", Key::Esc, MUTED)] {
            if v.tab == DefTab::Activity && matches!(key, Key::Enter) {
                continue;
            }
            let bw = g.text_width(12.0 * s, label, Face::Bold) + 12.0 * s * 1.7;
            g.button(bx, foot, 12.0 * s, label, c, Event::Key(key));
            bx += bw + 8.0 * s;
        }
        self.modal = Some(Modal::Defaults(v));
    }
}

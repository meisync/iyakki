//! FLOWS, Assist: what the screen shows of the agent that works on a flow by itself. A strip on the open flow when suggested fixes
//! wait for the person ("Review"), a list of suggestions with the stored diff, the reason, the checks and the trial and Approve and
//! Reject, a strip while a version autopilot published is being watched (one-click Roll back, Pause autopilot), and the settings
//! (in the flow's settings form, see `flows_props.rs`). Approve puts the change in the draft and opens the ordinary Publish dialog:
//! publishing is still the person's confirmation. Nothing here talks to a model. See `docs/FLOWS.md` ("Assist").

use iyakki_flow::assist_store::{Status, Suggestion, Watch};
use iyakki_flow::model::*;

use super::flows::*;
use super::flows_fail::Banner;
use super::flows_view::GfxFit;
use super::Flow as AppFlow;
use crate::fx::{self, AMBER, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// What the strips and the list read (refreshed about twice a second while a flow is open).
#[derive(Default)]
pub(super) struct AssistUi {
    pub mode: AssistMode,
    pub sugs: Vec<Suggestion>,
    pub watch: Option<Watch>,
    pub paused: bool,
    pub cooling: bool,
}

/// The suggestions dialog.
pub(super) struct SugView {
    pub entries: Vec<Suggestion>,
    pub sel: usize,
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}...")
    } else {
        t
    }
}

/// The lines of the detail side: why, the diff, the checks, the trial, and what it waits for.
pub(super) fn detail_lines(s: &Suggestion) -> Vec<(String, [u8; 4])> {
    let mut v: Vec<(String, [u8; 4])> = vec![];
    v.push((format!("{} for step {}, from version {}", s.class.label(), if s.node.is_empty() { "-" } else { &s.node }, s.base_version), TEXT));
    if !s.reason.is_empty() {
        v.push((format!("Why: {}", s.reason), MUTED));
    }
    for l in &s.diff {
        let c = if l.starts_with('+') { OK } else if l.starts_with('-') { ERR } else if l.starts_with('~') { AMBER } else { MUTED };
        v.push((l.clone(), c));
    }
    for q in &s.qa {
        v.push((format!("Check: {q}"), MUTED));
    }
    let trial = match &s.trial {
        iyakki_flow::assist::TrialState::Passed(n) => (format!("Trial: passed ({n})"), OK),
        iyakki_flow::assist::TrialState::Failed(n) => (format!("Trial: FAILED ({n})"), ERR),
        iyakki_flow::assist::TrialState::NotRun(n) => (format!("Trial: not run ({n})"), AMBER),
    };
    v.push(trial);
    for (name, ok, why) in &s.guards {
        if !ok {
            v.push((format!("Guard failed: {name}: {why}"), AMBER));
        }
    }
    if !s.asks.is_empty() {
        v.push((format!("Waits for you because: {}", s.asks), AMBER));
    }
    v
}

impl FlowsApp {
    /// Read the open flow's suggestions, the watch and the switches (small files).
    pub(super) fn update_assist(&mut self) {
        let Some(id) = self.cur.clone() else {
            self.assist = AssistUi::default();
            return;
        };
        let book = self.env.assist_store();
        let b = book.book(&id);
        let mode = self.env.store.published(&id).map(|f| f.defaults.assist.mode).unwrap_or_default();
        let ui = AssistUi { mode, sugs: book.open_list(&id), watch: b.watch.clone(), paused: b.paused, cooling: b.cooldown_until_ms > book.now() };
        if ui.sugs.iter().map(|s| (&s.id, s.status)).collect::<Vec<_>>() != self.assist.sugs.iter().map(|s| (&s.id, s.status)).collect::<Vec<_>>() || ui.watch != self.assist.watch || ui.paused != self.assist.paused || ui.cooling != self.assist.cooling || ui.mode != self.assist.mode {
            self.redraw = true;
        }
        self.assist = ui;
    }

    /// The strip for suggestions that wait for the person.
    pub(super) fn suggest_banner(&self) -> Option<Banner> {
        let s = self.assist.sugs.first()?;
        let more = if self.assist.sugs.len() > 1 { format!(" (and {} more)", self.assist.sugs.len() - 1) } else { String::new() };
        let text = if s.status == Status::Loaded {
            format!("You approved a suggested change; it is in the draft. Publish it to finish: {}{more}", short(&s.summary, 80))
        } else {
            format!("The agent suggests: {}{more}", short(&s.summary, 90))
        };
        Some(Banner { text, color: MAGENTA, buttons: vec![("Review (S)".into(), id::SUG_OPEN, MAGENTA, true)] })
    }

    /// The strip while autopilot's published version is watched, or autopilot is paused.
    pub(super) fn assist_banner(&self) -> Option<Banner> {
        let a = &self.assist;
        if a.mode == AssistMode::Off && a.watch.is_none() {
            return None;
        }
        if let Some(w) = &a.watch {
            let text = format!("Autopilot published version {}. It is watched for its first runs and rolled back if one fails.", w.version);
            let mut buttons = vec![(format!("Roll back to v{} (B)", w.previous), id::ASSIST_ROLLBACK, AMBER, true)];
            buttons.push((if a.paused { "Resume autopilot (U)".into() } else { "Pause autopilot (U)".into() }, id::ASSIST_PAUSE, CYAN, true));
            return Some(Banner { text, color: AMBER, buttons });
        }
        if a.mode == AssistMode::Autopilot && (a.paused || a.cooling) {
            let text = if a.paused { "Autopilot is paused: it drafts fixes but asks before it publishes anything.".to_string() } else { "Autopilot is cooling down after a rollback: it asks before it publishes anything.".to_string() };
            let buttons = if a.paused { vec![("Resume autopilot (U)".to_string(), id::ASSIST_PAUSE, CYAN, true)] } else { vec![("Pause autopilot (U)".to_string(), id::ASSIST_PAUSE, CYAN, true)] };
            return Some(Banner { text, color: AMBER, buttons });
        }
        None
    }

    /// Every strip of the open flow, in the order they are drawn.
    pub(super) fn banners(&self) -> Vec<Banner> {
        let mut v = vec![];
        if let Some(b) = self.fail_banner() {
            v.push(b);
        }
        if let Some(b) = self.suggest_banner() {
            v.push(b);
        }
        if let Some(b) = self.assist_banner() {
            v.push(b);
        }
        v
    }

    /// Keys of the strips (capital letters, only while the strip is there).
    pub(super) fn assist_key(&mut self, k: &Key) -> Option<AppFlow> {
        match k {
            Key::Char('S') if !self.assist.sugs.is_empty() => Some(self.tab_click(id::SUG_OPEN)),
            Key::Char('B') if self.assist.watch.is_some() => Some(self.tab_click(id::ASSIST_ROLLBACK)),
            Key::Char('U') if self.assist.watch.is_some() || (self.assist.mode == AssistMode::Autopilot && (self.assist.paused || self.assist.cooling)) => Some(self.tab_click(id::ASSIST_PAUSE)),
            _ => None,
        }
    }

    pub(super) fn open_suggestions(&mut self) {
        self.update_assist();
        if self.assist.sugs.is_empty() {
            self.say("No suggestion is waiting.", false);
            return;
        }
        self.modal = Some(Modal::Suggestions(SugView { entries: self.assist.sugs.clone(), sel: 0 }));
    }

    fn reload_draft(&mut self) {
        let Some(id) = self.cur.clone() else { return };
        if let Some(f) = self.env.store.get_draft(&id) {
            self.flow = f;
        }
        self.stamp = super::flows::stamp_of(&self.env, &id);
        self.revalidate();
        self.update_assist();
    }

    /// Approve: the change goes into the draft, then the ordinary Publish dialog asks for the person's confirmation.
    pub(super) fn sug_approve(&mut self, sid: &str) {
        let Some(id) = self.cur.clone() else { return };
        let r = self.env.with_assistant(|a| a.adopt(&id, sid));
        match r {
            Ok(_) => {
                self.reload_draft();
                self.say("The change is in the draft. Check it and publish.", true);
                self.open_publish();
            }
            Err(e) => {
                self.update_assist();
                self.say(e, false);
            }
        }
    }

    pub(super) fn sug_reject(&mut self, sid: &str) {
        let Some(id) = self.cur.clone() else { return };
        let who = self.env.who();
        match self.env.with_assistant(|a| a.reject(&id, sid, &who)) {
            Ok(_) => {
                self.reload_draft();
                self.say("Rejected. The agent will not offer this change again.", true);
            }
            Err(e) => self.say(e, false),
        }
    }

    /// One click: go back to the version before the one autopilot published (the ordinary rollback confirmation follows).
    pub(super) fn assist_rollback(&mut self) {
        let Some(w) = self.assist.watch.clone() else { return };
        self.modal = Some(Modal::Confirm(format!("Roll back to version {}? It is published again as a new version; nothing is lost. Autopilot then pauses.", w.previous), Confirmed::Rollback(w.previous)));
    }

    /// After a rollback the person confirmed: the watch ends, autopilot pauses and the suggestion is marked.
    pub(super) fn assist_rolled_back(&mut self) {
        let Some(id) = self.cur.clone() else { return };
        let book = self.env.assist_store();
        if let Some(w) = book.watch(&id) {
            let _ = book.set_watch(&id, None);
            let _ = book.set_status(&id, &w.suggestion, Status::RolledBack);
            let _ = book.set_paused(&id, true);
            iyakki_flow::host::Host::audit(self.env.host.as_ref(), "flow.assist.rollback", &format!("{id} v{} rolled back by {}; autopilot paused", w.version, self.env.who()));
        }
        self.update_assist();
    }

    pub(super) fn assist_toggle_pause(&mut self) {
        let Some(id) = self.cur.clone() else { return };
        let book = self.env.assist_store();
        let on = !book.paused(&id);
        match book.set_paused(&id, on) {
            Ok(()) => {
                iyakki_flow::host::Host::audit(self.env.host.as_ref(), if on { "flow.assist.pause" } else { "flow.assist.resume" }, &format!("{id} by {}", self.env.who()));
                self.say(if on { "Autopilot is paused for this flow." } else { "Autopilot is on again for this flow." }, true);
            }
            Err(e) => self.say(e, false),
        }
        self.update_assist();
    }

    /// Keys and clicks inside the suggestions dialog.
    pub(super) fn suggestions_event(&mut self, mut v: SugView, ev: &Event) {
        let mut keep = true;
        let cur = v.entries.get(v.sel).map(|s| s.id.clone());
        match ev {
            Event::Key(Key::Esc) => keep = false,
            Event::Key(Key::Up) | Event::Scroll(1..) => v.sel = v.sel.saturating_sub(1),
            Event::Key(Key::Down) | Event::Scroll(..=-1) => v.sel = (v.sel + 1).min(v.entries.len().saturating_sub(1)),
            Event::Row(i) => v.sel = (*i).min(v.entries.len().saturating_sub(1)),
            Event::Key(Key::Char('a')) => {
                if let Some(id) = cur {
                    keep = false;
                    self.sug_approve(&id);
                }
            }
            Event::Key(Key::Char('x')) => {
                if let Some(id) = cur {
                    self.sug_reject(&id);
                    v.entries = self.assist.sugs.clone();
                    v.sel = v.sel.min(v.entries.len().saturating_sub(1));
                    keep = !v.entries.is_empty();
                }
            }
            _ => {}
        }
        if keep {
            self.modal = Some(Modal::Suggestions(v));
        }
    }

    pub(super) fn draw_suggestions(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(Modal::Suggestions(v)) = self.modal.as_ref() else { return };
        let p = fx::modal(g, area, s, 820.0 * s, 560.0 * s, MAGENTA, false);
        let y0 = fx::modal_title(g, p, s, "Suggested changes", MAGENTA);
        let x = p.x + 22.0 * s;
        let inner_w = p.w - 44.0 * s;
        let narrow = p.w < 600.0 * s;
        let row = 36.0 * s;
        let by_h = 40.0 * s;
        let (list, detail) = if narrow {
            let n = v.entries.len().min(3).max(1);
            let lh = n as f32 * row;
            (R::new(x, y0, inner_w, lh), R::new(x, y0 + lh + 8.0 * s, inner_w, (p.y + p.h - by_h * 2.0 - y0 - lh - 8.0 * s).max(30.0 * s)))
        } else {
            let lw = (inner_w * 0.34).min(280.0 * s);
            (R::new(x, y0, lw, p.y + p.h - by_h * 2.0 - y0), R::new(x + lw + 14.0 * s, y0, inner_w - lw - 14.0 * s, p.y + p.h - by_h * 2.0 - y0))
        };
        let vis = ((list.h / row).floor() as usize).max(1);
        let first = v.sel.saturating_sub(vis - 1);
        for (i, e) in v.entries.iter().enumerate().skip(first).take(vis) {
            let rr = R::new(list.x, list.y + (i - first) as f32 * row, list.w, row - 4.0 * s);
            fx::list_row(g, rr, s, i == v.sel, g.hovered(rr), MAGENTA);
            let t = format!("{} · {}", e.id, e.status.label());
            g.text_fit(rr.x + 8.0 * s, rr.y + 3.0 * s, 12.0 * s, TEXT, &t, Face::Bold, rr.w - 16.0 * s);
            g.text_fit(rr.x + 8.0 * s, rr.y + 18.0 * s, 10.5 * s, DIM, &e.summary, Face::Sans, rr.w - 16.0 * s);
            g.hit(rr, Event::Row(i), &t);
        }
        let mut y = detail.y;
        let line = 14.0 * s;
        if let Some(e) = v.entries.get(v.sel) {
            for (l, c) in detail_lines(e) {
                if y + line > detail.y + detail.h {
                    break;
                }
                g.text_fit(detail.x, y, 11.0 * s, c, &l, Face::Mono, detail.w);
                y += line;
            }
        }
        // the buttons: in rows that wrap, never outside the panel
        let size = 12.0 * s;
        let mut bx = x;
        let mut by = p.y + p.h - by_h * 2.0 + 6.0 * s;
        let loaded = v.entries.get(v.sel).is_some_and(|e| e.status == Status::Loaded);
        let buttons: [(&str, Key, [u8; 4]); 3] = [(if loaded { "Publish now (a)" } else { "Approve and publish (a)" }, Key::Char('a'), MAGENTA), ("Reject (x)", Key::Char('x'), ERR), ("Close (Esc)", Key::Esc, MUTED)];
        for (label, key, c) in buttons {
            let w = g.text_width(size, label, Face::Bold) + size * 1.7;
            if bx > x && bx + w > x + inner_w {
                bx = x;
                by += by_h * 0.9;
            }
            g.button(bx, by, size, label, c, Event::Key(key));
            bx += w + 8.0 * s;
        }
    }
}

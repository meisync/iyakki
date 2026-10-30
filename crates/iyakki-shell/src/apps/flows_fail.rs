//! FLOWS, closing and results, and the agent jumping in on a failure. How a flow is closed (and what is kept), the banner that says
//! how a run ended (Run again, Open log, Stop), the card that offers the assistant when the last run of the open flow failed, the
//! one-line key hint of every screen. The assistant is only ever started by the person's click or key; what it then changes goes
//! through the ordinary approval card. See `docs/FLOWS.md`.

use iyakki_agent::flows::failfix::{self, Failure, MAX_FIX_ATTEMPTS};
use iyakki_flow::model::*;

use super::flows::*;
use super::{AppCx, Flow as AppFlow};
use crate::fx::{with_alpha, CYAN, MAGENTA};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

/// A calm strip with a line of text and a row of buttons (label, click id, colour, enabled).
pub(super) struct Banner {
    pub text: String,
    pub color: [u8; 4],
    pub buttons: Vec<(String, usize, [u8; 4], bool)>,
}

const LINE_H: f32 = 16.0;

/// Where each button of the banner goes: (x offset, row), and the number of rows.
fn place(g: &mut Gfx, b: &Banner, w: f32, size: f32, gap: f32) -> (Vec<(f32, usize, f32)>, usize) {
    let mut out = Vec::new();
    let (mut x, mut row) = (0.0f32, 0usize);
    for (label, ..) in &b.buttons {
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        if x > 0.0 && x + bw > w {
            x = 0.0;
            row += 1;
        }
        out.push((x, row, bw));
        x += bw + gap;
    }
    (out, if b.buttons.is_empty() { 0 } else { row + 1 })
}

/// Draw the banner across `r.w` from `(r.x, r.y)` and give back its height. Nothing in it can overlap: the text is wrapped to two
/// lines and the buttons flow onto further rows.
pub(super) fn draw_banner(g: &mut Gfx, r: R, s: f32, b: &Banner) -> f32 {
    let pad = 8.0 * s;
    let inner_w = (r.w - 2.0 * pad).max(40.0);
    let size = 11.5 * s;
    let bh = size * 2.1;
    let gap = 6.0 * s;
    let lines: Vec<String> = g.wrap(12.0 * s, &b.text, Face::Sans, inner_w - 16.0 * s).into_iter().take(2).collect();
    let (spots, rows) = place(g, b, inner_w, size, gap);
    let h = pad + lines.len().max(1) as f32 * LINE_H * s + if rows > 0 { 4.0 * s + rows as f32 * (bh + 4.0 * s) } else { 0.0 } + pad * 0.6;
    g.rrect(r.x, r.y, r.w, h, 8.0 * s, with_alpha(b.color, 30));
    g.rrect_outline(r.x, r.y, r.w, h, 8.0 * s, 1.0, with_alpha(b.color, 150));
    g.circle(r.x + pad + 4.0 * s, r.y + pad + 7.0 * s, 4.0 * s, b.color);
    let mut y = r.y + pad;
    for (i, l) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        let t = if last { g.fit(12.0 * s, l, Face::Sans, inner_w - 16.0 * s) } else { l.clone() };
        g.text(r.x + pad + 14.0 * s, y, 12.0 * s, TEXT, &t, Face::Sans);
        y += LINE_H * s;
    }
    y += 4.0 * s;
    for ((label, idn, accent, on), (dx, row, _)) in b.buttons.iter().zip(spots) {
        let (bx, by) = (r.x + pad + dx, y + row as f32 * (bh + 4.0 * s));
        if *on {
            g.button(bx, by, size, label, *accent, Event::Tab(*idn));
        } else {
            g.button_off(bx, by, size, label);
        }
    }
    h
}

impl FlowsApp {
    // ------------------------------------------------------------------------------------------ the failure card

    /// The failed last run of a flow that the person has not dismissed. A success starts the fix attempts again.
    fn latest_failure(&mut self, flow: &str) -> Option<Failure> {
        let s = self.runs.iter().find(|r| r.flow_id == flow && r.parent.is_none() && r.status != RunStatus::Scheduled)?.clone();
        match s.status {
            RunStatus::Failed => {}
            RunStatus::Succeeded => {
                self.book.reset(flow);
                return None;
            }
            _ => return None,
        }
        if self.book.dismissed(&s.id) {
            return None;
        }
        failfix::failure_of(&self.env.engine.get_run(&s.id)?)
    }

    /// Look at the open flow's last run: show the card when it failed, and (once per failed run, only when the assistant is
    /// already open) let the assistant say so in the thread.
    pub(super) fn update_fail_card(&mut self) {
        let prev = self.fail.as_ref().map(|f| f.run_id.clone());
        let found = match self.cur.clone() {
            Some(id) => self.latest_failure(&id),
            None => None,
        };
        let new = if self.book.offer() { found } else { None };
        if new.as_ref().map(|f| &f.run_id) != prev.as_ref() {
            if let Some(f) = &new {
                if self.chat_open && !self.book.announced(&f.run_id) {
                    self.pending_note = Some((f.run_id.clone(), f.offer()));
                }
            }
            self.redraw = true;
        }
        self.fail = new;
    }

    /// Said in the thread once the chat has switched to this flow's conversation.
    pub(super) fn deliver_note(&mut self, cx: &AppCx) {
        let Some((run, text)) = self.pending_note.take() else { return };
        if !self.chat_open || !self.fail.as_ref().is_some_and(|f| f.run_id == run) {
            return;
        }
        if self.chat.is_running() {
            self.pending_note = Some((run, text));
            return;
        }
        self.chat.local_note(cx, &text);
        self.book.mark_announced(&run);
        self.redraw = true;
    }

    /// The flow's own settings (nothing selected), where the permissions are: for a step that stopped for want of one.
    pub(super) fn open_flow_settings(&mut self) {
        let in_design = self.tab == Tab::Design && self.cur.is_some();
        let flow = self.fail.as_ref().map(|f| f.flow_id.clone()).filter(|_| !in_design).or_else(|| self.run_id.as_ref().and_then(|r| self.env.engine.get_run(r)).map(|r| r.flow_id));
        if let Some(id) = flow {
            if self.cur.as_deref() != Some(id.as_str()) {
                self.open_flow(&id);
            }
        }
        self.sel.clear();
        self.sel_edge = None;
        self.set_tab(Tab::Design);
        self.sync_props();
        self.redraw = true;
    }

    /// "Refresh models": read the providers' model lists again, forget the automatic vision pick, and run the stopped steps again.
    pub(super) fn refresh_models_and_retry(&mut self) {
        let line = self.env.svc.refresh_model_lists();
        let run = self.run_id.clone().or_else(|| self.fail.as_ref().map(|f| f.run_id.clone()));
        let mut retried = false;
        if let Some(r) = run.as_deref().and_then(|r| self.env.engine.get_run(r)) {
            for (id, n) in &r.nodes {
                if n.needs_person && n.permission == "choice" && self.env.engine.retry_node(&r.id, id).is_ok() {
                    retried = true;
                }
            }
        }
        self.say(format!("{line}{}", if retried { " The step runs again." } else { "" }), true);
    }

    pub(super) fn dismiss_fail(&mut self) {
        if let Some(f) = self.fail.take() {
            // dismissing the run withdraws its permission requests (the INBOX entries go with them)
            let user = self.env.user.clone();
            self.env.svc.browser.consent.withdraw_permissions(&user, &f.run_id);
            self.book.dismiss(&f.run_id);
            self.pending_note = None;
            self.say("Dismissed. The card does not come back for this run.", true);
        }
    }

    /// The person pressed Ask the agent to fix: open the assistant on this flow with the failure loaded and ask it to diagnose and
    /// propose. It changes nothing by itself: edits and runs ask first, and only this many attempts are allowed.
    pub(super) fn start_fix(&mut self, cx: &AppCx) {
        let Some(f) = self.fail.clone() else { return };
        if f.needs_person {
            self.say("This needs a permission or a choice only you can give: open the flow's settings. The assistant cannot switch it on.", false);
            return;
        }
        if !cx.session.policy.agent {
            self.say("The assistant's tools are not enabled for this account.", false);
            return;
        }
        if self.chat.is_running() {
            self.say("The assistant is busy. Try again when it has finished.", false);
            return;
        }
        let n = match self.book.try_attempt(&f.run_id, &f.flow_id) {
            Ok(n) => n,
            Err(e) => {
                self.say(e, false);
                return;
            }
        };
        self.pending_note = None;
        self.book.mark_announced(&f.run_id);
        self.chat_open = true;
        self.focus = Focus::Chat;
        self.chat.set_agent_mode(true);
        // a fix is approved change by change: Auto does not carry over into it
        if self.chat.approvals().is_auto() || self.chat.autopilot() {
            self.chat.end_auto_unless_pinned();
        }
        // the assistant is shown this run (the context names the flow, the run and its failed step)
        self.select_run(&f.run_id, false);
        self.sync_open();
        let draft = self.chat.input_text().to_string();
        self.chat.ask(cx, &f.request(n));
        if !draft.is_empty() {
            self.chat.prefill(&draft);
        }
        self.say(format!("Asked the assistant to look (attempt {n} of {MAX_FIX_ATTEMPTS}). Anything it wants to change asks you first."), true);
    }

    /// Show the log of a run: the failed step (or the step you were on) and the Log tab.
    pub(super) fn open_log(&mut self, run: Option<String>) {
        if let Some(r) = run {
            if self.run_id.as_deref() != Some(r.as_str()) {
                self.select_run(&r, false);
            }
        }
        if self.tab != Tab::Run {
            self.ret = self.tab;
            self.tab = Tab::Run;
        }
        self.focus = Focus::Canvas;
        if let Some(r) = &self.run {
            let failed = r.flow.nodes.iter().find(|n| r.nodes.get(&n.id).is_some_and(|x| matches!(x.state, NodeState::Failed | NodeState::Interrupted))).map(|n| n.id.clone());
            if let Some(n) = failed.filter(|_| self.run_node.is_none()) {
                self.run_node = Some(n);
            }
        }
        self.rv.tab = RunTab::Log;
        self.rv.shown = Default::default();
        self.sync_open();
    }

    pub(super) fn run_again(&mut self) {
        let Some(run) = self.run.clone() else {
            self.say("There is no run to repeat.", false);
            return;
        };
        let mut req = iyakki_flow::StartRequest::new(&run.flow_id).trigger("manual", &self.env.who());
        req.inputs = run.inputs.clone();
        req.draft = self.env.store.published(&run.flow_id).is_none();
        match self.env.engine.start_run(req) {
            Ok(o) => {
                self.refresh_runs();
                self.select_run(&o.run_id, false);
                self.say(format!("Started run {} again with the same inputs.", o.run_id), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub(super) fn toggle_offer(&mut self) {
        let on = !self.book.offer();
        self.book.set_offer(on);
        self.update_fail_card();
        self.say(if on { "The assistant is offered when a flow fails." } else { "The assistant is not offered when a flow fails." }, true);
    }

    // ------------------------------------------------------------------------------------------ closing

    /// Something typed that was not accepted yet (a refused field, text that is not a valid flow): closing would lose it.
    pub(super) fn has_unsaved(&self) -> bool {
        (self.text.is_some() && !self.text_err.is_empty()) || self.props.fields.iter().any(|f| f.err.is_some())
    }

    /// Close the open flow. Flows save after every change, so nothing is lost by closing; only text that was refused is asked about.
    pub(super) fn request_close(&mut self) {
        if self.cur.is_none() {
            return;
        }
        self.commit_props();
        if self.has_unsaved() {
            self.modal = Some(Modal::Confirm(format!("Close \"{}\" and drop what you are still typing (it was refused: {})? Everything else is already saved. Press y to close, Esc or Enter keeps editing.", self.flow.name, self.text_err_or_field()), Confirmed::CloseFlow));
            return;
        }
        self.finish_close();
    }

    fn text_err_or_field(&self) -> String {
        if !self.text_err.is_empty() {
            return self.text_err.chars().take(80).collect();
        }
        self.props.fields.iter().find_map(|f| f.err.clone()).unwrap_or_default().chars().take(80).collect()
    }

    pub(super) fn finish_close(&mut self) {
        let (name, id) = (self.flow.name.clone(), self.cur.clone().unwrap_or_default());
        let changed = self.flows.iter().find(|f| f.id == id).is_some_and(|f| f.draft_changed || f.published == 0);
        self.props = super::flows_props::Props::default();
        self.text_err.clear();
        self.tab = Tab::Design;
        self.focus = Focus::Canvas;
        self.close_flow();
        self.say(if changed { format!("Closed \"{name}\". Your changes are saved in its draft; Publish makes them a version.") } else { format!("Closed \"{name}\".") }, true);
    }

    /// Leave the run view for the screen it was opened from.
    pub(super) fn leave_run(&mut self) {
        let t = if self.ret == Tab::Run { Tab::Design } else { self.ret };
        self.set_tab(t);
    }

    // ------------------------------------------------------------------------------------------ banners

    /// The card on the open flow (design views): the failure and what can be done.
    pub(super) fn fail_banner(&self) -> Option<Banner> {
        let f = self.fail.as_ref()?;
        let used = self.book.attempts(&f.flow_id);
        let mut text = f.headline();
        if used > 0 {
            text.push_str(&format!("  (fix attempts used: {used} of {MAX_FIX_ATTEMPTS})"));
        }
        let left = self.book.attempts_left(&f.flow_id) > 0;
        let mut buttons = if f.choice {
            vec![("Open Settings -> Models".to_string(), id::CARD_MODELS, MAGENTA, true), ("Refresh models".to_string(), id::CARD_REFRESH, CYAN, true)]
        } else if f.needs_person {
            vec![("Open flow settings".to_string(), id::CARD_SETTINGS, MAGENTA, true)]
        } else {
            vec![(if left { "Ask the agent to fix (F)".to_string() } else { "No fix attempts left".to_string() }, id::FAIL_FIX, MAGENTA, left)]
        };
        buttons.push(("Open log (L)".into(), id::FAIL_LOG, CYAN, true));
        buttons.push(("Dismiss (x)".into(), id::FAIL_DISMISS, MUTED, true));
        Some(Banner { text, color: ERR, buttons })
    }

    /// How the shown run ended or is going, with what to do next.
    pub(super) fn run_banner(&self, run: &Run, now: i64) -> Banner {
        let sum = run.summary();
        let mut buttons: Vec<(String, usize, [u8; 4], bool)> = vec![];
        let again = ("Run again (a)".to_string(), id::R_AGAIN, OK, true);
        let log = ("Open log (l)".to_string(), id::FAIL_LOG, CYAN, true);
        let (text, color) = match run.status {
            RunStatus::Succeeded => {
                buttons.push(again);
                buttons.push(log);
                (format!("Succeeded in {}: {} of {} steps ran. The result is in each step's Output tab.", super::flows_view::dur_text(run.duration_ms(now)), sum.nodes_done, sum.nodes_total), OK)
            }
            RunStatus::Failed => {
                let f = failfix::failure_of(run);
                buttons.push(again);
                buttons.push(log);
                if let Some(f) = f.as_ref().filter(|f| self.fail.as_ref().is_some_and(|x| x.run_id == f.run_id)) {
                    let left = self.book.attempts_left(&f.flow_id) > 0;
                    if f.choice {
                        buttons.push(("Open Settings -> Models".to_string(), id::CARD_MODELS, MAGENTA, true));
                        buttons.push(("Refresh models".to_string(), id::CARD_REFRESH, CYAN, true));
                    } else if f.needs_person {
                        buttons.push(("Open flow settings".to_string(), id::CARD_SETTINGS, MAGENTA, true));
                    } else {
                        buttons.push((if left { "Ask the agent to fix (F)".to_string() } else { "No fix attempts left".to_string() }, id::FAIL_FIX, MAGENTA, left));
                    }
                    buttons.push(("Dismiss (x)".into(), id::FAIL_DISMISS, MUTED, true));
                }
                (f.map(|f| f.headline()).unwrap_or_else(|| "Run failed.".into()), ERR)
            }
            RunStatus::Cancelled => {
                buttons.push(again);
                ("Stopped before it finished.".to_string(), MUTED)
            }
            RunStatus::Blocked => (format!("Waiting for you. Decide it with the buttons above or on the step.{}", if run.blocked.is_empty() { String::new() } else { format!(" {}", iyakki_flow::driver::clip_words(&run.blocked, 150)) }), MAGENTA),
            RunStatus::Scheduled => {
                buttons.push(("Stop run (c)".into(), id::R_CANCEL, ERR, true));
                ("Scheduled. It starts by itself at its time.".to_string(), MUTED)
            }
            _ => {
                buttons.push(("Stop run (c)".into(), id::R_CANCEL, ERR, true));
                (format!("{}: {} of {} steps done. Click a step to watch it.", run.status.label(), sum.nodes_done, sum.nodes_total), CYAN)
            }
        };
        Banner { text, color, buttons }
    }

    // ------------------------------------------------------------------------------------------ the one-line key hint

    /// The keys of what has the keyboard right now.
    pub(super) fn footer_hint(&self) -> String {
        match self.focus {
            Focus::Chat if self.chat_open => return "Enter send · Esc back to the flow · F6 switch · Ctrl+I hide the assistant".into(),
            Focus::Search => return "type to find a step · Enter add the first match · Esc cancel".into(),
            Focus::BoardSearch => return "type to filter runs · Enter or Esc done".into(),
            Focus::Text => return "edits apply when the text is a valid flow · Esc back to the canvas".into(),
            Focus::Props => return "Tab next field · Enter or Ctrl+S apply · Esc undo this edit and leave".into(),
            _ => {}
        }
        let fix = if self.fail.is_some() { "F ask the agent to fix · x dismiss · " } else { "" };
        match (self.tab, self.cur.is_some()) {
            (Tab::Design, true) if !self.sel.is_empty() || self.sel_edge.is_some() => format!("{fix}Enter edit settings · T test step · Del delete · r Run · Esc deselect · p Publish · Ctrl+I assistant"),
            (Tab::Design, true) => format!("{fix}r Run · Esc Close · p Publish · a Add step · h History · Ctrl+Z Undo · f Fit · Ctrl+I assistant"),
            (Tab::Design, false) => "Enter open · n New · t Template · i Import · d Delete · o Agent help on failure · Esc leave FLOWS · Ctrl+I assistant".into(),
            (Tab::Run, _) => format!("{fix}Esc Close · c Stop · a Run again · l Log · r Retry step · Left Right other runs · Ctrl+I assistant"),
            (Tab::Board, _) => "Enter open · Space select · a Run again · c Cancel · / search · f flow · Esc leave FLOWS".into(),
        }
    }

    // ------------------------------------------------------------------------------------------ the inputs form

    /// The form for a run's inputs was submitted: build the inputs object (numbers and yes/no by their declared type).
    pub(super) fn run_inputs_submit(&mut self, form: &super::widgets::Form, draft: bool, params: &[Param]) {
        let mut m = serde_json::Map::new();
        for (i, p) in params.iter().enumerate() {
            let t = if p.ty == "secret" { form.raw(i) } else { form.value(i) };
            if t.is_empty() {
                continue;
            }
            let v = match p.ty.as_str() {
                "number" | "integer" => t.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(serde_json::Value::Number).unwrap_or(serde_json::Value::String(t)),
                "bool" | "boolean" | "yes-no" => serde_json::Value::Bool(matches!(t.to_ascii_lowercase().as_str(), "true" | "yes" | "y" | "1" | "on")),
                _ => serde_json::Value::String(t),
            };
            m.insert(p.name.clone(), v);
        }
        self.start_flow_run(draft, Some(serde_json::Value::Object(m)));
    }

    /// Whether a click or key belongs to the failure card (so the keyboard handler can route it).
    pub(super) fn fail_key(&mut self, k: &crate::ui::Key) -> Option<AppFlow> {
        use crate::ui::Key;
        if self.fail.is_none() {
            return None;
        }
        match k {
            Key::Char('F') => Some(self.tab_click(id::FAIL_FIX)),
            Key::Char('x') => Some(self.tab_click(id::FAIL_DISMISS)),
            Key::Char('L') => Some(self.tab_click(id::FAIL_LOG)),
            _ => None,
        }
    }
}

/// The strip at the top of the assistant's pane while the card is up: the answer to "want me to fix it?".
pub(super) fn draw_offer_strip(g: &mut Gfx, r: R, s: f32, needs_person: bool) -> f32 {
    let h = 30.0 * s;
    let size = 11.0 * s;
    g.rrect(r.x, r.y, r.w, h, 6.0 * s, with_alpha(MAGENTA, 28));
    let mut x = r.x + 8.0 * s;
    if needs_person {
        x += g.button(x, r.y + (h - size * 2.1) / 2.0, size, "Open flow settings", MAGENTA, Event::Tab(id::CARD_SETTINGS)) + 6.0 * s;
    } else {
        x += g.button(x, r.y + (h - size * 2.1) / 2.0, size, "Yes, fix it", MAGENTA, Event::Tab(id::FAIL_FIX)) + 6.0 * s;
    }
    x += g.button(x, r.y + (h - size * 2.1) / 2.0, size, "Not now", MUTED, Event::Tab(id::FAIL_DISMISS)) + 8.0 * s;
    let left = r.x + r.w - x - 6.0 * s;
    if left > 60.0 * s {
        let t = g.fit(10.5 * s, "Last run failed", Face::Sans, left);
        g.text(x, r.y + (h - 12.0 * s) / 2.0, 10.5 * s, DIM, &t, Face::Sans);
    }
    h
}

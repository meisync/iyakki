//! RUNS: what your agents did. Every agent run is recorded; here you can read each one step by step
//! (what the model asked for, what you approved or denied, what each tool returned) and see whether
//! the record is intact — the transcript is checked against its hash-chained ledger when opened.

use iyakki_agent::agent::RunEvent;
use iyakki_agent::runs::{Integrity, RunLog, RunMeta};

use super::widgets::render_confirm;
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 196, 94);
const CYAN_TASK: [u8; 4] = rgb(100, 230, 255);

pub struct RunsApp {
    runs: Vec<RunMeta>,
    /// Every run, before the filter.
    all: Vec<RunMeta>,
    /// Only the runs whose agent (`Agent 2`) or task id is this (key f cycles).
    filter: Option<String>,
    /// Asked of the shell: show a task or a row in the IDE kept aside (`("ide", "task:<id>")`).
    request: Option<(String, String)>,
    sel: usize,
    detail: Option<(RunLog, Integrity)>,
    scroll: usize,
    confirm: bool,
    status: Option<(String, bool)>,
    /// Selecting and copying the text of the record.
    pane: crate::textsel::Pane,
    /// The Jobs list (detached jobs of every workspace), shown instead of the runs when `jobs_mode` is on.
    jobs: super::jobs_ui::JobsUi,
    jobs_mode: bool,
}

/// `YYYY-MM-DD HH:MM` (UTC) from unix seconds, without a date library.
pub fn fmt_time(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let t: String = flat.chars().take(max).collect();
    if flat.chars().count() > max { format!("{t}…") } else { t }
}

/// The timeline as (text, colour) lines.
pub fn timeline(log: &RunLog) -> Vec<(String, [u8; 4])> {
    let mut v: Vec<(String, [u8; 4])> = vec![(format!("You: {}", log.prompt), TEXT)];
    for e in &log.events {
        match e {
            RunEvent::Model { text, calls } => {
                // a turn without tool calls is the final answer, which is shown once, as "Answer"
                if !text.is_empty() && !calls.is_empty() {
                    v.push((format!("Assistant: {text}"), TEXT));
                }
                for c in calls {
                    v.push((format!("→ asks to use {}  {}", c.name, one_line(&c.args.to_string(), 200)), ACCENT));
                }
            }
            RunEvent::Approval { summary, detail, .. } => {
                v.push((format!("? waiting for approval: {summary}"), rgb(255, 196, 94)));
                // the exact change that was put in front of the user
                for l in detail.lines().take(12) {
                    let col = match l.chars().next() {
                        Some('+') => OK,
                        Some('-') => ERR,
                        _ => MUTED,
                    };
                    v.push((format!("      {l}"), col));
                }
            }
            RunEvent::Decision { name, approved, by, .. } => {
                let who = match by.as_str() {
                    "auto" => "automatically (safe tool)",
                    "user" => "by you",
                    "always" => "automatically (you chose always allow)",
                    "grant" => "automatically (you allowed it for 5 minutes)",
                    _ => "by the system (no answer in time)",
                };
                v.push((format!("  {} {name} {who}", if *approved { "✓ allowed" } else { "✗ denied" }), if *approved { OK } else { ERR }));
            }
            RunEvent::ToolResult { name, ok, text, .. } => v.push((format!("  {} {name}: {}", if *ok { "result" } else { "error" }, one_line(text, 400)), if *ok { MUTED } else { ERR })),
            RunEvent::Usage { .. } | RunEvent::Delta { .. } | RunEvent::Thinking { .. } | RunEvent::Budget { .. } => {}
            RunEvent::Note { text } => v.push((format!("  · {text}"), rgb(255, 196, 94))),
            RunEvent::Timing { first_token_ms, first_text_ms, total_ms, model_ms, tools_ms, overhead_ms, thinking_ms, .. } => {
                let s = |ms: &u64| format!("{:.1} s", *ms as f64 / 1000.0);
                let first = if *first_text_ms > 0 { first_text_ms } else { first_token_ms };
                v.push((format!("Time: first token {} · thinking {} · total {} · model {} · tools {} · IYAKKI {} ms", s(first), s(thinking_ms), s(total_ms), s(model_ms), s(tools_ms), overhead_ms), MUTED));
            }
            RunEvent::Final { text } => v.push((format!("Answer: {text}"), OK)),
            RunEvent::Failed { error } => v.push((format!("Failed: {error}"), ERR)),
            RunEvent::Stopped { reason } => v.push((format!("Stopped: {reason}"), ERR)),
        }
    }
    if let Some(RunEvent::Usage { tokens, cost_cents, files, seconds }) = log.events.iter().rev().find(|e| matches!(e, RunEvent::Usage { .. })) {
        v.push((format!("Used about {} tokens{} · {files} file{} written · {seconds} s", tokens, if *cost_cents > 0.0 { format!(" (~${:.2})", cost_cents / 100.0) } else { String::new() }, if *files == 1 { "" } else { "s" }), MUTED));
    }
    v
}

impl RunsApp {
    /// The Jobs list: every detached job of the person (any workspace), one row each with Log and Stop or Dismiss; the log of
    /// the chosen job below the list.
    fn render_jobs(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let fs = (15.0 * s).max(12.0);
        let r = R::new(area.x + 24.0 * s, area.y + 4.0 * s, area.w - 48.0 * s, area.h - 20.0 * s);
        self.jobs.poll(cx, false);
        if self.jobs.jobs().is_empty() {
            crate::fx::empty_state(g, area, s, Icon::Runs, "No detached jobs", "When the assistant or Claude Code starts a long command with job_start, it shows here with its progress, a Log and a Stop button.", ACCENT);
            return;
        }
        let rh = super::jobs_ui::JobsUi::row_height(fs, s) + 4.0 * s;
        let want = self.jobs.jobs().len() as f32 * rh;
        let list_h = if self.jobs.log_open() { want.min(r.h * 0.4).max(rh) } else { want.min(r.h) };
        self.jobs.draw_list(g, R::new(r.x, r.y, r.w, list_h), fs, s, tick);
        if self.jobs.log_open() {
            self.jobs.draw_log(g, R::new(r.x, r.y + list_h + 6.0 * s, r.w, (r.h - list_h - 6.0 * s).max(60.0 * s)), fs, s);
        } else if self.jobs.overflow > 0 {
            g.text(r.x, r.y + list_h + 4.0 * s, 13.0 * s, DIM, &format!("{} more not shown: dismiss finished jobs to make room", self.jobs.overflow), Face::Sans);
        }
    }

    pub fn new(cx: &AppCx) -> RunsApp {
        let mut a = RunsApp { runs: vec![], all: vec![], filter: None, request: None, sel: 0, detail: None, scroll: 0, confirm: false, status: None, pane: crate::textsel::Pane::new(61_200), jobs: super::jobs_ui::JobsUi::new(None), jobs_mode: false };
        a.reload(cx);
        a
    }

    pub fn runs(&self) -> &[RunMeta] {
        &self.runs
    }
    pub fn integrity(&self) -> Option<&Integrity> {
        self.detail.as_ref().map(|d| &d.1)
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }

    /// Open the Jobs list (the shell's link from a notification, and the `j` key).
    pub fn show_jobs(&mut self, cx: &AppCx) {
        self.jobs_mode = true;
        self.jobs.poll(cx, true);
    }

    pub fn jobs_mode(&self) -> bool {
        self.jobs_mode
    }

    pub fn jobs(&self) -> &super::jobs_ui::JobsUi {
        &self.jobs
    }

    fn reload(&mut self, cx: &AppCx) {
        self.all = cx.services.runs.list(&cx.session.user);
        self.apply_filter();
        self.sel = self.sel.min(self.runs.len().saturating_sub(1));
        self.open_selected(cx);
    }

    fn apply_filter(&mut self) {
        self.runs = match &self.filter {
            None => self.all.clone(),
            Some(f) => self.all.iter().filter(|m| m.agent.as_deref() == Some(f.as_str()) || m.task_id.as_deref() == Some(f.as_str())).cloned().collect(),
        };
    }

    /// The agents and tasks that have runs, for the filter to cycle through.
    fn filter_values(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for m in &self.all {
            for x in [&m.agent, &m.task_id].into_iter().flatten() {
                if !v.contains(x) {
                    v.push(x.clone());
                }
            }
        }
        v.sort();
        v
    }

    fn cycle_filter(&mut self, cx: &AppCx) {
        let vals = self.filter_values();
        self.filter = match &self.filter {
            None => vals.first().cloned(),
            Some(f) => vals.iter().position(|x| x == f).and_then(|i| vals.get(i + 1)).cloned(),
        };
        self.apply_filter();
        self.sel = 0;
        self.open_selected(cx);
    }

    fn open_selected(&mut self, cx: &AppCx) {
        self.scroll = 0;
        self.detail = self.runs.get(self.sel).and_then(|m| cx.services.runs.load(&cx.session.user, &m.id).ok());
        if let Some((_, Integrity::Tampered(why))) = &self.detail {
            cx.audit.record(&cx.session.user, "runs.tampered", &format!("{}: {why}", self.runs[self.sel].id));
        }
    }

    fn transcript(&self) -> String {
        self.detail.as_ref().map(|(l, _)| timeline(l).into_iter().map(|(t, _)| t).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    }
}

impl App for RunsApp {
    fn title(&self) -> &'static str {
        "Runs"
    }
    fn icon(&self) -> Icon {
        Icon::Runs
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.jobs_mode {
            "click Log or Stop on a job · wheel scrolls the log · Esc closes the log · j back to the runs".into()
        } else if self.confirm {
            "y delete · Esc cancel".into()
        } else {
            "click a run · c copy transcript · d delete · f filter by agent or task · g task · b row · PgUp/PgDn scroll · j detached jobs".into()
        }
    }

    fn wants_redraw(&mut self) -> bool {
        self.jobs_mode && self.jobs.frame_due()
    }

    /// A notification about a job opens the Jobs list.
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        if item == "jobs" {
            self.show_jobs(cx);
        } else if let Some(id) = item.strip_prefix("run:") {
            // a task's run, opened from the board's agent panel
            self.jobs_mode = false;
            self.filter = None;
            self.reload(cx);
            if let Some(i) = self.runs.iter().position(|m| m.id == id) {
                self.sel = i;
                self.open_selected(cx);
            } else {
                self.status = Some(("That run is not recorded (yet).".into(), false));
            }
        }
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        if self.jobs_mode {
            return self.jobs.context_items(x, y);
        }
        self.pane.context_items(x, y)
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match ev {
            Event::Tab(super::jobs_ui::TAB_JOBS) => {
                self.show_jobs(cx);
                return Flow::Stay;
            }
            Event::Tab(super::jobs_ui::TAB_RUNS) => {
                self.jobs_mode = false;
                self.jobs.close_log();
                return Flow::Stay;
            }
            Event::Key(Key::Char('j')) if !self.confirm => {
                if self.jobs_mode {
                    self.jobs_mode = false;
                    self.jobs.close_log();
                } else {
                    self.show_jobs(cx);
                }
                return Flow::Stay;
            }
            _ => {}
        }
        if self.jobs_mode {
            if matches!(ev, Event::Tick) {
                self.jobs.poll(cx, false);
            }
            if matches!(ev, Event::Key(Key::Ctrl('a'))) && self.jobs.log_open() {
                self.jobs.select_all();
                return Flow::Stay;
            }
            if self.jobs.handle(ev, cx) {
                return Flow::Stay;
            }
            return match ev {
                Event::Key(Key::Esc) => Flow::Back,
                _ => Flow::Stay,
            };
        }
        if !self.confirm {
            if self.pane.handle(ev) {
                return Flow::Stay;
            }
            match ev {
                Event::Key(Key::Ctrl('a')) => {
                    self.pane.select_all();
                    return Flow::Stay;
                }
                // a drag held above or below the record scrolls it
                Event::Tick if self.pane.auto_scroll() != 0.0 => {
                    let a = self.pane.auto_scroll();
                    self.scroll = (self.scroll as i32 - a.signum() as i32 * 2).max(0) as usize;
                    self.pane.refresh_drag();
                }
                _ => {}
            }
        }
        if self.confirm {
            if let Event::Key(k) = ev {
                if *k == Key::Char('y') {
                    if let Some(m) = self.runs.get(self.sel).cloned() {
                        match cx.services.runs.delete(&cx.session.user, &m.id) {
                            Ok(()) => {
                                cx.audit.record(&cx.session.user, "runs.delete", &m.id);
                                self.status = Some(("Run deleted.".into(), true));
                            }
                            Err(e) => self.status = Some((e, false)),
                        }
                        self.reload(cx);
                    }
                }
                self.confirm = false;
            }
            return Flow::Stay;
        }
        match ev {
            Event::Row(i) => {
                self.sel = (*i).min(self.runs.len().saturating_sub(1));
                self.open_selected(cx);
            }
            Event::Scroll(d) => self.scroll = (self.scroll as i32 - d * 3).max(0) as usize,
            Event::Key(k) => match k {
                Key::Esc => return Flow::Back,
                Key::Up => {
                    self.sel = self.sel.saturating_sub(1);
                    self.open_selected(cx);
                }
                Key::Down => {
                    self.sel = (self.sel + 1).min(self.runs.len().saturating_sub(1));
                    self.open_selected(cx);
                }
                Key::PageDown => self.scroll += 12,
                Key::PageUp => self.scroll = self.scroll.saturating_sub(12),
                Key::Char('r') => self.reload(cx),
                Key::Char('f') => self.cycle_filter(cx),
                Key::Char('g') | Key::Char('b') => {
                    // from a task's run back to its task or its row on the board
                    if let Some(m) = self.runs.get(self.sel) {
                        let item = match (k, &m.task_id, &m.row) {
                            (Key::Char('g'), Some(t), _) => Some(format!("task:{t}")),
                            (Key::Char('b'), _, Some(r)) => Some(format!("row:{r}")),
                            _ => None,
                        };
                        if let Some(i) = item {
                            self.request = Some(("ide".into(), i));
                        }
                    }
                }
                Key::Char('d') if !self.runs.is_empty() => self.confirm = true,
                Key::Char('c') if self.detail.is_some() => crate::ui::clipboard::copy(&self.transcript()),
                _ => {}
            },
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // the switch between the recorded runs and the detached jobs
        let th = 40.0 * s;
        {
            let mut x = area.x + 24.0 * s;
            let n = self.jobs.jobs().iter().filter(|j| j.running()).count();
            let jobs_label = if n > 0 { format!("Jobs ({n} running)") } else { "Jobs".to_string() };
            for (label, on, ev) in [("Runs", !self.jobs_mode, Event::Tab(super::jobs_ui::TAB_RUNS)), (jobs_label.as_str(), self.jobs_mode, Event::Tab(super::jobs_ui::TAB_JOBS))] {
                x += g.button(x, area.y + 4.0 * s, 14.0 * s, label, if on { ACCENT } else { MUTED }, ev) + 8.0 * s;
            }
        }
        let area = R::new(area.x, area.y + th, area.w, (area.h - th).max(10.0));
        if self.jobs_mode {
            self.render_jobs(g, area, s, cx, tick);
            return;
        }
        let x = area.x + 24.0 * s;
        let list_w = (area.w * 0.34).min(400.0 * s);
        let mut y = area.y + 8.0 * s;
        self.pane.begin(R::new(0.0, 0.0, 0.0, 0.0));
        if self.runs.is_empty() {
            crate::fx::empty_state(g, area, s, Icon::Runs, "No agent runs yet", "Turn on Agent mode in Chat (Ctrl+G) and ask for something.", ACCENT);
            return;
        }
        for (i, m) in self.runs.iter().enumerate() {
            if y > area.y + area.h - 130.0 * s {
                break;
            }
            let r = R::new(x, y, list_w, 66.0 * s);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &m.id);
            let p = g.fit(15.0 * s, &one_line(&m.prompt, 80), Face::Bold, list_w - 28.0 * s);
            g.text(x + 14.0 * s, y + 8.0 * s, 15.0 * s, TEXT, &p, Face::Bold);
            let col = match m.status.as_str() {
                "completed" => OK,
                "running" => ACCENT,
                _ => ERR,
            };
            g.text(x + 14.0 * s, y + 34.0 * s, 12.5 * s, MUTED, &format!("{} UTC · {} tool call{}", fmt_time(m.started), m.tool_calls, if m.tool_calls == 1 { "" } else { "s" }), Face::Sans);
            let tl = m.task_label();
            if !tl.is_empty() {
                let t = g.fit(11.5 * s, &tl, Face::Bold, list_w - 28.0 * s);
                g.text(x + 14.0 * s, y + 50.0 * s, 11.5 * s, crate::fx::readable(CYAN_TASK, crate::fx::PANEL, 4.5), &t, Face::Bold);
            }
            let w = g.text_width(12.0 * s, &m.status, Face::Bold) + 17.0 * s;
            crate::fx::chip(g, x + list_w - w - 10.0 * s, y + 29.0 * s, s, &m.status, col, i == self.sel);
            y += 74.0 * s;
        }
        // detail
        let dx = x + list_w + 24.0 * s;
        let dw = area.x + area.w - dx - 24.0 * s;
        if let Some((log, integrity)) = &self.detail {
            let (label, col) = match integrity {
                Integrity::Verified => ("✓ record verified (hash chain intact)".to_string(), OK),
                Integrity::Tampered(why) => (format!("⚠ NOT TRUSTWORTHY: {why}"), ERR),
            };
            let label = g.fit(14.0 * s, &label, Face::Bold, dw - 24.0 * s);
            crate::fx::panel(g, R::new(dx - 12.0 * s, area.y + 4.0 * s, dw + 24.0 * s, area.h - 108.0 * s), s, col);
            g.text(dx, area.y + 14.0 * s, 14.0 * s, col, &label, Face::Bold);
            let top = area.y + 44.0 * s;
            let bottom = area.y + area.h - 100.0 * s;
            let lh = g.line_height(14.5 * s, Face::Sans);
            let mut vis: Vec<([u8; 4], String)> = Vec::new();
            let mut entry_of: Vec<u32> = Vec::new();
            for (n, (t, c)) in timeline(log).into_iter().enumerate() {
                for l in g.wrap(14.5 * s, &t, Face::Sans, dw) {
                    vis.push((c, l));
                    entry_of.push(n as u32);
                }
            }
            let fit = ((bottom - top) / lh) as usize;
            self.scroll = self.scroll.min(vis.len().saturating_sub(fit));
            for (i, (c, l)) in vis.iter().skip(self.scroll).take(fit).enumerate() {
                g.text(dx, top + i as f32 * lh, 14.5 * s, *c, l, Face::Sans);
            }
            self.pane.begin(R::new(dx, top, dw, bottom - top));
            for (i, (_, l)) in vis.iter().enumerate() {
                self.pane.line(g, dx, top + (i as f32 - self.scroll as f32) * lh, lh, 14.5 * s, Face::Sans, l, 0, entry_of[i]);
            }
            self.pane.end(g, ACCENT);
            if vis.len() > fit {
                g.text(dx, bottom + 4.0 * s, 12.0 * s, DIM, &format!("{}–{} of {} lines", self.scroll + 1, (self.scroll + fit).min(vis.len()), vis.len()), Face::Sans);
            }
        }
        let by = area.y + area.h - 78.0 * s;
        let mut bx = x;
        let flabel = format!("Filter: {}", self.filter.clone().unwrap_or_else(|| "all".into()));
        let sel_run = self.runs.get(self.sel).cloned();
        let mut btns: Vec<(String, [u8; 4], char)> = vec![];
        if let Some(m) = &sel_run {
            if m.task_id.is_some() {
                btns.push(("Show task".into(), CYAN_TASK, 'g'));
            }
            if m.row.is_some() {
                btns.push(("Show row".into(), CYAN_TASK, 'b'));
            }
        }
        btns.extend([("Copy transcript".to_string(), ACCENT, 'c'), ("Delete run".to_string(), ERR, 'd'), ("Refresh".to_string(), MUTED, 'r'), (flabel, MUTED, 'f')]);
        for (label, col, key) in btns {
            let w = g.text_width(14.0 * s, &label, Face::Bold) + 14.0 * s * 1.7;
            if bx + w > area.x + area.w - 12.0 * s && bx > x {
                break;
            }
            bx += g.button(bx, by, 14.0 * s, &label, col, Event::Key(Key::Char(key))) + 10.0 * s;
        }
        if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, area.y + area.h - 30.0 * s, area.w - 48.0 * s, s, m, *ok);
        }
        if self.confirm {
            render_confirm(g, area, s, "Delete this run and its record?", "Delete");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_agent::agent::CallInfo;
    use serde_json::json;

    #[test]
    fn formats_dates() {
        assert_eq!(fmt_time(0), "1970-01-01 00:00");
        assert_eq!(fmt_time(1_000_000_000), "2001-09-09 01:46");
        assert_eq!(fmt_time(1_791_335_444), "2026-10-07 01:10");
        assert_eq!(fmt_time(951_782_400 + 86_400 * 59 + 3600 * 13 + 60 * 7), "2000-04-28 13:07", "leap year handled (2000-02-29 exists)");
    }

    fn log(user: &str, id: &str, started: u64) -> RunLog {
        RunLog {
            id: id.into(),
            session: None,
            task_id: None,
            row: None,
            agent: None,
            user: user.into(),
            started,
            prompt: "tidy my notes".into(),
            status: "completed".into(),
            events: vec![
                RunEvent::Model { text: "Reading.".into(), calls: vec![CallInfo { id: "c1".into(), name: "read_file".into(), args: json!({"path": "n.txt"}) }] },
                RunEvent::Decision { id: "c1".into(), name: "read_file".into(), approved: true, by: "auto".into() },
                RunEvent::ToolResult { id: "c1".into(), name: "read_file".into(), ok: true, text: "line one\nline two".into() },
                RunEvent::Approval { id: "c2".into(), name: "write_file".into(), summary: "write 3 characters to /home/amy/n.txt".into(), detail: "@@ line 1 @@\n-old\n+new".into() },
                RunEvent::Decision { id: "c2".into(), name: "write_file".into(), approved: false, by: "user".into() },
                RunEvent::Final { text: "I left the file alone.".into() },
            ],
        }
    }

    #[test]
    fn timeline_is_readable_and_honest_about_decisions() {
        let t: Vec<String> = timeline(&log("amy", "aaaaaaaaaaaa", 1)).into_iter().map(|(t, _)| t).collect();
        assert_eq!(t[0], "You: tidy my notes");
        assert!(t.iter().any(|l| l.starts_with("→ asks to use read_file")));
        assert!(t.contains(&"  ✓ allowed read_file automatically (safe tool)".to_string()));
        assert!(t.contains(&"  ✗ denied write_file by you".to_string()));
        assert!(t.iter().any(|l| l.contains("waiting for approval: write 3 characters to /home/amy/n.txt")));
        assert!(t.iter().any(|l| l.contains("result read_file: line one line two")), "{t:?}");
        assert_eq!(t.last().unwrap(), "Answer: I left the file alone.");
    }

    #[test]
    fn lists_views_verifies_copies_and_deletes_only_my_runs() {
        let mut cx = cx_with_admin_store("runs1");
        cx.session.user = "amy".into();
        let store = &cx.services.runs;
        store.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
        store.save(&log("amy", "bbbbbbbbbbbb", 200)).unwrap();
        store.save(&log("bob", "cccccccccccc", 300)).unwrap();
        crate::ui::clipboard::clear();
        let mut app = RunsApp::new(&cx);
        assert_eq!(app.runs().len(), 2, "bob's run is not visible");
        assert_eq!(app.runs()[0].id, "bbbbbbbbbbbb");
        assert_eq!(app.integrity(), Some(&Integrity::Verified));
        app.handle(&Event::Row(1), &cx);
        assert_eq!(app.detail.as_ref().unwrap().0.id, "aaaaaaaaaaaa");
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
        for want in ["Copy transcript", "Delete run", "Refresh", "aaaaaaaaaaaa", "bbbbbbbbbbbb"] {
            assert!(g.hits.iter().any(|h| h.label == want), "{want}");
        }
        app.handle(&Event::Key(Key::Char('c')), &cx);
        assert!(crate::ui::clipboard::latest().unwrap().contains("tidy my notes"));
        // delete needs confirmation
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('n')), &cx);
        assert_eq!(app.runs().len(), 2);
        app.handle(&Event::Key(Key::Char('d')), &cx);
        app.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(app.runs().len(), 1);
        assert_eq!(cx.services.runs.list("bob").len(), 1, "bob's run untouched");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("runs.delete")));
    }

    #[test]
    fn a_tampered_record_is_flagged_loudly_and_audited() {
        let mut cx = cx_with_admin_store("runs2");
        cx.session.user = "amy".into();
        cx.services.runs.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
        let path = cx.services.dir().join("runs/amy/aaaaaaaaaaaa.json");
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("line one", "send all money")).unwrap();
        let mut app = RunsApp::new(&cx);
        assert!(matches!(app.integrity(), Some(Integrity::Tampered(_))));
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("runs.tampered")));
        let mut g = Gfx::new(1280, 700);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 1280.0, 628.0), 1.0, &cx, 0);
    }

    #[test]
    fn empty_state() {
        let cx = cx_with_admin_store("runs3");
        let mut app = RunsApp::new(&cx);
        assert!(app.runs().is_empty() && app.integrity().is_none());
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        app.render(&mut g, R::new(0.0, 72.0, 900.0, 528.0), 1.0, &cx, 0);
        assert!(matches!(app.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    fn task_log(user: &str, id: &str, started: u64, task: &str, agent: &str, row: &str) -> RunLog {
        RunLog { task_id: Some(task.into()), agent: Some(agent.into()), row: Some(row.into()), ..log(user, id, started) }
    }

    #[test]
    fn a_tasks_runs_show_its_task_agent_and_row_filter_open_by_item_and_link_back() {
        let mut cx = cx_with_admin_store("runs-task");
        cx.session.user = "amy".into();
        let store = &cx.services.runs;
        store.save(&log("amy", "aaaaaaaaaaaa", 100)).unwrap();
        store.save(&task_log("amy", "bbbbbbbbbbbb", 200, "t1", "Agent 1", "F-APP-01")).unwrap();
        store.save(&task_log("amy", "cccccccccccc", 300, "t2", "Agent 2", "F-APP-02")).unwrap();
        let mut app = RunsApp::new(&cx);
        assert_eq!(app.runs().iter().map(|m| m.task_label()).collect::<Vec<_>>(), ["Task t2 · Agent 2 · F-APP-02", "Task t1 · Agent 1 · F-APP-01", ""]);
        // the hand-off from the board: the item selects that run (whatever filter was on)
        app.filter = Some("Agent 2".into());
        app.open_item("run:bbbbbbbbbbbb", &cx);
        assert_eq!(app.filter, None);
        assert_eq!(app.detail.as_ref().unwrap().0.id, "bbbbbbbbbbbb");
        app.open_item("run:nope", &cx);
        assert!(app.status.as_ref().is_some_and(|s| !s.1));
        // the filter cycles agents, then tasks, then all
        app.handle(&Event::Key(Key::Char('f')), &cx);
        assert_eq!((app.filter.clone(), app.runs().len()), (Some("Agent 1".into()), 1));
        app.handle(&Event::Key(Key::Char('f')), &cx);
        assert_eq!(app.filter.as_deref(), Some("Agent 2"));
        app.handle(&Event::Key(Key::Char('f')), &cx);
        assert_eq!(app.filter.as_deref(), Some("t1"));
        for _ in 0..2 {
            app.handle(&Event::Key(Key::Char('f')), &cx);
        }
        assert_eq!((app.filter.clone(), app.runs().len()), (None, 3));
        // from a task's run back to its task and its row; a run that is not a task's has no such buttons
        app.open_item("run:cccccccccccc", &cx);
        app.handle(&Event::Key(Key::Char('g')), &cx);
        assert_eq!(app.take_request(), Some(("ide".into(), "task:t2".into())));
        app.handle(&Event::Key(Key::Char('b')), &cx);
        assert_eq!(app.take_request(), Some(("ide".into(), "row:F-APP-02".into())));
        app.open_item("run:aaaaaaaaaaaa", &cx);
        app.handle(&Event::Key(Key::Char('g')), &cx);
        assert_eq!(app.take_request(), None);
    }

    #[test]
    fn the_runs_screen_with_task_runs_fits_the_four_sizes_and_offers_the_links() {
        let mut cx = cx_with_admin_store("runs-geom");
        cx.session.user = "amy".into();
        for i in 0..6 {
            cx.services.runs.save(&task_log("amy", &format!("{i:012x}"), 100 + i as u64, &format!("t{i}"), &format!("Agent {}", i % 3 + 1), "F-APP-01")).unwrap();
        }
        let mut app = RunsApp::new(&cx);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            let mut g = Gfx::new(w, h);
            g.clear(rgb(0, 0, 0));
            app.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, &cx, 0);
            for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                assert!(hit.rect.x >= -0.5 && hit.rect.y >= -0.5 && hit.rect.x + hit.rect.w <= w as f32 + 0.5 && hit.rect.y + hit.rect.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, hit.rect);
            }
            for want in ["Show task", "Show row"] {
                assert!(g.hits.iter().any(|x| x.label == want || x.label.starts_with(want)), "{w}x{h}: {want}");
            }
        }
    }
}

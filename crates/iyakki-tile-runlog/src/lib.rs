//! RUNS: what your agents did. Every agent run is recorded; here you can read each one step by step
//! (what the model asked for, what you approved or denied, what each tool returned) and see whether
//! the record is intact — the transcript is checked against its hash-chained ledger when opened.
//!
//! The tile lives in this crate (it does not depend on the agent or the shared crate): the run store and the detached jobs come through the
//! [`RunsHost`] the shell hands over (`RunsPort` and `JobsPort` of `iyakki-tile-api::runs`). The Jobs strip and list the chat shares are
//! in [`jobs`].

pub mod jobs;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod jobs_tests;

use iyakki_tile_api::runs::{Integrity, JobsPort, RunEvent, RunLog, RunMeta, RunsPort};
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::textsel::Pane;
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::render_confirm;

/// What the shell hands the tile: the person's run store and the person's detached jobs.
pub trait RunsHost: RunsPort + JobsPort {}
impl<T: RunsPort + JobsPort + ?Sized> RunsHost for T {}

/// What the tile answers to an event: stay on the screen, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = rgb(255, 196, 94);
const CYAN_TASK: [u8; 4] = rgb(100, 230, 255);

pub struct RunsTile {
    pub runs: Vec<RunMeta>,
    /// Every run, before the filter.
    pub all: Vec<RunMeta>,
    /// Only the runs whose agent (`Agent 2`) or task id is this (key f cycles).
    pub filter: Option<String>,
    /// Asked of the shell: show a task or a row in the IDE kept aside (`("ide", "task:<id>")`).
    pub request: Option<(String, String)>,
    pub sel: usize,
    pub detail: Option<(RunLog, Integrity)>,
    pub scroll: usize,
    pub confirm: bool,
    pub status: Option<(String, bool)>,
    /// Selecting and copying the text of the record.
    pane: Pane,
    /// The Jobs list (detached jobs of every workspace), shown instead of the runs when `jobs_mode` is on.
    jobs: jobs::JobsUi,
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
                    v.push((format!("→ asks to use {}  {}", c.name, one_line(&c.args_text, 200)), ACCENT));
                }
            }
            RunEvent::Approval { summary, detail } => {
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
            RunEvent::Decision { name, approved, by } => {
                let who = match by.as_str() {
                    "auto" => "automatically (safe tool)",
                    "user" => "by you",
                    "always" => "automatically (you chose always allow)",
                    "grant" => "automatically (you allowed it for 5 minutes)",
                    _ => "by the system (no answer in time)",
                };
                v.push((format!("  {} {name} {who}", if *approved { "✓ allowed" } else { "✗ denied" }), if *approved { OK } else { ERR }));
            }
            RunEvent::ToolResult { name, ok, text } => v.push((format!("  {} {name}: {}", if *ok { "result" } else { "error" }, one_line(text, 400)), if *ok { MUTED } else { ERR })),
            RunEvent::Usage { .. } | RunEvent::Hidden => {}
            RunEvent::Note { text } => v.push((format!("  · {text}"), rgb(255, 196, 94))),
            RunEvent::Timing { first_token_ms, first_text_ms, total_ms, model_ms, tools_ms, overhead_ms, thinking_ms } => {
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

impl RunsTile {
    /// The Jobs list: every detached job of the person (any workspace), one row each with Log and Stop or Dismiss; the log of
    /// the chosen job below the list.
    fn render_jobs(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn JobsPort, tick: u64) {
        let fs = (15.0 * s).max(12.0);
        let r = R::new(area.x + 24.0 * s, area.y + 4.0 * s, area.w - 48.0 * s, area.h - 20.0 * s);
        self.jobs.poll(port, false);
        if self.jobs.jobs().is_empty() {
            iyakki_uikit::fx::empty_state(g, area, s, Icon::Runs, "No detached jobs", "When the assistant or Claude Code starts a long command with job_start, it shows here with its progress, a Log and a Stop button.", ACCENT);
            return;
        }
        let rh = jobs::JobsUi::row_height(fs, s) + 4.0 * s;
        let want = self.jobs.jobs().len() as f32 * rh;
        let list_h = if self.jobs.log_open() { want.min(r.h * 0.4).max(rh) } else { want.min(r.h) };
        self.jobs.draw_list(g, R::new(r.x, r.y, r.w, list_h), fs, s, tick);
        if self.jobs.log_open() {
            self.jobs.draw_log(g, R::new(r.x, r.y + list_h + 6.0 * s, r.w, (r.h - list_h - 6.0 * s).max(60.0 * s)), fs, s);
        } else if self.jobs.overflow > 0 {
            g.text(r.x, r.y + list_h + 4.0 * s, 13.0 * s, DIM, &format!("{} more not shown: dismiss finished jobs to make room", self.jobs.overflow), Face::Sans);
        }
    }

    /// The tile over a port (a fake in tests).
    pub fn new(port: &dyn RunsPort) -> RunsTile {
        let mut a = RunsTile { runs: vec![], all: vec![], filter: None, request: None, sel: 0, detail: None, scroll: 0, confirm: false, status: None, pane: Pane::new(61_200), jobs: jobs::JobsUi::new(None), jobs_mode: false };
        a.reload(port);
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
    pub fn show_jobs(&mut self, port: &dyn JobsPort) {
        self.jobs_mode = true;
        self.jobs.poll(port, true);
    }

    pub fn jobs_mode(&self) -> bool {
        self.jobs_mode
    }

    pub fn jobs(&self) -> &jobs::JobsUi {
        &self.jobs
    }

    pub fn jobs_mut(&mut self) -> &mut jobs::JobsUi {
        &mut self.jobs
    }

    pub fn reload(&mut self, port: &dyn RunsPort) {
        self.all = port.list();
        self.apply_filter();
        self.sel = self.sel.min(self.runs.len().saturating_sub(1));
        self.open_selected(port);
    }

    pub fn apply_filter(&mut self) {
        self.runs = match &self.filter {
            None => self.all.clone(),
            Some(f) => self.all.iter().filter(|m| m.agent.as_deref() == Some(f.as_str()) || m.task_id.as_deref() == Some(f.as_str())).cloned().collect(),
        };
    }

    /// The agents and tasks that have runs, for the filter to cycle through.
    pub fn filter_values(&self) -> Vec<String> {
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

    pub fn cycle_filter(&mut self, port: &dyn RunsPort) {
        let vals = self.filter_values();
        self.filter = match &self.filter {
            None => vals.first().cloned(),
            Some(f) => vals.iter().position(|x| x == f).and_then(|i| vals.get(i + 1)).cloned(),
        };
        self.apply_filter();
        self.sel = 0;
        self.open_selected(port);
    }

    pub fn open_selected(&mut self, port: &dyn RunsPort) {
        self.scroll = 0;
        self.detail = self.runs.get(self.sel).and_then(|m| port.load(&m.id));
        if let Some((_, Integrity::Tampered(why))) = &self.detail {
            port.audit("runs.tampered", &format!("{}: {why}", self.runs[self.sel].id));
        }
    }

    /// Delete the chosen run (the person confirmed), say so, and read the list again.
    pub fn delete_selected(&mut self, port: &dyn RunsPort) {
        if let Some(m) = self.runs.get(self.sel).cloned() {
            match port.delete(&m.id) {
                Ok(()) => {
                    port.audit("runs.delete", &m.id);
                    self.status = Some(("Run deleted.".into(), true));
                }
                Err(e) => self.status = Some((e, false)),
            }
            self.reload(port);
        }
    }

    pub fn transcript(&self) -> String {
        self.detail.as_ref().map(|(l, _)| timeline(l).into_iter().map(|(t, _)| t).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    }
}

impl RunsTile {
    pub fn title(&self) -> &'static str {
        "Runs"
    }
    pub fn icon(&self) -> Icon {
        Icon::Runs
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        if self.jobs_mode {
            "click Log or Stop on a job · wheel scrolls the log · Esc closes the log · j back to the runs".into()
        } else if self.confirm {
            "y delete · Esc cancel".into()
        } else {
            "click a run · c copy transcript · d delete · f filter by agent or task · g task · b row · PgUp/PgDn scroll · j detached jobs".into()
        }
    }

    pub fn wants_redraw(&mut self) -> bool {
        self.jobs_mode && self.jobs.frame_due()
    }

    /// A notification about a job opens the Jobs list.
    pub fn open_item(&mut self, item: &str, host: &dyn RunsHost) {
        if item == "jobs" {
            self.show_jobs(host);
        } else if let Some(id) = item.strip_prefix("run:") {
            // a task's run, opened from the board's agent panel
            self.jobs_mode = false;
            self.filter = None;
            self.reload(host);
            if let Some(i) = self.runs.iter().position(|m| m.id == id) {
                self.sel = i;
                self.open_selected(host);
            } else {
                self.status = Some(("That run is not recorded (yet).".into(), false));
            }
        }
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    pub fn context_items(&mut self, x: i32, y: i32) -> Vec<iyakki_uikit::ctxmenu::Item> {
        if self.jobs_mode {
            return self.jobs.context_items(x, y);
        }
        self.pane.context_items(x, y)
    }

    pub fn handle(&mut self, ev: &Event, host: &dyn RunsHost) -> Answer {
        let port: &dyn RunsPort = host;
        match ev {
            Event::Tab(jobs::TAB_JOBS) => {
                self.show_jobs(host);
                return Answer::Stay;
            }
            Event::Tab(jobs::TAB_RUNS) => {
                self.jobs_mode = false;
                self.jobs.close_log();
                return Answer::Stay;
            }
            Event::Key(Key::Char('j')) if !self.confirm => {
                if self.jobs_mode {
                    self.jobs_mode = false;
                    self.jobs.close_log();
                } else {
                    self.show_jobs(host);
                }
                return Answer::Stay;
            }
            _ => {}
        }
        if self.jobs_mode {
            if matches!(ev, Event::Tick) {
                self.jobs.poll(host, false);
            }
            if matches!(ev, Event::Key(Key::Ctrl('a'))) && self.jobs.log_open() {
                self.jobs.select_all();
                return Answer::Stay;
            }
            if self.jobs.handle(ev, host) {
                return Answer::Stay;
            }
            return match ev {
                Event::Key(Key::Esc) => Answer::Back,
                _ => Answer::Stay,
            };
        }
        if !self.confirm {
            if self.pane.handle(ev) {
                return Answer::Stay;
            }
            match ev {
                Event::Key(Key::Ctrl('a')) => {
                    self.pane.select_all();
                    return Answer::Stay;
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
                    self.delete_selected(port);
                }
                self.confirm = false;
            }
            return Answer::Stay;
        }
        match ev {
            Event::Row(i) => {
                self.sel = (*i).min(self.runs.len().saturating_sub(1));
                self.open_selected(port);
            }
            Event::Scroll(d) => self.scroll = (self.scroll as i32 - d * 3).max(0) as usize,
            Event::Key(k) => match k {
                Key::Esc => return Answer::Back,
                Key::Up => {
                    self.sel = self.sel.saturating_sub(1);
                    self.open_selected(port);
                }
                Key::Down => {
                    self.sel = (self.sel + 1).min(self.runs.len().saturating_sub(1));
                    self.open_selected(port);
                }
                Key::PageDown => self.scroll += 12,
                Key::PageUp => self.scroll = self.scroll.saturating_sub(12),
                Key::Char('r') => self.reload(port),
                Key::Char('f') => self.cycle_filter(port),
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
                Key::Char('c') if self.detail.is_some() => iyakki_uikit::ui::clipboard::copy(&self.transcript()),
                _ => {}
            },
            _ => {}
        }
        Answer::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn JobsPort, tick: u64) {
        // the switch between the recorded runs and the detached jobs
        let th = 40.0 * s;
        {
            let mut x = area.x + 24.0 * s;
            let n = self.jobs.jobs().iter().filter(|j| j.running()).count();
            let jobs_label = if n > 0 { format!("Jobs ({n} running)") } else { "Jobs".to_string() };
            for (label, on, ev) in [("Runs", !self.jobs_mode, Event::Tab(jobs::TAB_RUNS)), (jobs_label.as_str(), self.jobs_mode, Event::Tab(jobs::TAB_JOBS))] {
                x += g.button(x, area.y + 4.0 * s, 14.0 * s, label, if on { ACCENT } else { MUTED }, ev) + 8.0 * s;
            }
        }
        let area = R::new(area.x, area.y + th, area.w, (area.h - th).max(10.0));
        if self.jobs_mode {
            self.render_jobs(g, area, s, port, tick);
            return;
        }
        let x = area.x + 24.0 * s;
        let list_w = (area.w * 0.34).min(400.0 * s);
        let mut y = area.y + 8.0 * s;
        self.pane.begin(R::new(0.0, 0.0, 0.0, 0.0));
        if self.runs.is_empty() {
            iyakki_uikit::fx::empty_state(g, area, s, Icon::Runs, "No agent runs yet", "Turn on Agent mode in Chat (Ctrl+G) and ask for something.", ACCENT);
            return;
        }
        for (i, m) in self.runs.iter().enumerate() {
            if y > area.y + area.h - 130.0 * s {
                break;
            }
            let r = R::new(x, y, list_w, 66.0 * s);
            iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel, g.hovered(r), ACCENT);
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
                g.text(x + 14.0 * s, y + 50.0 * s, 11.5 * s, iyakki_uikit::fx::readable(CYAN_TASK, iyakki_uikit::fx::PANEL, 4.5), &t, Face::Bold);
            }
            let w = g.text_width(12.0 * s, &m.status, Face::Bold) + 17.0 * s;
            iyakki_uikit::fx::chip(g, x + list_w - w - 10.0 * s, y + 29.0 * s, s, &m.status, col, i == self.sel);
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
            iyakki_uikit::fx::panel(g, R::new(dx - 12.0 * s, area.y + 4.0 * s, dw + 24.0 * s, area.h - 108.0 * s), s, col);
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
            iyakki_uikit::fx::status_line(g, x, area.y + area.h - 30.0 * s, area.w - 48.0 * s, s, m, *ok);
        }
        if self.confirm {
            render_confirm(g, area, s, "Delete this run and its record?", "Delete");
        }
    }
}


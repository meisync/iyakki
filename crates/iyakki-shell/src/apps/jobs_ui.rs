//! The Jobs strip and the job log view (see `docs/JOBS.md`): what the person sees of the detached jobs the assistant (or an
//! external coding agent) started. One row per job: a spinner or a check or a cross, the label, how long it has run, a bar
//! (a sliding one while the output gives no percentage, a filled one when it does), the last line of output and the buttons
//! Log and Stop (Dismiss once it ended). The same drawing serves the chat, the AI IDE's chat pane and the Jobs list in RUNS.
//!
//! Nothing here runs a job: the engine is `iyakki_agent::jobs`. This module reads it about once a second while there is
//! something to show, and says it is animating only while a job runs, so an idle shell stays idle.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use iyakki_agent::jobs::{fmt_duration, now_ms, Job, State};

use super::{AppCx, ScrollLog};
use crate::fx::{with_alpha, CYAN};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// Click ids of the rows: `BASE + 4 * row` opens the log, `+ 1` is Stop (or Dismiss when the job ended).
pub const BASE: usize = 400_000;
pub const LOG_CLOSE: usize = BASE + 900;
pub const LOG_FOLLOW: usize = BASE + 901;
/// The Runs / Jobs switch in the RUNS tile.
pub const TAB_RUNS: usize = BASE + 950;
pub const TAB_JOBS: usize = BASE + 951;

/// Most rows the strip shows at once (the rest are counted, and all are in RUNS → Jobs).
pub const MAX_ROWS: usize = 3;
const MIN_FONT: f32 = 12.0;
/// How much of a log the view keeps.
const LOG_BYTES: usize = 256 * 1024;

pub fn ev_log(i: usize) -> Event {
    Event::Tab(BASE + 4 * i)
}

pub fn ev_act(i: usize) -> Event {
    Event::Tab(BASE + 4 * i + 1)
}

/// Where one row's parts were drawn the last time (for the tests and the hit checks).
#[derive(Clone, Debug, PartialEq)]
pub struct RowGeom {
    pub id: String,
    pub rect: R,
    pub log_btn: R,
    pub act_btn: R,
    pub bar: R,
}

struct OpenLog {
    id: String,
    label: String,
    state: State,
    log: ScrollLog,
    seen_len: u64,
    shown_lines: usize,
}

pub struct JobsUi {
    /// The workspace whose jobs are shown (`""` outside the IDE); `None` shows all of the person's jobs (the RUNS list).
    ws: Option<String>,
    jobs: Vec<Job>,
    polled: Option<Instant>,
    open: Option<OpenLog>,
    stopping: HashSet<String>,
    /// Lines for jobs that finished since the last look: the chat shows them as events.
    events: Vec<String>,
    pub geoms: Vec<RowGeom>,
    pub overflow: usize,
    /// What is drawn changed since the last frame was asked for (a job ended, the log grew).
    dirty: bool,
    frame: u64,
    /// The chat has a run going (a job may be started any moment): look more often.
    hurry: bool,
}

impl JobsUi {
    pub fn new(ws: Option<&str>) -> JobsUi {
        JobsUi { ws: ws.map(str::to_string), jobs: Vec::new(), polled: None, open: None, stopping: HashSet::new(), events: Vec::new(), geoms: Vec::new(), overflow: 0, dirty: false, frame: 0, hurry: false }
    }

    pub fn set_workspace(&mut self, ws: &str) {
        if self.ws.as_deref() != Some(ws) {
            self.ws = Some(ws.to_string());
            self.jobs.clear();
            self.polled = None;
            self.open = None;
        }
    }

    /// A run is going in this chat: a job may start any moment, so an empty strip looks again sooner.
    pub fn set_hurry(&mut self, on: bool) {
        self.hurry = on;
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    /// A job is running, so the strip animates: the shell redraws about ten times a second. False otherwise (nothing moves).
    pub fn animating(&self) -> bool {
        self.jobs.iter().any(Job::running)
    }

    pub fn log_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn open_id(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.id.as_str())
    }

    /// Lines to show in the transcript for jobs that finished (each once).
    pub fn take_events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events)
    }

    /// Read the jobs again (at most once a second, every few seconds while there are none). `force` reads now.
    /// True when something that is drawn changed.
    pub fn poll(&mut self, cx: &AppCx, force: bool) -> bool {
        let every = if self.hurry && self.jobs.is_empty() {
            Duration::from_millis(400)
        } else if self.jobs.is_empty() && self.open.is_none() {
            Duration::from_secs(3)
        } else {
            Duration::from_millis(1000)
        };
        if !force && self.polled.is_some_and(|t| t.elapsed() < every) {
            return false;
        }
        self.polled = Some(Instant::now());
        let user = &cx.session.user;
        // the events first, then the list: a job whose end is announced is never drawn as still running
        if let Some(w) = &self.ws {
            for j in cx.services.jobs.take_finished(user, w, iyakki_agent::jobs::Channel::Ui) {
                self.events.push(iyakki_agent::jobs::Jobs::finish_line(&j));
            }
        }
        let mut list = match &self.ws {
            Some(w) => cx.services.jobs.visible(user, w),
            None => cx.services.jobs.visible_all(user),
        };
        // the running ones first, each group newest first (the engine lists newest first)
        list.sort_by_key(|j| !j.running());
        let changed = list != self.jobs;
        self.jobs = list;
        self.stopping.retain(|id| self.jobs.iter().any(|j| &j.id == id && j.running()));
        let logged = self.refresh_log(cx);
        self.dirty |= changed || logged;
        changed || logged
    }

    /// Asked on every heartbeat (about 30 a second) by the screen that shows the strip: true when a new frame is needed. About
    /// ten a second while a job runs (the spinner and the sliding bar), once when something changed (a job ended: the check
    /// or cross must appear), and never otherwise: an idle shell draws nothing here.
    pub fn frame_due(&mut self) -> bool {
        if std::mem::take(&mut self.dirty) {
            return true;
        }
        if self.animating() {
            self.frame += 1;
            self.frame % 3 == 0
        } else {
            self.frame = 0;
            false
        }
    }

    fn refresh_log(&mut self, cx: &AppCx) -> bool {
        let Some(o) = self.open.as_mut() else { return false };
        let len = cx.services.jobs.log_len(&cx.session.user, &o.id);
        let state = self.jobs.iter().find(|j| j.id == o.id).map(|j| j.state.clone()).unwrap_or(o.state.clone());
        if len == o.seen_len && state == o.state {
            return false;
        }
        let Ok(text) = cx.services.jobs.log_text(&cx.session.user, &o.id, LOG_BYTES) else { return false };
        o.seen_len = len;
        o.state = state;
        let lines: Vec<(String, [u8; 4])> = text.lines().map(|l| (l.to_string(), [0, 0, 0, 0])).collect();
        // reading older output (scrolled up) keeps its place while new lines arrive at the bottom
        if o.log.scroll > 0 {
            o.log.scroll += lines.len().saturating_sub(o.shown_lines);
        }
        o.shown_lines = lines.len();
        o.log.lines = lines;
        true
    }

    pub fn open_log(&mut self, cx: &AppCx, id: &str) {
        let Some(j) = self.jobs.iter().find(|j| j.id == id) else { return };
        self.open = Some(OpenLog { id: j.id.clone(), label: j.label.clone(), state: j.state.clone(), log: ScrollLog::default(), seen_len: u64::MAX, shown_lines: 0 });
        self.refresh_log(cx);
    }

    pub fn close_log(&mut self) {
        self.open = None;
    }

    /// The log text as a person would copy it (tests, copy all).
    pub fn log_text(&self) -> String {
        self.open.as_ref().map(|o| o.log.lines.iter().map(|l| l.0.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    }

    pub fn log_following(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.log.scroll == 0)
    }

    fn stop_or_dismiss(&mut self, cx: &AppCx, i: usize) {
        let Some(j) = self.jobs.get(i).cloned() else { return };
        let user = cx.session.user.clone();
        if j.running() {
            if !self.stopping.insert(j.id.clone()) {
                return;
            }
            cx.audit.record(&user, "job.stop", &format!("{} {}", j.id, j.label));
            let jobs = cx.services.jobs.clone();
            // TERM, a pause, then KILL: not on the drawing thread
            std::thread::spawn(move || {
                let _ = jobs.stop(&user, &j.id, iyakki_agent::jobs::STOP_GRACE);
            });
        } else {
            let _ = cx.services.jobs.dismiss(&user, &j.id);
            if self.open.as_ref().is_some_and(|o| o.id == j.id) {
                self.open = None;
            }
            self.poll(cx, true);
        }
    }

    /// A click on the strip, or a key or the wheel over the log view. True when the event was used.
    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> bool {
        match ev {
            Event::Tab(n) if (BASE..BASE + 900).contains(n) => {
                let (row, what) = ((n - BASE) / 4, (n - BASE) % 4);
                match what {
                    0 => {
                        if let Some(id) = self.jobs.get(row).map(|j| j.id.clone()) {
                            if self.open_id() == Some(id.as_str()) {
                                self.open = None;
                            } else {
                                self.open_log(cx, &id);
                            }
                        }
                    }
                    1 => self.stop_or_dismiss(cx, row),
                    _ => return false,
                }
                true
            }
            Event::Tab(LOG_CLOSE) => {
                self.open = None;
                true
            }
            Event::Tab(LOG_FOLLOW) => {
                if let Some(o) = self.open.as_mut() {
                    o.log.scroll = 0;
                }
                true
            }
            _ if self.open.is_none() => false,
            Event::Key(Key::Esc) => {
                self.open = None;
                true
            }
            Event::Scroll(d) => {
                if let Some(o) = self.open.as_mut() {
                    o.log.scroll_by(*d * 3);
                }
                true
            }
            Event::Key(Key::PageUp) => {
                if let Some(o) = self.open.as_mut() {
                    o.log.scroll_by(12);
                }
                true
            }
            Event::Key(Key::PageDown) => {
                if let Some(o) = self.open.as_mut() {
                    o.log.scroll_by(-12);
                }
                true
            }
            Event::Key(Key::End) => {
                if let Some(o) = self.open.as_mut() {
                    o.log.scroll = 0;
                }
                true
            }
            Event::Tick => {
                if let Some(o) = self.open.as_mut() {
                    o.log.on_tick();
                }
                false
            }
            // presses, drags and Ctrl+C on the log text
            _ => self.open.as_mut().is_some_and(|o| o.log.handle(ev)),
        }
    }

    /// Copy / Select all entries for the right-click menu over the log text.
    pub fn context_items(&self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.open.as_ref().map(|o| o.log.pane.context_items(x, y)).unwrap_or_default()
    }

    pub fn select_all(&mut self) {
        if let Some(o) = self.open.as_mut() {
            o.log.pane.select_all();
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    fn btn_font(fs: f32) -> f32 {
        (fs * 0.95).max(MIN_FONT)
    }

    /// Height of one row for a font of `fs` pixels.
    pub fn row_height(fs: f32, s: f32) -> f32 {
        let bf = Self::btn_font(fs);
        6.0 * s + bf * 2.1 + 3.0 * s + fs * 1.3 + 6.0 * s
    }

    /// The strip's height: as many rows as fit in `max_h` (at most [`MAX_ROWS`], at least one when there is room), plus a line
    /// for the jobs that did not fit. 0 when there is nothing to show.
    pub fn strip_height(&self, fs: f32, s: f32, max_h: f32) -> f32 {
        let n = self.jobs.len();
        if n == 0 {
            return 0.0;
        }
        let rh = Self::row_height(fs, s) + 4.0 * s;
        let more = fs * 1.4;
        let mut rows = n.min(MAX_ROWS);
        while rows > 1 && rows as f32 * rh + if n > rows { more } else { 0.0 } > max_h {
            rows -= 1;
        }
        let h = rows as f32 * rh + if n > rows { more } else { 0.0 };
        if h > max_h { 0.0 } else { h }
    }

    /// Draw the strip into `r` (its height from [`JobsUi::strip_height`]). `fs` is the text size in pixels.
    pub fn draw_strip(&mut self, g: &mut Gfx, r: R, fs: f32, s: f32, tick: u64) {
        self.geoms.clear();
        self.overflow = 0;
        let fs = fs.max(MIN_FONT);
        let rh = Self::row_height(fs, s);
        let gap = 4.0 * s;
        let n = self.jobs.len();
        let fits = (((r.h + gap) / (rh + gap)).floor() as usize).max(1);
        let rows = n.min(MAX_ROWS).min(fits);
        for i in 0..rows {
            let row = R::new(r.x, r.y + i as f32 * (rh + gap), r.w, rh);
            self.draw_row(g, row, i, fs, s, tick);
        }
        if n > rows {
            self.overflow = n - rows;
            let t = format!("+{} more job{} — all of them are in RUNS → Jobs", n - rows, if n - rows == 1 { "" } else { "s" });
            let t = g.fit(fs * 0.95, &t, Face::Sans, r.w);
            g.text(r.x + 4.0 * s, r.y + rows as f32 * (rh + gap) + 1.0 * s, fs * 0.95, DIM, &t, Face::Sans);
        }
    }

    /// Draw every job as a row, `row_h` apart, starting at `r` (the RUNS list). Returns the rows drawn.
    pub fn draw_list(&mut self, g: &mut Gfx, r: R, fs: f32, s: f32, tick: u64) -> usize {
        self.geoms.clear();
        let fs = fs.max(MIN_FONT);
        let rh = Self::row_height(fs, s);
        let gap = 4.0 * s;
        let fits = (((r.h + gap) / (rh + gap)).floor() as usize).max(0);
        let rows = self.jobs.len().min(fits).min(100);
        for i in 0..rows {
            let row = R::new(r.x, r.y + i as f32 * (rh + gap), r.w, rh);
            self.draw_row(g, row, i, fs, s, tick);
        }
        self.overflow = self.jobs.len().saturating_sub(rows);
        rows
    }

    fn draw_row(&mut self, g: &mut Gfx, row: R, i: usize, fs: f32, s: f32, tick: u64) {
        let j = self.jobs[i].clone();
        let now = now_ms();
        let running = j.running();
        let stopping = self.stopping.contains(&j.id);
        let opened = self.open_id() == Some(j.id.as_str());
        let accent = match &j.state {
            State::Running => CYAN,
            State::Succeeded => OK,
            State::Failed(_) | State::Lost => ERR,
            State::Stopped => MUTED,
        };
        g.rrect(row.x, row.y, row.w, row.h, 8.0 * s, rgb(20, 26, 46));
        g.rrect_outline(row.x, row.y, row.w, row.h, 8.0 * s, if opened { 1.8 } else { 1.0 }, with_alpha(accent, if opened { 230 } else { 110 }));
        let bf = Self::btn_font(fs);
        let bh = bf * 2.1;
        let pad = 8.0 * s;
        let y1 = row.y + 6.0 * s;
        // the buttons, from the right
        let act_label = if stopping { "Stopping…" } else if running { "Stop" } else { "Dismiss" };
        let aw = g.text_width(bf, act_label, Face::Bold) + bf * 1.7;
        let lw = g.text_width(bf, "Log", Face::Bold) + bf * 1.7;
        let act = R::new(row.x + row.w - pad - aw, y1, aw, bh);
        let logb = R::new(act.x - 6.0 * s - lw, y1, lw, bh);
        g.button(logb.x, logb.y, bf, "Log", CYAN, ev_log(i));
        if stopping {
            g.button_off(act.x, act.y, bf, act_label);
        } else {
            g.button(act.x, act.y, bf, act_label, if running { ERR } else { MUTED }, ev_act(i));
        }
        // the indicator, the label and the time
        let d = fs * 1.15;
        let (cx0, cy0) = (row.x + pad + d / 2.0, y1 + bh / 2.0);
        match &j.state {
            State::Running => super::dash::spinner(g, cx0, cy0, d * 0.5, tick, CYAN),
            State::Succeeded => {
                g.line(cx0 - d * 0.32, cy0, cx0 - d * 0.08, cy0 + d * 0.26, 2.2 * s.max(0.8), OK);
                g.line(cx0 - d * 0.08, cy0 + d * 0.26, cx0 + d * 0.36, cy0 - d * 0.26, 2.2 * s.max(0.8), OK);
            }
            State::Failed(_) | State::Lost | State::Stopped => {
                let c = if j.state == State::Stopped { MUTED } else { ERR };
                g.line(cx0 - d * 0.28, cy0 - d * 0.28, cx0 + d * 0.28, cy0 + d * 0.28, 2.2 * s.max(0.8), c);
                g.line(cx0 - d * 0.28, cy0 + d * 0.28, cx0 + d * 0.28, cy0 - d * 0.28, 2.2 * s.max(0.8), c);
            }
        }
        let elapsed = fmt_duration(j.elapsed_secs(now));
        let ew = g.text_width(fs, &elapsed, Face::Sans);
        let tx = row.x + pad + d + 8.0 * s;
        let ex = logb.x - 10.0 * s - ew;
        let label = g.fit(fs, &j.label, Face::Bold, (ex - tx - 8.0 * s).max(30.0 * s));
        let ty = y1 + (bh - g.line_height(fs, Face::Bold)) / 2.0;
        g.text(tx, ty, fs, TEXT, &label, Face::Bold);
        g.text(ex, y1 + (bh - g.line_height(fs, Face::Sans)) / 2.0, fs, MUTED, &elapsed, Face::Sans);
        // second line: the bar and the last line of output (or how it ended)
        let y2 = y1 + bh + 3.0 * s;
        let lh2 = fs * 1.3;
        let bw = (row.w * 0.26).clamp(70.0 * s, 170.0 * s);
        let bar = R::new(row.x + pad + 2.0 * s, y2 + (lh2 - 7.0 * s) / 2.0, bw, 7.0 * s);
        let mut after = bar.x + bar.w + 8.0 * s;
        if running {
            self.draw_bar(g, bar, j.progress, tick, s);
            if let Some(p) = j.progress {
                let t = format!("{p:.0}%");
                g.text(after, y2, fs * 0.95, MUTED, &t, Face::Sans);
                after += g.text_width(fs * 0.95, &t, Face::Sans) + 8.0 * s;
            }
        } else {
            let (word, col) = match &j.state {
                State::Succeeded => ("succeeded".to_string(), OK),
                State::Failed(c) => (format!("failed (exit {c})"), ERR),
                State::Stopped => ("stopped".to_string(), MUTED),
                _ => ("lost".to_string(), ERR),
            };
            g.text(bar.x, y2, fs * 0.95, col, &word, Face::Bold);
            after = bar.x + g.text_width(fs * 0.95, &word, Face::Bold).max(bw * 0.6) + 8.0 * s;
        }
        let last = if j.last_line.is_empty() { "(no output yet)".to_string() } else { j.last_line.clone() };
        let last = g.fit(fs * 0.95, &last, Face::Mono, (row.x + row.w - pad - after).max(20.0 * s));
        g.text(after, y2, fs * 0.95, DIM, &last, Face::Mono);
        self.geoms.push(RowGeom { id: j.id.clone(), rect: row, log_btn: logb, act_btn: act, bar });
    }

    /// The bar: filled to the percentage when the output gives one, else a segment that slides along it.
    fn draw_bar(&self, g: &mut Gfx, bar: R, progress: Option<f32>, tick: u64, s: f32) {
        g.rrect(bar.x, bar.y, bar.w, bar.h, bar.h / 2.0, rgb(34, 42, 72));
        match progress {
            Some(p) => {
                let w = (bar.w * p.clamp(0.0, 100.0) / 100.0).max(bar.h);
                g.rrect(bar.x, bar.y, w, bar.h, bar.h / 2.0, CYAN);
            }
            None => {
                let seg = bar.w * 0.3;
                let travel = bar.w + seg;
                let pos = (tick as f32 * 2.2 * s.max(0.8)) % travel - seg;
                g.clipped(bar, |g| g.rrect(bar.x + pos, bar.y, seg, bar.h, bar.h / 2.0, with_alpha(CYAN, 230)));
            }
        }
    }

    /// The log view over `r`: a header with the job and the Follow and Close buttons, then the text (scrolls with the wheel,
    /// follows the end until the person scrolls up, selectable and copyable).
    pub fn draw_log(&mut self, g: &mut Gfx, r: R, fs: f32, s: f32) {
        let Some(o) = self.open.as_mut() else { return };
        let fs = fs.max(MIN_FONT);
        g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(10, 13, 24));
        g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.2, with_alpha(CYAN, 150));
        // a click on the view never reaches the conversation under it
        g.hit(r, Event::Tick, "log-backdrop");
        let bf = Self::btn_font(fs);
        let bh = bf * 2.1;
        let pad = 8.0 * s;
        let mut x = r.x + r.w - pad;
        let cw = g.text_width(bf, "Close", Face::Bold) + bf * 1.7;
        x -= cw;
        g.button(x, r.y + 5.0 * s, bf, "Close", MUTED, Event::Tab(LOG_CLOSE));
        let following = o.log.scroll == 0;
        let fl = if following { "Following" } else { "Follow" };
        let fw = g.text_width(bf, fl, Face::Bold) + bf * 1.7;
        x -= fw + 6.0 * s;
        if following {
            g.button_off(x, r.y + 5.0 * s, bf, fl);
        } else {
            g.button(x, r.y + 5.0 * s, bf, fl, CYAN, Event::Tab(LOG_FOLLOW));
        }
        let head = format!("Log · {} · {}", o.label, o.state.label());
        let head = g.fit(fs, &head, Face::Bold, (x - r.x - 2.0 * pad).max(30.0 * s));
        g.text(r.x + pad, r.y + 5.0 * s + (bh - g.line_height(fs, Face::Bold)) / 2.0, fs, TEXT, &head, Face::Bold);
        let body = R::new(r.x + pad, r.y + bh + 10.0 * s, r.w - 2.0 * pad, (r.h - bh - 16.0 * s).max(10.0));
        if o.log.lines.is_empty() {
            g.text(body.x, body.y, fs, DIM, "(no output yet)", Face::Sans);
        }
        g.clipped(body, |g| o.log.render(g, body, fs * 0.95, Face::Mono));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use iyakki_agent::jobs::{Cmd, Spec};
    use std::path::Path;

    fn start(cx: &AppCx, ws: &str, label: &str, line: &str) -> String {
        let dir = std::env::temp_dir();
        cx.services.jobs.start(&cx.session.user, &Spec { label: label.into(), cmd: Cmd::Shell(line.into()), cwd: dir, workspace: ws.into() }, &|_: &Path| Ok(())).unwrap().id
    }

    fn wait_for(cx: &AppCx, id: &str) {
        cx.services.jobs.wait(&cx.session.user, id, Duration::from_secs(20)).unwrap();
    }

    fn cleanup(cx: &AppCx) {
        for j in cx.services.jobs.list(&cx.session.user) {
            let _ = cx.services.jobs.stop(&cx.session.user, &j.id, Duration::from_millis(500));
        }
    }

    #[test]
    fn it_animates_only_while_a_job_runs() {
        let cx = cx_with_admin_store("jobsui-anim");
        let mut ui = JobsUi::new(Some(""));
        assert!(!ui.poll(&cx, true) && !ui.animating(), "nothing to show, nothing moves");
        let id = start(&cx, "", "nap", "sleep 30");
        assert!(ui.poll(&cx, true) && ui.animating());
        cx.services.jobs.stop(&cx.session.user, &id, Duration::from_secs(1)).unwrap();
        ui.poll(&cx, true);
        assert!(!ui.animating(), "a finished job does not animate");
        assert_eq!(ui.jobs().len(), 1, "it stays until dismissed");
        cleanup(&cx);
    }

    #[test]
    fn a_finished_job_gives_one_event_line_and_stays_until_dismissed() {
        let cx = cx_with_admin_store("jobsui-events");
        let mut ui = JobsUi::new(Some(""));
        let id = start(&cx, "", "build", "echo built; exit 2");
        wait_for(&cx, &id);
        ui.poll(&cx, true);
        let ev = ui.take_events();
        assert_eq!(ev.len(), 1);
        assert!(ev[0].contains("build") && ev[0].contains("failed (exit 2)"), "{ev:?}");
        ui.poll(&cx, true);
        assert!(ui.take_events().is_empty(), "once");
        // Dismiss (the second button of a finished row)
        assert!(ui.handle(&ev_act(0), &cx));
        assert!(ui.jobs().is_empty());
    }

    #[test]
    fn the_log_view_follows_scrolls_and_keeps_its_place() {
        let cx = cx_with_admin_store("jobsui-log");
        let mut ui = JobsUi::new(Some(""));
        let id = start(&cx, "", "noisy", "seq 1 300; sleep 2; seq 301 400; sleep 30");
        let t0 = Instant::now();
        loop {
            ui.poll(&cx, true);
            if ui.jobs().first().is_some_and(|j| j.last_line == "300") {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ui.handle(&ev_log(0), &cx));
        assert_eq!(ui.open_id(), Some(id.as_str()));
        assert!(ui.log_text().ends_with("300") && ui.log_following(), "it starts at the end");
        // the wheel scrolls up: no longer following, and output that arrives does not move what is being read
        assert!(ui.handle(&Event::Scroll(5), &cx));
        assert!(!ui.log_following());
        let before = ui.open.as_ref().unwrap().log.scroll;
        let end = Instant::now() + Duration::from_secs(10);
        while !ui.log_text().ends_with("400") {
            ui.poll(&cx, true);
            assert!(Instant::now() < end, "the new output never arrived");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(ui.open.as_ref().unwrap().log.scroll, before + 100, "100 new lines below: the same text stays in view");
        // Follow goes back to the end
        assert!(ui.handle(&Event::Tab(LOG_FOLLOW), &cx) && ui.log_following());
        // keys: other keys are not taken (typing still goes to the chat); Esc closes the view and is used up
        assert!(!ui.handle(&Event::Key(Key::Char('x')), &cx));
        assert!(ui.handle(&Event::Key(Key::Esc), &cx) && !ui.log_open());
        assert!(!ui.handle(&Event::Key(Key::Esc), &cx), "with no log open Esc belongs to the chat");
        cleanup(&cx);
    }

    #[test]
    fn stop_ends_the_job_without_blocking_and_a_second_click_does_nothing() {
        let cx = cx_with_admin_store("jobsui-stop");
        let mut ui = JobsUi::new(Some(""));
        let id = start(&cx, "", "stubborn", "trap '' TERM; while true; do sleep 0.1; done");
        std::thread::sleep(Duration::from_millis(300));
        ui.poll(&cx, true);
        let t0 = Instant::now();
        assert!(ui.handle(&ev_act(0), &cx));
        assert!(t0.elapsed() < Duration::from_millis(500), "the click returns at once (stopping takes its grace on a thread)");
        assert!(ui.handle(&ev_act(0), &cx), "a second click is harmless");
        let end = Instant::now() + Duration::from_secs(10);
        while cx.services.jobs.get(&cx.session.user, &id).unwrap().running() {
            assert!(Instant::now() < end, "it never stopped");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(cx.services.jobs.get(&cx.session.user, &id).unwrap().state, State::Stopped);
        ui.poll(&cx, true);
        assert!(!ui.animating());
        cleanup(&cx);
    }

    #[test]
    fn rows_fit_and_their_buttons_are_big_enough_to_click_at_every_screen_size() {
        let cx = cx_with_admin_store("jobsui-geom");
        let mut ui = JobsUi::new(Some(""));
        let a = start(&cx, "", "Android emulator and SDK install with a very long name that has to be cut to fit the row", "echo 'Downloading system-images 42%'; sleep 30");
        start(&cx, "", "Second job", "echo plain; sleep 30");
        start(&cx, "", "Quick one", "echo done");
        start(&cx, "", "Fourth", "sleep 30");
        wait_for(&cx, "j3");
        let _ = a;
        std::thread::sleep(Duration::from_millis(400));
        ui.poll(&cx, true);
        assert_eq!(ui.jobs().len(), 4);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            let fs = (14.0 * s).max(MIN_FONT);
            let strip_w = w as f32 * 0.5;
            let max_h = h as f32 * 0.4;
            let sh = ui.strip_height(fs, s, max_h);
            assert!(sh > 0.0 && sh <= max_h, "{w}x{h}: strip {sh} of {max_h}");
            let mut g = Gfx::new(w, h);
            let r = R::new(20.0, 100.0, strip_w, sh);
            ui.draw_strip(&mut g, r, fs, s, 7);
            assert!(!ui.geoms.is_empty() && ui.geoms.len() <= MAX_ROWS, "{w}x{h}");
            let mut prev_bottom = r.y;
            for gm in &ui.geoms {
                assert!(gm.rect.x >= r.x - 0.5 && gm.rect.x + gm.rect.w <= r.x + r.w + 0.5, "{w}x{h}: row inside the strip");
                assert!(gm.rect.y >= prev_bottom - 0.5, "rows do not overlap");
                prev_bottom = gm.rect.y + gm.rect.h;
                assert!(prev_bottom <= r.y + sh + 0.5, "{w}x{h}: row inside the strip's height");
                for b in [gm.log_btn, gm.act_btn] {
                    assert!(b.h >= 22.0 && b.w >= 36.0, "{w}x{h}: button {b:?} is large enough to click");
                    assert!(b.x >= gm.rect.x && b.x + b.w <= gm.rect.x + gm.rect.w && b.y >= gm.rect.y && b.y + b.h <= gm.rect.y + gm.rect.h, "{w}x{h}: button inside its row");
                }
                assert!(gm.log_btn.x + gm.log_btn.w <= gm.act_btn.x, "buttons do not overlap");
                assert!(gm.bar.x + gm.bar.w <= gm.rect.x + gm.rect.w);
            }
            // every button is registered as a click region with the right event
            let hits: Vec<_> = g.hits.iter().map(|h| (h.label.clone(), h.ev.clone())).collect();
            assert!(hits.contains(&("Log".into(), ev_log(0))), "{w}x{h}: {hits:?}");
            assert!(hits.iter().any(|(l, e)| l == "Stop" && *e == ev_act(0)), "{w}x{h}");
            // the same drawing in the full list
            let rows = ui.draw_list(&mut g, R::new(10.0, 60.0, w as f32 * 0.9, h as f32 * 0.8), fs, s, 3);
            assert_eq!(rows, 4, "{w}x{h}: the list shows every job");
            assert!(g.hits.iter().any(|h| h.label == "Dismiss"), "the finished one has Dismiss");
        }
        cleanup(&cx);
    }

    #[test]
    fn the_bar_slides_without_a_percentage_and_fills_with_one_and_the_frames_differ() {
        let cx = cx_with_admin_store("jobsui-bar");
        let mut ui = JobsUi::new(Some(""));
        start(&cx, "", "indeterminate", "echo working; sleep 30");
        std::thread::sleep(Duration::from_millis(400));
        ui.poll(&cx, true);
        assert_eq!(ui.jobs()[0].progress, None);
        let bar_pixels = |ui: &mut JobsUi, tick: u64| -> Vec<u8> {
            let mut g = Gfx::new(800, 300);
            ui.draw_strip(&mut g, R::new(10.0, 10.0, 700.0, 120.0), 14.0, 1.0, tick);
            let b = ui.geoms[0].bar;
            let mut v = Vec::new();
            for x in (b.x as u32)..((b.x + b.w) as u32) {
                let o = (((b.y + b.h / 2.0) as usize) * g.width() as usize + x as usize) * 4;
                v.extend_from_slice(&g.rgba()[o..o + 4]);
            }
            v
        };
        let (f0, f1, f0b) = (bar_pixels(&mut ui, 0), bar_pixels(&mut ui, 20), bar_pixels(&mut ui, 0));
        assert_ne!(f0, f1, "the indeterminate bar moves between frames");
        assert_eq!(f0, f0b, "and draws the same for the same tick");
        cleanup(&cx);
        let cx2 = cx_with_admin_store("jobsui-bar2");
        let mut ui2 = JobsUi::new(Some(""));
        start(&cx2, "", "determinate", "echo 'Downloading 50%'; sleep 30");
        std::thread::sleep(Duration::from_millis(400));
        ui2.poll(&cx2, true);
        assert_eq!(ui2.jobs()[0].progress, Some(50.0));
        assert_eq!(bar_pixels(&mut ui2, 0), bar_pixels(&mut ui2, 20), "a determinate bar does not slide");
        cleanup(&cx2);
    }
}

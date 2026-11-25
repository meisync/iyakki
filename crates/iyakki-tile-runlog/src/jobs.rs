//! The Jobs strip and the job log view (see `docs/JOBS.md`): what the person sees of the detached jobs the assistant (or an
//! external coding agent) started. One row per job: a spinner or a check or a cross, the label, how long it has run, a bar
//! (a sliding one while the output gives no percentage, a filled one when it does), the last line of output and the buttons
//! Log and Stop (Dismiss once it ended). The same drawing serves the chat, the AI IDE's chat pane and the Jobs list in RUNS.
//!
//! Nothing here runs a job: the engine is behind the [`JobsPort`] the shell hands over. This module reads it about once a second while there is
//! something to show, and says it is animating only while a job runs, so an idle shell stays idle.

use std::collections::HashSet;

use iyakki_tile_api::runs::{fmt_duration, Job, JobState as State, JobsPort};
use iyakki_uikit::fx::{with_alpha, CYAN};
use iyakki_uikit::gfx::{rgb, Face, Gfx};
use iyakki_uikit::safeclamp::SafeClamp;
use iyakki_uikit::scrolllog::ScrollLog;
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// The clock of the strip, in milliseconds: the real one, or a test's.
pub type Clock = Box<dyn Fn() -> u64>;

fn real_now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

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
    /// When the jobs were read last (on `clock`).
    polled: Option<u64>,
    /// The clock the rows are drawn against and the reading is paced by; a test sets one it moves itself, so two frames never differ by a second that ticked between them.
    clock: Clock,
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
        JobsUi { ws: ws.map(str::to_string), jobs: Vec::new(), polled: None, clock: Box::new(real_now_ms), open: None, stopping: HashSet::new(), events: Vec::new(), geoms: Vec::new(), overflow: 0, dirty: false, frame: 0, hurry: false }
    }

    /// Use another clock (tests).
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
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
    pub fn poll(&mut self, port: &dyn JobsPort, force: bool) -> bool {
        let every_ms: u64 = if self.hurry && self.jobs.is_empty() {
            400
        } else if self.jobs.is_empty() && self.open.is_none() {
            3000
        } else {
            1000
        };
        let now = (self.clock)();
        if !force && self.polled.is_some_and(|t| now.saturating_sub(t) < every_ms) {
            return false;
        }
        self.polled = Some(now);
        // the events first, then the list: a job whose end is announced is never drawn as still running
        if let Some(w) = &self.ws {
            self.events.extend(port.take_finished(w));
        }
        let mut list = port.visible(self.ws.as_deref());
        // the running ones first, each group newest first (the engine lists newest first)
        list.sort_by_key(|j| !j.running());
        let changed = list != self.jobs;
        self.jobs = list;
        self.stopping.retain(|id| self.jobs.iter().any(|j| &j.id == id && j.running()));
        let logged = self.refresh_log(port);
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

    fn refresh_log(&mut self, port: &dyn JobsPort) -> bool {
        let Some(o) = self.open.as_mut() else { return false };
        let len = port.log_len(&o.id);
        let state = self.jobs.iter().find(|j| j.id == o.id).map(|j| j.state.clone()).unwrap_or(o.state.clone());
        if len == o.seen_len && state == o.state {
            return false;
        }
        let Ok(text) = port.log_text(&o.id, LOG_BYTES) else { return false };
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

    pub fn open_log(&mut self, port: &dyn JobsPort, id: &str) {
        let Some(j) = self.jobs.iter().find(|j| j.id == id) else { return };
        self.open = Some(OpenLog { id: j.id.clone(), label: j.label.clone(), state: j.state.clone(), log: ScrollLog::default(), seen_len: u64::MAX, shown_lines: 0 });
        self.refresh_log(port);
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

    fn stop_or_dismiss(&mut self, port: &dyn JobsPort, i: usize) {
        let Some(j) = self.jobs.get(i).cloned() else { return };
        if j.running() {
            if !self.stopping.insert(j.id.clone()) {
                return;
            }
            port.audit("job.stop", &format!("{} {}", j.id, j.label));
            // TERM, a pause, then KILL: not on the drawing thread
            let stop = port.stop_job(&j.id);
            std::thread::spawn(stop);
        } else {
            let _ = port.dismiss(&j.id);
            if self.open.as_ref().is_some_and(|o| o.id == j.id) {
                self.open = None;
            }
            self.poll(port, true);
        }
    }

    /// A click on the strip, or a key or the wheel over the log view. True when the event was used.
    pub fn handle(&mut self, ev: &Event, port: &dyn JobsPort) -> bool {
        match ev {
            Event::Tab(n) if (BASE..BASE + 900).contains(n) => {
                let (row, what) = ((n - BASE) / 4, (n - BASE) % 4);
                match what {
                    0 => {
                        if let Some(id) = self.jobs.get(row).map(|j| j.id.clone()) {
                            if self.open_id() == Some(id.as_str()) {
                                self.open = None;
                            } else {
                                self.open_log(port, &id);
                            }
                        }
                    }
                    1 => self.stop_or_dismiss(port, row),
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
    pub fn context_items(&self, x: i32, y: i32) -> Vec<iyakki_uikit::ctxmenu::Item> {
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
        let now = (self.clock)();
        let running = j.running();
        let quiet = j.quiet_secs(now);
        let stalled = running && quiet >= QUIET_AFTER_SECS;
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
            // the spinner turns only while the job shows real signs of work (new output, or CPU used by its processes); when it has gone quiet
            // it stands still in amber and the row says for how long
            State::Running if stalled => iyakki_uikit::fx::spinner(g, cx0, cy0, d * 0.5, 0, AMBER),
            State::Running => iyakki_uikit::fx::spinner(g, cx0, cy0, d * 0.5, tick, CYAN),
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
        let bw = (row.w * 0.26).clamp_safe(70.0 * s, 170.0 * s);
        let bar = R::new(row.x + pad + 2.0 * s, y2 + (lh2 - 7.0 * s) / 2.0, bw, 7.0 * s);
        let mut after = bar.x + bar.w + 8.0 * s;
        if running {
            self.draw_bar(g, bar, j.progress, if stalled { None } else { Some(tick) }, stalled, s);
            if let Some(p) = j.progress {
                let t = format!("{p:.0}%");
                g.text(after, y2, fs * 0.95, MUTED, &t, Face::Sans);
                after += g.text_width(fs * 0.95, &t, Face::Sans) + 8.0 * s;
            }
        } else {
            let (word, col) = match &j.state {
                State::Succeeded => ("succeeded".to_string(), OK),
                State::Failed(iyakki_tile_api::runs::TIMED_OUT_EXIT) => ("timed out".to_string(), ERR),
                State::Failed(c) => (format!("failed (exit {c})"), ERR),
                State::Stopped => ("stopped".to_string(), MUTED),
                _ => ("lost".to_string(), ERR),
            };
            g.text(bar.x, y2, fs * 0.95, col, &word, Face::Bold);
            after = bar.x + g.text_width(fs * 0.95, &word, Face::Bold).max(bw * 0.6) + 8.0 * s;
        }
        let last = if j.last_line.is_empty() { "(no output yet)".to_string() } else { j.last_line.clone() };
        let last = if stalled { format!("no activity for {} · {last}", fmt_duration(quiet)) } else { last };
        let last = g.fit(fs * 0.95, &last, Face::Mono, (row.x + row.w - pad - after).max(20.0 * s));
        g.text(after, y2, fs * 0.95, DIM, &last, Face::Mono);
        self.geoms.push(RowGeom { id: j.id.clone(), rect: row, log_btn: logb, act_btn: act, bar });
    }

    /// The bar: filled to the percentage when the output gives one, else a segment that slides along it.
    fn draw_bar(&self, g: &mut Gfx, bar: R, progress: Option<f32>, moving: Option<u64>, stalled: bool, s: f32) {
        g.rrect(bar.x, bar.y, bar.w, bar.h, bar.h / 2.0, rgb(34, 42, 72));
        let col = if stalled { AMBER } else { CYAN };
        match (progress, moving) {
            (Some(p), _) => {
                let w = (bar.w * p.clamp(0.0, 100.0) / 100.0).max(bar.h);
                g.rrect(bar.x, bar.y, w, bar.h, bar.h / 2.0, col);
            }
            (None, Some(tick)) => {
                let seg = bar.w * 0.3;
                let travel = bar.w + seg;
                let pos = (tick as f32 * 2.2 * s.max(0.8)) % travel - seg;
                g.clipped(bar, |g| g.rrect(bar.x + pos, bar.y, seg, bar.h, bar.h / 2.0, with_alpha(CYAN, 230)));
            }
            // quiet and no percentage: nothing slides, a still segment says it is waiting
            (None, None) => g.rrect(bar.x, bar.y, bar.w * 0.12, bar.h, bar.h / 2.0, with_alpha(col, 200)),
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

/// A running job that has shown no sign of work (output, CPU) for this long is shown as quiet, not as busy.
pub const QUIET_AFTER_SECS: u64 = 20;
const AMBER: [u8; 4] = rgb(255, 190, 90);


// ---- views for the shell's adapter and the tests ----

impl JobsUi {
    /// How far the open log is scrolled up, in lines (0 while following).
    pub fn log_scroll(&self) -> usize {
        self.open.as_ref().map_or(0, |o| o.log.scroll)
    }
    /// The jobs, for a test to change what is shown without a port.
    pub fn set_jobs(&mut self, jobs: Vec<Job>) {
        self.jobs = jobs;
    }
}

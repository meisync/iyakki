//! The installer indicator beside the clock in the top bar, and the list it opens.
//!
//! While an installer job runs (Quick install's k3s, DuckDB, Elasticsearch: the jobs of [`iyakki_agent::quickinstall::WORKSPACE`])
//! a small ring turns next to the date and time, filled to the average progress when the jobs report one, with the number of
//! installs running beside it. For ten minutes after the last one ended it stays as a check (all went well) or a cross (one
//! failed) so the result is not missed; Dismiss in the list hides finished ones. A click (or Enter in the menu's "Installer runs")
//! opens the list: every running and recent install with its bar, last line, Log (live, follows the end) and Stop, drawn by the
//! same [`JobsUi`] as the chat's jobs strip and RUNS → Jobs.
//!
//! The jobs are read about once a second while one runs and every few seconds otherwise; nothing animates when nothing runs.

use crate::safeclamp::SafeClamp;
use std::time::{Duration, Instant};

use crate::apps::jobs_ui::JobsUi;
use crate::apps::AppCx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_agent::jobs::{now_ms, Job, State};

/// The indicator's click.
pub const INDICATOR: usize = 1_300_000;
pub const CLOSE: usize = 1_300_001;
pub const RUNS: usize = 1_300_002;
/// A finished install keeps the indicator (as a check or a cross) this long.
pub const RECENT: Duration = Duration::from_secs(600);

const ACCENT: [u8; 4] = crate::fx::CYAN;

/// What the indicator shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Badge {
    Hidden,
    /// Installs running, and their average progress when every one reports one.
    Running { count: usize, progress: Option<f32> },
    /// Nothing runs; the last ones ended recently: all succeeded, or not.
    Done { ok: bool, count: usize },
}

/// The indicator for a list of installer jobs at `now` (ms).
pub fn badge(jobs: &[Job], now: u64) -> Badge {
    let running: Vec<&Job> = jobs.iter().filter(|j| j.running()).collect();
    if !running.is_empty() {
        let progress = if running.iter().all(|j| j.progress.is_some()) { Some(running.iter().map(|j| j.progress.unwrap_or(0.0)).sum::<f32>() / running.len() as f32) } else { None };
        return Badge::Running { count: running.len(), progress };
    }
    let recent: Vec<&Job> = jobs.iter().filter(|j| j.ended_ms > 0 && now.saturating_sub(j.ended_ms) < RECENT.as_millis() as u64).collect();
    if recent.is_empty() {
        return Badge::Hidden;
    }
    Badge::Done { ok: recent.iter().all(|j| j.state == State::Succeeded), count: recent.len() }
}

/// What a key or click asks the shell to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Close,
    /// RUNS → Jobs (every job of the person).
    OpenRuns,
}

pub struct InstallPop {
    pub open: bool,
    ui: JobsUi,
    polled: Option<Instant>,
    /// Where the indicator was drawn (the list hangs under it).
    pub anchor: Option<R>,
    /// The time the badge is worked out for (tests fix it).
    pub now: Option<u64>,
}

impl Default for InstallPop {
    fn default() -> InstallPop {
        InstallPop::new()
    }
}

impl InstallPop {
    pub fn new() -> InstallPop {
        InstallPop { open: false, ui: JobsUi::new(Some(iyakki_agent::quickinstall::WORKSPACE)), polled: None, anchor: None, now: None }
    }

    pub fn jobs(&self) -> &[Job] {
        self.ui.jobs()
    }

    pub fn badge(&self) -> Badge {
        badge(self.ui.jobs(), self.now.unwrap_or_else(now_ms))
    }

    pub fn log_open(&self) -> bool {
        self.ui.log_open()
    }

    /// Read the installer jobs again when it is time (about every second while one runs or the list is open, else every three).
    /// True when what is drawn changed.
    pub fn poll(&mut self, cx: &AppCx, force: bool) -> bool {
        let every = if self.open || self.ui.animating() { Duration::from_millis(900) } else { Duration::from_secs(3) };
        if !force && self.polled.is_some_and(|t| t.elapsed() < every) {
            return false;
        }
        self.polled = Some(Instant::now());
        let before = self.badge();
        let changed = self.ui.poll(cx, true);
        let _ = self.ui.take_events();
        changed || before != self.badge()
    }

    /// A new frame is needed now (the ring turns about ten times a second while an install runs).
    pub fn frame_due(&mut self) -> bool {
        self.ui.frame_due()
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.ui.close_log();
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.ui.close_log();
    }

    /// The indicator's width at this scale (0 when hidden).
    pub fn indicator_width(&self, g: &mut Gfx, s: f32) -> f32 {
        match self.badge() {
            Badge::Hidden => 0.0,
            Badge::Running { count, .. } | Badge::Done { count, .. } => 34.0 * s + g.text_width(13.0 * s, &count.to_string(), Face::Bold),
        }
    }

    /// Draw the indicator with its right edge at `right`, vertically centred on `cy`. Returns its box (None when hidden).
    pub fn draw_indicator(&mut self, g: &mut Gfx, right: f32, cy: f32, s: f32, tick: u64) -> Option<R> {
        let w = self.indicator_width(g, s);
        if w <= 0.0 {
            self.anchor = None;
            return None;
        }
        let b = R::new(right - w, cy - 14.0 * s, w, 28.0 * s);
        let hot = g.hovered(b) || self.open;
        if hot {
            g.rrect(b.x, b.y, b.w, b.h, 8.0 * s, rgb(28, 36, 66));
        }
        let (cx, rad) = (b.x + 15.0 * s, 8.0 * s);
        let (count, label) = match self.badge() {
            Badge::Running { count, progress } => {
                match progress {
                    Some(p) => crate::fx::ring_gauge(g, cx, cy, rad, 2.2 * s, p / 100.0, ACCENT),
                    None => g.ring(cx, cy, rad, 2.2 * s, rgb(26, 34, 62)),
                }
                crate::apps::dash::spinner(g, cx, cy, rad, tick, ACCENT);
                let pct = progress.map(|p| format!(" · {p:.0}%")).unwrap_or_default();
                (count, format!("{count} install{} running{pct}", if count == 1 { "" } else { "s" }))
            }
            Badge::Done { ok, count } => {
                let c = if ok { OK } else { ERR };
                g.ring(cx, cy, rad, 2.0 * s, crate::fx::with_alpha(c, 160));
                if ok {
                    g.line(cx - 4.0 * s, cy, cx - 1.0 * s, cy + 3.5 * s, 2.0 * s, c);
                    g.line(cx - 1.0 * s, cy + 3.5 * s, cx + 4.5 * s, cy - 3.5 * s, 2.0 * s, c);
                } else {
                    g.line(cx - 3.5 * s, cy - 3.5 * s, cx + 3.5 * s, cy + 3.5 * s, 2.0 * s, c);
                    g.line(cx - 3.5 * s, cy + 3.5 * s, cx + 3.5 * s, cy - 3.5 * s, 2.0 * s, c);
                }
                (count, if ok { "installs finished".to_string() } else { "an install did not finish".to_string() })
            }
            Badge::Hidden => return None,
        };
        g.text(cx + rad + 6.0 * s, cy - g.line_height(13.0 * s, Face::Bold) / 2.0, 13.0 * s, TEXT, &count.to_string(), Face::Bold);
        g.hit(b, Event::Tab(INDICATOR), &label);
        self.anchor = Some(b);
        Some(b)
    }

    /// The list's panel: under the indicator (or the top right), kept on screen.
    pub fn panel(&self, w: u32, h: u32, s: f32) -> R {
        let pw = (620.0 * s).min(w as f32 - 24.0 * s);
        let top = self.anchor.map_or(56.0 * s, |a| a.y + a.h + 8.0 * s);
        let right = self.anchor.map_or(w as f32 - 12.0 * s, |a| a.x + a.w + 40.0 * s);
        let ph = (if self.ui.log_open() { 560.0 } else { 120.0 + 74.0 * self.ui.jobs().len().clamp(1, 5) as f32 } * s).min(h as f32 - top - 12.0 * s);
        let x = (right - pw).clamp_safe(12.0 * s, (w as f32 - pw - 12.0 * s).max(12.0 * s));
        R::new(x, top, pw, ph)
    }

    /// Draw the list. Registers its own click regions and clears the others (a click outside it closes it).
    pub fn render(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32, tick: u64) {
        if !self.open {
            return;
        }
        g.clear_hits();
        let p = self.panel(w, h, s);
        crate::fx::modal_panel(g, p, s, ACCENT, false);
        g.text(p.x + 20.0 * s, p.y + 14.0 * s, 19.0 * s, TEXT, "Installs", Face::Bold);
        let running = self.ui.jobs().iter().filter(|j| j.running()).count();
        let sub = format!("{running} running · {} in the list", self.ui.jobs().len());
        let tw = g.text_width(19.0 * s, "Installs", Face::Bold);
        g.text(p.x + 20.0 * s + tw + 12.0 * s, p.y + 20.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
        let close = R::new(p.x + p.w - 44.0 * s, p.y + 8.0 * s, 34.0 * s, 34.0 * s);
        let hot = g.hovered(close);
        crate::fx::chrome_button(g, close, s, hot, ACCENT);
        let cw = g.text_width(18.0 * s, "x", Face::Bold);
        g.text(close.x + (close.w - cw) / 2.0, close.y + 6.0 * s, 18.0 * s, MUTED, "x", Face::Bold);
        g.hit(close, Event::Tab(CLOSE), "close");
        let fs = (13.5 * s).max(12.0);
        let foot = 46.0 * s;
        let body = R::new(p.x + 14.0 * s, p.y + 54.0 * s, p.w - 28.0 * s, p.h - 54.0 * s - foot);
        if self.ui.jobs().is_empty() {
            crate::fx::empty_state(g, body, s, crate::gfx::Icon::Cli, "No installs yet", "Installs start in Store \u{2192} Setup (the IYAKKI menu has a shortcut to it).", ACCENT);
        } else if self.ui.log_open() {
            let list_h = (JobsUi::row_height(fs, s) + 4.0 * s).min(body.h * 0.3);
            self.ui.draw_list(g, R::new(body.x, body.y, body.w, list_h), fs, s, tick);
            self.ui.draw_log(g, R::new(body.x, body.y + list_h + 6.0 * s, body.w, body.h - list_h - 6.0 * s), fs, s);
        } else {
            self.ui.draw_list(g, body, fs, s, tick);
        }
        let by = p.y + p.h - foot + 6.0 * s;
        let bw = g.button(p.x + 16.0 * s, by, 13.0 * s, "All jobs in RUNS", MUTED, Event::Tab(RUNS));
        let hint = "Log shows the live output · Esc closes";
        let hint = g.fit(12.0 * s, hint, Face::Sans, p.w - bw - 50.0 * s);
        g.text(p.x + 16.0 * s + bw + 12.0 * s, by + 8.0 * s, 12.0 * s, DIM, &hint, Face::Sans);
    }

    /// A key, a click on the list's own regions, or the wheel.
    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> Outcome {
        match ev {
            Event::Tab(CLOSE) => return Outcome::Close,
            Event::Tab(RUNS) => return Outcome::OpenRuns,
            Event::Tab(INDICATOR) => return Outcome::Close,
            _ => {}
        }
        if self.ui.handle(ev, cx) {
            self.poll(cx, true);
            return Outcome::Nothing;
        }
        match ev {
            Event::Key(Key::Esc) => Outcome::Close,
            _ => Outcome::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, state: State, progress: Option<f32>, ended_ms: u64) -> Job {
        Job { id: id.into(), label: format!("Install {id}"), command: format!("iyakki quick-install {id}"), cwd: "/".into(), owner: "root".into(), workspace: iyakki_agent::quickinstall::WORKSPACE.into(), pid: 1, state, started_ms: 1, ended_ms, progress, last_line: String::new(), dismissed: false }
    }

    #[test]
    fn the_badge_counts_running_installs_averages_their_progress_and_remembers_recent_results() {
        let now = 10_000_000;
        assert_eq!(badge(&[], now), Badge::Hidden);
        let a = job("k3s", State::Running, Some(40.0), 0);
        let b = job("elasticsearch", State::Running, Some(20.0), 0);
        assert_eq!(badge(&[a.clone(), b.clone()], now), Badge::Running { count: 2, progress: Some(30.0) });
        let c = job("duckdb", State::Running, None, 0);
        assert_eq!(badge(&[a.clone(), c], now), Badge::Running { count: 2, progress: None }, "one without a percentage: no average");
        let ok = job("k3s", State::Succeeded, Some(100.0), now - 1_000);
        let bad = job("elasticsearch", State::Failed(1), None, now - 2_000);
        assert_eq!(badge(&[ok.clone()], now), Badge::Done { ok: true, count: 1 });
        assert_eq!(badge(&[ok.clone(), bad], now), Badge::Done { ok: false, count: 2 });
        let old = job("k3s", State::Succeeded, Some(100.0), now - RECENT.as_millis() as u64 - 1);
        assert_eq!(badge(&[old], now), Badge::Hidden, "an old result no longer shows");
        assert_eq!(badge(&[ok, a], now), Badge::Running { count: 1, progress: Some(40.0) }, "running wins");
    }
}

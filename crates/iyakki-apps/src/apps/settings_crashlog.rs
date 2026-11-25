//! Settings -> Last session log: the kernel log that IYAKKI keeps on the data volume across crashes (`iyakki_core_api::device_types::storage::crashlog`,
//! docs/TROUBLESHOOTING.md). Read-only: the last 200 lines of the log of the session that ended unexpectedly (or of the previous or the
//! current boot), scrollable, and "I have seen it", which clears the "ended unexpectedly" note on the Overview page.

use super::*;
use super::system_port::CrashPort;
use iyakki_core_api::crashlog as cl;

/// How many lines the viewer keeps.
pub(super) const VIEW_LINES: usize = 200;

#[derive(Default)]
pub(super) struct CrashPanel {
    /// 0 the session that ended unexpectedly (else the newest crash file), 1 the boot before this one, 2 this boot.
    pub(super) view: u8,
    pub(super) lines: Vec<String>,
    pub(super) name: String,
    /// First line shown.
    pub(super) scroll: usize,
    /// A folder to read instead of `<data folder>/log` (tests).
    dir: Option<std::path::PathBuf>,
    loaded: bool,
}

impl SettingsApp {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn with_crash_dir(mut self, dir: std::path::PathBuf) -> SettingsApp {
        self.crash.dir = Some(dir);
        self
    }

    pub(super) fn crash_dir(&self) -> std::path::PathBuf {
        self.crash.dir.clone().unwrap_or_else(|| self.storage.data_dir.join(cl::LOG_SUBDIR))
    }

    /// The note that the last session ended unexpectedly, until "I have seen it" is pressed.
    pub(super) fn crash_note(&self, p: &dyn CrashPort) -> Option<cl::Unclean> {
        p.crashlog().read_unclean(&self.crash_dir()).filter(|u| !u.seen)
    }

    fn crash_target(&self, p: &dyn CrashPort) -> Option<String> {
        let dir = self.crash_dir();
        let log = p.crashlog();
        match self.crash.view {
            0 => log.read_unclean(&dir).map(|u| u.file).filter(|f| !f.is_empty() && log.has_file(&dir, f)).or_else(|| log.latest_crash(&dir)),
            1 => Some("kmsg-previous-1.log".to_string()).filter(|f| log.has_file(&dir, f)),
            _ => log.current_file(&dir),
        }
    }

    pub(super) fn crash_reload(&mut self, p: &dyn CrashPort) {
        let dir = self.crash_dir();
        let name = self.crash_target(p);
        self.crash.lines = name.as_ref().map(|n| p.crashlog().last_lines(&dir, n, VIEW_LINES)).unwrap_or_default();
        self.crash.name = name.unwrap_or_default();
        self.crash.scroll = self.crash.lines.len().saturating_sub(1);
        self.crash.loaded = true;
    }

    pub(super) fn handle_crash(&mut self, k: &Key, p: &dyn CrashPort) {
        if !self.crash.loaded {
            self.crash_reload(p);
        }
        let last = self.crash.lines.len().saturating_sub(1);
        let page = 10usize;
        match k {
            Key::Up => self.crash.scroll = self.crash.scroll.saturating_sub(1),
            Key::Down => self.crash.scroll = (self.crash.scroll + 1).min(last),
            Key::PageUp => self.crash.scroll = self.crash.scroll.saturating_sub(page),
            Key::PageDown => self.crash.scroll = (self.crash.scroll + page).min(last),
            Key::Home => self.crash.scroll = 0,
            Key::End => self.crash.scroll = last,
            Key::Char('p') => {
                self.crash.view = (self.crash.view + 1) % 3;
                self.crash_reload(p);
            }
            Key::Char('r') => self.crash_reload(p),
            Key::Char('y') => {
                if self.crash_note(p).is_some() {
                    p.crashlog().mark_seen(&self.crash_dir());
                    p.audit("system.unclean-shutdown.seen", "the note about the session that ended unexpectedly was cleared");
                    self.ok("Cleared. The log file is kept; delete it from the data folder (log) when you no longer need it.");
                } else {
                    self.ok("There is no note to clear.");
                }
            }
            Key::Char('c') => self.crash_copy(p),
            _ => {}
        }
    }

    /// "Copy to a file in my home": the log the viewer shows goes, as it is, into a new private file in the person's home folder
    /// (administrators; the assistant's `last_session_log_copy` does the same after a yes).
    fn crash_copy(&mut self, p: &dyn CrashPort) {
        if !p.is_admin() {
            return self.ok("Only an administrator can copy the log.");
        }
        let Some(name) = self.crash_target(p) else {
            return self.ok("There is no kept log to copy for this choice.");
        };
        let home = p.home_for_writes();
        match p.crashlog().copy_to_folder(&self.crash_dir(), &name, &home, iyakki_clock::now_unix()) {
            Ok(file) => {
                p.audit("system.unclean-shutdown.copy", &format!("the kept kernel log {name} was copied to a file in the home folder"));
                self.ok(format!("Copied to {} (private to you).", file.display()));
            }
            Err(e) => self.ok(format!("Could not copy the log: {e}")),
        }
    }

    pub(super) fn crash_buttons(&self) -> Vec<Btn> {
        vec![("I have seen it", ACCENT, 'y'), ("Other boot", TEXT, 'p'), ("Read again", TEXT, 'r'), ("Copy to my home", TEXT, 'c')]
    }

    pub(super) fn render_crash(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn CrashPort) {
        if !self.crash.loaded {
            self.crash_reload(p);
        }
        let mut y = a.y;
        let what = match self.crash.view {
            0 => "the session that ended unexpectedly",
            1 => "the boot before this one",
            _ => "this boot",
        };
        match self.crash_note(p) {
            Some(u) => {
                let t = g.fit(15.0 * s, &format!("The session that started {} ended unexpectedly.", cl::wall(u.started)), Face::Bold, a.w);
                g.text(a.x, y, 15.0 * s, crate::fx::AMBER, &t, Face::Bold);
                y += 22.0 * s;
                let t = g.fit(13.0 * s, &format!("Up for about {} s. Last kernel line: {}", u.uptime_s, if u.last_line.is_empty() { "(none kept)" } else { &u.last_line }), Face::Mono, a.w);
                g.text(a.x, y, 13.0 * s, MUTED, &t, Face::Mono);
                y += 22.0 * s;
            }
            None => {
                for w in g.wrap(14.0 * s, "No session ended unexpectedly since you last cleared the note.", Face::Sans, a.w) {
                    g.text(a.x, y, 14.0 * s, MUTED, &w, Face::Sans);
                    y += 22.0 * s;
                }
            }
        }
        let head = if self.crash.name.is_empty() { format!("Showing {what}: no kernel log is kept for it.") } else { format!("Showing {what}: last {} lines of {}", self.crash.lines.len(), self.crash.name) };
        let t = g.fit(13.0 * s, &head, Face::Sans, a.w);
        g.text(a.x, y, 13.0 * s, DIM, &t, Face::Sans);
        y += 22.0 * s;
        let lh = 16.0 * s;
        let room = (((a.y + a.h - y) / lh) as usize).max(1);
        let n = self.crash.lines.len();
        // the window of lines ends at the scroll position (the end of the log is where a crash is)
        let end = (self.crash.scroll + 1).min(n);
        let first = end.saturating_sub(room);
        for l in &self.crash.lines[first..end] {
            let t = g.fit(12.0 * s, l, Face::Mono, a.w);
            g.text(a.x, y, 12.0 * s, TEXT, &t, Face::Mono);
            y += lh;
        }
        if n > room {
            let bar_h = (a.y + a.h - (a.y + 66.0 * s)).max(10.0);
            let thumb = (room as f32 / n as f32 * bar_h).max(12.0 * s);
            let top = a.y + 66.0 * s + (end.saturating_sub(room) as f32 / (n - room).max(1) as f32) * (bar_h - thumb);
            g.rect(a.x + a.w - 4.0 * s, top, 3.0 * s, thumb, DIM);
        }
    }
}

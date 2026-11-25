//! DOCTOR: what is wrong with this computer, in plain words, each finding with ONE fix button; and the redacted support bundle. The tile
//! itself lives in this crate (it does not depend on the agent or the shared crate). The rules and their facts stay in
//! `iyakki_shared::doctor` and the bundle and its redaction in `iyakki_shared::diagshare`; the shell's port ([`DoctorPort`]) reads the
//! machine, judges it, builds the bundle and saves it. This file is the state and the keys; `ui.rs` draws.
//!
//! * The cheap facts are read when the tile opens and when the person checks again; the slow ones (a connection to each local
//!   model server, the audit log) come from a thread of their own, so the screen never waits. The list is drawn at once and gains
//!   those findings when the thread answers.
//! * A fix button opens the Settings page where the person does the thing, or (administrators) pushes developer mode's switch-off
//!   time four hours on. It never accepts a permission popup, a licence or a vault password and never restarts anything.
//! * Share diagnostics shows the exact text of the bundle (redacted) and saves THAT text to a file in the person's home folder or on a
//!   USB stick opened by DISKS. Nothing is sent anywhere.
//! * The "Manual tests" tab is not drawn here: the shell owns it and the tile hands the tab to it through [`ManualPane`].
//!
//! Keys: Up/Down choose a finding, Enter does its fix, `r` checks again, `s` shares diagnostics, `m` the manual tests, Esc leaves (closes
//! the preview first).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

pub use iyakki_tile_api::doctor::DoctorPort;
use iyakki_tile_api::doctor::{Dest, Finding, FixAction, Severity, SlowDone};
use iyakki_uikit::gfx::{rgb, Icon};
use iyakki_uikit::scrolllist::ScrollList;
use iyakki_uikit::tooltip::Tooltip;
use iyakki_uikit::ui::{clipboard, Event, Key};

mod ui;
#[cfg(test)]
mod tests;

/// What the tile answers to an event: stay on the screen, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

/// What the manual tab answers to an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneOut {
    Stay,
    /// Esc at the top of the tab: back to the findings.
    Leave,
}

/// The "Manual tests" tab, owned by the shell and reached through this small trait (the tile draws its two tab buttons and nothing more).
pub trait ManualPane {
    /// The checklists have been read (the tab was opened once).
    fn is_open(&self) -> bool;
    /// Read the checklists (the first time the tab is shown).
    fn open(&mut self);
    fn handle(&mut self, ev: &Event) -> PaneOut;
    fn draw(&mut self, g: &mut iyakki_uikit::gfx::Gfx, area: iyakki_uikit::ui::R, s: f32);
    fn wheel_px(&mut self, px: i32) -> bool;
    /// The tab changed since the last frame (reading takes the flag).
    fn take_dirty(&mut self) -> bool;
}

/// A pane for a shell that has no manual tab (tests of the findings): never open, never changes.
pub struct NoPane;

impl ManualPane for NoPane {
    fn is_open(&self) -> bool {
        false
    }
    fn open(&mut self) {}
    fn handle(&mut self, _ev: &Event) -> PaneOut {
        PaneOut::Leave
    }
    fn draw(&mut self, _g: &mut iyakki_uikit::gfx::Gfx, _area: iyakki_uikit::ui::R, _s: f32) {}
    fn wheel_px(&mut self, _px: i32) -> bool {
        false
    }
    fn take_dirty(&mut self) -> bool {
        false
    }
}

pub const ACCENT: [u8; 4] = rgb(236, 92, 80);

// click and hit ids
pub const T_RECHECK: usize = 1;
pub const T_SHARE: usize = 2;
pub const T_CLOSE: usize = 3;
pub const T_DISMISS: usize = 4;
pub const T_COPY: usize = 5;
pub const T_TAB_FINDINGS: usize = 6;
pub const T_TAB_MANUAL: usize = 7;
/// The fix button of finding `i`.
pub const T_FIX: usize = 100;
/// The save button for destination `i` of the preview.
pub const T_SAVE: usize = 300;
pub const A_LIST: usize = 61_310;
pub const A_PREVIEW: usize = 61_311;

/// The preview of the bundle on screen.
pub struct Preview {
    /// The exact bytes that get saved.
    pub text: String,
    pub lines: Vec<String>,
    pub dests: Vec<Dest>,
    pub scroll: ScrollList,
    /// Where it was saved, once it was.
    pub saved: Option<PathBuf>,
}

pub struct DoctorTile {
    pub admin: bool,
    pub found: Vec<Finding>,
    pub sel: usize,
    pub list: ScrollList,
    pub tip: Tooltip,
    /// The answer to the last action: (text, ok).
    pub msg: Option<(String, bool)>,
    pub preview: Option<Preview>,
    /// The slow reads are still going.
    pub checking: bool,
    slow_rx: Option<Receiver<SlowDone>>,
    pub request: Option<(String, String)>,
    pub dirty: bool,
    /// The "Manual tests" tab is the one showing.
    pub tab_manual: bool,
    /// Checks made since the tile opened (tests).
    pub rounds: u32,
}

impl DoctorTile {
    /// The tile over a port; the machine is read once now.
    pub fn new(port: &dyn DoctorPort) -> DoctorTile {
        let mut a = DoctorTile {
            admin: port.admin(),
            found: Vec::new(),
            sel: 0,
            list: ScrollList::default(),
            tip: Tooltip::new(),
            msg: None,
            preview: None,
            checking: false,
            slow_rx: None,
            request: None,
            dirty: true,
            tab_manual: false,
            rounds: 0,
        };
        a.check(port);
        a
    }

    /// Read the machine again: the cheap facts now, the slow ones on a thread.
    pub fn check(&mut self, port: &dyn DoctorPort) {
        self.rounds += 1;
        let found = port.gather();
        self.rediagnose(found);
        let job = port.slow_job();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("iyakki-doctor".into()).spawn(move || {
            let _ = tx.send(job());
        });
        self.checking = spawned.is_ok();
        self.slow_rx = if spawned.is_ok() { Some(rx) } else { None };
        self.dirty = true;
    }

    fn rediagnose(&mut self, found: Vec<Finding>) {
        let keep = self.found.get(self.sel).map(|f| f.id);
        self.found = found;
        self.sel = keep.and_then(|id| self.found.iter().position(|f| f.id == id)).unwrap_or(0).min(self.found.len().saturating_sub(1));
    }

    /// The answer of the slow thread, if it came.
    pub fn take_slow(&mut self, port: &dyn DoctorPort) {
        let Some(rx) = &self.slow_rx else { return };
        if let Ok(out) = rx.try_recv() {
            self.slow_rx = None;
            self.checking = false;
            let found = port.finish_slow(out);
            self.rediagnose(found);
            self.dirty = true;
        }
    }

    /// Wait for the slow reads to come back (tests and the harness: a blocking read, no polling loop).
    pub fn wait_checks(&mut self, port: &dyn DoctorPort) {
        if let Some(rx) = self.slow_rx.take() {
            self.checking = false;
            if let Ok(out) = rx.recv() {
                let found = port.finish_slow(out);
                self.rediagnose(found);
            }
            self.dirty = true;
        }
    }

    pub fn ok(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), true));
        self.dirty = true;
    }

    pub fn fail(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), false));
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ fixes

    /// The fix of finding `i`.
    pub fn do_fix(&mut self, i: usize, port: &dyn DoctorPort) {
        let Some(f) = self.found.get(i).cloned() else { return };
        let Some(fix) = f.fix else { return };
        port.audit("doctor.fix", f.id);
        match fix.action {
            FixAction::Settings(item) => {
                self.request = Some(("settings".into(), item.into()));
            }
            FixAction::ExtendDeveloperMode => {
                if !self.admin {
                    // not allowed to change it: the page says who can
                    self.request = Some(("settings".into(), "tab:developer".into()));
                    return;
                }
                match port.extend_devmode(4) {
                    Ok(until) => {
                        port.audit("devmode.extend", "4 h (from DOCTOR)");
                        self.ok(format!("Developer mode now switches off at {}.", port.stamp(until)));
                        self.check(port);
                    }
                    Err(e) => self.fail(e),
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ share diagnostics

    /// Build the bundle and show exactly what would be saved.
    pub fn open_share(&mut self, port: &dyn DoctorPort) {
        let text = port.build_share();
        let lines = text.lines().map(str::to_string).collect();
        let dests = port.share_dests();
        self.preview = Some(Preview { text, lines, dests, scroll: ScrollList::default(), saved: None });
        port.audit("doctor.share.preview", "");
        self.msg = None;
        self.dirty = true;
    }

    /// Save to destination `i` of the open preview.
    pub fn save_to(&mut self, i: usize, port: &dyn DoctorPort) {
        let Some(p) = self.preview.as_mut() else { return };
        if p.saved.is_some() {
            return;
        }
        let Some(d) = p.dests.get(i).cloned() else { return };
        match port.save_share(&d.dir) {
            Ok(path) => {
                p.saved = Some(path.clone());
                port.audit("doctor.share.saved", &format!("{} bytes to {}", p.text.len(), d.label));
                self.ok(format!("Saved on {}: {}", d.label, path.display()));
            }
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------------------------------------ manual tests tab

    /// Show the "Manual tests" tab (its checklists are read the first time).
    pub fn open_manual(&mut self, port: &dyn DoctorPort, pane: &mut dyn ManualPane) {
        if !pane.is_open() {
            pane.open();
            if pane.is_open() {
                port.audit("doctor.manual.open", "");
            }
        }
        self.tab_manual = true;
        self.preview = None;
        self.dirty = true;
    }

    /// One event for the manual tab (shown): Esc at its top leaves to the findings.
    fn handle_manual(&mut self, ev: &Event, pane: &mut dyn ManualPane) -> Answer {
        match ev {
            Event::Tab(T_TAB_FINDINGS) => self.tab_manual = false,
            Event::Tab(T_TAB_MANUAL) => {}
            other => {
                if pane.handle(other) == PaneOut::Leave {
                    self.tab_manual = false;
                }
            }
        }
        self.dirty = true;
        Answer::Stay
    }

    // ------------------------------------------------------------------------------------------------ keys

    fn handle_preview(&mut self, ev: &Event, port: &dyn DoctorPort) -> Answer {
        match ev {
            Event::Key(Key::Esc) | Event::Tab(T_CLOSE) => {
                self.preview = None;
                self.dirty = true;
            }
            Event::Tab(T_COPY) => self.copy_saved_path(),
            Event::Key(Key::Char('c')) => self.copy_saved_path(),
            Event::Tab(n) if *n >= T_SAVE => self.save_to(*n - T_SAVE, port),
            Event::Key(Key::Char(c)) if c.is_ascii_digit() && *c != '0' => self.save_to(*c as usize - '1' as usize, port),
            Event::Key(Key::Char('s')) | Event::Key(Key::Enter) => self.save_to(0, port),
            Event::Key(k) => {
                if let Some(p) = self.preview.as_mut() {
                    match k {
                        Key::Up => p.scroll.by(-p.scroll.row),
                        Key::Down => p.scroll.by(p.scroll.row),
                        other => {
                            p.scroll.key(other);
                        }
                    }
                }
                self.dirty = true;
            }
            other => {
                if let Some(p) = self.preview.as_mut() {
                    p.scroll.event(other, A_PREVIEW);
                }
            }
        }
        Answer::Stay
    }

    fn copy_saved_path(&mut self) {
        if let Some(path) = self.preview.as_ref().and_then(|p| p.saved.clone()) {
            clipboard::copy(&path.display().to_string());
            self.ok("The file's path is copied.");
        }
    }

    /// Open the share preview or the manual tab when asked by another tile (Settings' button): `open_item("share")`.
    pub fn open_item(&mut self, item: &str, port: &dyn DoctorPort, pane: &mut dyn ManualPane) {
        if item == "share" {
            self.open_share(port);
        } else if item == "manual" {
            self.open_manual(port, pane);
        }
    }

    // ------------------------------------------------------------------------------------------------ the App's side

    pub fn title(&self) -> &'static str {
        "Doctor"
    }
    pub fn page(&self) -> String {
        if self.preview.is_some() {
            "share".into()
        } else if self.tab_manual {
            "manual".into()
        } else {
            "findings".into()
        }
    }
    pub fn icon(&self) -> Icon {
        Icon::Doctor
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        if self.tab_manual {
            "n/p next/previous scenario · 1 pass · 2 fail · 3 cannot test · 4 skip · Enter next step · t note · e export · i import · Esc back".into()
        } else if self.preview.is_some() {
            "s save in my home folder · 1-5 save in a place · ↑↓ scroll · Esc close".into()
        } else {
            "↑↓ choose · Enter fix · r check again · s share diagnostics · m manual tests · Esc back".into()
        }
    }
    pub fn help(&self) -> Vec<String> {
        [
            "DOCTOR looks at this computer and lists what is wrong in plain words. It only reads; it never changes anything by itself.",
            "Up / Down · choose a finding      Enter · its fix button      r · check again",
            "A fix button opens the page where you do the thing (the firewall, the vault, Sound, Time, the dev cluster). It never accepts a permission popup, a licence or your vault password for you, and it never restarts anything.",
            "Administrators also see the technical detail (addresses, ports, log lines) under each finding.",
            "s · Share diagnostics: builds a support bundle with secrets, page addresses, cookies and passwords removed and shows you exactly what would be saved. Save it in your home folder or on a USB stick (the drives DISKS opened) and send it yourself. Nothing is uploaded.",
            "m · Manual tests: checklists a person goes through (sound by ear, touch, Bluetooth, camera, ...). Left: the scenarios by area with their state; right: the steps with the expected result. n / p next and previous scenario, 1 Pass, 2 Fail, 3 Cannot test, 4 Skip, Enter next step, t a note, f only this machine, o only what needs a person, e export all answers as one file, i merge the files of other machines. Each Pass, Fail or Cannot test writes a small evidence file in your home folder (manual-tests/human); Skip writes nothing.",
            "In the preview: s or Enter saves in the first place, 1 to 5 pick the place, c copies the saved file's path, Esc closes.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn handle(&mut self, ev: &Event, port: &dyn DoctorPort, pane: &mut dyn ManualPane) -> Answer {
        self.tip.event(ev);
        if self.tab_manual && pane.is_open() {
            return self.handle_manual(ev, pane);
        }
        if self.preview.is_some() {
            return self.handle_preview(ev, port);
        }
        match ev {
            Event::Key(Key::Esc) => return Answer::Back,
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(T_RECHECK) | Event::Key(Key::Char('r')) => {
                self.msg = None;
                self.check(port);
            }
            Event::Tab(T_SHARE) | Event::Key(Key::Char('s')) => self.open_share(port),
            Event::Tab(T_TAB_MANUAL) | Event::Key(Key::Char('m')) => self.open_manual(port, pane),
            Event::Tab(T_TAB_FINDINGS) => {}
            Event::Tab(n) if *n >= T_FIX && *n < T_SAVE => {
                self.sel = *n - T_FIX;
                self.do_fix(*n - T_FIX, port);
            }
            Event::Row(i) => self.sel = (*i).min(self.found.len().saturating_sub(1)),
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(self.found.len().saturating_sub(1)),
            Event::Key(Key::Enter) => {
                let i = self.sel;
                self.do_fix(i, port);
            }
            Event::Key(k) => {
                self.list.key(k);
            }
            other => {
                self.list.event(other, A_LIST);
            }
        }
        self.dirty = true;
        Answer::Stay
    }

    pub fn scroll_px(&mut self, px: i32, pane: &mut dyn ManualPane) -> bool {
        if self.tab_manual && pane.is_open() {
            return pane.wheel_px(px);
        }
        match self.preview.as_mut() {
            Some(p) => p.scroll.wheel_px(px),
            None => self.list.wheel_px(px),
        }
    }
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    pub fn wants_redraw(&mut self, port: &dyn DoctorPort, pane: &mut dyn ManualPane) -> bool {
        self.take_slow(port);
        self.tip.take_changed() | std::mem::take(&mut self.dirty) | self.checking | pane.take_dirty()
    }
    pub fn render(&mut self, g: &mut iyakki_uikit::gfx::Gfx, area: iyakki_uikit::ui::R, s: f32, port: &dyn DoctorPort, pane: &mut dyn ManualPane, tick: u64) {
        self.take_slow(port);
        self.draw(g, area, s, pane, tick);
    }
}

/// The colour a severity is drawn in is the drawing's business; this is the word the tests and the accessibility tree use.
pub fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Problem => "problem",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

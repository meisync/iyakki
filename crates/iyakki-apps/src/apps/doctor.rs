//! DOCTOR: what is wrong with this computer, in plain words, each finding with ONE fix button; and the redacted support bundle.
//!
//! The rules and their facts live in `iyakki_shared::doctor` (pure, tested with made-up machines); the bundle and its redaction in
//! `iyakki_shared::diagshare`. This file is the state and the keys; `doctor_ui.rs` draws; `doctor_facts.rs` reads the real machine.
//!
//! * The cheap facts are read when the tile opens and when the person checks again; the slow ones (a connection to each local
//!   model server, the audit log) come from a thread of their own, so the screen never waits. The list is drawn at once and gains
//!   those findings when the thread answers.
//! * A fix button opens the Settings page where the person does the thing, or (administrators) pushes developer mode's switch-off
//!   time four hours on. It never accepts a permission popup, a licence or a vault password and never restarts anything.
//! * Share diagnostics shows the exact text of the bundle (redacted, see `diagshare.rs`) and saves THAT text to a file in the
//!   person's home folder or on a USB stick opened by DISKS. Nothing is sent anywhere.
//!
//! Keys: Up/Down choose a finding, Enter does its fix, `r` checks again, `s` shares diagnostics, Esc leaves (closes the preview first).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use iyakki_shared::diagshare::{self, Bundle};
use iyakki_shared::doctor::{Action, Finding};
use iyakki_shared::doctor::{self, Facts};

pub use super::doctor_manual::ManualTab;
pub use super::doctor_port::Parts;
use super::doctor_facts::SlowOut;
use super::doctor_port::{CxPort, DoctorPort};
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx, Icon};
use crate::scrolllist::ScrollList;
use crate::tooltip::Tooltip;
use crate::ui::{clipboard, Event, Key, R};

pub const ACCENT: [u8; 4] = rgb(236, 92, 80);

// click and hit ids
pub(super) const T_RECHECK: usize = 1;
pub(super) const T_SHARE: usize = 2;
pub(super) const T_CLOSE: usize = 3;
pub(super) const T_DISMISS: usize = 4;
pub(super) const T_COPY: usize = 5;
pub(super) const T_TAB_FINDINGS: usize = 6;
pub(super) const T_TAB_MANUAL: usize = 7;
/// The fix button of finding `i`.
pub(super) const T_FIX: usize = 100;
/// The save button for destination `i` of the preview.
pub(super) const T_SAVE: usize = 300;
pub(super) const A_LIST: usize = 61_310;
pub(super) const A_PREVIEW: usize = 61_311;

/// One place the bundle can be saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Dest {
    pub label: String,
    pub dir: PathBuf,
}

/// The preview of the bundle on screen.
pub(super) struct Preview {
    pub bundle: Bundle,
    /// `diagshare::preview(&bundle)`, the exact bytes that get saved.
    pub text: String,
    pub lines: Vec<String>,
    pub dests: Vec<Dest>,
    pub scroll: ScrollList,
    /// Where it was saved, once it was.
    pub saved: Option<PathBuf>,
}

pub struct DoctorApp {
    pub(super) parts: Parts,
    pub(super) admin: bool,
    pub(super) facts: Facts,
    pub(super) found: Vec<Finding>,
    pub(super) sel: usize,
    pub(super) list: ScrollList,
    pub(super) tip: Tooltip,
    /// The answer to the last action: (text, ok).
    pub(super) msg: Option<(String, bool)>,
    pub(super) preview: Option<Preview>,
    /// The slow reads are still going.
    pub(super) checking: bool,
    pub(super) slow_rx: Option<Receiver<SlowOut>>,
    pub(super) request: Option<(String, String)>,
    pub(super) dirty: bool,
    /// The "Manual tests" tab (made when it is first opened) and whether it is the one showing.
    pub(super) manual: Option<ManualTab>,
    pub(super) tab_manual: bool,
    /// Checks made since the tile opened (tests).
    pub(super) rounds: u32,
}

impl DoctorApp {
    pub fn new(cx: &AppCx) -> DoctorApp {
        DoctorApp::with_parts(cx, Parts::real())
    }

    pub fn with_parts(cx: &AppCx, parts: Parts) -> DoctorApp {
        DoctorApp::with_port(&CxPort(cx), parts)
    }

    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn DoctorPort, parts: Parts) -> DoctorApp {
        let mut a = DoctorApp {
            parts,
            admin: port.admin(),
            facts: Facts::default(),
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
            manual: None,
            tab_manual: false,
            rounds: 0,
        };
        a.check(port);
        a
    }

    /// Read the machine again: the cheap facts now, the slow ones on a thread.
    pub(super) fn check(&mut self, port: &dyn DoctorPort) {
        self.rounds += 1;
        self.facts = (self.parts.gather)(port);
        self.rediagnose();
        let inp = port.slow_input();
        let probe = self.parts.slow.clone();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("iyakki-doctor".into()).spawn(move || {
            let _ = tx.send(probe(inp));
        });
        self.checking = spawned.is_ok();
        self.slow_rx = if spawned.is_ok() { Some(rx) } else { None };
        self.dirty = true;
    }

    fn rediagnose(&mut self) {
        let keep = self.found.get(self.sel).map(|f| f.id);
        self.found = doctor::diagnose(&self.facts);
        self.sel = keep.and_then(|id| self.found.iter().position(|f| f.id == id)).unwrap_or(0).min(self.found.len().saturating_sub(1));
    }

    /// The answer of the slow thread, if it came.
    fn take_slow(&mut self) {
        let Some(rx) = &self.slow_rx else { return };
        if let Ok(out) = rx.try_recv() {
            out.apply(&mut self.facts);
            self.slow_rx = None;
            self.checking = false;
            self.rediagnose();
            self.dirty = true;
        }
    }

    pub(super) fn ok(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), true));
        self.dirty = true;
    }

    pub(super) fn fail(&mut self, m: impl Into<String>) {
        self.msg = Some((m.into(), false));
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ fixes

    /// The fix of finding `i`.
    pub(super) fn do_fix(&mut self, i: usize, port: &dyn DoctorPort) {
        let Some(f) = self.found.get(i).cloned() else { return };
        let Some(fix) = f.fix else { return };
        port.audit("doctor.fix", f.id);
        match fix.action {
            Action::Settings(item) => {
                self.request = Some(("settings".into(), item.into()));
            }
            Action::ExtendDeveloperMode => {
                if !self.admin {
                    // not allowed to change it: the page says who can
                    self.request = Some(("settings".into(), "tab:developer".into()));
                    return;
                }
                match port.extend_devmode(4) {
                    Ok(until) => {
                        port.audit("devmode.extend", "4 h (from DOCTOR)");
                        self.ok(format!("Developer mode now switches off at {}.", iyakki_shared::devmode::stamp(until)));
                        self.check(port);
                    }
                    Err(e) => self.fail(e),
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ share diagnostics

    /// Build the bundle and show exactly what would be saved.
    pub(super) fn open_share(&mut self, port: &dyn DoctorPort) {
        let inp = (self.parts.inputs)(port, &self.found);
        let bundle = diagshare::build(&inp);
        let text = diagshare::preview(&bundle);
        let lines = text.lines().map(str::to_string).collect();
        let home = (self.parts.home)(port);
        let mut dests = vec![Dest { label: "my home folder".into(), dir: home.clone() }];
        // USB sticks and cards that DISKS opened in the home folder
        let mut media: Vec<Dest> = std::fs::read_dir(home.join("media"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| Dest { label: e.file_name().to_string_lossy().into_owned(), dir: e.path() })
            .collect();
        media.sort_by(|a, b| a.label.cmp(&b.label));
        media.truncate(4);
        dests.extend(media);
        self.preview = Some(Preview { bundle, text, lines, dests, scroll: ScrollList::default(), saved: None });
        port.audit("doctor.share.preview", "");
        self.msg = None;
        self.dirty = true;
    }

    /// Save to destination `i` of the open preview.
    pub(super) fn save_to(&mut self, i: usize, port: &dyn DoctorPort) {
        let now = (self.parts.now)();
        let Some(p) = self.preview.as_mut() else { return };
        if p.saved.is_some() {
            return;
        }
        let Some(d) = p.dests.get(i).cloned() else { return };
        match diagshare::save(&p.bundle, &d.dir, &diagshare::file_name(now)) {
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
    pub(super) fn open_manual(&mut self, port: &dyn DoctorPort) {
        if self.manual.is_none() {
            self.manual = Some(ManualTab::open(port, self.parts.now.clone()));
            port.audit("doctor.manual.open", "");
        }
        self.tab_manual = true;
        self.preview = None;
        self.dirty = true;
    }

    /// One event for the manual tab (shown): Esc at its top leaves to the findings.
    pub(super) fn handle_manual(&mut self, ev: &Event, port: &dyn DoctorPort) -> Flow {
        match ev {
            Event::Tab(T_TAB_FINDINGS) => self.tab_manual = false,
            Event::Tab(T_TAB_MANUAL) => {}
            other => {
                if let Some(m) = self.manual.as_mut() {
                    if m.handle(other, port) == super::doctor_manual::Out::Leave {
                        self.tab_manual = false;
                    }
                }
            }
        }
        self.dirty = true;
        Flow::Stay
    }

    // ------------------------------------------------------------------------------------------------ keys

    fn handle_preview(&mut self, ev: &Event, port: &dyn DoctorPort) -> Flow {
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
        Flow::Stay
    }

    fn copy_saved_path(&mut self) {
        if let Some(path) = self.preview.as_ref().and_then(|p| p.saved.clone()) {
            clipboard::copy(&path.display().to_string());
            self.ok("The file's path is copied.");
        }
    }

    /// Open the share preview when asked by another tile (Settings' button): `open_item("share")`.
    pub(super) fn open_item_inner(&mut self, item: &str, port: &dyn DoctorPort) {
        if item == "share" {
            self.open_share(port);
        } else if item == "manual" {
            self.open_manual(port);
        }
    }
}

impl App for DoctorApp {
    fn title(&self) -> &'static str {
        "Doctor"
    }
    fn page(&self) -> String {
        if self.preview.is_some() {
            "share".into()
        } else if self.tab_manual {
            "manual".into()
        } else {
            "findings".into()
        }
    }
    fn icon(&self) -> Icon {
        Icon::Doctor
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.tab_manual {
            "n/p next/previous scenario · 1 pass · 2 fail · 3 cannot test · 4 skip · Enter next step · t note · e export · i import · Esc back".into()
        } else if self.preview.is_some() {
            "s save in my home folder · 1-5 save in a place · ↑↓ scroll · Esc close".into()
        } else {
            "↑↓ choose · Enter fix · r check again · s share diagnostics · m manual tests · Esc back".into()
        }
    }
    fn help(&self) -> Vec<String> {
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

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let port = CxPort(cx);
        self.tip.event(ev);
        if self.tab_manual && self.manual.is_some() {
            return self.handle_manual(ev, &port);
        }
        if self.preview.is_some() {
            return self.handle_preview(ev, &port);
        }
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(T_RECHECK) | Event::Key(Key::Char('r')) => {
                self.msg = None;
                self.check(&port);
            }
            Event::Tab(T_SHARE) | Event::Key(Key::Char('s')) => self.open_share(&port),
            Event::Tab(T_TAB_MANUAL) | Event::Key(Key::Char('m')) => self.open_manual(&port),
            Event::Tab(T_TAB_FINDINGS) => {}
            Event::Tab(n) if *n >= T_FIX && *n < T_SAVE => {
                self.sel = *n - T_FIX;
                self.do_fix(*n - T_FIX, &port);
            }
            Event::Row(i) => self.sel = (*i).min(self.found.len().saturating_sub(1)),
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(self.found.len().saturating_sub(1)),
            Event::Key(Key::Enter) => {
                let i = self.sel;
                self.do_fix(i, &port);
            }
            Event::Key(k) => {
                self.list.key(k);
            }
            other => {
                self.list.event(other, A_LIST);
            }
        }
        self.dirty = true;
        Flow::Stay
    }

    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        if let Some(m) = self.manual.as_mut().filter(|_| self.tab_manual) {
            return m.main.wheel_px(px) || m.nav.wheel_px(px);
        }
        match self.preview.as_mut() {
            Some(p) => p.scroll.wheel_px(px),
            None => self.list.wheel_px(px),
        }
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        self.open_item_inner(item, &CxPort(cx));
    }
    fn wants_redraw(&mut self) -> bool {
        self.take_slow();
        self.tip.take_changed() | std::mem::take(&mut self.dirty) | self.checking | self.manual.as_mut().is_some_and(|m| std::mem::take(&mut m.dirty))
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.take_slow();
        self.draw(g, area, s, cx, tick);
    }
}

//! DOCTOR: what is wrong with this computer, in plain words, each finding with ONE fix button; and the redacted support bundle.
//!
//! The rules and their facts live in `iyakki_agent::doctor` (pure, tested with made-up machines); the bundle and its redaction in
//! `iyakki_agent::diagshare`. This file is the state and the keys; `doctor_ui.rs` draws; `doctor_facts.rs` reads the real machine.
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
use std::sync::Arc;

use iyakki_agent::diagshare::{self, Bundle};
use iyakki_agent::doctor::{self, Action, Facts, Finding};

use super::doctor_facts::{self as facts, SlowOut, SlowProbe};
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
/// The fix button of finding `i`.
pub(super) const T_FIX: usize = 100;
/// The save button for destination `i` of the preview.
pub(super) const T_SAVE: usize = 300;
pub(super) const A_LIST: usize = 61_310;
pub(super) const A_PREVIEW: usize = 61_311;

/// How the tile reads the world; tests replace every part.
#[derive(Clone)]
pub struct Parts {
    pub gather: Arc<dyn Fn(&AppCx) -> Facts + Send + Sync>,
    pub slow: SlowProbe,
    pub inputs: Arc<dyn Fn(&AppCx, &[Finding]) -> diagshare::Inputs + Send + Sync>,
    /// The person's home folder (the first place a bundle can be saved).
    pub home: Arc<dyn Fn(&AppCx) -> PathBuf + Send + Sync>,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Parts {
    pub fn real() -> Parts {
        Parts {
            gather: Arc::new(facts::gather),
            slow: Arc::new(facts::slow_probe),
            inputs: Arc::new(facts::bundle_inputs),
            home: Arc::new(|cx| iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir())),
            now: Arc::new(iyakki_agent::clock::now_unix),
        }
    }
}

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
    /// Checks made since the tile opened (tests).
    pub(super) rounds: u32,
}

impl DoctorApp {
    pub fn new(cx: &AppCx) -> DoctorApp {
        DoctorApp::with_parts(cx, Parts::real())
    }

    pub fn with_parts(cx: &AppCx, parts: Parts) -> DoctorApp {
        let mut a = DoctorApp {
            parts,
            admin: cx.session.policy.manage_settings,
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
            rounds: 0,
        };
        a.check(cx);
        a
    }

    /// Read the machine again: the cheap facts now, the slow ones on a thread.
    pub(super) fn check(&mut self, cx: &AppCx) {
        self.rounds += 1;
        self.facts = (self.parts.gather)(cx);
        self.rediagnose();
        let inp = facts::slow_input(cx);
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
            self.facts.models_unreachable = out.models_unreachable;
            self.facts.exposure_unexpected = out.exposure;
            self.facts.net = out.net;
            self.facts.dev_port_answers = out.dev_port_answers;
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
    pub(super) fn do_fix(&mut self, i: usize, cx: &AppCx) {
        let Some(f) = self.found.get(i).cloned() else { return };
        let Some(fix) = f.fix else { return };
        cx.audit.record(&cx.session.user, "doctor.fix", f.id);
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
                let d = iyakki_agent::devmode::DevMode::open(cx.services.dir());
                match d.extend(&cx.session.policy, &cx.session.user, 4) {
                    Ok(st) => {
                        cx.audit.record(&cx.session.user, "devmode.extend", "4 h (from DOCTOR)");
                        self.ok(format!("Developer mode now switches off at {}.", iyakki_agent::devmode::stamp(st.until)));
                        self.check(cx);
                    }
                    Err(e) => self.fail(e),
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ share diagnostics

    /// Build the bundle and show exactly what would be saved.
    pub(super) fn open_share(&mut self, cx: &AppCx) {
        let inp = (self.parts.inputs)(cx, &self.found);
        let bundle = diagshare::build(&inp);
        let text = diagshare::preview(&bundle);
        let lines = text.lines().map(str::to_string).collect();
        let home = (self.parts.home)(cx);
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
        cx.audit.record(&cx.session.user, "doctor.share.preview", "");
        self.msg = None;
        self.dirty = true;
    }

    /// Save to destination `i` of the open preview.
    pub(super) fn save_to(&mut self, i: usize, cx: &AppCx) {
        let now = (self.parts.now)();
        let Some(p) = self.preview.as_mut() else { return };
        if p.saved.is_some() {
            return;
        }
        let Some(d) = p.dests.get(i).cloned() else { return };
        match diagshare::save(&p.bundle, &d.dir, &diagshare::file_name(now)) {
            Ok(path) => {
                p.saved = Some(path.clone());
                cx.audit.record(&cx.session.user, "doctor.share.saved", &format!("{} bytes to {}", p.text.len(), d.label));
                self.ok(format!("Saved on {}: {}", d.label, path.display()));
            }
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------------------------------------ keys

    fn handle_preview(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match ev {
            Event::Key(Key::Esc) | Event::Tab(T_CLOSE) => {
                self.preview = None;
                self.dirty = true;
            }
            Event::Tab(T_COPY) => self.copy_saved_path(),
            Event::Key(Key::Char('c')) => self.copy_saved_path(),
            Event::Tab(n) if *n >= T_SAVE => self.save_to(*n - T_SAVE, cx),
            Event::Key(Key::Char(c)) if c.is_ascii_digit() && *c != '0' => self.save_to(*c as usize - '1' as usize, cx),
            Event::Key(Key::Char('s')) | Event::Key(Key::Enter) => self.save_to(0, cx),
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
    pub(super) fn open_item_inner(&mut self, item: &str, cx: &AppCx) {
        if item == "share" {
            self.open_share(cx);
        }
    }
}

impl App for DoctorApp {
    fn title(&self) -> &'static str {
        "Doctor"
    }
    fn page(&self) -> String {
        if self.preview.is_some() { "share".into() } else { "findings".into() }
    }
    fn icon(&self) -> Icon {
        Icon::Doctor
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.preview.is_some() {
            "s save in my home folder · 1-5 save in a place · ↑↓ scroll · Esc close".into()
        } else {
            "↑↓ choose · Enter fix · r check again · s share diagnostics · Esc back".into()
        }
    }
    fn help(&self) -> Vec<String> {
        [
            "DOCTOR looks at this computer and lists what is wrong in plain words. It only reads; it never changes anything by itself.",
            "Up / Down · choose a finding      Enter · its fix button      r · check again",
            "A fix button opens the page where you do the thing (the firewall, the vault, Sound, Time, the dev cluster). It never accepts a permission popup, a licence or your vault password for you, and it never restarts anything.",
            "Administrators also see the technical detail (addresses, ports, log lines) under each finding.",
            "s · Share diagnostics: builds a support bundle with secrets, page addresses, cookies and passwords removed and shows you exactly what would be saved. Save it in your home folder or on a USB stick (the drives DISKS opened) and send it yourself. Nothing is uploaded.",
            "In the preview: s or Enter saves in the first place, 1 to 5 pick the place, c copies the saved file's path, Esc closes.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.tip.event(ev);
        if self.preview.is_some() {
            return self.handle_preview(ev, cx);
        }
        match ev {
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Tab(T_DISMISS) => self.msg = None,
            Event::Tab(T_RECHECK) | Event::Key(Key::Char('r')) => {
                self.msg = None;
                self.check(cx);
            }
            Event::Tab(T_SHARE) | Event::Key(Key::Char('s')) => self.open_share(cx),
            Event::Tab(n) if *n >= T_FIX && *n < T_SAVE => {
                self.sel = *n - T_FIX;
                self.do_fix(*n - T_FIX, cx);
            }
            Event::Row(i) => self.sel = (*i).min(self.found.len().saturating_sub(1)),
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(self.found.len().saturating_sub(1)),
            Event::Key(Key::Enter) => {
                let i = self.sel;
                self.do_fix(i, cx);
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
        match self.preview.as_mut() {
            Some(p) => p.scroll.wheel_px(px),
            None => self.list.wheel_px(px),
        }
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        self.open_item_inner(item, cx);
    }
    fn wants_redraw(&mut self) -> bool {
        self.take_slow();
        self.tip.take_changed() | std::mem::take(&mut self.dirty) | self.checking
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.take_slow();
        self.draw(g, area, s, cx, tick);
    }
}

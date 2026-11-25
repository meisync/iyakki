//! The DOCTOR tile on a fake port: no machine, no shell, no files outside a test's own recording. The slow reads are a job the test holds
//! back and lets go with a channel (nothing sleeps, nothing polls on a timer).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use iyakki_tile_api::doctor::{Dest, Finding, Fix, FixAction, Severity, SlowDone, SlowJob};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

fn finding(id: &'static str, sev: Severity, text: &str, detail: &str, fix: Option<Fix>) -> Finding {
    Finding { id, severity: sev, text: text.into(), detail: detail.into(), fix }
}

fn settings(id: &'static str, text: &str, item: &'static str) -> Finding {
    finding(id, Severity::Problem, text, "detail 192.0.2.7:7979", Some(Fix { label: "Open it", action: FixAction::Settings(item) }))
}

fn sick() -> Vec<Finding> {
    vec![
        settings("cluster-stopped", "The dev cluster is stopped.", "tab:devcluster"),
        settings("firewall-dev-port", "The firewall blocks the developer-mode port.", "tab:firewall"),
        finding("dev-mode-ending", Severity::Warning, "Developer mode switches off soon.", "until 1800001500", Some(Fix { label: "Keep it on", action: FixAction::ExtendDeveloperMode })),
        settings("vault-locked", "The vault is locked.", "tab:vault"),
        finding("engine-crashed", Severity::Note, "The web engine closed once.", "the helper closed the connection", None),
    ]
}

/// A port with scripted findings; it records what the tile asked.
struct Fake {
    admin: bool,
    found: RefCell<Vec<Finding>>,
    slow_found: Vec<Finding>,
    gate: Mutex<Option<Receiver<()>>>,
    audits: RefCell<Vec<String>>,
    extended: RefCell<Vec<u32>>,
    extend_answer: Result<i64, String>,
    share: String,
    dests: Vec<Dest>,
    saved: RefCell<Vec<PathBuf>>,
    save_answer: Result<PathBuf, String>,
    gathers: RefCell<u32>,
}

impl Fake {
    fn new(admin: bool, found: Vec<Finding>) -> Fake {
        Fake {
            admin,
            found: RefCell::new(found),
            slow_found: Vec::new(),
            gate: Mutex::new(None),
            audits: RefCell::new(Vec::new()),
            extended: RefCell::new(Vec::new()),
            extend_answer: Ok(1_800_000_000 + 4 * 3600),
            share: "IYAKKI diagnostics.\n=== Version ===\nIYAKKI test\n=== Doctor ===\nsomething".into(),
            dests: vec![Dest { label: "my home folder".into(), dir: PathBuf::from("/home/test") }],
            saved: RefCell::new(Vec::new()),
            save_answer: Ok(PathBuf::from("/home/test/iyakki-diagnostics-1.txt")),
            gathers: RefCell::new(0),
        }
    }
    /// The slow job waits for the returned sender.
    fn gated(self) -> (Fake, Sender<()>) {
        let (tx, rx) = channel();
        *self.gate.lock().unwrap() = Some(rx);
        (self, tx)
    }
    fn audit_text(&self) -> String {
        self.audits.borrow().join("\n")
    }
}

impl DoctorPort for Fake {
    fn admin(&self) -> bool {
        self.admin
    }
    fn gather(&self) -> Vec<Finding> {
        *self.gathers.borrow_mut() += 1;
        self.found.borrow().clone()
    }
    fn slow_job(&self) -> SlowJob {
        let gate = self.gate.lock().unwrap().take();
        Box::new(move || {
            if let Some(rx) = gate {
                let _ = rx.recv();
            }
            Box::new(()) as SlowDone
        })
    }
    fn finish_slow(&self, _done: SlowDone) -> Vec<Finding> {
        let mut all = self.found.borrow().clone();
        all.extend(self.slow_found.clone());
        all
    }
    fn build_share(&self) -> String {
        self.share.clone()
    }
    fn share_dests(&self) -> Vec<Dest> {
        self.dests.clone()
    }
    fn save_share(&self, dir: &Path) -> Result<PathBuf, String> {
        self.saved.borrow_mut().push(dir.to_path_buf());
        self.save_answer.clone()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audits.borrow_mut().push(format!("{action} {subject}"));
    }
    fn extend_devmode(&self, hours: u32) -> Result<i64, String> {
        self.extended.borrow_mut().push(hours);
        self.extend_answer.clone()
    }
    fn stamp(&self, unix: i64) -> String {
        format!("T{unix}")
    }
}

/// A manual pane that records what it was asked.
#[derive(Default)]
struct Pane {
    open: bool,
    opened: u32,
    events: Vec<Event>,
    leave_on_esc: bool,
    dirty: bool,
    wheeled: Vec<i32>,
    drawn: u32,
}

impl ManualPane for Pane {
    fn is_open(&self) -> bool {
        self.open
    }
    fn open(&mut self) {
        self.open = true;
        self.opened += 1;
    }
    fn handle(&mut self, ev: &Event) -> PaneOut {
        self.events.push(ev.clone());
        if self.leave_on_esc && *ev == Event::Key(Key::Esc) {
            PaneOut::Leave
        } else {
            PaneOut::Stay
        }
    }
    fn draw(&mut self, _g: &mut Gfx, _area: R, _s: f32) {
        self.drawn += 1;
    }
    fn wheel_px(&mut self, px: i32) -> bool {
        self.wheeled.push(px);
        true
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

struct Fx {
    t: DoctorTile,
    p: Fake,
    pane: Pane,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

fn fixture_with(admin: bool, found: Vec<Finding>, w: u32, h: u32, s: f32) -> Fx {
    let p = Fake::new(admin, found);
    let mut t = DoctorTile::new(&p);
    t.wait_checks(&p);
    let mut f = Fx { t, p, pane: Pane::default(), g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0 };
    f.draw();
    f
}

fn fixture() -> Fx {
    fixture_with(true, sick(), 1280, 720, 1.0)
}

impl Fx {
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.t.render(&mut self.g, self.area, self.s, &self.p, &mut self.pane, self.tick);
    }
    fn ev(&mut self, e: Event) -> Answer {
        let r = self.t.handle(&e, &self.p, &mut self.pane);
        self.draw();
        r
    }
    fn key(&mut self, k: Key) -> Answer {
        self.ev(Event::Key(k))
    }
    fn chr(&mut self, c: char) {
        self.key(Key::Char(c));
    }
    fn ids(&self) -> Vec<&'static str> {
        self.t.found.iter().map(|f| f.id).collect()
    }
    fn has_hit(&self, e: &Event) -> bool {
        self.g.hits.iter().any(|h| &h.ev == e)
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
}

fn assert_hits_sane(f: &Fx, what: &str) {
    let a = f.area;
    for h in &f.g.hits {
        let r = h.rect;
        assert!(r.w > 0.0 && r.h > 0.0, "{what}: {:?} is empty", h.label);
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: {:?} {:?} leaves {:?}", h.label, r, a);
    }
    for (i, p) in f.g.hits.iter().enumerate() {
        for q in &f.g.hits[i + 1..] {
            let (x0, y0) = (p.rect.x.max(q.rect.x), p.rect.y.max(q.rect.y));
            let (x1, y1) = ((p.rect.x + p.rect.w).min(q.rect.x + q.rect.w), (p.rect.y + p.rect.h).min(q.rect.y + q.rect.h));
            if x1 - x0 > 1.0 && y1 - y0 > 1.0 {
                let holds = |o: &R, n: &R| n.x >= o.x - 0.5 && n.y >= o.y - 0.5 && n.x + n.w <= o.x + o.w + 0.5 && n.y + n.h <= o.y + o.h + 0.5;
                assert!(holds(&p.rect, &q.rect) || holds(&q.rect, &p.rect), "{what}: {:?} {:?} overlaps {:?} {:?}", p.label, p.rect, q.label, q.rect);
            }
        }
    }
    assert!(f.g.rgba().iter().any(|b| *b != 0), "{what}: nothing drawn");
}

const SIZES: [(u32, u32, f32); 6] = [(360, 740, 1.0), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0), (2880, 1800, 2.25)];

#[test]
fn the_tile_lists_the_findings_the_port_gave_each_with_its_fix_button() {
    let f = fixture();
    assert_eq!(f.ids(), ["cluster-stopped", "firewall-dev-port", "dev-mode-ending", "vault-locked", "engine-crashed"]);
    assert_eq!(f.t.title(), "Doctor");
    assert!(f.t.hint().contains("Enter fix") && !f.t.help().is_empty());
    assert_eq!(f.g.hits.iter().filter(|h| h.label == "finding").count(), 5);
    for i in 0..4 {
        assert!(f.has_hit(&Event::Tab(T_FIX + i)), "finding {i} has its fix button");
    }
    assert!(!f.has_hit(&Event::Tab(T_FIX + 4)), "a finding without a fix has no button");
    let t = f.text();
    assert!(t.contains("5 things to look at") && t.contains("The dev cluster is stopped"), "{t}");
    assert!(f.has_hit(&Event::Tab(T_RECHECK)) && f.has_hit(&Event::Tab(T_SHARE)));
    assert_eq!(f.t.rounds, 1, "one check when the tile opened");
    assert!(f.p.audit_text().is_empty(), "reading is silent");
}

#[test]
fn administrators_see_the_technical_detail_and_other_people_do_not() {
    let admin = fixture();
    assert!(admin.text().contains("192.0.2.7"), "the address is in the detail line");
    let user = fixture_with(false, sick(), 1280, 720, 1.0);
    assert!(!user.text().contains("192.0.2.7"), "{}", user.text());
    assert!(user.text().contains("The firewall blocks the developer-mode port"), "the plain sentence stays");
}

#[test]
fn a_healthy_machine_shows_one_calm_line_and_still_offers_sharing() {
    let f = fixture_with(true, Vec::new(), 1280, 720, 1.0);
    assert!(f.t.found.is_empty());
    assert!(f.text().contains("Everything checked is fine."), "{}", f.text());
    assert!(!f.g.hits.iter().any(|h| h.label == "finding"));
    assert!(f.has_hit(&Event::Tab(T_SHARE)));
}

#[test]
fn up_down_and_clicks_choose_a_finding_and_esc_leaves() {
    let mut f = fixture();
    assert_eq!(f.t.sel, 0);
    f.key(Key::Down);
    f.key(Key::Down);
    assert_eq!(f.t.sel, 2);
    f.key(Key::Up);
    assert_eq!(f.t.sel, 1);
    for _ in 0..10 {
        f.key(Key::Down);
    }
    assert_eq!(f.t.sel, 4, "stops at the last");
    f.ev(Event::Row(3));
    assert_eq!(f.t.sel, 3);
    assert_eq!(f.key(Key::Esc), Answer::Back);
}

#[test]
fn a_fix_opens_the_settings_page_and_is_audited_and_nothing_else_happens() {
    let mut f = fixture();
    f.key(Key::Down);
    f.key(Key::Enter);
    assert_eq!(f.t.take_request(), Some(("settings".into(), "tab:firewall".into())));
    assert_eq!(f.t.take_request(), None, "taken once");
    assert!(f.p.audit_text().contains("doctor.fix firewall-dev-port"));
    f.ev(Event::Tab(T_FIX));
    assert_eq!(f.t.take_request(), Some(("settings".into(), "tab:devcluster".into())));
    // a finding without a fix does nothing
    f.ev(Event::Row(4));
    f.key(Key::Enter);
    assert_eq!(f.t.take_request(), None);
    assert!(f.p.extended.borrow().is_empty());
}

#[test]
fn the_developer_mode_fix_asks_the_port_for_four_hours_for_administrators_and_checks_again() {
    let mut f = fixture();
    let i = f.ids().iter().position(|x| *x == "dev-mode-ending").unwrap();
    let before = f.t.rounds;
    f.ev(Event::Tab(T_FIX + i));
    assert_eq!(f.p.extended.borrow().as_slice(), [4]);
    let a = f.p.audit_text();
    assert!(a.contains("doctor.fix dev-mode-ending") && a.contains("devmode.extend 4 h (from DOCTOR)"), "{a}");
    assert!(f.t.msg.as_ref().is_some_and(|m| m.1 && m.0.contains("switches off at T1800014400")), "{:?}", f.t.msg);
    assert_eq!(f.t.rounds, before + 1, "it checks again after the fix");
}

#[test]
fn an_ordinary_account_is_sent_to_the_page_and_the_port_is_not_asked_to_extend() {
    let mut f = fixture_with(false, sick(), 1280, 720, 1.0);
    let i = f.ids().iter().position(|x| *x == "dev-mode-ending").unwrap();
    f.ev(Event::Tab(T_FIX + i));
    assert!(f.p.extended.borrow().is_empty());
    assert_eq!(f.t.take_request(), Some(("settings".into(), "tab:developer".into())));
    assert!(!f.p.audit_text().contains("devmode.extend"));
}

#[test]
fn a_refused_extension_says_why_and_audits_only_the_fix() {
    let mut p = Fake::new(true, sick());
    p.extend_answer = Err("developer mode is not on".into());
    let mut t = DoctorTile::new(&p);
    t.wait_checks(&p);
    let i = t.found.iter().position(|x| x.id == "dev-mode-ending").unwrap();
    t.do_fix(i, &p);
    assert_eq!(t.msg, Some(("developer mode is not on".to_string(), false)));
    assert!(!p.audit_text().contains("devmode.extend"));
}

#[test]
fn the_slow_checks_arrive_later_and_the_screen_does_not_wait() {
    let (mut p, release) = Fake::new(true, sick()).gated();
    p.slow_found = vec![settings("model-unreachable", "A model server does not answer.", "tab:models")];
    let mut t = DoctorTile::new(&p);
    let mut pane = Pane::default();
    let mut g = Gfx::new(1280, 720);
    g.clear(rgb(8, 10, 22));
    t.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, &p, &mut pane, 3);
    assert!(t.checking && t.found.len() == 5, "the cheap findings are on screen already");
    assert!(g.text_log.iter().any(|x| x.2.starts_with("still checking")));
    assert!(t.wants_redraw(&p, &mut pane), "it keeps asking for frames while it waits");
    release.send(()).unwrap();
    t.wait_checks(&p);
    assert!(!t.checking);
    assert!(t.found.iter().any(|f| f.id == "model-unreachable"), "the slow finding joined the list");
    assert_eq!(t.found.len(), 6);
}

#[test]
fn checking_again_reads_the_machine_again_and_keeps_the_chosen_finding() {
    let mut f = fixture();
    f.ev(Event::Row(2));
    let g0 = *f.p.gathers.borrow();
    f.chr('r');
    f.t.wait_checks(&f.p);
    assert_eq!(*f.p.gathers.borrow(), g0 + 1);
    assert_eq!(f.t.rounds, 2);
    assert_eq!(f.t.sel, 2, "still on the same finding");
    // the list shrinks: the choice stays inside it
    *f.p.found.borrow_mut() = vec![settings("cluster-stopped", "The dev cluster is stopped.", "tab:devcluster")];
    f.chr('r');
    f.t.wait_checks(&f.p);
    assert_eq!(f.t.sel, 0);
}

#[test]
fn it_draws_inside_its_area_at_every_size_and_state() {
    for (w, h, s) in SIZES {
        let mut f = fixture_with(true, sick(), w, h, s);
        assert_hits_sane(&f, &format!("{w}x{h} list"));
        f.key(Key::Down);
        f.key(Key::End);
        assert_hits_sane(&f, &format!("{w}x{h} selected"));
        f.t.fail("Something failed with a rather long explanation that has to be cut to the width of the screen somewhere.");
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} message"));
        f.chr('s');
        assert!(f.t.preview.is_some(), "{w}x{h}");
        assert_hits_sane(&f, &format!("{w}x{h} preview"));
        f.key(Key::Down);
        f.key(Key::Char('2'));
        assert_hits_sane(&f, &format!("{w}x{h} preview scrolled"));
        f.key(Key::Esc);
        assert!(f.t.preview.is_none());
        let calm = fixture_with(true, Vec::new(), w, h, s);
        assert_hits_sane(&calm, &format!("{w}x{h} calm"));
    }
}

#[test]
fn it_draws_inside_a_pane_that_does_not_start_at_the_corner() {
    for (x, y, w, h) in [(640.0f32, 60.0f32, 640.0f32, 600.0f32), (0.0, 360.0, 1280.0, 360.0), (320.0, 0.0, 320.0, 720.0)] {
        let mut f = fixture();
        f.area = R::new(x, y, w, h);
        f.draw();
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h}"));
        f.chr('s');
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h} preview"));
    }
}

#[test]
fn many_findings_scroll_and_the_chosen_one_stays_in_view() {
    let many: Vec<Finding> = (0..30).map(|i| settings("svc", &format!("Service {i} failed to start."), "tab:shares")).collect();
    let mut f = fixture_with(true, many, 800, 480, 1.0);
    assert!(f.t.list.scrolls());
    for _ in 0..25 {
        f.key(Key::Down);
    }
    assert_eq!(f.t.sel, 25);
    assert!(f.t.list.offset > 0.0, "scrolled down to follow the choice");
    let shown = f.g.hits.iter().filter(|h| h.label == "finding").count();
    assert!(shown < 30 && shown > 0);
    assert!(f.has_hit(&Event::Tab(T_FIX + 25)), "the chosen finding's button is on screen");
    assert!(f.t.scroll_px(-120, &mut f.pane), "the wheel scrolls the list");
    assert_hits_sane(&f, "scrolled list");
}

#[test]
fn the_preview_shows_what_the_port_built_audits_and_closing_it_saves_nothing() {
    let mut f = fixture();
    f.chr('s');
    let p = f.t.preview.as_ref().expect("preview is open");
    assert_eq!(p.text, f.p.share);
    assert!(f.text().contains("=== Version"), "the lines are drawn");
    assert!(f.p.audit_text().contains("doctor.share.preview"));
    assert!(f.has_hit(&Event::Tab(T_SAVE)) && f.has_hit(&Event::Tab(T_CLOSE)));
    assert!(f.t.hint().contains("save") && f.t.page() == "share");
    f.key(Key::Esc);
    assert!(f.t.preview.is_none());
    assert!(f.p.saved.borrow().is_empty(), "nothing was saved");
}

#[test]
fn saving_goes_to_the_chosen_place_once_and_is_audited_once() {
    let mut f = fixture();
    f.chr('s');
    f.chr('s');
    f.chr('s');
    assert_eq!(f.p.saved.borrow().as_slice(), [PathBuf::from("/home/test")]);
    let a = f.p.audit_text();
    assert_eq!(a.matches("doctor.share.saved").count(), 1, "{a}");
    assert!(a.contains(&format!("{} bytes to my home folder", f.p.share.len())), "{a}");
    assert!(f.t.preview.as_ref().unwrap().saved.is_some());
    assert!(f.t.msg.as_ref().unwrap().0.contains("Saved on my home folder: /home/test/iyakki-diagnostics-1.txt"));
    assert!(f.has_hit(&Event::Tab(T_COPY)) && !f.has_hit(&Event::Tab(T_SAVE)), "the save buttons give way to Copy path");
    f.chr('c');
    assert!(f.t.msg.as_ref().unwrap().0.contains("copied"));
}

#[test]
fn a_stick_the_port_lists_is_a_place_by_number_and_a_missing_place_does_nothing() {
    let mut p = Fake::new(true, sick());
    p.dests.push(Dest { label: "STICK".into(), dir: PathBuf::from("/home/test/media/STICK") });
    let mut t = DoctorTile::new(&p);
    t.wait_checks(&p);
    let mut pane = Pane::default();
    t.open_share(&p);
    t.handle(&Event::Key(Key::Char('9')), &p, &mut pane);
    assert!(t.preview.as_ref().unwrap().saved.is_none() && p.saved.borrow().is_empty());
    t.handle(&Event::Key(Key::Char('2')), &p, &mut pane);
    assert_eq!(p.saved.borrow().as_slice(), [PathBuf::from("/home/test/media/STICK")]);
    assert!(t.msg.as_ref().unwrap().0.contains("Saved on STICK"));
    // the button of a place saves there too
    let mut u = DoctorTile::new(&p);
    u.open_share(&p);
    u.handle(&Event::Tab(T_SAVE), &p, &mut pane);
    assert_eq!(p.saved.borrow().last(), Some(&PathBuf::from("/home/test")));
}

#[test]
fn a_save_that_fails_says_so_and_the_preview_stays_open() {
    let mut p = Fake::new(true, sick());
    p.save_answer = Err("cannot write the file".into());
    let mut t = DoctorTile::new(&p);
    t.open_share(&p);
    t.save_to(0, &p);
    let pv = t.preview.as_ref().expect("still open");
    assert!(pv.saved.is_none());
    assert_eq!(t.msg, Some(("cannot write the file".to_string(), false)));
    assert!(!p.audit_text().contains("doctor.share.saved"));
}

#[test]
fn another_tile_asks_for_the_share_preview_or_the_manual_tab_by_item_name() {
    let p = Fake::new(true, Vec::new());
    let mut pane = Pane::default();
    let mut t = DoctorTile::new(&p);
    t.open_item("nothing", &p, &mut pane);
    assert!(t.preview.is_none() && !t.tab_manual);
    t.open_item("share", &p, &mut pane);
    assert!(t.preview.is_some());
    t.open_item("manual", &p, &mut pane);
    assert!(t.tab_manual && t.preview.is_none(), "the manual tab closes the preview");
    assert_eq!(t.page(), "manual");
}

#[test]
fn the_manual_tab_is_the_shells_pane_opened_once_and_given_the_events() {
    let mut f = fixture();
    assert!(f.has_hit(&Event::Tab(T_TAB_MANUAL)) && f.has_hit(&Event::Tab(T_TAB_FINDINGS)));
    f.chr('m');
    assert!(f.t.tab_manual && f.pane.is_open() && f.pane.opened == 1);
    assert!(f.p.audit_text().contains("doctor.manual.open"));
    assert!(f.pane.drawn > 0, "the pane draws the tab");
    assert!(f.t.hint().contains("scenario"));
    // events go to the pane; Esc at its top leaves to the findings
    f.pane.leave_on_esc = true;
    f.chr('n');
    assert_eq!(f.pane.events.last(), Some(&Event::Key(Key::Char('n'))));
    assert_eq!(f.key(Key::Esc), Answer::Stay);
    assert!(!f.t.tab_manual, "back on the findings, not out of the tile");
    // the pane is not read again, nor audited again
    f.ev(Event::Tab(T_TAB_MANUAL));
    f.pane.dirty = true;
    assert!(f.t.wants_redraw(&f.p, &mut f.pane), "the pane's change asks for a frame");
    assert_eq!(f.pane.opened, 1);
    assert_eq!(f.p.audit_text().matches("doctor.manual.open").count(), 1);
    // the wheel goes to the pane while it shows, the findings' tab button goes back
    assert!(f.t.scroll_px(40, &mut f.pane) && f.pane.wheeled == [40]);
    f.ev(Event::Tab(T_TAB_FINDINGS));
    assert!(!f.t.tab_manual);
}

#[test]
fn without_a_manual_pane_the_tab_stays_on_the_findings() {
    let p = Fake::new(true, sick());
    let mut t = DoctorTile::new(&p);
    t.open_manual(&p, &mut NoPane);
    assert!(t.tab_manual);
    // the pane is not open, so the findings' keys still work and Esc leaves the tile
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p, &mut NoPane), Answer::Back);
    assert!(!p.audit_text().contains("doctor.manual.open"));
}

#[test]
fn a_dismissed_message_goes_and_the_words_of_severity_are_stable() {
    let mut f = fixture();
    f.t.ok("done");
    f.draw();
    assert!(f.has_hit(&Event::Tab(T_DISMISS)));
    f.ev(Event::Tab(T_DISMISS));
    assert!(f.t.msg.is_none());
    assert_eq!([severity_word(Severity::Problem), severity_word(Severity::Warning), severity_word(Severity::Note)], ["problem", "warning", "note"]);
    assert!(Severity::Problem < Severity::Warning && Severity::Warning < Severity::Note);
}

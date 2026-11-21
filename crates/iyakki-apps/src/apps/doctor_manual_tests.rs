//! The "Manual tests" tab of DOCTOR on a fake port: nothing here reads the real machine. The checklists come from the port, the answers go
//! into a store in memory, and the real `dist/manual` files are parsed once to prove they are sound.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use iyakki_shared::diagshare::Inputs;
use iyakki_shared::doctor::{Facts, Finding};
use iyakki_shared::manualtest::{self as mt, EvidenceStore, MemStore, State, Verdict, Who};

use super::doctor_manual::*;
use super::doctor_port::DoctorPort;
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

const SOUND: &str = "scenario: SND-01\ntitle: Sound by ear\narea: Sound\nrows: F-SND-01, F-SND-02\nmachines: mac mini\nneeds: person\nprereq: Headphones are plugged in.\nstep: Press Play test tone.\nexpect: Two notes, left then right.\nclause: F-SND-01.tone\nshot: shots/tone.png\nstep: Press Stop.\nexpect: The sound stops.\nclause: stop\nstep: Look at the card.\nexpect: It names the output.\n";
const TOUCH: &str = "scenario: TCH-01\ntitle: Touch screen\narea: Touch\nrows: F-BOOT-06\nmachines: any\nneeds: person\nstep: Tap the field.\nexpect: The field is chosen.\nclause: tap\nstep: Drag a finger.\nexpect: The pointer follows.\nclause: drag\n";
const HOTSPOT: &str = "scenario: HOT-01\ntitle: Hotspot with a second device\narea: Network\nrows: F-HOTSPOT-12\nmachines: lenovo\nstep: Switch the hotspot on and join it with a phone and then open a page on the phone to see that it loads, which proves the internet is shared.\nexpect: The page loads.\nclause: join\n";

struct Port {
    files: Vec<(String, String)>,
    who: Who,
    hardware: String,
    store: MemStore,
    audit: RefCell<Vec<String>>,
    /// Refuse every write (the person said no to the core's question, or the disk is full).
    refuse: bool,
}

impl Port {
    fn new(machine: &str, hardware: &str) -> Port {
        Port {
            files: vec![("snd.chk".into(), SOUND.into()), ("tch.chk".into(), TOUCH.into()), ("hot.chk".into(), HOTSPOT.into())],
            who: Who { who: "ana".into(), admin: true, machine: machine.into(), image_commit: "v0.1.0-alpha.7-12-gabc1234".into() },
            hardware: hardware.into(),
            store: MemStore::default(),
            audit: Default::default(),
            refuse: false,
        }
    }
    fn audit_text(&self) -> String {
        self.audit.borrow().join("\n")
    }
}

impl DoctorPort for Port {
    fn admin(&self) -> bool {
        true
    }
    fn gather(&self) -> Facts {
        Facts::default()
    }
    fn slow_input(&self) -> super::doctor_facts::SlowIn {
        super::doctor_facts::SlowIn { servers: Vec::new(), audit_recent: Arc::new(|_| Vec::new()), dev_port: None }
    }
    fn bundle_inputs(&self, _: &[Finding]) -> Inputs {
        Inputs { version: vec![], services: vec![], hardware: String::new(), doctor: vec![], engine_log: String::new(), boot_log: String::new(), switches: vec![], checkup: vec![], compat: vec![] }
    }
    fn home(&self) -> PathBuf {
        PathBuf::new()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push(format!("{action} {subject}"));
    }
    fn extend_devmode(&self, _: u32) -> Result<i64, String> {
        Err("no".into())
    }
    fn manual_files(&self) -> Vec<(String, String)> {
        self.files.clone()
    }
    fn manual_who(&self) -> Who {
        self.who.clone()
    }
    fn manual_hardware(&self) -> String {
        self.hardware.clone()
    }
    fn store_read(&self, rel: &str) -> Result<Option<String>, String> {
        self.store.read(rel)
    }
    fn store_write(&self, rel: &str, text: &str) -> Result<(), String> {
        if self.refuse {
            return Err("you said no".into());
        }
        self.store.write(rel, text)
    }
    fn store_list(&self, dir: &str) -> Result<Vec<String>, String> {
        self.store.list(dir)
    }
    fn store_folders(&self, dir: &str) -> Result<Vec<String>, String> {
        self.store.folders(dir)
    }
}

fn now() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(|| 1_800_000_000)
}

struct Fx {
    tab: ManualTab,
    port: Port,
    g: Gfx,
    area: R,
    s: f32,
}

impl Fx {
    fn new(port: Port, w: u32, h: u32, s: f32) -> Fx {
        let tab = ManualTab::open(&port, now());
        let mut f = Fx { tab, port, g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s };
        f.draw();
        f
    }
    fn draw(&mut self) {
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.tab.draw(&mut self.g, self.area, self.s);
    }
    fn ev(&mut self, e: Event) -> Out {
        let o = self.tab.handle(&e, &self.port);
        self.draw();
        o
    }
    fn chr(&mut self, c: char) {
        self.ev(Event::Key(Key::Char(c)));
    }
    fn key(&mut self, k: Key) -> Out {
        self.ev(Event::Key(k))
    }
    fn files(&self) -> Vec<String> {
        self.port.store.names()
    }
    fn state_of(&self, id: &str) -> State {
        self.tab.state(self.tab.scenarios.iter().position(|s| s.id == id).unwrap())
    }
    fn has_hit(&self, e: &Event) -> bool {
        self.g.hits.iter().any(|h| &h.ev == e)
    }
}

fn fx() -> Fx {
    Fx::new(Port::new("iyakki-aaa111", "Apple Mac mini (2018)"), 1280, 720, 1.0)
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

/// The four standard sizes of the shell, plus the phone width.
const SIZES: [(u32, u32, f32); 5] = [(360, 740, 1.0), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2880, 1800, 2.25)];

#[test]
fn the_scenarios_are_listed_by_area_each_with_a_state_and_its_machines() {
    let f = fx();
    let ids: Vec<&str> = f.tab.scenarios.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["HOT-01", "SND-01", "TCH-01"], "by area (Network, Sound, Touch), then by id");
    for i in 0..3 {
        assert!(f.has_hit(&Event::Tab(T_NAV + i)), "scenario {i} has a row to click");
        assert_eq!(f.tab.state(i), State::NotStarted);
    }
    assert_eq!(f.tab.counts(), (0, 5, 3, 0), "steps that write evidence are counted; a guidance step is not");
    assert!(f.tab.problems.is_empty());
}

#[test]
fn a_file_that_does_not_parse_is_named_and_the_rest_still_work() {
    let mut p = Port::new("m", "Mac mini");
    p.files.push(("bad.chk".into(), "junk".into()));
    let f = Fx::new(p, 1280, 720, 1.0);
    assert_eq!(f.tab.scenarios.len(), 3);
    assert_eq!(f.tab.problems.len(), 1);
    // nothing at all: a calm empty screen
    let mut p = Port::new("m", "x");
    p.files.clear();
    let f = Fx::new(p, 1280, 720, 1.0);
    assert!(f.tab.scenarios.is_empty());
    assert_hits_sane(&f, "empty");
}

#[test]
fn n_and_p_move_between_scenarios_and_clicks_choose_one() {
    let mut f = fx();
    let first = f.tab.sel;
    f.chr('n');
    assert_ne!(f.tab.sel, first);
    f.chr('n');
    f.chr('n');
    let last = f.tab.sel;
    f.chr('n');
    assert_eq!(f.tab.sel, last, "the end stays");
    f.chr('p');
    assert_ne!(f.tab.sel, last);
    f.ev(Event::Tab(T_NAV + 0));
    assert_eq!(f.tab.sel, 0);
    f.ev(Event::Tab(T_NEXT));
    assert_eq!(f.tab.sel, 1);
    f.ev(Event::Tab(T_PREV));
    assert_eq!(f.tab.sel, 0);
}

#[test]
fn the_machine_filter_uses_the_hardware_text_and_the_open_filter_hides_finished_ones() {
    let mut f = fx();
    f.chr('f');
    let names: Vec<&str> = f.tab.vis().into_iter().map(|i| f.tab.scenarios[i].id.as_str()).collect();
    assert_eq!(names, ["SND-01", "TCH-01"], "the hotspot checklist is for a Lenovo");
    assert!(f.tab.vis().contains(&f.tab.sel), "the chosen scenario stays visible");
    f.chr('f');
    assert_eq!(f.tab.vis().len(), 3);
    // finish the touch scenario, then only what still needs a person
    let t = f.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    f.ev(Event::Tab(T_NAV + t));
    f.chr('1');
    f.chr('1');
    assert_eq!(f.state_of("TCH-01"), State::Pass);
    f.chr('o');
    let open: Vec<&str> = f.tab.vis().into_iter().map(|i| f.tab.scenarios[i].id.as_str()).collect();
    assert_eq!(open, ["SND-01"], "HOT-01 does not say it needs a person and TCH-01 is done");
    assert_eq!(f.tab.counts().2, 1);
}

#[test]
fn keys_one_two_three_record_and_move_to_the_next_step_and_write_the_human_evidence_files() {
    let mut f = fx();
    let s = f.tab.scenarios.iter().position(|s| s.id == "SND-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    assert_eq!(f.tab.step, 0);
    f.chr('1');
    assert_eq!(f.tab.step, 1);
    assert_eq!(f.state_of("SND-01"), State::InProgress);
    assert_eq!(f.files(), ["manual-tests/human/F-SND-01.tone.json"], "a clause with its row writes for that row only");
    f.chr('2');
    assert_eq!(f.state_of("SND-01"), State::Fail);
    let mut names = f.files();
    names.sort();
    assert_eq!(names, ["manual-tests/human/F-SND-01.stop.json", "manual-tests/human/F-SND-01.tone.json", "manual-tests/human/F-SND-02.stop.json"], "a bare clause writes for every row of the scenario");
    // the evidence says who, when, where and which image
    let ev = mt::load_evidence(&f.port.store);
    let tone = ev.iter().find(|e| e.clause == "tone").unwrap();
    assert_eq!((tone.who.as_str(), tone.by.as_str(), tone.machine.as_str(), tone.result.as_str(), tone.row.as_str()), ("ana", "owner", "iyakki-aaa111", "pass", "F-SND-01"));
    assert_eq!((tone.image_commit.as_str(), tone.date.as_str(), tone.scenario.as_str()), ("v0.1.0-alpha.7-12-gabc1234", "2027-01-15T08:00:00", "SND-01"));
    assert_eq!(ev.iter().find(|e| e.clause == "stop").unwrap().result, "fail");
    // the last step: Cannot test, and the state says so only when nothing failed
    f.chr('3');
    assert_eq!(f.state_of("SND-01"), State::Fail);
    assert!(f.port.audit_text().contains("manual.record SND-01 step 1 pass") && f.port.audit_text().contains("manual.record SND-01 step 2 fail"), "{}", f.port.audit_text());
    assert!(f.tab.msg.as_ref().is_some_and(|m| m.1 && m.0.contains("Last step")), "{:?}", f.tab.msg);
}

#[test]
fn skip_writes_nothing_and_never_removes_an_answer_and_the_buttons_do_the_same_as_the_keys() {
    let mut f = fx();
    let s = f.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    f.chr('4');
    assert!(f.files().is_empty());
    assert_eq!(f.tab.verdicts[s][0], Some(Verdict::Skip));
    assert_eq!(f.state_of("TCH-01"), State::InProgress);
    // go back and answer it by the button
    f.key(Key::Up);
    assert_eq!(f.tab.step, 0);
    assert!(f.has_hit(&Event::Tab(T_SVERD)) && f.has_hit(&Event::Tab(T_SVERD + 3)));
    f.ev(Event::Tab(T_SVERD));
    assert_eq!(f.tab.verdicts[s][0], Some(Verdict::Pass));
    f.key(Key::Up);
    f.chr('s');
    assert_eq!(f.tab.verdicts[s][0], Some(Verdict::Pass), "a skip does not undo an answer");
    assert_eq!(f.files().len(), 1);
}

#[test]
fn a_whole_scenario_can_be_answered_at_once_and_only_the_open_steps_change() {
    let mut f = fx();
    let s = f.tab.scenarios.iter().position(|s| s.id == "SND-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    f.chr('2');
    f.ev(Event::Tab(T_AVERD));
    assert_eq!(f.tab.verdicts[s], vec![Some(Verdict::Fail), Some(Verdict::Pass), Some(Verdict::Pass)]);
    assert_eq!(f.state_of("SND-01"), State::Fail);
    assert!(f.tab.msg.as_ref().unwrap().0.contains("2 steps marked pass"));
    f.ev(Event::Tab(T_AVERD + 2));
    assert!(f.tab.msg.as_ref().unwrap().0.contains("0 steps"), "nothing was open any more");
}

#[test]
fn a_guidance_step_is_answered_on_screen_but_leaves_no_file_and_does_not_hold_the_scenario_open() {
    let mut f = fx();
    let s = f.tab.scenarios.iter().position(|s| s.id == "SND-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    f.chr('1');
    f.chr('1');
    assert_eq!(f.state_of("SND-01"), State::Pass, "the third step has no clause, so it is not scored");
    f.chr('1');
    assert!(f.port.audit_text().contains("step 3 pass"));
    assert_eq!(f.files().len(), 3);
    // a restart (a new tab over the same store) shows the same state
    let again = ManualTab::open(&f.port, now());
    let i = again.scenarios.iter().position(|s| s.id == "SND-01").unwrap();
    assert_eq!(again.state(i), State::Pass);
}

#[test]
fn the_state_comes_back_from_the_files_after_a_restart_but_only_for_this_machine() {
    let mut f = fx();
    let s = f.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    f.chr('1');
    f.chr('2');
    f.chr('t');
    for c in "no pointer".chars() {
        f.chr(c);
    }
    f.key(Key::Enter);
    let store = std::mem::take(&mut f.port.store);
    let mut p2 = Port::new("iyakki-aaa111", "Apple Mac mini (2018)");
    p2.store = store;
    let t2 = ManualTab::open(&p2, now());
    let i = t2.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    assert_eq!(t2.verdicts[i], vec![Some(Verdict::Pass), Some(Verdict::Fail)]);
    // another machine sees the same files as not answered by itself
    let mut p3 = Port::new("iyakki-bbb222", "Apple Mac mini (2018)");
    p3.store = std::mem::take(&mut p2.store);
    let t3 = ManualTab::open(&p3, now());
    assert_eq!(t3.state(i), State::NotStarted);
}

#[test]
fn the_note_is_typed_with_t_ends_with_enter_and_is_kept_in_the_evidence() {
    let mut f = fx();
    let s = f.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    f.chr('t');
    assert!(f.tab.editing);
    // while typing, the shortcut letters are text
    for c in "n 1 p".chars() {
        f.chr(c);
    }
    assert_eq!(f.tab.sel, s, "n and p did not move");
    assert!(f.files().is_empty(), "1 did not answer");
    f.key(Key::Backspace);
    f.ev(Event::Paste(" ok\u{7}".into()));
    f.key(Key::Esc);
    assert!(!f.tab.editing);
    assert_eq!(f.tab.notes[s], "n 1  ok");
    f.chr('2');
    let ev = mt::load_evidence(&f.port.store);
    assert_eq!(ev[0].note, "n 1  ok");
    // the note is cut at its limit
    f.chr('t');
    for _ in 0..(mt::MAX_NOTE + 50) {
        f.chr('x');
    }
    assert_eq!(f.tab.notes[s].chars().count(), mt::MAX_NOTE);
    // and the Note button does the same as t
    f.key(Key::Enter);
    f.ev(Event::Tab(T_NOTE));
    assert!(f.tab.editing);
}

#[test]
fn a_refused_write_says_so_and_records_nothing() {
    let mut p = Port::new("m", "Mac mini");
    p.refuse = true;
    let mut f = Fx::new(p, 1280, 720, 1.0);
    let s = f.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    f.ev(Event::Tab(T_NAV + s));
    f.chr('1');
    assert_eq!(f.tab.verdicts[s][0], None);
    assert_eq!(f.tab.step, 0, "it did not move on");
    let m = f.tab.msg.clone().unwrap();
    assert!(!m.1 && m.0.contains("you said no"), "{m:?}");
    assert!(!f.port.audit_text().contains("manual.record"));
}

#[test]
fn export_writes_one_file_and_import_merges_it_on_another_machine() {
    let mut a = fx();
    let s = a.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    a.ev(Event::Tab(T_NAV + s));
    a.chr('1');
    a.chr('2');
    a.chr('e');
    let m = a.tab.msg.clone().unwrap();
    assert!(m.1 && m.0.contains("manual-tests/export/manual-evidence-iyakki-aaa111-2027-01-15.json"), "{m:?}");
    let bundle = a.port.store.read("manual-tests/export/manual-evidence-iyakki-aaa111-2027-01-15.json").unwrap().unwrap();
    assert!(a.port.audit_text().contains("manual.export"));

    // the other machine: nothing to merge yet, then the file arrives
    let mut b = Fx::new(Port::new("iyakki-bbb222", "Lenovo ThinkPad"), 1280, 720, 1.0);
    b.chr('i');
    assert!(!b.tab.msg.as_ref().unwrap().1, "no file yet is said plainly");
    b.port.store.put("manual-tests/import/a.json", &bundle);
    b.chr('i');
    let m = b.tab.msg.clone().unwrap();
    assert!(m.1 && m.0.contains("2 new"), "{m:?}");
    assert!(b.files().contains(&"manual-tests/human/iyakki-aaa111/F-BOOT-06.tap.json".to_string()));
    // the other machine's answers are not b's own: b still shows it as not started
    let i = b.tab.scenarios.iter().position(|s| s.id == "TCH-01").unwrap();
    assert_eq!(b.tab.state(i), State::NotStarted);
    // a second import changes nothing
    b.chr('i');
    assert!(b.tab.msg.as_ref().unwrap().0.contains("0 new, 0 newer, 2 already here"), "{:?}", b.tab.msg);
    // a stick DISKS opened is read too
    b.port.store.put("media/STICK/manual-tests/export/x.json", &mt::export(&a.port.store, "iyakki-aaa111", 1_800_000_100).map(|p| a.port.store.read(&p).unwrap().unwrap()).unwrap());
    b.chr('i');
    assert!(b.tab.msg.as_ref().unwrap().1);
}

#[test]
fn it_draws_inside_its_area_at_every_size_and_state() {
    for (w, h, s) in SIZES {
        let mut f = Fx::new(Port::new("iyakki-aaa111", "Apple Mac mini (2018)"), w, h, s);
        assert_hits_sane(&f, &format!("{w}x{h} list"));
        f.ev(Event::Tab(T_NAV + 1));
        assert_hits_sane(&f, &format!("{w}x{h} scenario"));
        f.chr('1');
        f.chr('2');
        f.tab.fail("A rather long explanation that has to be cut to the width of the screen somewhere in the middle of the line.");
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} message"));
        f.chr('t');
        f.chr('a');
        assert_hits_sane(&f, &format!("{w}x{h} note"));
        f.key(Key::Esc);
        f.chr('f');
        f.chr('o');
        assert_hits_sane(&f, &format!("{w}x{h} filters"));
        // the big buttons are big enough to touch: 36 logical pixels tall at least
        f.chr('f');
        f.chr('o');
        f.ev(Event::Tab(T_NAV + 1));
        if let Some(b) = f.g.hits.iter().find(|h| h.ev == Event::Tab(T_SVERD)) {
            assert!(b.rect.h >= 34.0 * s, "{w}x{h}: the Pass button is {} high", b.rect.h);
        }
    }
}

#[test]
fn on_a_narrow_screen_the_list_and_the_canvas_take_turns() {
    let mut f = Fx::new(Port::new("m", "Mac mini"), 360, 740, 1.0);
    assert!(f.tab.narrow && !f.tab.opened);
    assert!(f.has_hit(&Event::Tab(T_NAV)) && !f.has_hit(&Event::Tab(T_SVERD)));
    f.ev(Event::Tab(T_NAV));
    assert!(f.tab.opened);
    assert!(f.has_hit(&Event::Tab(T_SVERD)) && f.has_hit(&Event::Tab(T_BACKLIST)) && !f.has_hit(&Event::Tab(T_NAV)));
    assert_eq!(f.key(Key::Esc), Out::Stay);
    assert!(!f.tab.opened);
    assert_eq!(f.key(Key::Esc), Out::Leave, "from the list Esc leaves the tab");
    f.ev(Event::Tab(T_NAV));
    f.ev(Event::Tab(T_BACKLIST));
    assert!(!f.tab.opened);
}

#[test]
fn a_long_scenario_scrolls_and_the_chosen_step_stays_in_view() {
    let mut steps = String::from("scenario: LONG-01\ntitle: A long one\narea: Zed\nrows: F-X-01\nmachines: any\nneeds: person\n");
    for i in 1..=25 {
        steps.push_str(&format!("step: Do thing number {i} and look at the screen for a while.\nexpect: Thing {i} happened.\nclause: c{i}\n"));
    }
    let mut p = Port::new("m", "Mac mini");
    p.files = vec![("long.chk".into(), steps)];
    let mut f = Fx::new(p, 800, 480, 1.0);
    assert!(f.tab.main.scrolls());
    let first = f.tab.main.offset;
    for _ in 0..20 {
        f.chr('1');
    }
    assert_eq!(f.tab.step, 20);
    assert!(f.tab.main.offset > first + 200.0, "the canvas scrolled to the chosen step");
    assert_hits_sane(&f, "long");
    f.ev(Event::Tab(T_STEP + 3));
    assert_eq!(f.tab.step, 3);
}

#[test]
fn enter_goes_to_the_next_step_and_then_the_next_scenario() {
    let mut f = fx();
    f.ev(Event::Tab(T_NAV));
    let first = f.tab.sel;
    let steps = f.tab.scenarios[first].steps.len();
    for _ in 0..steps - 1 {
        f.key(Key::Enter);
    }
    assert_eq!((f.tab.sel, f.tab.step), (first, steps - 1));
    f.key(Key::Enter);
    assert_ne!(f.tab.sel, first);
    assert!(f.files().is_empty(), "moving on answers nothing");
}

#[test]
fn the_bundled_checklists_parse_cover_real_rows_and_are_detailed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/manual");
    let files = super::doctor_facts::manual_files(&root);
    assert!(files.len() >= 25, "the first 25 checklists: {}", files.len());
    let (all, bad) = mt::load_all(&mt::ChkParser, &files);
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(all.len(), files.len(), "one scenario per file");
    let matrix = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../FEATURES.md")).unwrap();
    for sc in &all {
        assert!(sc.needs_person, "{}: a human checklist says it needs a person", sc.id);
        assert!(sc.steps.len() >= 3, "{}: detailed steps", sc.id);
        for r in &sc.rows {
            assert!(matrix.contains(&format!("| {r} |")), "{}: {r} is not a row of FEATURES.md", sc.id);
        }
        for (i, st) in sc.steps.iter().enumerate() {
            assert!(st.text.len() >= 12 && st.expect.len() >= 10, "{} step {}", sc.id, i + 1);
            assert!(!sc.targets(i).is_empty() || st.clause.is_empty(), "{} step {}: the clause names a row it does not cover", sc.id, i + 1);
        }
        assert!(sc.steps.iter().any(|st| !st.clause.is_empty()), "{}: at least one step writes evidence", sc.id);
        assert!(!sc.area.is_empty() && !sc.title.is_empty());
    }
    // the owner-only list of the plan is covered
    let text: String = all.iter().map(|s| format!("{} {} {}", s.area, s.title, s.rows.join(" "))).collect::<Vec<_>>().join("\n").to_lowercase();
    for need in ["sound", "touch", "bluetooth", "microphone", "camera", "4k", "hotspot", "stick", "wake", "print"] {
        assert!(text.contains(need), "no checklist for {need}");
    }
}

#[test]
fn every_clause_of_every_bundled_step_makes_a_file_name_the_runner_reads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/manual");
    let (all, _) = mt::load_all(&mt::ChkParser, &super::doctor_facts::manual_files(&root));
    for sc in &all {
        for i in 0..sc.steps.len() {
            for (row, clause) in sc.targets(i) {
                let n = mt::file_name(&row, &clause);
                assert!(n.ends_with(".json") && !n.contains('/') && n.starts_with("F-"), "{n}");
            }
        }
    }
}

#[test]
fn the_tab_is_reached_from_the_findings_and_esc_comes_back() {
    use super::doctor::*;
    let port = Port::new("iyakki-aaa111", "Apple Mac mini (2018)");
    let parts = Parts { gather: Arc::new(|_| Facts::default()), slow: Arc::new(|_| Default::default()), inputs: Arc::new(|_, _| unreachable!()), home: Arc::new(|_| PathBuf::new()), now: now() };
    let mut app = DoctorApp::with_port(&port, parts);
    assert!(!app.tab_manual && app.manual.is_none());
    app.open_item_inner("manual", &port);
    assert!(app.tab_manual && app.manual.as_ref().unwrap().scenarios.len() == 3);
    assert!(port.audit_text().contains("doctor.manual.open"));
    let _ = app.handle_manual(&Event::Key(Key::Char('1')), &port);
    assert!(!port.store.names().is_empty(), "a key in the tab answers");
    let _ = app.handle_manual(&Event::Key(Key::Esc), &port);
    // Esc at the top of the list (wide screen) leaves
    assert!(!app.tab_manual);
    let _ = app.handle_manual(&Event::Tab(T_TAB_MANUAL), &port);
    app.open_manual(&port);
    assert!(app.tab_manual);
    let _ = app.handle_manual(&Event::Tab(T_TAB_FINDINGS), &port);
    assert!(!app.tab_manual);
}

/// Pictures of the tab at the four standard sizes, for the team's video and the release notes. Run on purpose:
/// `cargo test -p iyakki-apps screenshots_of_the_manual_tab -- --ignored`; they land in IYAKKI_SHOTS_DIR (default: the temp folder).
#[test]
#[ignore = "writes pictures; run on purpose"]
fn screenshots_of_the_manual_tab() {
    let dir = std::env::var("IYAKKI_SHOTS_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("iyakki-manual-shots")); // flaky-ok: output folder of an ignored picture-taking run
    std::fs::create_dir_all(&dir).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/manual");
    for (w, h, s) in [(800u32, 480u32, 1.0f32), (1280, 720, 1.0), (1920, 1080, 1.5), (2880, 1800, 2.25)] {
        let mut p = Port::new("iyakki-aaa111", "Apple Mac mini (2018)");
        p.files = super::doctor_facts::manual_files(&root);
        let mut f = Fx::new(p, w, h, s);
        let i = f.tab.scenarios.iter().position(|s| s.id == "SND-EAR-01").unwrap();
        f.ev(Event::Tab(T_NAV + i));
        f.chr('1');
        f.chr('1');
        f.draw();
        f.g.save_png(&dir.join(format!("manual-tests-tab-{w}x{h}.png"))).unwrap();
        eprintln!("saved {}", dir.join(format!("manual-tests-tab-{w}x{h}.png")).display());
    }
}

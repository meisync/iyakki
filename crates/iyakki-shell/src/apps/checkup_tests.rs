//! The CHECK-UP tile on a made-up machine: fake recorders, a fake speaker, a fake camera, a fake sign. Nothing here records, plays or opens
//! a real device. The tests share the "newest report" slot of Share diagnostics, so they take turns.

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use iyakki_agent::checkup::{Adapter, Inventory, Outcome, PowerFacts, ScreenOut, StepId};
use iyakki_agent::voicedesk::{Capture, Mic, Recorders};
use iyakki_sec::policy::{Policy, Role};

use super::checkup::*;
use super::tests::cx_with_admin_store;
use super::{App, AppCx, Flow};
use crate::camera::fake::{device, FakeOpener};
use crate::camera::probe::{self, Shot};
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

static TURN: Mutex<()> = Mutex::new(());

thread_local! {
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static HELD: std::cell::RefCell<Option<MutexGuard<'static, ()>>> = const { std::cell::RefCell::new(None) };
}

/// The turn at the shared "newest report" slot. A test may build several fixtures; the turn is kept until the last one is gone.
struct Turn;

fn take_turn() -> Turn {
    if DEPTH.with(|d| d.replace(d.get() + 1)) == 0 {
        let g = TURN.lock().unwrap_or_else(|e| e.into_inner());
        HELD.with(|h| *h.borrow_mut() = Some(g));
    }
    Turn
}

impl Drop for Turn {
    fn drop(&mut self) {
        if DEPTH.with(|d| d.replace(d.get() - 1)) == 1 {
            HELD.with(|h| h.borrow_mut().take());
        }
    }
}

const MIC: u8 = iyakki_webview::proto::media_kind::MICROPHONE;
const CAM: u8 = iyakki_webview::proto::media_kind::CAMERA;

fn inv() -> Inventory {
    Inventory {
        keyboards: 1,
        pointers: 1,
        touchscreens: 0,
        sound_cards: 1,
        sound_chosen_present: true,
        mics: vec![],
        product: "Mac mini".into(),
        screens: vec![ScreenOut { name: "HDMI-A-1".into(), connected: true, current: Some((1920, 1080)) }],
        ethernet: vec![Adapter { name: "eth0".into(), up: true }],
        wifi: vec![Adapter { name: "wlan0".into(), up: true }],
        route: true,
        bluetooth: 1,
        power: PowerFacts::default(),
        ..Inventory::default()
    }
}

struct Cap {
    ms: Arc<AtomicU64>,
    pcm: Vec<i16>,
}
impl Capture for Cap {
    fn level(&self) -> f32 {
        iyakki_agent::checkup::level_of(&self.pcm)
    }
    fn elapsed(&self) -> Duration {
        Duration::from_millis(self.ms.load(Ordering::SeqCst))
    }
    fn is_full(&self) -> bool {
        false
    }
    fn is_alive(&mut self) -> bool {
        true
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        Ok(self.pcm)
    }
}

struct Recs {
    mics: Vec<Mic>,
    product: String,
    pcm: Vec<i16>,
    ms: Arc<AtomicU64>,
    started: Mutex<Vec<String>>,
}
impl Recorders for Recs {
    fn mics(&self) -> Vec<Mic> {
        self.mics.clone()
    }
    fn product(&self) -> String {
        self.product.clone()
    }
    fn start(&self, cmd: &str, _max: usize) -> Result<Box<dyn Capture>, String> {
        self.started.lock().unwrap().push(cmd.to_string());
        Ok(Box::new(Cap { ms: self.ms.clone(), pcm: self.pcm.clone() }))
    }
}

fn usb(id: &str) -> Mic {
    Mic { id: id.into(), label: format!("USB Audio {id} (USB)"), usb: true }
}

/// What the fakes saw.
struct Spy {
    recs: Arc<Recs>,
    played: Arc<AtomicUsize>,
    signs: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    camera_opened: Arc<AtomicUsize>,
}

struct Fx {
    app: CheckupApp,
    cx: AppCx,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
    spy: Spy,
    _turn: Turn,
}

struct Setup {
    inv: Inventory,
    mics: Vec<Mic>,
    pcm: Vec<i16>,
    play_err: Option<String>,
    cams: Vec<crate::camera::CamDevice>,
    w: u32,
    h: u32,
    s: f32,
    role: Role,
}

impl Setup {
    fn new() -> Setup {
        Setup { inv: inv(), mics: vec![usb("plughw:1,0")], pcm: iyakki_agent::voice::tone(300.0, 1200), play_err: None, cams: vec![device("video0", "USB Camera")], w: 1280, h: 720, s: 1.0, role: Role::Admin }
    }
}

fn fixture(tag: &str) -> Fx {
    fixture_with(tag, Setup::new())
}

fn fixture_with(tag: &str, st: Setup) -> Fx {
    let turn = take_turn();
    let mut cx = cx_with_admin_store(&format!("checkup-{tag}"));
    cx.session.role = st.role;
    cx.session.policy = Policy::resolve(st.role, &Default::default());
    let recs = Arc::new(Recs { mics: st.mics, product: "Mac mini".into(), pcm: st.pcm, ms: Arc::new(AtomicU64::new(0)), started: Mutex::new(vec![]) });
    let played = Arc::new(AtomicUsize::new(0));
    let signs = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let camera_opened = Arc::new(AtomicUsize::new(0));
    let (p2, s2, st2, co2) = (played.clone(), signs.clone(), stop.clone(), camera_opened.clone());
    let (inv, play_err, cams) = (st.inv, st.play_err, st.cams);
    let parts = Parts {
        inventory: Arc::new(move |_| inv.clone()),
        recorders: recs.clone(),
        play: Arc::new(move |_| {
            p2.fetch_add(1, Ordering::SeqCst);
            match &play_err {
                Some(e) => Err(e.clone()),
                None => Ok(Box::new(()) as Box<dyn Any + Send>),
            }
        }),
        camera: Arc::new(move |_| {
            let (cams, co) = (cams.clone(), co2.clone());
            Box::new(move || {
                co.fetch_add(1, Ordering::SeqCst);
                probe::grab(&cams, "", &FakeOpener::default(), "No camera found. Connect a USB camera.", Duration::from_secs(3))
            }) as Box<dyn FnOnce() -> Shot + Send>
        }),
        sign: Arc::new(move |b| s2.lock().unwrap().push(b)),
        stop_asked: Arc::new(move || st2.swap(false, Ordering::SeqCst)),
    };
    let app = CheckupApp::with_parts(&cx, parts);
    let mut f = Fx { app, cx, g: Gfx::new(st.w, st.h), area: R::new(0.0, 0.0, st.w as f32, st.h as f32), s: st.s, tick: 0, spy: Spy { recs, played, signs, stop, camera_opened }, _turn: turn };
    f.draw();
    f
}

impl Fx {
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    fn ev(&mut self, e: Event) -> Flow {
        let r = self.app.handle(&e, &self.cx);
        self.draw();
        r
    }
    fn key(&mut self, k: Key) -> Flow {
        self.ev(Event::Key(k))
    }
    fn chr(&mut self, c: char) {
        self.key(Key::Char(c));
    }
    fn goto(&mut self, id: StepId) {
        let i = StepId::ALL.iter().position(|s| *s == id).unwrap();
        self.ev(Event::Row(i));
    }
    fn start(&mut self, id: StepId) {
        self.goto(id);
        self.key(Key::Enter);
    }
    fn outcome(&self, id: StepId) -> Outcome {
        self.app.results[StepId::ALL.iter().position(|s| *s == id).unwrap()].outcome
    }
    fn result(&self, id: StepId) -> &iyakki_agent::checkup::StepResult {
        &self.app.results[StepId::ALL.iter().position(|s| *s == id).unwrap()]
    }
    fn has_hit(&self, e: &Event) -> bool {
        self.g.hits.iter().any(|h| &h.ev == e)
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
    /// Run the frames until the live step ends (a recording that reaches its time, a camera picture).
    fn settle(&mut self) {
        for _ in 0..600 {
            if !self.app.live() {
                return;
            }
            self.app.wants_redraw();
            self.draw();
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the live step never ended");
    }
    fn signs(&self) -> Vec<u8> {
        self.spy.signs.lock().unwrap().clone()
    }
    fn audit(&self) -> String {
        std::fs::read_to_string(self.cx.audit.path()).unwrap_or_default()
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

// ------------------------------------------------------------------------------------------------ the list

#[test]
fn it_lists_every_step_not_done_yet_and_opens_the_chosen_one() {
    let f = fixture("list");
    assert_eq!(f.app.results.len(), 12);
    assert!(f.app.results.iter().all(|r| r.outcome == Outcome::Pending));
    assert!(f.g.hits.iter().filter(|h| h.label == "step").count() >= 5, "the steps are rows");
    let t = f.text();
    assert!(t.contains("0 passed, 0 failed, 0 skipped, 12 not done yet") && t.contains("Screen") && t.contains("Shows four colour bars"), "{t}");
    assert!(f.has_hit(&Event::Tab(T_RUN)) && f.has_hit(&Event::Tab(T_AGAIN)) && f.has_hit(&Event::Tab(T_SHARE)) && f.has_hit(&Event::Tab(T_DOCTOR)));
    assert_eq!((f.app.title(), f.app.icon()), ("Check-up", crate::gfx::Icon::Checkup));
    assert!(f.app.hint().contains("Enter start") && !f.app.help().is_empty());
    assert!(iyakki_agent::checkup::last_report().is_empty(), "nothing done: nothing to share");
}

#[test]
fn the_machines_own_steps_answer_at_once_and_the_list_moves_on() {
    let mut f = fixture("machine");
    f.start(StepId::Monitor);
    assert_eq!(f.outcome(StepId::Monitor), Outcome::Pass, "HDMI-A-1 is an external output in the made-up machine");
    assert!(f.result(StepId::Monitor).note.contains("HDMI-A-1 1920 x 1080"));
    f.start(StepId::Network);
    assert_eq!(f.outcome(StepId::Network), Outcome::Pass);
    f.start(StepId::Wifi);
    f.start(StepId::Bluetooth);
    f.start(StepId::Power);
    assert_eq!([f.outcome(StepId::Wifi), f.outcome(StepId::Bluetooth), f.outcome(StepId::Power)], [Outcome::Pass, Outcome::Pass, Outcome::Skipped]);
    assert!(f.text().contains("No battery"), "the power row says it");
}

#[test]
fn up_and_down_choose_steps_and_enter_starts_the_chosen_one() {
    let mut f = fixture("nav");
    f.key(Key::Down);
    assert_eq!(f.app.sel, 1);
    f.key(Key::Up);
    f.key(Key::Up);
    assert_eq!(f.app.sel, 0);
    for _ in 0..30 {
        f.key(Key::Down);
    }
    assert_eq!(f.app.sel, 11, "stays on the last step");
    assert!(f.app.active.is_none(), "choosing never starts anything");
    assert!(matches!(f.key(Key::Esc), Flow::Back));
}

// ------------------------------------------------------------------------------------------------ the steps the person answers

#[test]
fn the_screen_step_shows_bars_and_y_or_n_decides() {
    let mut f = fixture("display");
    f.chr('x');
    f.start(StepId::Display);
    assert!(matches!(f.app.active, Some(Active::Bars)));
    assert!(f.text().contains("four colour bars") && f.has_hit(&Event::Tab(T_YES)) && f.has_hit(&Event::Tab(T_NO)));
    f.chr('y');
    assert_eq!(f.outcome(StepId::Display), Outcome::Pass);
    assert_eq!(f.app.sel, 1, "the next step is open");
    f.start(StepId::Display);
    f.chr('n');
    let r = f.result(StepId::Display).clone();
    assert_eq!(r.outcome, Outcome::Fail);
    assert!(r.advice.contains("another cable") && r.fix.is_some());
    f.goto(StepId::Display);
    assert!(f.has_hit(&Event::Tab(T_FIX)) && f.has_hit(&Event::Tab(T_ROW_DOCTOR)), "a failed step offers its fix and DOCTOR");
    f.ev(Event::Tab(T_FIX));
    assert_eq!(f.app.take_request(), Some(("settings".into(), "tab:display".into())));
    f.ev(Event::Tab(T_ROW_DOCTOR));
    assert_eq!(f.app.take_request(), Some(("doctor".into(), String::new())));
    assert!(f.audit().contains("checkup.fix"));
}

#[test]
fn touch_is_skipped_without_a_touch_screen_and_otherwise_wants_the_circle_touched() {
    let mut f = fixture("touch-none");
    f.start(StepId::Touch);
    assert_eq!(f.outcome(StepId::Touch), Outcome::Skipped);
    assert!(f.result(StepId::Touch).note.contains("No touch screen"));
    let mut st = Setup::new();
    st.inv.touchscreens = 1;
    let mut f = fixture_with("touch", st);
    f.start(StepId::Touch);
    assert!(matches!(f.app.active, Some(Active::Target)) && f.has_hit(&Event::Tab(T_TARGET)));
    f.ev(Event::Tab(T_TARGET));
    assert_eq!(f.outcome(StepId::Touch), Outcome::Pass);
    f.start(StepId::Touch);
    f.chr('n');
    assert_eq!(f.outcome(StepId::Touch), Outcome::Fail);
}

#[test]
fn the_keyboard_step_passes_after_three_different_keys_and_fails_on_enter_with_fewer() {
    let mut f = fixture("keys");
    f.start(StepId::Keyboard);
    // while listening every key is a test key, even the letters that are shortcuts elsewhere
    f.chr('y');
    f.chr('y');
    assert!(matches!(&f.app.active, Some(Active::Keys(k)) if k.len() == 1), "the same key twice counts once");
    assert!(f.text().contains("1 so far"));
    f.chr('r');
    assert_eq!(f.outcome(StepId::Keyboard), Outcome::Pending);
    f.key(Key::Left);
    assert_eq!(f.outcome(StepId::Keyboard), Outcome::Pass, "three different keys");
    f.start(StepId::Keyboard);
    f.chr('a');
    f.key(Key::Enter);
    assert_eq!(f.outcome(StepId::Keyboard), Outcome::Fail);
    assert!(f.result(StepId::Keyboard).note.contains("Only 1 key"));
    f.start(StepId::Keyboard);
    f.key(Key::Esc);
    assert!(f.app.active.is_none(), "Esc stops listening");
    assert_eq!(f.outcome(StepId::Keyboard), Outcome::Fail, "the earlier result stays");
}

#[test]
fn the_pointer_step_wants_a_click_on_the_target() {
    let mut f = fixture("pointer");
    f.start(StepId::Pointer);
    assert!(f.has_hit(&Event::Tab(T_TARGET)));
    f.ev(Event::Tab(T_TARGET));
    assert_eq!(f.outcome(StepId::Pointer), Outcome::Pass);
}

#[test]
fn a_step_can_be_skipped() {
    let mut f = fixture("skip");
    f.goto(StepId::Speaker);
    assert!(f.has_hit(&Event::Tab(T_SKIP)));
    f.chr('k');
    assert_eq!(f.outcome(StepId::Speaker), Outcome::Skipped);
    assert_eq!(f.spy.played.load(Ordering::SeqCst), 0, "nothing was played");
}

#[test]
fn the_speaker_plays_one_tone_through_the_audio_entry_point_and_asks() {
    let mut f = fixture("speaker");
    assert_eq!(f.spy.played.load(Ordering::SeqCst), 0, "opening the tile plays nothing");
    f.start(StepId::Speaker);
    assert_eq!(f.spy.played.load(Ordering::SeqCst), 1);
    assert!(f.text().contains("Did you hear it?") && f.has_hit(&Event::Tab(T_AGAIN_TONE)));
    f.ev(Event::Tab(T_AGAIN_TONE));
    assert_eq!(f.spy.played.load(Ordering::SeqCst), 2, "Play again plays again");
    f.chr('y');
    assert_eq!(f.outcome(StepId::Speaker), Outcome::Pass);
    f.start(StepId::Speaker);
    f.chr('n');
    assert_eq!(f.outcome(StepId::Speaker), Outcome::Fail);
    assert!(f.result(StepId::Speaker).advice.contains("volume"));
}

#[test]
fn a_speaker_that_cannot_play_or_a_machine_without_a_sound_card_fails_in_words() {
    let mut st = Setup::new();
    st.play_err = Some("the speaker is busy (Radio)".into());
    let mut f = fixture_with("speaker-busy", st);
    f.start(StepId::Speaker);
    assert!(f.text().contains("could not be played"), "{}", f.text());
    f.chr('y');
    assert_eq!(f.outcome(StepId::Speaker), Outcome::Fail);
    assert!(f.result(StepId::Speaker).note.contains("busy"));
    let mut st = Setup::new();
    st.inv.sound_cards = 0;
    let mut f = fixture_with("speaker-none", st);
    f.start(StepId::Speaker);
    f.chr('n');
    assert!(f.result(StepId::Speaker).note.contains("No sound card"));
}

// ------------------------------------------------------------------------------------------------ the microphone

#[test]
fn nothing_is_recorded_until_the_person_starts_the_microphone_step() {
    let mut f = fixture("mic-consent");
    for _ in 0..12 {
        f.key(Key::Down);
        f.app.wants_redraw();
    }
    f.goto(StepId::Microphone);
    f.draw();
    assert!(f.spy.recs.started.lock().unwrap().is_empty(), "choosing the step opens no microphone");
    assert!(f.app.active.is_none() && f.signs().is_empty(), "and lights no sign");
    assert!(f.text().contains("Starts only when you press the button") && f.has_hit(&Event::Tab(T_RUN)));
    // r (check again), s (share) and Down never start it either
    f.chr('r');
    f.goto(StepId::Microphone);
    assert!(f.spy.recs.started.lock().unwrap().is_empty());
    // a click on its own button starts it
    f.ev(Event::Tab(T_RUN));
    assert_eq!(f.spy.recs.started.lock().unwrap().len(), 1);
    assert!(f.audit().contains("checkup.microphone"));
}

#[test]
fn the_microphone_step_records_two_seconds_shows_the_sign_and_a_level_and_keeps_nothing() {
    let mut f = fixture_with("mic-run", Setup { mics: vec![usb("plughw:1,0"), usb("plughw:2,0")], ..Setup::new() });
    f.cx.services.set_voice_mic("plughw:2,0").unwrap();
    f.start(StepId::Microphone);
    assert!(f.spy.recs.started.lock().unwrap()[0].contains("-D plughw:2,0"), "the microphone chosen for the Voice tile is the one used: {:?}", f.spy.recs.started.lock().unwrap());
    assert!(matches!(f.app.active, Some(Active::Record { .. })));
    assert_eq!(f.signs().last().copied(), Some(MIC), "the same sign the browser lights");
    assert!(f.text().contains("Microphone on") && f.text().contains("USB Audio plughw:2,0"), "{}", f.text());
    f.spy.recs.ms.store(1000, Ordering::SeqCst);
    f.app.wants_redraw();
    f.draw();
    assert!(f.app.level > 0.3, "the meter moves: {}", f.app.level);
    assert!((f.app.progress - 0.5).abs() < 0.01);
    assert!(f.app.live() && f.has_hit(&Event::Tab(T_STOP)));
    f.spy.recs.ms.store(2000, Ordering::SeqCst);
    f.settle();
    assert_eq!(f.outcome(StepId::Microphone), Outcome::Pass);
    assert!(f.result(StepId::Microphone).note.contains("nothing was kept"));
    assert_eq!(f.signs().last().copied(), Some(0), "the sign goes out");
    assert!(f.app.active.is_none());
    assert_eq!(f.app.level, 0.0);
}

#[test]
fn a_silent_microphone_fails_and_the_stop_button_and_the_signs_click_throw_the_recording_away() {
    let mut f = fixture_with("mic-quiet", Setup { pcm: vec![2; 32000], ..Setup::new() });
    f.start(StepId::Microphone);
    f.spy.recs.ms.store(2500, Ordering::SeqCst);
    f.settle();
    assert_eq!(f.outcome(StepId::Microphone), Outcome::Fail);
    assert!(f.result(StepId::Microphone).note.contains("Only silence"));
    // stop button
    let mut f = fixture("mic-stop");
    f.start(StepId::Microphone);
    f.ev(Event::Tab(T_STOP));
    assert!(f.app.active.is_none() && f.outcome(StepId::Microphone) == Outcome::Pending);
    assert_eq!(f.signs().last().copied(), Some(0));
    // the click on the shell's sign
    f.start(StepId::Microphone);
    f.spy.stop.store(true, Ordering::SeqCst);
    f.app.wants_redraw();
    assert!(f.app.active.is_none() && f.outcome(StepId::Microphone) == Outcome::Pending, "stopped by the sign");
    // Esc
    f.start(StepId::Microphone);
    f.key(Key::Esc);
    assert!(f.app.active.is_none());
}

#[test]
fn leaving_the_tile_or_the_step_while_recording_lets_go_and_puts_the_sign_out() {
    let mut f = fixture("mic-leave");
    f.start(StepId::Microphone);
    f.key(Key::Up);
    assert!(f.app.active.is_none(), "moving to another step stops the recording");
    assert_eq!(f.signs().last().copied(), Some(0));
    f.start(StepId::Microphone);
    let signs = f.spy.signs.clone();
    drop(f.app);
    assert_eq!(signs.lock().unwrap().last().copied(), Some(0), "closing the tile puts the sign out");
}

#[test]
fn no_microphone_listed_says_so_and_a_t2_mac_says_the_capture_switch_is_off() {
    let mut f = fixture_with("mic-none", Setup { mics: vec![], ..Setup::new() });
    f.start(StepId::Microphone);
    assert_eq!(f.outcome(StepId::Microphone), Outcome::Skipped);
    assert!(f.result(StepId::Microphone).note.contains("No microphone found"), "{}", f.result(StepId::Microphone).note);
    assert!(f.spy.recs.started.lock().unwrap().is_empty() && f.signs().is_empty(), "no sign: nothing was opened");
    let mut st = Setup::new();
    st.mics = vec![];
    st.inv.t2_mic_hidden = true;
    st.inv.product = "MacBookPro16,2".into();
    let mut f = fixture_with("mic-t2", st);
    f.start(StepId::Microphone);
    let r = f.result(StepId::Microphone);
    assert_eq!(r.outcome, Outcome::Skipped);
    assert!(r.note.contains("switched off") && r.advice.contains("capture switch"), "{} / {}", r.note, r.advice);
}

// ------------------------------------------------------------------------------------------------ the camera

#[test]
fn the_camera_is_not_opened_until_the_person_starts_the_step_then_shows_one_picture() {
    let mut f = fixture("camera");
    f.goto(StepId::Camera);
    for _ in 0..5 {
        f.app.wants_redraw();
        f.draw();
    }
    assert_eq!(f.spy.camera_opened.load(Ordering::SeqCst), 0, "choosing the step opens no camera");
    assert!(f.signs().is_empty());
    f.ev(Event::Tab(T_RUN));
    assert_eq!(f.signs().first().copied(), Some(CAM), "the browser's sign for the camera");
    assert!(f.text().contains("Camera on"));
    f.settle();
    assert_eq!(f.outcome(StepId::Camera), Outcome::Pass);
    assert!(f.result(StepId::Camera).note.contains("USB Camera") && f.result(StepId::Camera).note.contains("nothing was kept"));
    assert_eq!(f.signs().last().copied(), Some(0));
    assert!(f.app.preview.is_some(), "the picture is shown");
    f.goto(StepId::Camera);
    assert_hits_sane(&f, "camera preview");
    assert!(f.audit().contains("checkup.camera"));
    f.key(Key::Down);
    f.key(Key::Up);
    assert!(f.app.preview.is_none(), "the picture is dropped when the person moves on");
}

#[test]
fn no_camera_is_skipped_with_the_reason_and_a_missing_camera_never_lights_a_sign_for_long() {
    let mut f = fixture_with("camera-none", Setup { cams: vec![], ..Setup::new() });
    f.start(StepId::Camera);
    f.settle();
    assert_eq!(f.outcome(StepId::Camera), Outcome::Skipped);
    assert!(f.result(StepId::Camera).note.contains("No camera found"));
    assert_eq!(f.signs().last().copied(), Some(0));
}

#[test]
fn the_sign_click_stops_a_camera_that_is_being_opened() {
    let mut f = fixture("camera-stop");
    f.start(StepId::Camera);
    f.spy.stop.store(true, Ordering::SeqCst);
    f.app.wants_redraw();
    assert!(f.app.active.is_none() && f.outcome(StepId::Camera) == Outcome::Pending);
    assert_eq!(f.signs().last().copied(), Some(0));
}

// ------------------------------------------------------------------------------------------------ the result

#[test]
fn the_result_goes_to_share_diagnostics_through_doctors_preview() {
    let mut f = fixture("share");
    f.chr('s');
    assert!(f.text().contains("Nothing has been checked yet") && f.app.take_request().is_none());
    f.start(StepId::Display);
    f.chr('n');
    f.start(StepId::Network);
    let report = iyakki_agent::checkup::last_report();
    assert_eq!(report[0], "1 passed, 1 failed, 0 skipped, 10 not done yet");
    assert!(report.iter().any(|l| l.starts_with("Screen: Fail")) && report.iter().any(|l| l.starts_with("Network: Pass")));
    f.chr('s');
    assert_eq!(f.app.take_request(), Some(("doctor".into(), "share".into())));
    // DOCTOR's bundle holds the report as its own section, redacted like the rest
    let inp = super::doctor_facts::bundle_inputs(&f.cx, &[]);
    assert_eq!(inp.checkup, report);
    let text = iyakki_agent::diagshare::preview(&iyakki_agent::diagshare::build(&inp));
    assert!(text.contains("=== Check-up ===") && text.contains("Network: Pass"), "{text}");
    f.chr('r');
    assert!(iyakki_agent::checkup::last_report().is_empty(), "check again starts over");
    assert!(f.app.results.iter().all(|r| r.outcome == Outcome::Pending));
}

#[test]
fn open_doctor_is_a_button_and_a_key() {
    let mut f = fixture("doctor");
    f.ev(Event::Tab(T_DOCTOR));
    assert_eq!(f.app.take_request(), Some(("doctor".into(), String::new())));
    f.chr('d');
    assert_eq!(f.app.take_request(), Some(("doctor".into(), String::new())));
}

#[test]
fn a_whole_check_up_by_keyboard_alone() {
    let mut f = fixture("keyboard-only");
    // screen
    f.key(Key::Enter);
    f.chr('y');
    // touch (none listed)
    f.key(Key::Enter);
    // keyboard
    f.key(Key::Enter);
    for c in ['a', 'b', 'c'] {
        f.chr(c);
    }
    // pointer: the target needs a click; skip it by key
    f.chr('k');
    // speaker
    f.key(Key::Enter);
    f.key(Key::Enter);
    assert_eq!(f.app.sel, 5, "now at the microphone");
    assert!(f.app.active.is_none(), "the guided walk stops in front of the microphone");
    assert_eq!(
        [StepId::Display, StepId::Touch, StepId::Keyboard, StepId::Pointer, StepId::Speaker].map(|i| f.outcome(i)),
        [Outcome::Pass, Outcome::Skipped, Outcome::Pass, Outcome::Skipped, Outcome::Pass]
    );
    assert!(f.spy.recs.started.lock().unwrap().is_empty());
}

// ------------------------------------------------------------------------------------------------ drawing

#[test]
fn it_draws_inside_its_area_at_every_size_and_in_every_state() {
    for (w, h, s) in SIZES {
        let mut f = fixture_with("size", Setup { w, h, s, ..Setup::new() });
        assert_hits_sane(&f, &format!("{w}x{h} start"));
        for id in StepId::ALL {
            f.goto(id);
            assert_hits_sane(&f, &format!("{w}x{h} {id:?} chosen"));
            if matches!(id, StepId::Microphone | StepId::Camera) {
                continue; // started by the capture tests below
            }
            f.key(Key::Enter);
            assert_hits_sane(&f, &format!("{w}x{h} {id:?} running"));
            f.chr('n');
            f.draw();
            assert_hits_sane(&f, &format!("{w}x{h} {id:?} done"));
        }
        f.start(StepId::Microphone);
        f.spy.recs.ms.store(1000, Ordering::SeqCst);
        f.app.wants_redraw();
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} recording"));
        f.key(Key::Esc);
        f.start(StepId::Camera);
        assert_hits_sane(&f, &format!("{w}x{h} camera opening"));
        f.settle();
        f.goto(StepId::Camera);
        assert_hits_sane(&f, &format!("{w}x{h} camera picture"));
        f.app.fail("Something failed with a rather long explanation that has to be cut to the width of the screen somewhere.");
        f.draw();
        assert_hits_sane(&f, &format!("{w}x{h} message"));
    }
}

#[test]
fn it_draws_inside_a_pane_that_does_not_start_at_the_corner() {
    for (x, y, w, h) in [(640.0f32, 60.0f32, 640.0f32, 600.0f32), (0.0, 360.0, 1280.0, 360.0), (320.0, 0.0, 320.0, 720.0)] {
        let mut f = fixture("pane");
        f.area = R::new(x, y, w, h);
        f.draw();
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h}"));
        f.start(StepId::Display);
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h} bars"));
        f.chr('n');
        f.start(StepId::Pointer);
        assert_hits_sane(&f, &format!("pane {x},{y} {w}x{h} target"));
    }
}

#[test]
fn a_long_open_step_scrolls_and_the_chosen_step_stays_in_view() {
    let mut f = fixture_with("scroll", Setup { w: 360, h: 480, ..Setup::new() });
    f.start(StepId::Display);
    f.chr('n');
    f.goto(StepId::Display);
    assert!(f.app.list.scrolls());
    for _ in 0..11 {
        f.key(Key::Down);
    }
    assert!(f.app.list.offset > 0.0, "followed the choice down");
    assert!(f.app.scroll_px(-120, &f.cx));
    assert_hits_sane(&f, "scrolled");
}

#[test]
fn the_wheel_and_the_drawing_do_not_start_anything() {
    let mut f = fixture("quiet");
    for _ in 0..20 {
        f.app.wants_redraw();
        f.draw();
    }
    f.app.scroll_px(-300, &f.cx);
    assert!(f.app.active.is_none() && f.spy.played.load(Ordering::SeqCst) == 0 && f.spy.recs.started.lock().unwrap().is_empty() && f.spy.camera_opened.load(Ordering::SeqCst) == 0);
    assert!(f.signs().is_empty());
}

#[test]
fn the_real_readers_run_on_this_computer_without_panicking() {
    let _t = take_turn();
    let cx = cx_with_admin_store("checkup-real");
    let mut a = CheckupApp::new(&cx);
    let mut g = Gfx::new(800, 480);
    g.clear(rgb(8, 10, 22));
    a.render(&mut g, R::new(0.0, 0.0, 800.0, 480.0), 1.0, &cx, 1);
    // the steps that only read: no sound, no picture, nothing opened
    for id in [StepId::Monitor, StepId::Network, StepId::Wifi, StepId::Bluetooth, StepId::Power] {
        let i = StepId::ALL.iter().position(|s| *s == id).unwrap();
        a.begin(i, &cx);
        assert_ne!(a.results[i].outcome, Outcome::Pending, "{id:?}");
    }
}

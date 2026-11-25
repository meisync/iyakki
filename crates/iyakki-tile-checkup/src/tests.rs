//! The CHECK-UP tile on a fake port: no machine, no shell, no real sound or camera. Time is a counter the test moves; nothing sleeps.

use std::any::Any;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_tile_api::checkup::{bits, CamHeard, Check, Fix, FixAction, Listening, MicCapture, MicHeard, MicStart, Outcome, Said, Shot, StepId, StepResult};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use super::*;

struct Cap {
    ms: Arc<AtomicU64>,
    pcm: Vec<i16>,
}
impl MicCapture for Cap {
    fn level(&self) -> f32 {
        0.5
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

/// A port with a scripted machine; it keeps what the tile asked.
struct Fake {
    touchscreens: usize,
    mic: Result<String, MicStart>,
    ms: Arc<AtomicU64>,
    play_err: Option<String>,
    shot: Option<(CamHeard, Option<(u32, u32, Vec<u8>)>)>,
    stop: RefCell<bool>,
    refreshed: RefCell<usize>,
    asked: RefCell<Vec<Check>>,
    signs: RefCell<Vec<u8>>,
    audits: RefCell<Vec<String>>,
    reports: RefCell<Vec<Vec<Outcome>>>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            touchscreens: 0,
            mic: Ok("USB Audio".into()),
            ms: Arc::new(AtomicU64::new(0)),
            play_err: None,
            shot: Some((CamHeard::Picture { name: "USB Camera".into(), width: 2, height: 2, spread: 80 }, Some((2, 2, vec![200; 16])))),
            stop: RefCell::new(false),
            refreshed: RefCell::new(0),
            asked: RefCell::new(vec![]),
            signs: RefCell::new(vec![]),
            audits: RefCell::new(vec![]),
            reports: RefCell::new(vec![]),
        }
    }
    fn tick_ms(&self, ms: u64) {
        self.ms.store(ms, Ordering::SeqCst);
    }
}

fn pass(id: StepId, note: &str) -> StepResult {
    StepResult { id, outcome: Outcome::Pass, note: note.into(), advice: String::new(), fix: None, doctor: None }
}

impl CheckupPort for Fake {
    fn refresh(&self) -> usize {
        *self.refreshed.borrow_mut() += 1;
        self.touchscreens
    }
    fn verdict(&self, check: &Check) -> StepResult {
        self.asked.borrow_mut().push(check.clone());
        match check {
            Check::Display(Said::No) => StepResult { id: StepId::Display, outcome: Outcome::Fail, note: "The person did not see the bars.".into(), advice: "Check the cable.".into(), fix: Some(Fix { label: "Open Display", action: FixAction::Settings("tab:display") }), doctor: Some("display-dark") },
            Check::Display(_) => pass(StepId::Display, "seen"),
            Check::Touch(_) => StepResult { outcome: Outcome::Skipped, ..pass(StepId::Touch, "no touch screen") },
            Check::Keyboard { keys, .. } => pass(StepId::Keyboard, &format!("{keys} keys")),
            Check::Pointer { clicked, .. } => pass(StepId::Pointer, &format!("clicked {clicked}")),
            Check::Speaker { played: Err(e), .. } => StepResult { outcome: Outcome::Fail, ..pass(StepId::Speaker, e) },
            Check::Speaker { .. } => pass(StepId::Speaker, "heard"),
            Check::Microphone { heard: MicHeard::Samples(p), name } => pass(StepId::Microphone, &format!("{} samples from {name}", p.len())),
            Check::Microphone { .. } => StepResult { outcome: Outcome::Skipped, ..pass(StepId::Microphone, "no mic") },
            Check::Camera(_) => pass(StepId::Camera, "picture"),
            Check::Machine(id) => pass(*id, "machine"),
            Check::Skipped(id) => StepResult { outcome: Outcome::Skipped, ..pass(*id, "skipped") },
        }
    }
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String> {
        match &self.play_err {
            Some(e) => Err(e.clone()),
            None => Ok(Box::new(())),
        }
    }
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send> {
        let (heard, picture) = self.shot.clone().unwrap();
        Box::new(move || Shot { heard, picture })
    }
    fn start_mic(&self) -> Result<Listening, MicStart> {
        match &self.mic {
            Ok(name) => Ok(Listening { capture: Box::new(Cap { ms: self.ms.clone(), pcm: vec![1000; 8] }), name: name.clone() }),
            Err(e) => Err(e.clone()),
        }
    }
    fn keep_report(&self, results: &[StepResult]) {
        self.reports.borrow_mut().push(results.iter().map(|r| r.outcome).collect());
    }
    fn sign(&self, b: u8) {
        self.signs.borrow_mut().push(b);
    }
    fn sign_text(&self, b: u8) -> String {
        if b == bits::MICROPHONE { "Microphone on".into() } else { "Camera on".into() }
    }
    fn stop_asked(&self) -> bool {
        self.stop.replace(false)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audits.borrow_mut().push(format!("{action} {subject}"));
    }
}

fn step(id: StepId) -> usize {
    StepId::ALL.iter().position(|s| *s == id).unwrap()
}

fn start(t: &mut CheckupTile, p: &Fake, id: StepId) {
    t.handle(&Event::Row(step(id)), p);
    t.handle(&Event::Key(Key::Enter), p);
}

/// Run frames until the live step ends (a camera picture arrives from its thread). The wait is a generous wall-clock deadline, so a loaded
/// host cannot make it fail by a count of attempts running out.
fn settle(t: &mut CheckupTile, p: &Fake) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        if !t.is_live() {
            return;
        }
        t.wants_redraw(p);
        std::thread::yield_now();
    }
    panic!("the live step never ended");
}

fn draw(t: &mut CheckupTile, p: &Fake, w: u32, h: u32, s: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(8, 10, 22));
    g.clear_hits();
    t.render(&mut g, R::new(0.0, 0.0, w as f32, h as f32), s, p, 1);
    g
}

fn text(g: &Gfx) -> String {
    g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
}

#[test]
fn it_lists_fourteen_steps_not_done_yet_reads_the_machine_once_and_keeps_an_empty_report() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    assert_eq!(*p.refreshed.borrow(), 1);
    assert_eq!(t.results().len(), 14);
    assert!(t.results().iter().all(|r| r.outcome == Outcome::Pending));
    assert_eq!(p.reports.borrow().last().unwrap().len(), 14);
    let g = draw(&mut t, &p, 1280, 720, 1.0);
    let tx = text(&g);
    assert!(tx.contains("0 passed, 0 failed, 0 skipped, 14 not done yet") && tx.contains("Screen") && tx.contains("Shows four colour bars"), "{tx}");
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_RUN)) && g.hits.iter().any(|h| h.ev == Event::Tab(T_SHARE)));
    assert_eq!((t.title(), t.page().as_str()), ("Check-up", "display"));
    assert!(t.hint().contains("Enter start") && t.help().len() >= 5);
}

#[test]
fn the_machines_own_steps_are_judged_by_the_port_at_once_and_the_list_moves_on() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Monitor);
    assert_eq!(t.results()[step(StepId::Monitor)].note, "machine");
    assert_eq!(t.selected(), step(StepId::Network), "the next step that is not done opens");
    assert_eq!(p.asked.borrow().as_slice(), [Check::Machine(StepId::Monitor)]);
    assert!(p.signs.borrow().is_empty() && p.audits.borrow().is_empty());
    assert!(*p.refreshed.borrow() >= 2, "the machine is read again before a step that reads it");
}

#[test]
fn the_screen_step_shows_the_bars_and_y_or_n_decides_with_a_fix_for_a_no() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Display);
    assert!(matches!(t.active, Some(Active::Bars)) && t.keeps_screen_on());
    assert!(t.hint().contains("y yes"));
    t.handle(&Event::Key(Key::Char('n')), &p);
    assert_eq!(t.results()[0].outcome, Outcome::Fail);
    assert!(!t.is_active());
    t.handle(&Event::Row(0), &p);
    let g = draw(&mut t, &p, 1280, 720, 1.0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_FIX)), "the failed step offers its fix button");
    t.handle(&Event::Key(Key::Char('f')), &p);
    assert_eq!(t.take_request(), Some(("settings".into(), "tab:display".into())));
    assert_eq!(p.audits.borrow().as_slice(), ["checkup.fix display"]);
    assert_eq!(t.take_request(), None, "a request is handed over once");
}

#[test]
fn the_touch_step_without_a_touch_screen_is_judged_at_once_and_with_one_it_shows_a_target() {
    let mut p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Touch);
    assert_eq!(t.results()[step(StepId::Touch)].outcome, Outcome::Skipped);
    assert_eq!(p.asked.borrow().as_slice(), [Check::Touch(Said::No)]);
    p.touchscreens = 1;
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Touch);
    assert!(matches!(t.active, Some(Active::Target)));
    let g = draw(&mut t, &p, 1280, 720, 1.0);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(T_TARGET)));
    t.handle(&Event::Tab(T_TARGET), &p);
    assert_eq!(t.results()[step(StepId::Touch)].outcome, Outcome::Skipped);
    assert_eq!(p.asked.borrow().last(), Some(&Check::Touch(Said::Yes)));
}

#[test]
fn the_pointer_step_counts_a_click_on_the_target() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Pointer);
    assert!(matches!(t.active, Some(Active::Target)));
    t.handle(&Event::Tab(T_TARGET), &p);
    assert_eq!(t.results()[step(StepId::Pointer)].note, "clicked true");
}

#[test]
fn the_keyboard_step_counts_different_keys_and_ends_at_three() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Keyboard);
    t.handle(&Event::Key(Key::Char('a')), &p);
    t.handle(&Event::Key(Key::Char('a')), &p);
    assert!(matches!(&t.active, Some(Active::Keys(k)) if k.len() == 1), "the same key twice counts once");
    t.handle(&Event::Key(Key::Char('b')), &p);
    t.handle(&Event::Key(Key::Char('c')), &p);
    assert!(!t.is_active());
    assert_eq!(t.results()[step(StepId::Keyboard)].note, "3 keys");
}

#[test]
fn escape_in_the_keys_step_stops_it_and_a_second_escape_leaves() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Keyboard);
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Stay);
    assert!(!t.is_active());
    assert_eq!(t.results()[step(StepId::Keyboard)].outcome, Outcome::Pending, "nothing was judged");
    assert_eq!(t.handle(&Event::Key(Key::Esc), &p), Answer::Back);
}

#[test]
fn the_speaker_step_plays_through_the_port_and_says_a_refusal_in_words() {
    let mut p = Fake::new();
    p.play_err = Some("No sound card answered.".into());
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Speaker);
    assert!(matches!(&t.active, Some(Active::Tone { played: Err(e), .. }) if e == "No sound card answered."));
    let g = draw(&mut t, &p, 1280, 720, 1.0);
    assert!(text(&g).contains("The tone could not be played: No sound card answered."));
    t.handle(&Event::Key(Key::Char('n')), &p);
    assert_eq!(t.results()[step(StepId::Speaker)].outcome, Outcome::Fail);
}

#[test]
fn the_microphone_step_lights_the_sign_records_on_the_manual_clock_and_drops_the_sound() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Microphone);
    assert!(matches!(t.active, Some(Active::Record { .. })) && t.is_live());
    assert_eq!(p.audits.borrow().as_slice(), ["checkup.microphone "]);
    assert_eq!(p.signs.borrow().as_slice(), [bits::MICROPHONE]);
    p.tick_ms(1000);
    t.wants_redraw(&p);
    assert!((t.progress - 0.5).abs() < 0.01 && t.level > 0.3, "the meter moves with the clock");
    let g = draw(&mut t, &p, 1280, 720, 1.0);
    let tx = text(&g);
    assert!(tx.contains("Microphone on") && tx.contains("Recording 2 seconds from USB Audio"), "{tx}");
    p.tick_ms(2000);
    t.wants_redraw(&p);
    assert!(!t.is_live());
    assert_eq!(t.results()[step(StepId::Microphone)].note, "8 samples from USB Audio");
    assert_eq!(p.signs.borrow().last(), Some(&0), "the sign goes out");
    assert_eq!((t.level, t.progress), (0.0, 0.0));
}

#[test]
fn no_microphone_or_a_failed_start_is_judged_without_a_recording_or_a_sign() {
    for why in [MicStart::NoMic("none listed".into()), MicStart::Failed("busy".into())] {
        let mut p = Fake::new();
        p.mic = Err(why);
        let mut t = CheckupTile::new(&p);
        start(&mut t, &p, StepId::Microphone);
        assert!(!t.is_active());
        assert_eq!(t.results()[step(StepId::Microphone)].outcome, Outcome::Skipped);
        assert!(p.signs.borrow().is_empty() && p.audits.borrow().is_empty());
    }
}

#[test]
fn the_sign_click_stops_a_recording_and_nothing_is_kept() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Microphone);
    *p.stop.borrow_mut() = true;
    t.wants_redraw(&p);
    assert!(!t.is_active());
    assert_eq!(t.results()[step(StepId::Microphone)].outcome, Outcome::Pending);
    assert_eq!(t.msg.as_ref().map(|m| m.0.as_str()), Some("Stopped. Nothing was kept."));
    assert_eq!(p.signs.borrow().last(), Some(&0));
}

#[test]
fn the_camera_step_takes_one_picture_shows_it_and_drops_it_when_the_person_moves_on() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Camera);
    assert_eq!(p.audits.borrow().as_slice(), ["checkup.camera "]);
    settle(&mut t, &p);
    assert_eq!(t.results()[step(StepId::Camera)].note, "picture");
    assert!(t.preview.is_some(), "the picture is shown");
    assert_eq!(t.selected(), step(StepId::Camera), "the camera step stays open");
    let g = draw(&mut t, &p, 1280, 720, 1.0);
    assert!(g.rgba().iter().any(|b| *b != 0));
    t.handle(&Event::Key(Key::Down), &p);
    assert!(t.preview.is_none(), "moving on drops the picture");
    assert_eq!((p.signs.borrow().first().copied(), p.signs.borrow().last().copied()), (Some(bits::CAMERA), Some(0)), "the sign lights for the picture and goes out");
}

#[test]
fn a_picture_that_is_too_short_is_padded_and_an_empty_one_is_not_shown() {
    assert_eq!(opaque(2, 1, vec![9; 4]).unwrap().2, vec![9, 9, 9, 255, 255, 255, 255, 255]);
    assert!(opaque(0, 5, vec![1; 4]).is_none() && opaque(5, 0, vec![]).is_none());
}

#[test]
fn closing_the_tile_while_it_records_says_the_sign_must_go_out() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    assert!(!t.release(), "nothing runs: nothing to put out");
    start(&mut t, &p, StepId::Microphone);
    assert!(t.release());
    assert!(!t.is_active() && !t.release());
    start(&mut t, &p, StepId::Display);
    assert!(!t.release(), "the bars light no sign");
}

#[test]
fn share_needs_a_result_then_hands_the_report_to_doctor_and_d_opens_doctor() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    t.handle(&Event::Key(Key::Char('s')), &p);
    assert_eq!(t.take_request(), None);
    assert_eq!(t.msg.as_ref().map(|m| (m.0.as_str(), m.1)), Some(("Nothing has been checked yet. Start a step first.", false)));
    start(&mut t, &p, StepId::Monitor);
    t.handle(&Event::Tab(T_SHARE), &p);
    assert_eq!(t.take_request(), Some(("doctor".into(), "share".into())));
    t.handle(&Event::Key(Key::Char('d')), &p);
    assert_eq!(t.take_request(), Some(("doctor".into(), String::new())));
    t.handle(&Event::Tab(T_DISMISS), &p);
    assert!(t.msg.is_none());
}

#[test]
fn check_again_reads_the_machine_again_and_clears_every_result() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    start(&mut t, &p, StepId::Monitor);
    let before = *p.refreshed.borrow();
    t.handle(&Event::Key(Key::Char('r')), &p);
    assert_eq!(*p.refreshed.borrow(), before + 1);
    assert!(t.results().iter().all(|r| r.outcome == Outcome::Pending) && t.selected() == 0);
    assert_eq!(p.reports.borrow().last().unwrap().iter().filter(|o| **o == Outcome::Pending).count(), 14);
}

#[test]
fn skip_ends_the_chosen_step_as_skipped() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    t.handle(&Event::Row(step(StepId::Camera)), &p);
    t.handle(&Event::Key(Key::Char('k')), &p);
    assert_eq!(t.results()[step(StepId::Camera)].outcome, Outcome::Skipped);
    assert_eq!(p.asked.borrow().as_slice(), [Check::Skipped(StepId::Camera)]);
}

#[test]
fn every_finished_step_keeps_the_newest_report_with_the_port() {
    let p = Fake::new();
    let mut t = CheckupTile::new(&p);
    let n = p.reports.borrow().len();
    start(&mut t, &p, StepId::Monitor);
    start(&mut t, &p, StepId::Wifi);
    let r = p.reports.borrow();
    assert_eq!(r.len(), n + 2);
    assert_eq!(r.last().unwrap().iter().filter(|o| **o == Outcome::Pass).count(), 2);
}

#[test]
fn the_tile_draws_inside_its_area_at_every_size_with_a_step_open() {
    const SIZES: [(u32, u32, f32); 6] = [(360, 740, 1.0), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0), (2880, 1800, 2.25)];
    for (w, h, s) in SIZES {
        for id in [None, Some(StepId::Display), Some(StepId::Pointer), Some(StepId::Microphone), Some(StepId::Camera)] {
            let p = Fake::new();
            let mut t = CheckupTile::new(&p);
            if let Some(id) = id {
                start(&mut t, &p, id);
            }
            let g = draw(&mut t, &p, w, h, s);
            let a = R::new(0.0, 0.0, w as f32, h as f32);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.w > 0.0 && r.h > 0.0, "{w}x{h} {id:?}: {:?} is empty", hit.label);
                assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{w}x{h} {id:?}: {:?} {r:?} leaves the area", hit.label);
            }
            assert!(g.rgba().iter().any(|b| *b != 0), "{w}x{h} {id:?}: nothing drawn");
            t.release();
        }
    }
}

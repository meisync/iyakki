//! The Jobs strip and list on a fake port with a clock the test moves: no process, no sleep, no wall time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{channel, Sender};

use iyakki_tile_api::runs::{Job, JobState, JobsPort};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::ui::{Event, Key, R};

use super::jobs::*;

struct Fake {
    jobs: RefCell<Vec<Job>>,
    finished: RefCell<Vec<String>>,
    log: RefCell<String>,
    audits: RefCell<Vec<String>>,
    stops: RefCell<Vec<String>>,
    stopped: Sender<String>,
    reads: Cell<u32>,
}

impl Fake {
    fn new() -> (Fake, std::sync::mpsc::Receiver<String>) {
        let (tx, rx) = channel();
        (Fake { jobs: Default::default(), finished: Default::default(), log: Default::default(), audits: Default::default(), stops: Default::default(), stopped: tx, reads: Cell::new(0) }, rx)
    }
}

impl JobsPort for Fake {
    fn visible(&self, _ws: Option<&str>) -> Vec<Job> {
        self.reads.set(self.reads.get() + 1);
        self.jobs.borrow().clone()
    }
    fn take_finished(&self, _ws: &str) -> Vec<String> {
        std::mem::take(&mut *self.finished.borrow_mut())
    }
    fn log_len(&self, _id: &str) -> u64 {
        self.log.borrow().len() as u64
    }
    fn log_text(&self, _id: &str, _max: usize) -> Result<String, String> {
        Ok(self.log.borrow().clone())
    }
    fn stop_job(&self, id: &str) -> Box<dyn FnOnce() + Send> {
        self.stops.borrow_mut().push(id.to_string());
        let tx = self.stopped.clone();
        let id = id.to_string();
        Box::new(move || {
            let _ = tx.send(id);
        })
    }
    fn dismiss(&self, id: &str) -> Result<(), String> {
        self.jobs.borrow_mut().retain(|j| j.id != id);
        Ok(())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audits.borrow_mut().push(format!("{action} {subject}"));
    }
}

const T0: u64 = 1_000_000;

fn running(id: &str, label: &str) -> Job {
    Job { id: id.into(), label: label.into(), command: "sleep 30".into(), state: JobState::Running, started_ms: T0 - 70_000, ended_ms: 0, progress: None, last_line: "started".into(), last_active_ms: T0 }
}

fn ended(id: &str, state: JobState) -> Job {
    Job { state, ended_ms: T0 - 5_000, last_active_ms: 0, ..running(id, "done job") }
}

/// A strip with a clock the test moves.
fn ui(ws: Option<&str>) -> (JobsUi, Rc<Cell<u64>>) {
    let mut u = JobsUi::new(ws);
    let c = Rc::new(Cell::new(T0));
    let c2 = c.clone();
    u.set_clock(Box::new(move || c2.get()));
    (u, c)
}

#[test]
fn it_animates_only_while_a_job_runs() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    assert!(!u.poll(&p, true) && !u.animating(), "nothing to show, nothing moves");
    p.jobs.borrow_mut().push(running("j1", "nap"));
    assert!(u.poll(&p, true) && u.animating());
    p.jobs.borrow_mut()[0] = ended("j1", JobState::Stopped);
    u.poll(&p, true);
    assert!(!u.animating(), "a finished job does not animate");
    assert_eq!(u.jobs().len(), 1, "it stays until dismissed");
    // frames: ten a second while running, once when something changed, never otherwise
    assert!(u.frame_due(), "the change is drawn once");
    assert!(!u.frame_due() && !u.frame_due());
    p.jobs.borrow_mut()[0] = running("j1", "nap");
    u.poll(&p, true);
    assert!(u.frame_due(), "the change");
    let due: Vec<bool> = (0..6).map(|_| u.frame_due()).collect();
    assert_eq!(due, [false, false, true, false, false, true]);
}

#[test]
fn the_poll_is_paced_by_the_clock_the_strip_was_given() {
    let (p, _rx) = Fake::new();
    let (mut u, c) = ui(Some(""));
    u.poll(&p, false);
    assert_eq!(p.reads.get(), 1);
    // nothing to show: every three seconds
    c.set(T0 + 2_999);
    u.poll(&p, false);
    assert_eq!(p.reads.get(), 1);
    c.set(T0 + 3_000);
    u.poll(&p, false);
    assert_eq!(p.reads.get(), 2);
    // a run is going in the chat and there is no job yet: look every 400 ms
    u.set_hurry(true);
    c.set(T0 + 3_399);
    u.poll(&p, false);
    assert_eq!(p.reads.get(), 2);
    c.set(T0 + 3_400);
    u.poll(&p, false);
    assert_eq!(p.reads.get(), 3);
    // with a job: every second
    u.set_hurry(false);
    p.jobs.borrow_mut().push(running("j1", "nap"));
    c.set(T0 + 3_400 + 3_000);
    u.poll(&p, false);
    let r = p.reads.get();
    c.set(T0 + 3_400 + 3_999);
    u.poll(&p, false);
    assert_eq!(p.reads.get(), r);
    c.set(T0 + 3_400 + 4_000);
    u.poll(&p, false);
    assert_eq!(p.reads.get(), r + 1);
    // a workspace change forgets the list and reads at once
    u.set_workspace("w2");
    assert!(u.jobs().is_empty());
    u.poll(&p, false);
    assert_eq!(p.reads.get(), r + 2);
}

#[test]
fn the_running_jobs_come_first_each_group_as_the_port_listed_them() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(None);
    p.jobs.borrow_mut().extend([ended("j3", JobState::Succeeded), running("j2", "b"), ended("j1", JobState::Lost), running("j0", "a")]);
    u.poll(&p, true);
    assert_eq!(u.jobs().iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), ["j2", "j0", "j3", "j1"]);
}

#[test]
fn a_finished_job_gives_one_event_line_and_stays_until_dismissed() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    p.jobs.borrow_mut().push(ended("j1", JobState::Failed(2)));
    p.finished.borrow_mut().push("Job “build” (j1) failed: failed (exit 2) after 3s".into());
    u.poll(&p, true);
    let ev = u.take_events();
    assert_eq!(ev.len(), 1);
    assert!(ev[0].contains("build") && ev[0].contains("failed (exit 2)"), "{ev:?}");
    u.poll(&p, true);
    assert!(u.take_events().is_empty(), "once");
    assert!(u.handle(&ev_act(0), &p), "Dismiss is the second button of a finished row");
    assert!(u.jobs().is_empty() && p.jobs.borrow().is_empty());
    assert!(!p.audits.borrow().iter().any(|a| a.starts_with("job.stop")), "dismissing is not stopping");
}

#[test]
fn the_log_view_follows_scrolls_and_keeps_its_place() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    p.jobs.borrow_mut().push(running("j1", "noisy"));
    *p.log.borrow_mut() = (1..=300).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
    u.poll(&p, true);
    assert!(u.handle(&ev_log(0), &p));
    assert_eq!(u.open_id(), Some("j1"));
    assert!(u.log_text().ends_with("300") && u.log_following(), "it starts at the end");
    // the wheel scrolls up: no longer following, and output that arrives does not move what is being read
    assert!(u.handle(&Event::Scroll(5), &p));
    assert!(!u.log_following());
    let before = u.log_scroll();
    *p.log.borrow_mut() = (1..=400).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
    u.poll(&p, true);
    assert!(u.log_text().ends_with("400"));
    assert_eq!(u.log_scroll(), before + 100, "100 new lines below: the same text stays in view");
    // Follow goes back to the end
    assert!(u.handle(&Event::Tab(LOG_FOLLOW), &p) && u.log_following());
    // keys: other keys are not taken (typing still goes to the chat); Esc closes the view and is used up
    assert!(!u.handle(&Event::Key(Key::Char('x')), &p));
    assert!(u.handle(&Event::Key(Key::Esc), &p) && !u.log_open());
    assert!(!u.handle(&Event::Key(Key::Esc), &p), "with no log open Esc belongs to the chat");
    // clicking the same Log button twice closes it again
    u.handle(&ev_log(0), &p);
    assert!(u.log_open());
    u.handle(&ev_log(0), &p);
    assert!(!u.log_open());
}

#[test]
fn the_log_view_pages_and_ends_and_dismissing_the_job_closes_its_log() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    p.jobs.borrow_mut().push(ended("j1", JobState::Succeeded));
    *p.log.borrow_mut() = "a\nb".into();
    u.poll(&p, true);
    u.handle(&ev_log(0), &p);
    u.handle(&Event::Key(Key::PageUp), &p);
    assert_eq!(u.log_scroll(), 12);
    u.handle(&Event::Key(Key::PageDown), &p);
    assert_eq!(u.log_scroll(), 0);
    u.handle(&Event::Key(Key::PageUp), &p);
    u.handle(&Event::Key(Key::End), &p);
    assert!(u.log_following());
    u.handle(&Event::Tab(LOG_CLOSE), &p);
    assert!(!u.log_open());
    u.handle(&ev_log(0), &p);
    u.handle(&ev_act(0), &p);
    assert!(!u.log_open() && u.jobs().is_empty());
}

#[test]
fn stop_ends_the_job_on_a_thread_of_its_own_and_a_second_click_does_nothing() {
    let (p, rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    p.jobs.borrow_mut().push(running("j1", "stubborn"));
    u.poll(&p, true);
    assert!(u.handle(&ev_act(0), &p));
    assert_eq!(rx.recv().unwrap(), "j1", "the stop ran on its thread");
    assert!(u.handle(&ev_act(0), &p), "a second click is harmless");
    assert_eq!(p.stops.borrow().as_slice(), ["j1"], "asked once");
    assert_eq!(p.audits.borrow().as_slice(), ["job.stop j1 stubborn"]);
    // while it stops the button says so and is off
    let mut g = Gfx::new(800, 300);
    u.draw_strip(&mut g, R::new(10.0, 10.0, 700.0, 120.0), 14.0, 1.0, 1);
    assert!(g.text_log.iter().any(|t| t.2 == "Stopping…"));
    assert!(!g.hits.iter().any(|h| h.label == "Stop"));
    // once the engine reports it ended, the mark goes and a later job with that id could be stopped again
    p.jobs.borrow_mut()[0] = ended("j1", JobState::Stopped);
    u.poll(&p, true);
    assert!(!u.animating());
}

#[test]
fn rows_fit_and_their_buttons_are_big_enough_to_click_at_every_screen_size() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    let mut a = running("j0", "Android emulator and SDK install with a very long name that has to be cut to fit the row");
    a.progress = Some(42.0);
    p.jobs.borrow_mut().extend([a, running("j1", "Second job"), ended("j2", JobState::Succeeded), running("j3", "Fourth")]);
    u.poll(&p, true);
    assert_eq!(u.jobs().len(), 4);
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 1.0), (800, 480, 1.0)] {
        let fs = (14.0 * s).max(12.0);
        let max_h = h as f32 * 0.4;
        let sh = u.strip_height(fs, s, max_h);
        assert!(sh > 0.0 && sh <= max_h, "{w}x{h}: strip {sh} of {max_h}");
        let mut g = Gfx::new(w, h);
        let r = R::new(20.0, 100.0, w as f32 * 0.5, sh);
        u.draw_strip(&mut g, r, fs, s, 7);
        assert!(!u.geoms.is_empty() && u.geoms.len() <= MAX_ROWS, "{w}x{h}");
        let mut prev_bottom = r.y;
        for gm in &u.geoms {
            assert!(gm.rect.x >= r.x - 0.5 && gm.rect.x + gm.rect.w <= r.x + r.w + 0.5, "{w}x{h}: row inside the strip");
            assert!(gm.rect.y >= prev_bottom - 0.5, "rows do not overlap");
            prev_bottom = gm.rect.y + gm.rect.h;
            assert!(prev_bottom <= r.y + sh + 0.5, "{w}x{h}: row inside the strip's height");
            for b in [gm.log_btn, gm.act_btn] {
                assert!(b.h >= 22.0 && b.w >= 36.0, "{w}x{h}: button {b:?} is large enough to click");
                assert!(b.x >= gm.rect.x && b.x + b.w <= gm.rect.x + gm.rect.w && b.y >= gm.rect.y && b.y + b.h <= gm.rect.y + gm.rect.h, "{w}x{h}: button inside its row");
            }
            assert!(gm.log_btn.x + gm.log_btn.w <= gm.act_btn.x, "buttons do not overlap");
            assert!(gm.bar.x + gm.bar.w <= gm.rect.x + gm.rect.w);
        }
        let hits: Vec<_> = g.hits.iter().map(|h| (h.label.clone(), h.ev.clone())).collect();
        assert!(hits.contains(&("Log".into(), ev_log(0))), "{w}x{h}: {hits:?}");
        assert!(hits.iter().any(|(l, e)| l == "Stop" && *e == ev_act(0)), "{w}x{h}");
        let rows = u.draw_list(&mut g, R::new(10.0, 60.0, w as f32 * 0.9, h as f32 * 0.8), fs, s, 3);
        assert_eq!(rows, 4, "{w}x{h}: the list shows every job");
        assert!(g.hits.iter().any(|h| h.label == "Dismiss"), "the finished one has Dismiss");
    }
    // more jobs than rows: the rest are counted
    let mut g = Gfx::new(800, 480);
    u.draw_strip(&mut g, R::new(10.0, 10.0, 700.0, u.strip_height(14.0, 1.0, 300.0)), 14.0, 1.0, 1);
    assert_eq!(u.overflow, 1);
    assert!(g.text_log.iter().any(|t| t.2.starts_with("+1 more job")));
}

fn bar_pixels(u: &mut JobsUi, tick: u64) -> Vec<u8> {
    let mut g = Gfx::new(800, 300);
    u.draw_strip(&mut g, R::new(10.0, 10.0, 700.0, 120.0), 14.0, 1.0, tick);
    let b = u.geoms[0].bar;
    let mut v = Vec::new();
    for x in (b.x as u32)..((b.x + b.w) as u32) {
        let o = (((b.y + b.h / 2.0) as usize) * g.width() as usize + x as usize) * 4;
        v.extend_from_slice(&g.rgba()[o..o + 4]);
    }
    v
}

#[test]
fn the_bar_slides_without_a_percentage_and_fills_with_one_and_the_frames_differ() {
    let (p, _rx) = Fake::new();
    let (mut u, _c) = ui(Some(""));
    p.jobs.borrow_mut().push(running("j1", "indeterminate"));
    u.poll(&p, true);
    assert_eq!(u.jobs()[0].progress, None);
    let (f0, f1, f0b) = (bar_pixels(&mut u, 0), bar_pixels(&mut u, 20), bar_pixels(&mut u, 0));
    assert_ne!(f0, f1, "the indeterminate bar moves between frames");
    assert_eq!(f0, f0b, "and draws the same for the same tick");
    p.jobs.borrow_mut()[0].progress = Some(50.0);
    u.poll(&p, true);
    assert_eq!(u.jobs()[0].progress, Some(50.0));
    assert_eq!(bar_pixels(&mut u, 0), bar_pixels(&mut u, 20), "a determinate bar does not slide");
}

#[test]
fn a_quiet_running_job_is_drawn_still_and_amber_with_how_long_it_has_been_quiet_and_a_busy_one_is_not() {
    let (mut u, _c) = ui(Some(""));
    u.set_jobs(vec![running("j1", "waiting")]);
    let frame = |u: &mut JobsUi, tick: u64| -> Gfx {
        let mut g = Gfx::new(800, 300);
        g.clear(rgb(0, 0, 0));
        u.draw_strip(&mut g, R::new(10.0, 10.0, 700.0, 120.0), 14.0, 1.0, tick);
        g
    };
    let text = |g: &Gfx| g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join("\n");
    let amber = |g: &Gfx, row: R| -> usize {
        let mut n = 0;
        for y in (row.y as u32)..((row.y + row.h) as u32) {
            for x in (row.x as u32)..((row.x + row.w) as u32) {
                let o = (y as usize * g.width() as usize + x as usize) * 4;
                let p = &g.rgba()[o..o + 4];
                if p[0] > 200 && (130..230).contains(&p[1]) && p[2] < 140 {
                    n += 1;
                }
            }
        }
        n
    };
    let (b0, b1) = (frame(&mut u, 0), frame(&mut u, 20));
    let row = u.geoms[0].rect;
    assert!(!text(&b0).contains("no activity"), "{}", text(&b0));
    assert_ne!(b0.rgba(), b1.rgba(), "a busy job animates");
    assert_eq!(amber(&b0, row), 0, "no amber while it shows signs of work");
    let mut quiet = running("j1", "waiting");
    quiet.last_active_ms = T0 - 65_000;
    u.set_jobs(vec![quiet]);
    let (q0, q1) = (frame(&mut u, 0), frame(&mut u, 20));
    assert!(text(&q0).contains("no activity for 1m"), "{}", text(&q0));
    assert!(text(&q0).contains("started"), "the last line is still shown: {}", text(&q0));
    assert_eq!(q0.rgba(), q1.rgba(), "a quiet job stands still between frames");
    assert!(amber(&q0, row) > 20, "amber spinner and bar: {} pixels", amber(&q0, row));
    u.set_jobs(vec![running("j1", "waiting")]);
    assert!(!text(&frame(&mut u, 0)).contains("no activity"));
}

#[test]
fn how_a_job_ended_is_said_in_words() {
    let (mut u, _c) = ui(Some(""));
    u.set_jobs(vec![ended("a", JobState::Succeeded), ended("b", JobState::Failed(124)), ended("c", JobState::Failed(3)), ended("d", JobState::Stopped), ended("e", JobState::Lost)]);
    let mut g = Gfx::new(1000, 900);
    u.draw_list(&mut g, R::new(10.0, 10.0, 900.0, 800.0), 14.0, 1.0, 0);
    let t: Vec<String> = g.text_log.iter().map(|x| x.2.clone()).collect();
    for want in ["succeeded", "timed out", "failed (exit 3)", "stopped", "lost"] {
        assert!(t.iter().any(|x| x == want), "{want}: {t:?}");
    }
}

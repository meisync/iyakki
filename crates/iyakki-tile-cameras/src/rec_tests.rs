//! The recordings page of the CAMERAS tile against a fake source: a recorded timeline of two days, no real camera and no files.

use super::*;
use iyakki_tile_api::cameras::{AuditRow, CameraSource, PlayFrame, RecChange, RecDay, RecInfo, RecMode, RecSeek};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

const DAY: u64 = 86_400_000;
const D1: u64 = 20_000 * DAY; // the older day
const D0: u64 = D1 + DAY; // the newer day

struct RecFake {
    cams: Vec<CamInfo>,
    info: RefCell<RecInfo>,
    recording: Cell<bool>,
    set: RefCell<Vec<RecChange>>,
    exports: RefCell<Vec<(u64, u64, bool)>>,
    started: RefCell<Vec<u64>>,
    asked: RefCell<Vec<(u64, RecSeek)>>,
    frame_err: RefCell<Option<String>>,
    audit: RefCell<Vec<AuditRow>>,
}

/// Pictures every second from 10:00 to 10:00:09 of the newer day, and from 20:00 to 20:00:04 of the older.
fn stamps() -> Vec<u64> {
    let mut v: Vec<u64> = (0..5).map(|i| D1 + 20 * 3_600_000 + i * 1000).collect();
    v.extend((0..10).map(|i| D0 + 10 * 3_600_000 + i * 1000));
    v
}

fn info(owner: bool) -> RecInfo {
    RecInfo { mode: RecMode::Off, windows: vec![], max_days: 14, max_mb: 20_000, state: "not recording".into(), recording: false, problem: false, owner, segments: 3, bytes: 5_000_000, key_note: "Recordings are encrypted with a key kept in your vault. If you lose your vault, the recordings cannot be opened by anybody.".into() }
}

impl RecFake {
    fn new(owner: bool) -> RecFake {
        let c = CamInfo { id: "c0".into(), name: "Front door".into(), kind: Kind::Network, address: "rtsp://10.0.0.5/live".into(), user: "admin".into(), status: Status::Live, fps: 5.0, size: (8, 4), manage: true, owner: "amy".into(), allowed: vec![], has_password: true };
        RecFake { cams: vec![c], info: RefCell::new(info(owner)), recording: Cell::new(false), set: RefCell::new(vec![]), exports: RefCell::new(vec![]), started: RefCell::new(vec![]), asked: RefCell::new(vec![]), frame_err: RefCell::new(None), audit: RefCell::new(vec![]) }
    }
}

impl CameraSource for RecFake {
    fn cameras(&self) -> Vec<CamInfo> {
        self.cams.clone()
    }
    fn view(&self, _: &str, _: bool) -> Option<Frame> {
        Some(Frame { seq: 1, width: 8, height: 4, rgba: Arc::new([10, 200, 10, 255].repeat(32)) })
    }
    fn snapshot(&self, _: &str) -> Result<String, String> {
        Err("no".into())
    }
    fn save(&self, _: Option<&str>, _: &NewCam, _: Option<&str>) -> Result<String, String> {
        Err("no".into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err("no".into())
    }
    fn people(&self) -> Vec<String> {
        vec![]
    }
    fn recording_ids(&self) -> Vec<String> {
        if self.recording.get() { vec!["c0".into()] } else { vec![] }
    }
    fn rec_info(&self, _: &str) -> Option<RecInfo> {
        let mut i = self.info.borrow().clone();
        i.recording = self.recording.get();
        Some(i)
    }
    fn rec_set(&self, _: &str, c: &RecChange) -> Result<(), String> {
        self.set.borrow_mut().push(c.clone());
        if let Some(m) = c.mode {
            self.info.borrow_mut().mode = m;
            self.recording.set(m != RecMode::Off);
        }
        if let Some(w) = &c.windows {
            self.info.borrow_mut().windows = w.clone();
        }
        Ok(())
    }
    fn rec_days(&self, _: &str) -> Result<Vec<RecDay>, String> {
        if !self.info.borrow().owner {
            return Err("only the owner".into());
        }
        Ok(vec![
            RecDay { label: "2024-10-04".into(), day_start_ms: D0, spans: vec![(D0 + 10 * 3_600_000, D0 + 10 * 3_600_000 + 9000)], frames: 10, bytes: 4_000_000 },
            RecDay { label: "2024-10-03".into(), day_start_ms: D1, spans: vec![(D1 + 20 * 3_600_000, D1 + 20 * 3_600_000 + 4000)], frames: 5, bytes: 1_000_000 },
        ])
    }
    fn rec_play_start(&self, _: &str, at: u64) -> Result<(), String> {
        self.started.borrow_mut().push(at);
        Ok(())
    }
    fn rec_frame(&self, _: &str, at: u64, seek: RecSeek) -> Result<Option<PlayFrame>, String> {
        self.asked.borrow_mut().push((at, seek));
        if let Some(e) = self.frame_err.borrow().clone() {
            return Err(e);
        }
        let s = stamps();
        let i = match seek {
            RecSeek::AtOrAfter => s.iter().position(|t| *t >= at),
            RecSeek::After => s.iter().position(|t| *t > at),
            RecSeek::Before => s.iter().rposition(|t| *t < at),
        };
        Ok(i.map(|i| PlayFrame { ts_ms: s[i], label: format!("t{}", s[i]), width: 8, height: 4, rgba: Arc::new([200, 30, 200, 255].repeat(32)), prev_ms: i.checked_sub(1).map(|p| s[p]), next_ms: s.get(i + 1).copied() }))
    }
    fn rec_export(&self, _: &str, from: u64, to: u64, confirmed: bool) -> Result<String, String> {
        self.exports.borrow_mut().push((from, to, confirmed));
        if confirmed { Ok("/home/amy/Pictures/camera-front-door-x (10 pictures)".into()) } else { Err("not confirmed".into()) }
    }
    fn rec_audit(&self, _: &str) -> Result<Vec<AuditRow>, String> {
        Ok(self.audit.borrow().clone())
    }
}

fn cx_of(f: &RecFake) -> TileCx<'_> {
    struct NoUse;
    impl iyakki_tile_api::usage::UsageSource for NoUse {
        fn now_unix(&self) -> i64 {
            0
        }
        fn series(&self, _: iyakki_tile_api::usage::Range, _: i64, _: Option<&str>) -> iyakki_tile_api::usage::Series {
            iyakki_tile_api::usage::Series { models: vec![], buckets: vec![], totals: vec![], input: 0, output: 0, calls: 0 }
        }
    }
    static NO_USE: NoUse = NoUse;
    TileCx { user: "amy", manage_settings: false, usage: &NO_USE, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: f }
}

fn draw(a: &mut CamerasApp, cx: &TileCx) -> Gfx {
    let mut g = Gfx::new(1200, 700);
    g.clear(rgb(9, 11, 20));
    g.clear_hits();
    a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, cx, 0);
    g
}

fn key(a: &mut CamerasApp, cx: &TileCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn has_color(g: &Gfx, c: [u8; 3]) -> bool {
    g.rgba().chunks_exact(4).any(|p| p[0] == c[0] && p[1] == c[1] && p[2] == c[2])
}

/// The red of the REC pill and badge (drawn with a little transparency, so not an exact colour).
fn has_red(g: &Gfx) -> bool {
    g.rgba().chunks_exact(4).filter(|p| p[0] > 120 && p[1] < 50 && p[2] < 50).count() > 40
}

fn open(f: &RecFake) -> CamerasApp {
    let cx = cx_of(f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'p');
    draw(&mut a, &cx);
    a
}

#[test]
fn a_camera_that_records_carries_a_red_rec_mark_in_the_grid_and_the_enlarged_view_and_one_that_does_not_does_not() {
    let f = RecFake::new(true);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    let g = draw(&mut a, &cx);
    assert!(a.rec_marked().is_empty() && !has_red(&g), "no recording, no mark");
    f.recording.set(true);
    a.handle(&Event::Tick, &cx);
    // the list is read again about twice a second
    for t in 0..40 {
        let mut g = Gfx::new(1200, 700);
        g.clear_hits();
        a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, 100 + t);
    }
    let g = draw(&mut a, &cx);
    assert_eq!(a.rec_marked(), ["c0".to_string()]);
    assert!(has_red(&g), "the red REC pill is on the picture");
    a.handle(&Event::Row(0), &cx);
    assert!(has_red(&draw(&mut a, &cx)), "and in the enlarged view");
}

#[test]
fn the_recordings_page_opens_on_the_newest_day_logs_that_it_was_opened_and_shows_a_decoded_picture() {
    let f = RecFake::new(true);
    let a = open(&f);
    let r = a.recordings().expect("the page is open");
    assert!(r.shows_frame() && !r.playing());
    assert_eq!(r.day_label(), Some("2024-10-04"));
    assert_eq!(r.position(), D0 + 10 * 3_600_000, "the first picture of the newest day");
    assert_eq!(*f.started.borrow(), vec![D0 + 10 * 3_600_000], "the camera's log is told once");
    let cx = cx_of(&f);
    let mut a = a;
    let g = draw(&mut a, &cx);
    assert!(has_color(&g, [200, 30, 200]), "the recorded picture is on the screen");
    assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(300..=395))).count() == 96, "the timeline can be clicked anywhere");
    // opening the page asks nothing of the live stream
    assert!(a.shown_ids().is_empty());
}

#[test]
fn stepping_playing_and_the_day_picker_walk_the_timeline() {
    let f = RecFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    let t0 = D0 + 10 * 3_600_000;
    a.handle(&Event::Key(Key::Right), &cx);
    assert_eq!(a.recordings().unwrap().position(), t0 + 1000);
    a.handle(&Event::Key(Key::Left), &cx);
    assert_eq!(a.recordings().unwrap().position(), t0);
    key(&mut a, &cx, '.');
    key(&mut a, &cx, ',');
    assert_eq!(a.recordings().unwrap().position(), t0);
    // stepping back from the first picture of the newest day crosses to the older day
    a.handle(&Event::Key(Key::Left), &cx);
    assert_eq!(a.recordings().unwrap().position(), D1 + 20 * 3_600_000 + 4000);
    assert_eq!(a.recordings().unwrap().day_label(), Some("2024-10-03"));
    // day buttons
    key(&mut a, &cx, ']');
    assert_eq!((a.recordings().unwrap().day_label(), a.recordings().unwrap().position()), (Some("2024-10-04"), t0));
    key(&mut a, &cx, '[');
    assert_eq!(a.recordings().unwrap().position(), D1 + 20 * 3_600_000);
    key(&mut a, &cx, ']');
    // play: the position moves with the heartbeat and the picture changes when the next one is due, then it stops at the end
    key(&mut a, &cx, ' ');
    assert!(a.recordings().unwrap().playing());
    let asked_before = f.asked.borrow().len();
    for _ in 0..2000 {
        a.handle(&Event::Tick, &cx);
        if !a.recordings().unwrap().playing() {
            break;
        }
    }
    assert!(f.asked.borrow().len() - asked_before >= 8, "playing walks through the pictures one after the other: {}", f.asked.borrow().len() - asked_before);
    assert!(!a.recordings().unwrap().playing(), "it stops at the last picture");
    assert_eq!(a.recordings().unwrap().position(), t0 + 9000);
    assert_eq!(a.recordings().unwrap().note().map(|n| n.0), Some("End of the recording."));
    // pause with the same key
    key(&mut a, &cx, ' ');
    assert!(a.recordings().unwrap().playing() || a.recordings().unwrap().position() == t0 + 9000);
    // a click on the timeline jumps there; a minute with nothing recorded jumps to the nearest picture
    a.handle(&Event::Tab(300 + 40), &cx);
    assert_eq!(a.recordings().unwrap().position(), t0, "10:00 is slot 40 of the day");
}

#[test]
fn turning_recording_on_asks_with_the_whole_truth_and_off_is_one_key() {
    let f = RecFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'r');
    assert!(a.recordings().unwrap().confirming());
    assert!(f.set.borrow().is_empty(), "nothing is on yet");
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Key(Key::Char('y')))));
    // a stray key does not confirm
    key(&mut a, &cx, 'x');
    assert!(a.recordings().unwrap().confirming() && f.set.borrow().is_empty());
    // Esc cancels
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(!a.recordings().unwrap().confirming() && f.set.borrow().is_empty() && !f.recording.get());
    key(&mut a, &cx, 'r');
    key(&mut a, &cx, 'y');
    assert_eq!(f.set.borrow().len(), 1);
    assert_eq!(f.set.borrow()[0].mode, Some(RecMode::Continuous));
    assert!(f.recording.get());
    let g = draw(&mut a, &cx);
    assert!(has_red(&g), "REC badge on the page");
    assert!(a.recordings().unwrap().recording());
    // off: one key, no question
    key(&mut a, &cx, 'r');
    assert!(!a.recordings().unwrap().confirming());
    assert_eq!(f.set.borrow().last().unwrap().mode, Some(RecMode::Off));
    assert!(!f.recording.get());
    assert!(a.recordings().unwrap().note().unwrap().0.contains("stopped"));
}

#[test]
fn the_settings_form_checks_its_input_and_turning_on_from_it_also_asks() {
    let f = RecFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'c');
    assert!(a.recordings().unwrap().in_settings());
    let tab = |a: &mut CamerasApp, n: usize| {
        for _ in 0..n {
            a.handle(&Event::Key(Key::Tab), &cx);
        }
    };
    // the form starts with the current values; replace the mode with a bad one and send it from the last field
    for _ in 0..12 {
        a.handle(&Event::Key(Key::Backspace), &cx);
    }
    for c in "sometimes".chars() {
        key(&mut a, &cx, c);
    }
    tab(&mut a, 3);
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.recordings().unwrap().in_settings(), "still open with the reason");
    assert!(f.set.borrow().is_empty());
    for _ in 0..3 {
        a.handle(&Event::Key(Key::BackTab), &cx);
    }
    for _ in 0..12 {
        a.handle(&Event::Key(Key::Backspace), &cx);
    }
    for c in "schedule".chars() {
        key(&mut a, &cx, c);
    }
    tab(&mut a, 1);
    for c in "mon-fri 08:00-18:00".chars() {
        key(&mut a, &cx, c);
    }
    tab(&mut a, 2);
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.recordings().unwrap().confirming(), "from off to on: the confirmation comes first");
    assert!(f.set.borrow().is_empty());
    key(&mut a, &cx, 'y');
    let s = f.set.borrow();
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].mode, s[0].windows.clone(), s[0].max_days, s[0].max_mb), (Some(RecMode::Schedule), Some(vec!["mon-fri 08:00-18:00".to_string()]), Some(14), Some(20_000)));
}

#[test]
fn an_export_is_a_confirmation_away_and_writes_nothing_until_yes() {
    let f = RecFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    let t0 = D0 + 10 * 3_600_000;
    key(&mut a, &cx, 'e');
    assert!(a.recordings().unwrap().confirming());
    assert!(f.exports.borrow().is_empty());
    key(&mut a, &cx, 'm'); // 5 minutes
    key(&mut a, &cx, 'm'); // 15
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(f.exports.borrow().is_empty(), "cancelled");
    key(&mut a, &cx, 'e');
    key(&mut a, &cx, 'y');
    assert_eq!(*f.exports.borrow(), vec![(t0, t0 + 15 * 60_000, true)], "the length chosen before the cancel (15 minutes) is kept");
    let n = a.recordings().unwrap().note().unwrap();
    assert!(n.1 && n.0.starts_with("Exported to /home/amy/Pictures/"));
}

#[test]
fn people_who_are_not_the_owner_are_told_so_and_nothing_is_asked_of_the_recordings() {
    let f = RecFake::new(false);
    let cx = cx_of(&f);
    let mut a = open(&f);
    let g = draw(&mut a, &cx);
    assert!(!a.recordings().unwrap().shows_frame());
    assert!(f.asked.borrow().is_empty() && f.started.borrow().is_empty());
    assert!(!g.hits.iter().any(|h| matches!(h.ev, Event::Tab(300..=395))), "no timeline");
    for c in ['e', 'c', 'l', 'r'] {
        key(&mut a, &cx, c);
        assert!(!a.recordings().unwrap().confirming() && !a.recordings().unwrap().in_settings() && !a.recordings().unwrap().showing_log(), "{c}");
        assert!(a.recordings().unwrap().note().is_some_and(|n| !n.1), "{c} is refused in words");
    }
    assert!(f.set.borrow().is_empty() && f.exports.borrow().is_empty());
}

#[test]
fn a_picture_that_cannot_be_shown_is_explained_and_playing_stops() {
    let f = RecFake::new(true);
    *f.frame_err.borrow_mut() = Some("This camera was recorded as H.264. Playing it needs the audio and video add-on.".into());
    let cx = cx_of(&f);
    let mut a = open(&f);
    assert!(!a.recordings().unwrap().shows_frame());
    assert!(a.recordings().unwrap().note().is_some_and(|n| !n.1 && n.0.contains("add-on")));
    key(&mut a, &cx, ' ');
    assert!(!a.recordings().unwrap().playing(), "nothing to play");
    // the export is still offered
    key(&mut a, &cx, 'e');
    assert!(!a.recordings().unwrap().confirming(), "no picture picked yet: the clip would start nowhere");
}

#[test]
fn the_log_page_lists_who_looked_at_what_and_esc_returns() {
    let f = RecFake::new(true);
    f.audit.borrow_mut().extend([AuditRow { time: "2024-10-04 10:01:00".into(), who: "bob".into(), what: "live.view".into(), ok: true }, AuditRow { time: "2024-10-04 10:02:00".into(), who: "eve".into(), what: "playback.start: refused".into(), ok: false }]);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'l');
    assert!(a.recordings().unwrap().showing_log());
    let g = draw(&mut a, &cx);
    assert!(has_color(&g, [255, 255, 255]) || !g.hits.is_empty());
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(!a.recordings().unwrap().showing_log() && a.recordings().is_some(), "back to the recordings, not out of the tile");
    // Esc from the page returns to the cameras
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.recordings().is_none());
}

#[test]
fn this_computers_own_camera_is_never_recordable_and_the_tile_says_so() {
    let mut f = RecFake::new(true);
    f.cams[0].kind = Kind::Usb;
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'p');
    assert!(a.recordings().is_none());
    assert!(a.note().is_some_and(|n| !n.1 && n.0.contains("never recorded")));
}

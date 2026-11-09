//! The motion page of the CAMERAS tile against a fake source: no real camera, no analysis (the tile only shows and sends choices).

use super::*;
use crate::motion::Draw;
use iyakki_tile_api::cameras::{CameraSource, MotionChange, MotionInfo, MotionMode, MotionRow, PlayFrame, RecDay, RecInfo, RecMode, RecSeek, ShapeInfo};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

const DAY: u64 = 86_400_000;
const D0: u64 = 20_000 * DAY;

struct MFake {
    cams: Vec<CamInfo>,
    info: RefCell<MotionInfo>,
    set: RefCell<Vec<MotionChange>>,
    shapes_saved: RefCell<Vec<(bool, ShapeInfo)>>,
    removed: RefCell<Vec<(bool, String)>>,
    refuse_shape: RefCell<Option<String>>,
    events: Vec<MotionRow>,
    analysing: Cell<bool>,
    thumb_asked: RefCell<Vec<u64>>,
}

fn minfo(owner: bool) -> MotionInfo {
    MotionInfo {
        mode: MotionMode::Off,
        sensitivity: 5,
        zones: vec![],
        masks: vec![],
        mask_storage: false,
        alerts_on: false,
        alerts_max_per_hour: 4,
        quiet: String::new(),
        alerts_thumbnail: false,
        person: false,
        person_note: "Person detection is not available on this computer.".into(),
        state: "Motion watching is off.".into(),
        analysing: false,
        owner,
        keep_days: 14,
    }
}

fn row(id: u64, zone: &str) -> MotionRow {
    MotionRow { id, start_ms: id, end_ms: Some(id + 9000), label: format!("2024-10-04 {}", id % DAY / 1000), zone: zone.into(), score: 40, person: None, has_thumb: true }
}

impl MFake {
    fn new(owner: bool) -> MFake {
        let c = CamInfo { id: "c0".into(), name: "Front door".into(), kind: Kind::Network, address: "rtsp://10.0.0.5/live".into(), user: "admin".into(), status: Status::Live, fps: 5.0, size: (8, 4), manage: true, owner: "amy".into(), allowed: vec![], has_password: true };
        MFake { cams: vec![c], info: RefCell::new(minfo(owner)), set: RefCell::new(vec![]), shapes_saved: RefCell::new(vec![]), removed: RefCell::new(vec![]), refuse_shape: RefCell::new(None), events: vec![row(D0 + 10 * 3_600_000, "Porch"), row(D0 + 14 * 3_600_000, "Driveway")], analysing: Cell::new(false), thumb_asked: RefCell::new(vec![]) }
    }
}

impl CameraSource for MFake {
    fn cameras(&self) -> Vec<CamInfo> {
        self.cams.clone()
    }
    fn view(&self, _: &str, _: bool) -> Option<Frame> {
        Some(Frame { seq: 1, width: 16, height: 9, rgba: Arc::new([200, 200, 200, 255].repeat(144)) })
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
    fn rec_info(&self, _: &str) -> Option<RecInfo> {
        Some(RecInfo { mode: RecMode::Off, windows: vec![], max_days: 14, max_mb: 1000, state: "not recording".into(), recording: false, problem: false, owner: self.info.borrow().owner, segments: 1, bytes: 1000, key_note: "key".into() })
    }
    fn rec_days(&self, _: &str) -> Result<Vec<RecDay>, String> {
        Ok(vec![RecDay { label: "2024-10-04".into(), day_start_ms: D0, spans: vec![(D0 + 10 * 3_600_000, D0 + 10 * 3_600_000 + 9000)], frames: 10, bytes: 4_000_000 }])
    }
    fn rec_frame(&self, _: &str, at: u64, _: RecSeek) -> Result<Option<PlayFrame>, String> {
        Ok(Some(PlayFrame { ts_ms: at, label: "t".into(), width: 8, height: 4, rgba: Arc::new([10, 10, 200, 255].repeat(32)), prev_ms: None, next_ms: None }))
    }
    fn motion_ids(&self) -> Vec<String> {
        if self.analysing.get() { vec!["c0".into()] } else { vec![] }
    }
    fn motion_info(&self, _: &str) -> Option<MotionInfo> {
        let mut i = self.info.borrow().clone();
        i.analysing = self.analysing.get();
        Some(i)
    }
    fn motion_set(&self, _: &str, c: &MotionChange) -> Result<(), String> {
        if !self.info.borrow().owner && c.mode != Some(MotionMode::Off) {
            return Err("only the person who added the camera can change its motion watching".into());
        }
        self.set.borrow_mut().push(c.clone());
        let mut i = self.info.borrow_mut();
        if let Some(m) = c.mode {
            i.mode = m;
            i.state = format!("mode {m:?}");
        }
        if let Some(s) = c.sensitivity {
            i.sensitivity = s;
        }
        if let Some(b) = c.alerts_on {
            i.alerts_on = b;
        }
        if let Some(b) = c.alerts_thumbnail {
            i.alerts_thumbnail = b;
        }
        if let Some(b) = c.mask_storage {
            i.mask_storage = b;
        }
        if let Some(n) = c.alerts_max_per_hour {
            i.alerts_max_per_hour = n;
        }
        if let Some(q) = &c.quiet {
            i.quiet = q.clone();
        }
        Ok(())
    }
    fn motion_shape_set(&self, _: &str, mask: bool, s: &ShapeInfo) -> Result<(), String> {
        if let Some(e) = self.refuse_shape.borrow().clone() {
            return Err(e);
        }
        self.shapes_saved.borrow_mut().push((mask, s.clone()));
        let mut i = self.info.borrow_mut();
        if mask { i.masks.push(s.clone()) } else { i.zones.push(s.clone()) }
        Ok(())
    }
    fn motion_shape_remove(&self, _: &str, mask: bool, name: &str) -> Result<(), String> {
        self.removed.borrow_mut().push((mask, name.into()));
        let mut i = self.info.borrow_mut();
        if mask { i.masks.retain(|m| m.name != name) } else { i.zones.retain(|m| m.name != name) }
        Ok(())
    }
    fn motion_events(&self, _: &str, _: u64, _: u64, _: usize) -> Result<Vec<MotionRow>, String> {
        if !self.info.borrow().owner {
            return Err("only the person who added the camera can do that with its motion watching".into());
        }
        let mut v = self.events.clone();
        v.sort_by(|a, b| b.start_ms.cmp(&a.start_ms));
        Ok(v)
    }
    fn motion_thumb(&self, _: &str, e: u64) -> Result<Option<Frame>, String> {
        self.thumb_asked.borrow_mut().push(e);
        Ok(Some(Frame { seq: e, width: 16, height: 9, rgba: Arc::new([250, 20, 20, 255].repeat(144)) }))
    }
}

fn cx_of(f: &MFake) -> TileCx<'_> {
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

fn press(a: &mut CamerasApp, cx: &TileCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn open(f: &MFake) -> CamerasApp {
    let cx = cx_of(f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'm');
    draw(&mut a, &cx);
    a
}

/// Click a place on the picture given as fractions of its width and height.
fn click(a: &mut CamerasApp, cx: &TileCx, fx: f32, fy: f32) {
    let (w, h) = a.motion_page().and_then(|m| m.picture_size()).expect("the picture was drawn");
    a.handle(&Event::AreaAt(motion::AREA_PIC, fx * w, fy * h), cx);
    draw(a, cx);
}

fn type_text(a: &mut CamerasApp, cx: &TileCx, t: &str) {
    for c in t.chars() {
        key(a, cx, c);
    }
}

#[test]
fn motion_is_off_until_the_owner_confirms_and_the_confirmation_says_what_is_analysed_and_stored() {
    let f = MFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    assert!(a.motion_page().is_some());
    let m = a.motion_page().unwrap();
    assert_eq!(m.info().unwrap().mode, MotionMode::Off);
    assert!(f.set.borrow().is_empty(), "opening the page changes nothing");
    // w: asks first, nothing is sent until y
    key(&mut a, &cx, 'w');
    assert!(a.motion_page().unwrap().confirming() && f.set.borrow().is_empty());
    let text = a.motion_page().unwrap().hint();
    assert!(text.contains("y yes"));
    draw(&mut a, &cx);
    press(&mut a, &cx, Key::Esc);
    assert!(!a.motion_page().unwrap().confirming() && f.set.borrow().is_empty(), "Esc cancels the question");
    key(&mut a, &cx, 'w');
    key(&mut a, &cx, 'y');
    assert_eq!(f.set.borrow().last().unwrap().mode, Some(MotionMode::Watched));
    // w again: the next mode also asks (the camera is then kept running all the time and a red sign shows)
    key(&mut a, &cx, 'w');
    assert!(a.motion_page().unwrap().confirming());
    key(&mut a, &cx, 'y');
    assert_eq!(f.set.borrow().last().unwrap().mode, Some(MotionMode::Always));
    // turning off is one key and asks nothing
    key(&mut a, &cx, 'w');
    assert!(!a.motion_page().unwrap().confirming());
    assert_eq!(f.set.borrow().last().unwrap().mode, Some(MotionMode::Off));
    // sensitivity, alerts and the picture switches
    key(&mut a, &cx, '+');
    assert_eq!(a.motion_page().unwrap().info().unwrap().sensitivity, 6);
    key(&mut a, &cx, '-');
    key(&mut a, &cx, '-');
    assert_eq!(a.motion_page().unwrap().info().unwrap().sensitivity, 4);
    key(&mut a, &cx, 'a');
    key(&mut a, &cx, 't');
    key(&mut a, &cx, 's');
    let i = a.motion_page().unwrap().info().unwrap().clone();
    assert!(i.alerts_on && i.alerts_thumbnail && i.mask_storage);
    // Esc leaves the page
    press(&mut a, &cx, Key::Esc);
    assert!(a.motion_page().is_none());
}

#[test]
fn a_person_who_is_not_the_owner_cannot_turn_it_on_draw_or_read_events_but_can_turn_it_off() {
    let f = MFake::new(false);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'w');
    assert!(!a.motion_page().unwrap().confirming(), "no question: nothing to turn on");
    assert!(a.motion_page().unwrap().note().is_some_and(|(t, ok)| !ok && t.contains("Only the person who added")));
    key(&mut a, &cx, 'z');
    assert!(a.motion_page().unwrap().drawing().is_none());
    assert!(a.motion_page().unwrap().note().unwrap().0.contains("Only the person who added"));
    key(&mut a, &cx, 'e');
    assert!(!a.motion_page().unwrap().showing_events());
    key(&mut a, &cx, 'a');
    key(&mut a, &cx, 's');
    assert!(f.set.borrow().is_empty(), "nothing was sent");
    // somebody else switched it on; this person (an administrator) can switch it off
    f.info.borrow_mut().mode = MotionMode::Always;
    a.handle(&Event::Tick, &cx);
    let mut a2 = open(&f);
    key(&mut a2, &cx, 'w');
    assert_eq!(f.set.borrow().last().unwrap().mode, Some(MotionMode::Off));
    draw(&mut a2, &cx);
}

#[test]
fn a_zone_is_drawn_with_clicks_closed_by_the_first_corner_named_and_saved() {
    let f = MFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'z');
    assert_eq!(a.motion_page().unwrap().drawing(), Some((Draw::Zone, 0)));
    click(&mut a, &cx, 0.1, 0.2);
    click(&mut a, &cx, 0.6, 0.2);
    click(&mut a, &cx, 0.6, 0.8);
    click(&mut a, &cx, 0.1, 0.8);
    assert_eq!(a.motion_page().unwrap().drawing(), Some((Draw::Zone, 4)));
    // Backspace takes a corner back; a click puts it again
    press(&mut a, &cx, Key::Backspace);
    assert_eq!(a.motion_page().unwrap().drawing(), Some((Draw::Zone, 3)));
    click(&mut a, &cx, 0.1, 0.8);
    // a click on the first corner closes the shape and asks for a name
    click(&mut a, &cx, 0.1, 0.2);
    assert!(a.motion_page().unwrap().naming() && a.motion_page().unwrap().drawing().is_none());
    draw(&mut a, &cx);
    type_text(&mut a, &cx, "Driveway");
    press(&mut a, &cx, Key::Tab);
    press(&mut a, &cx, Key::Backspace);
    type_text(&mut a, &cx, "7");
    press(&mut a, &cx, Key::Enter);
    let saved = f.shapes_saved.borrow();
    assert_eq!(saved.len(), 1, "{:?}", a.motion_page().unwrap().note());
    let (mask, z) = &saved[0];
    assert!(!mask && z.include && z.name == "Driveway" && z.sensitivity == 7);
    assert_eq!(z.points.len(), 4, "the closing click on the first corner is not a fifth corner");
    let near = |p: (f32, f32), q: (f32, f32)| (p.0 - q.0).abs() < 0.01 && (p.1 - q.1).abs() < 0.01;
    assert!(near(z.points[0], (0.1, 0.2)) && near(z.points[1], (0.6, 0.2)) && near(z.points[2], (0.6, 0.8)) && near(z.points[3], (0.1, 0.8)), "{:?}", z.points);
    assert!(a.motion_page().unwrap().note().unwrap().0.contains("Saved"));
    assert_eq!(a.motion_page().unwrap().info().unwrap().zones.len(), 1);
}

#[test]
fn drawing_can_be_cancelled_two_corners_are_not_a_shape_and_a_refused_shape_keeps_the_form_with_the_reason() {
    let f = MFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'm');
    assert_eq!(a.motion_page().unwrap().drawing(), Some((Draw::Mask, 0)));
    click(&mut a, &cx, 0.5, 0.1);
    click(&mut a, &cx, 0.9, 0.1);
    press(&mut a, &cx, Key::Enter);
    assert!(a.motion_page().unwrap().drawing().is_some() && !a.motion_page().unwrap().naming(), "two corners are not a shape");
    assert!(a.motion_page().unwrap().note().unwrap().0.contains("at least 3"));
    press(&mut a, &cx, Key::Esc);
    assert!(a.motion_page().unwrap().drawing().is_none() && f.shapes_saved.borrow().is_empty(), "cancelled: nothing saved");
    // a bow tie: the source refuses it, the form stays with the reason
    *f.refuse_shape.borrow_mut() = Some("the shape crosses itself; draw the outline without crossing lines".into());
    key(&mut a, &cx, 'x');
    for (x, y) in [(0.1, 0.1), (0.9, 0.9), (0.9, 0.1), (0.1, 0.9)] {
        click(&mut a, &cx, x, y);
    }
    press(&mut a, &cx, Key::Enter);
    assert!(a.motion_page().unwrap().naming());
    type_text(&mut a, &cx, "Bow");
    press(&mut a, &cx, Key::Tab);
    press(&mut a, &cx, Key::Enter);
    assert!(a.motion_page().unwrap().naming(), "the form stays open");
    assert!(f.shapes_saved.borrow().is_empty());
    let g = draw(&mut a, &cx);
    let _ = g;
    // a bad sensitivity is caught in the form before anything is sent
    *f.refuse_shape.borrow_mut() = None;
    press(&mut a, &cx, Key::Backspace);
    type_text(&mut a, &cx, "99");
    press(&mut a, &cx, Key::Enter);
    assert!(a.motion_page().unwrap().naming() && f.shapes_saved.borrow().is_empty());
}

#[test]
fn masks_are_drawn_black_on_the_picture_listed_selected_and_removed() {
    let f = MFake::new(true);
    f.info.borrow_mut().masks.push(ShapeInfo { name: "Street".into(), include: false, sensitivity: 5, points: vec![(0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (0.5, 0.5)] });
    f.info.borrow_mut().zones.push(ShapeInfo { name: "Porch".into(), include: true, sensitivity: 6, points: vec![(0.0, 0.5), (0.5, 0.5), (0.5, 1.0), (0.0, 1.0)] });
    let cx = cx_of(&f);
    let mut a = open(&f);
    let g = draw(&mut a, &cx);
    // the picture is a light grey; the masked quarter of it is dark
    let m = a.motion_page().unwrap();
    let (pw, ph) = m.picture_size().unwrap();
    assert!(pw > 100.0 && ph > 50.0);
    let dark = g.rgba().chunks_exact(4).filter(|p| p[0] < 60 && p[1] < 60 && p[2] < 70).count();
    let light = g.rgba().chunks_exact(4).filter(|p| p[0] > 150 && p[1] > 150 && p[2] > 150).count();
    assert!(light > 5000, "the camera picture is drawn");
    assert!(dark as f32 > pw * ph * 0.15, "the privacy mask is drawn black over it ({dark} dark pixels)");
    assert_eq!(m.selected(), 0);
    // zones first, then masks; pick the mask and remove it
    press(&mut a, &cx, Key::Down);
    assert_eq!(a.motion_page().unwrap().selected(), 1);
    key(&mut a, &cx, 'd');
    assert_eq!(f.removed.borrow().as_slice(), [(true, "Street".to_string())]);
    assert!(a.motion_page().unwrap().info().unwrap().masks.is_empty());
    key(&mut a, &cx, 'd');
    assert_eq!(f.removed.borrow().last().unwrap(), &(false, "Porch".to_string()));
    key(&mut a, &cx, 'd');
    assert!(a.motion_page().unwrap().note().unwrap().0.contains("no zone or mask"));
}

#[test]
fn the_events_page_lists_events_with_their_thumbnail_and_the_recordings_timeline_carries_marks() {
    let f = MFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'e');
    assert!(a.motion_page().unwrap().showing_events());
    assert_eq!(a.motion_page().unwrap().events().len(), 2);
    assert_eq!(a.motion_page().unwrap().events()[0].zone, "Driveway", "newest first");
    let g = draw(&mut a, &cx);
    assert_eq!(f.thumb_asked.borrow().as_slice(), [D0 + 14 * 3_600_000], "the selected event's thumbnail is fetched");
    let reddish = g.rgba().chunks_exact(4).filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60).count();
    assert!(reddish > 200, "and drawn ({reddish})");
    press(&mut a, &cx, Key::Down);
    draw(&mut a, &cx);
    assert_eq!(f.thumb_asked.borrow().len(), 2);
    press(&mut a, &cx, Key::Esc);
    assert!(!a.motion_page().unwrap().showing_events());
    press(&mut a, &cx, Key::Esc);
    // the recordings page: one mark for each event of the day
    key(&mut a, &cx, 'p');
    draw(&mut a, &cx);
    assert_eq!(a.recordings().unwrap().motion_marks(), 2);
    // a person who is not the owner sees no marks (and no events)
    let f2 = MFake::new(false);
    let cx2 = cx_of(&f2);
    let mut b = CamerasApp::new();
    draw(&mut b, &cx2);
    key(&mut b, &cx2, 'p');
    draw(&mut b, &cx2);
    assert_eq!(b.recordings().unwrap().motion_marks(), 0);
}

#[test]
fn a_camera_being_analysed_carries_a_motion_mark_in_the_grid_and_on_its_page_and_one_that_is_not_does_not() {
    let f = MFake::new(true);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    let g0 = draw(&mut a, &cx);
    assert!(a.motion_marked().is_empty());
    let blue = |g: &Gfx| g.rgba().chunks_exact(4).filter(|p| p[2] > 120 && p[0] < 60 && p[1] > 50 && p[1] < 110).count();
    let before = blue(&g0);
    f.analysing.set(true);
    for t in 0..40 {
        let mut g = Gfx::new(1200, 700);
        g.clear_hits();
        a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, 100 + t);
    }
    let g = draw(&mut a, &cx);
    assert_eq!(a.motion_marked(), ["c0".to_string()]);
    assert!(blue(&g) > before + 100, "the MOTION pill is drawn");
    // on the motion page
    f.info.borrow_mut().mode = MotionMode::Always;
    key(&mut a, &cx, 'm');
    let g = draw(&mut a, &cx);
    assert!(blue(&g) > 100);
    // this computer's own camera is never watched in the background
    let mut f2 = MFake::new(true);
    f2.cams[0].kind = Kind::Usb;
    let cx2 = cx_of(&f2);
    let mut b = CamerasApp::new();
    draw(&mut b, &cx2);
    key(&mut b, &cx2, 'm');
    assert!(b.motion_page().is_none() && b.note().is_some_and(|(t, ok)| !ok && t.contains("never watched")));
}

#[test]
fn alert_settings_are_checked_in_the_form_and_sent_to_the_source() {
    let f = MFake::new(true);
    let cx = cx_of(&f);
    let mut a = open(&f);
    key(&mut a, &cx, 'c');
    assert!(a.motion_page().unwrap().in_settings());
    draw(&mut a, &cx);
    // a number outside 1 to 30 keeps the form open
    press(&mut a, &cx, Key::Backspace);
    type_text(&mut a, &cx, "99");
    press(&mut a, &cx, Key::Tab);
    press(&mut a, &cx, Key::Enter);
    assert!(a.motion_page().unwrap().in_settings() && f.set.borrow().is_empty());
    // fix it and give quiet hours
    press(&mut a, &cx, Key::Tab);
    press(&mut a, &cx, Key::Backspace);
    press(&mut a, &cx, Key::Backspace);
    type_text(&mut a, &cx, "3");
    press(&mut a, &cx, Key::Tab);
    type_text(&mut a, &cx, "22:00-06:30");
    press(&mut a, &cx, Key::Enter);
    let sent = f.set.borrow();
    assert_eq!(sent.len(), 1, "{:?}", a.motion_page().unwrap().note());
    assert_eq!((sent[0].alerts_max_per_hour, sent[0].quiet.as_deref()), (Some(3), Some("22:00-06:30")));
}

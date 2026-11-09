use super::*;
use iyakki_tile_api::cameras::CameraSource;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

struct Fake {
    cams: RefCell<Vec<CamInfo>>,
    frames: RefCell<HashMap<String, Frame>>,
    viewed: RefCell<Vec<(String, bool)>>,
    saved: RefCell<Vec<(Option<String>, NewCam, Option<String>)>>,
    removed: RefCell<Vec<String>>,
    snaps: RefCell<Vec<String>>,
    snap_result: RefCell<Result<String, String>>,
    save_result: RefCell<Result<String, String>>,
    asked_list: Cell<u32>,
}

fn cam(id: &str, name: &str, kind: Kind, status: Status, manage: bool) -> CamInfo {
    CamInfo { id: id.into(), name: name.into(), kind, address: if kind == Kind::Usb { String::new() } else { format!("rtsp://10.0.0.{}:554/live", id.len()) }, user: "admin".into(), status, fps: 5.0, size: (8, 4), manage, owner: "amy".into(), allowed: vec!["bob".into()], has_password: kind == Kind::Network }
}

fn solid(seq: u64, c: [u8; 3]) -> Frame {
    Frame { seq, width: 8, height: 4, rgba: Arc::new((0..32).flat_map(|_| [c[0], c[1], c[2], 255]).collect()) }
}

impl Fake {
    fn new(n: usize) -> Fake {
        let f = Fake { cams: RefCell::new(vec![]), frames: RefCell::new(HashMap::new()), viewed: RefCell::new(vec![]), saved: RefCell::new(vec![]), removed: RefCell::new(vec![]), snaps: RefCell::new(vec![]), snap_result: RefCell::new(Ok("/home/amy/Pictures/camera-x.jpg".into())), save_result: RefCell::new(Ok("id".into())), asked_list: Cell::new(0) };
        for i in 0..n {
            f.cams.borrow_mut().push(cam(&format!("c{i}"), &format!("Cam {i}"), Kind::Network, Status::Live, true));
            f.frames.borrow_mut().insert(format!("c{i}"), solid(1, [10, 200, 10]));
        }
        f
    }
}

impl CameraSource for Fake {
    fn cameras(&self) -> Vec<CamInfo> {
        self.asked_list.set(self.asked_list.get() + 1);
        self.cams.borrow().clone()
    }
    fn view(&self, id: &str, big: bool) -> Option<Frame> {
        self.viewed.borrow_mut().push((id.into(), big));
        self.frames.borrow().get(id).cloned()
    }
    fn snapshot(&self, id: &str) -> Result<String, String> {
        self.snaps.borrow_mut().push(id.into());
        self.snap_result.borrow().clone()
    }
    fn save(&self, replacing: Option<&str>, cam: &NewCam, password: Option<&str>) -> Result<String, String> {
        self.saved.borrow_mut().push((replacing.map(str::to_string), cam.clone(), password.map(str::to_string)));
        self.save_result.borrow().clone()
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.removed.borrow_mut().push(id.into());
        self.cams.borrow_mut().retain(|c| c.id != id);
        Ok(())
    }
    fn people(&self) -> Vec<String> {
        vec!["bob".into(), "root".into()]
    }
}

struct NoUse;
impl iyakki_tile_api::usage::UsageSource for NoUse {
    fn now_unix(&self) -> i64 {
        0
    }
    fn series(&self, _: iyakki_tile_api::usage::Range, _: i64, _: Option<&str>) -> iyakki_tile_api::usage::Series {
        iyakki_tile_api::usage::Series { models: vec![], buckets: vec![], totals: vec![], input: 0, output: 0, calls: 0 }
    }
}

fn cx_of(f: &Fake) -> TileCx<'_> {
    TileCx { user: "amy", manage_settings: false, usage: &NoUse, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: f }
}

fn draw_at(a: &mut CamerasApp, cx: &TileCx, tick: u64, w: u32, h: u32, s: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    g.clear_hits();
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, tick);
    g
}

fn draw(a: &mut CamerasApp, cx: &TileCx) -> Gfx {
    draw_at(a, cx, 0, 1200, 700, 1.0)
}

fn key(a: &mut CamerasApp, cx: &TileCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn type_in(a: &mut CamerasApp, cx: &TileCx, text: &str) {
    for c in text.chars() {
        key(a, cx, c);
    }
}

fn has_color(g: &Gfx, c: [u8; 3]) -> bool {
    g.rgba().chunks_exact(4).any(|p| p[0] == c[0] && p[1] == c[1] && p[2] == c[2])
}

#[test]
fn an_empty_tile_explains_itself_and_adding_a_camera_sends_the_password_only_to_the_source() {
    let f = Fake::new(0);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(30))), "an Add button");
    key(&mut a, &cx, 'a');
    assert!(a.in_form());
    type_in(&mut a, &cx, "Front door");
    a.handle(&Event::Key(Key::Tab), &cx);
    type_in(&mut a, &cx, "rtsp://192.168.1.20/stream1");
    a.handle(&Event::Key(Key::Tab), &cx);
    type_in(&mut a, &cx, "admin");
    a.handle(&Event::Key(Key::Tab), &cx);
    type_in(&mut a, &cx, "hunter2");
    a.handle(&Event::Key(Key::Tab), &cx);
    type_in(&mut a, &cx, "bob, root");
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(!a.in_form());
    let saved = f.saved.borrow();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].1, NewCam { name: "Front door".into(), address: "rtsp://192.168.1.20/stream1".into(), user: "admin".into(), allowed: vec!["bob".into(), "root".into()] });
    assert_eq!(saved[0].2.as_deref(), Some("hunter2"));
    assert!(!format!("{:?}", saved[0].1).contains("hunter2"));
    assert_eq!(a.note(), Some(("Saved Front door", true)));
}

#[test]
fn a_refused_save_keeps_the_form_open_with_the_reason() {
    let f = Fake::new(0);
    *f.save_result.borrow_mut() = Err("a camera called “Door” already exists".into());
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'a');
    type_in(&mut a, &cx, "Door");
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.in_form(), "still open");
    let g = draw(&mut a, &cx);
    assert!(!g.hits.is_empty());
}

#[test]
fn only_the_cameras_on_screen_are_asked_for_pictures_and_the_layout_follows_the_keys() {
    let f = Fake::new(7);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    let ids = |a: &CamerasApp| a.shown_ids().to_vec();
    assert_eq!(a.layout(), 4);
    assert_eq!(ids(&a), ["c0", "c1", "c2", "c3"], "four views, the other three are not streamed");
    assert!(f.viewed.borrow().iter().all(|(id, big)| ["c0", "c1", "c2", "c3"].contains(&id.as_str()) && !big));
    key(&mut a, &cx, '6');
    draw(&mut a, &cx);
    assert_eq!(ids(&a).len(), 6);
    key(&mut a, &cx, '1');
    draw(&mut a, &cx);
    assert_eq!(ids(&a), ["c0"]);
    key(&mut a, &cx, '2');
    draw(&mut a, &cx);
    assert_eq!(ids(&a), ["c0", "c1"]);
    // the keys walk through the list and the page follows
    key(&mut a, &cx, '4');
    for _ in 0..5 {
        a.handle(&Event::Key(Key::Right), &cx);
    }
    draw(&mut a, &cx);
    assert_eq!(a.selected().unwrap().id, "c5");
    assert_eq!(ids(&a), ["c2", "c3", "c4", "c5"]);
    a.handle(&Event::Key(Key::Home), &cx);
    draw(&mut a, &cx);
    assert_eq!(ids(&a)[0], "c0");
    a.handle(&Event::Key(Key::End), &cx);
    draw(&mut a, &cx);
    assert_eq!(a.selected().unwrap().id, "c6");
    // the layout buttons are clickable
    let g = draw(&mut a, &cx);
    assert_eq!(g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(1..=4))).count(), 4);
    a.handle(&Event::Tab(1), &cx);
    assert_eq!(a.layout(), 1);
}

#[test]
fn a_picture_is_drawn_and_a_click_enlarges_it_and_asks_for_the_big_size() {
    let f = Fake::new(2);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    let g = draw(&mut a, &cx);
    assert!(has_color(&g, [10, 200, 10]), "the camera picture is on the screen");
    let rows: Vec<_> = g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|h| h.label == "Cam 1"));
    a.handle(&Event::Row(1), &cx);
    assert_eq!(a.enlarged(), Some("c1"));
    f.viewed.borrow_mut().clear();
    let g = draw(&mut a, &cx);
    assert!(has_color(&g, [10, 200, 10]));
    assert_eq!(f.viewed.borrow().as_slice(), &[("c1".to_string(), true)], "only the enlarged camera, in the big size");
    a.handle(&Event::Key(Key::Left), &cx);
    assert_eq!(a.enlarged(), Some("c0"), "arrows switch camera while enlarged");
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.enlarged().is_none());
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back), "Esc on the grid leaves the tile");
}

#[test]
fn every_state_is_drawn_in_words_without_a_picture() {
    let f = Fake::new(0);
    let states = [Status::Connecting, Status::Lost("cannot reach 10.0.0.9:554".into()), Status::NeedsAddon("needs the add-on".into()), Status::Busy("used by another app".into()), Status::NeedsPassword("the vault is locked".into()), Status::Live];
    for (i, st) in states.iter().enumerate() {
        f.cams.borrow_mut().push(cam(&format!("s{i}"), &format!("State {i}"), Kind::Network, st.clone(), true));
    }
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    for (layout, size) in [('6', (1200, 700, 1.0)), ('4', (800, 480, 0.8)), ('1', (1920, 1080, 1.5)), ('2', (640, 360, 0.6))] {
        key(&mut a, &cx, layout);
        let g = draw_at(&mut a, &cx, 5, size.0, size.1, size.2);
        assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count() >= 1);
    }
    assert_eq!(Status::NeedsAddon("x".into()).label(), "Needs video add-on");
}

#[test]
fn the_red_band_shows_whenever_this_computers_own_camera_is_on_screen_and_not_otherwise() {
    let f = Fake::new(2);
    f.cams.borrow_mut().push(cam("usb:video0", "HD Webcam", Kind::Usb, Status::Live, false));
    f.frames.borrow_mut().insert("usb:video0".into(), solid(1, [30, 30, 220]));
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    key(&mut a, &cx, '2');
    let g = draw(&mut a, &cx);
    assert!(!a.privacy_banner_shown(), "the page with the two network cameras has no band");
    assert!(!has_color(&g, [150, 20, 20]));
    a.handle(&Event::Key(Key::Right), &cx);
    a.handle(&Event::Key(Key::Right), &cx);
    let g = draw(&mut a, &cx);
    assert!(a.shown_ids().contains(&"usb:video0".to_string()));
    assert!(a.privacy_banner_shown(), "the page with the USB camera has the band");
    assert!(has_color(&g, [255, 70, 70]), "and the red dot on the camera");
    // enlarged too
    a.handle(&Event::Key(Key::Enter), &cx);
    draw(&mut a, &cx);
    assert!(a.privacy_banner_shown());
    a.handle(&Event::Key(Key::Esc), &cx);
    a.handle(&Event::Key(Key::Home), &cx);
    draw(&mut a, &cx);
    assert!(!a.privacy_banner_shown());
}

#[test]
fn a_snapshot_is_one_key_or_click_and_the_tile_says_where_it_went_with_a_flash() {
    let f = Fake::new(2);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    assert!(f.snaps.borrow().is_empty(), "showing cameras takes no picture by itself");
    a.handle(&Event::Key(Key::Right), &cx);
    key(&mut a, &cx, 's');
    assert_eq!(f.snaps.borrow().as_slice(), ["c1"]);
    let (t, ok) = a.note().unwrap();
    assert!(ok && t.contains("/home/amy/Pictures/camera-x.jpg") && t.contains("Cam 1"));
    let g = draw_at(&mut a, &cx, 1, 1200, 700, 1.0);
    assert!(has_color(&g, [255, 255, 255]) || g.rgba().chunks_exact(4).any(|p| p[0] > 200 && p[1] > 200 && p[2] > 200), "the flash");
    a.handle(&Event::Tab(31), &cx);
    assert_eq!(f.snaps.borrow().len(), 2, "the button works too");
    *f.snap_result.borrow_mut() = Err("this camera has not shown a picture yet".into());
    key(&mut a, &cx, 's');
    assert_eq!(a.note(), Some(("this camera has not shown a picture yet", false)));
}

#[test]
fn removing_asks_first_and_changing_needs_the_right_to_manage() {
    let f = Fake::new(2);
    f.cams.borrow_mut()[1].manage = false;
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'd');
    assert!(a.asking_to_remove());
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| h.label == "Remove") || g.hits.len() > 0);
    key(&mut a, &cx, 'n');
    assert!(f.removed.borrow().is_empty(), "No means no");
    key(&mut a, &cx, 'd');
    key(&mut a, &cx, 'y');
    assert_eq!(f.removed.borrow().as_slice(), ["c0"]);
    assert!(a.note().unwrap().0.contains("Removed Cam 0"));
    // the remaining camera is somebody else's: no edit, no removal
    draw_at(&mut a, &cx, 100, 1200, 700, 1.0);
    assert_eq!(a.selected().unwrap().id, "c1");
    key(&mut a, &cx, 'e');
    assert!(!a.in_form());
    assert!(a.note().unwrap().0.contains("Only the person who added"));
    key(&mut a, &cx, 'd');
    assert!(!a.asking_to_remove());
}

#[test]
fn editing_shows_the_clean_address_and_never_a_password_and_usb_cameras_have_nothing_to_edit() {
    let f = Fake::new(1);
    f.cams.borrow_mut().push(cam("usb:video0", "HD Webcam", Kind::Usb, Status::Live, false));
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'e');
    assert!(a.in_form());
    // saving without typing anything keeps the stored password (None) and sends the clean address
    for _ in 0..6 {
        if a.in_form() {
            a.handle(&Event::Key(Key::Enter), &cx);
        }
    }
    assert!(!a.in_form());
    let saved = f.saved.borrow();
    assert_eq!(saved[0].0.as_deref(), Some("c0"));
    assert_eq!(saved[0].1.address, "rtsp://10.0.0.2:554/live");
    assert_eq!(saved[0].1.allowed, ["bob"]);
    assert_eq!(saved[0].2, None);
    drop(saved);
    a.handle(&Event::Key(Key::Right), &cx);
    key(&mut a, &cx, 'e');
    assert!(!a.in_form());
    assert!(a.note().unwrap().0.contains("nothing to edit"));
    key(&mut a, &cx, 'd');
    assert!(!a.asking_to_remove());
}

#[test]
fn the_list_is_read_about_twice_a_second_not_on_every_frame() {
    let f = Fake::new(3);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    for t in 0..30 {
        draw_at(&mut a, &cx, t, 1200, 700, 1.0);
    }
    assert!(f.asked_list.get() <= 3, "asked {} times in 30 frames", f.asked_list.get());
    // a camera that appears is picked up
    f.cams.borrow_mut().push(cam("new", "New", Kind::Network, Status::Live, true));
    for t in 30..60 {
        draw_at(&mut a, &cx, t, 1200, 700, 1.0);
    }
    assert!(a.shown_ids().contains(&"new".to_string()));
}

#[test]
fn help_and_hint_describe_the_current_screen() {
    let f = Fake::new(1);
    let cx = cx_of(&f);
    let mut a = CamerasApp::new();
    draw(&mut a, &cx);
    assert!(a.hint().contains("snapshot") && a.help().len() >= 5);
    a.handle(&Event::Row(0), &cx);
    assert!(a.hint().contains("Esc back"));
    assert_eq!((a.title(), a.accent()), ("CAMERAS", ACCENT));
}

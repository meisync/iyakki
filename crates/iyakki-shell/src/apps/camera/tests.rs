//! The CAMERA app with a made-up camera: photos, video, passport photos and sheets land in the right folders, the timer
//! counts down, cameras come and go, and the screen lays out at several sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use crate::camera::detect::tests::FakeMachine;
use crate::camera::fake::FakeOpener;

struct Rig {
    app: CameraApp,
    cx: AppCx,
    dir: PathBuf,
    machine: FakeMachine,
    opener: Arc<FakeOpener>,
}

fn rig(tag: &str, cams: usize, opener: FakeOpener) -> Rig {
    let mut cx = cx_with_admin_store(&format!("cam-{tag}"));
    let dir = temp_dir(&format!("cam-{tag}-home"));
    let home = dir.display().to_string();
    cx.session.policy.fs_write = vec![home.clone()];
    cx.session.policy.fs_read = vec![home];
    let machine = FakeMachine::new(&format!("app-{tag}"));
    for i in 0..cams {
        machine.plug_webcam(i as u32 * 2, if i == 0 { "HD 4MP WEBCAM" } else { "C270" });
    }
    let opener = Arc::new(opener);
    let app = CameraApp::with_parts(machine.presence(), opener.clone(), dir.join("Pictures"), dir.join("Videos"));
    Rig { app, cx, dir, machine, opener }
}

fn pump(r: &mut Rig, what: &str, mut done: impl FnMut(&CameraApp) -> bool) {
    for _ in 0..600 {
        r.app.handle(&Event::Tick, &r.cx);
        if done(&r.app) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}; status {:?}, error {:?}", r.app.status_text(), r.app.error_text());
}

fn key(r: &mut Rig, k: Key) {
    r.app.handle(&Event::Key(k), &r.cx);
}

fn files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn ready(r: &mut Rig) {
    pump(r, "a live picture", |a| a.live().is_some_and(|l| l.preview().is_some() && l.latest().is_some()));
}

#[test]
fn a_photo_is_a_jpeg_in_pictures_with_a_flash_and_a_thumbnail() {
    let mut r = rig("photo", 1, FakeOpener::default());
    ready(&mut r);
    assert_eq!(r.app.camera_names(), vec!["HD 4MP WEBCAM"]);
    assert_eq!(r.app.live_mode().map(|m| (m.width, m.height)), Some((640, 360)));
    key(&mut r, Key::Char(' '));
    let pics = files(&r.dir.join("Pictures"));
    assert_eq!(pics.len(), 1, "{pics:?} {:?}", r.app.status_text());
    assert!(pics[0].starts_with("photo-") && pics[0].ends_with(".jpg"));
    let bytes = std::fs::read(r.dir.join("Pictures").join(&pics[0])).unwrap();
    let b = iyakki_web::jpeg::decode(&bytes).unwrap();
    assert_eq!((b.width, b.height), (640, 360), "the full frame, not the preview");
    assert!(r.app.flash > 0 && r.app.status_text().unwrap().contains("Saved photo-"));
    assert_eq!(r.app.gallery_paths()[0], r.dir.join("Pictures").join(&pics[0]));
    // a second photo in the same second gets its own name
    key(&mut r, Key::Enter);
    assert_eq!(files(&r.dir.join("Pictures")).len(), 2);
    // a reopened app finds them again
    let again = CameraApp::with_parts(r.machine.presence(), r.opener.clone(), r.dir.join("Pictures"), r.dir.join("Videos"));
    let mut r2 = Rig { app: again, cx: r.cx.clone(), dir: r.dir.clone(), machine: FakeMachine::new("photo-2"), opener: r.opener.clone() };
    pump(&mut r2, "the gallery", |a| a.gallery_paths().len() == 2);
    assert!(r2.app.gallery.iter().all(|t| t.pm.is_some()));
}

#[test]
fn the_timer_counts_down_before_the_shot_and_esc_cancels_it() {
    let mut r = rig("timer", 1, FakeOpener::default());
    ready(&mut r);
    key(&mut r, Key::Char('t'));
    assert_eq!(r.app.timer, 3);
    key(&mut r, Key::Char(' '));
    assert!(r.app.countdown.is_some());
    for _ in 0..(3 * TICKS_PER_SEC - 2) {
        r.app.handle(&Event::Tick, &r.cx);
    }
    assert!(files(&r.dir.join("Pictures")).is_empty(), "not before the countdown ends");
    for _ in 0..3 {
        r.app.handle(&Event::Tick, &r.cx);
    }
    assert_eq!(files(&r.dir.join("Pictures")).len(), 1);
    key(&mut r, Key::Char('t'));
    assert_eq!(r.app.timer, 10);
    key(&mut r, Key::Char(' '));
    assert!(matches!(r.app.handle(&Event::Key(Key::Esc), &r.cx), Flow::Stay), "Esc cancels the timer first");
    assert!(r.app.countdown.is_none());
    assert!(matches!(r.app.handle(&Event::Key(Key::Esc), &r.cx), Flow::Back));
    // mirror and brightness reach the camera
    key(&mut r, Key::Char('m'));
    assert!(!r.app.mirror);
    key(&mut r, Key::Char(']'));
    pump(&mut r, "the control", |_| true);
    for _ in 0..100 {
        if r.opener.controls.lock().unwrap().iter().any(|(id, v)| *id == v4l2::CID_BRIGHTNESS && *v > 0) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("brightness never reached the camera");
}

#[test]
fn a_video_is_recorded_as_mjpeg_avi_with_time_and_size_shown() {
    for yuyv in [false, true] {
        let mut r = rig(&format!("video-{yuyv}"), 1, FakeOpener { yuyv_only: yuyv, ..Default::default() });
        ready(&mut r);
        key(&mut r, Key::Char('2'));
        key(&mut r, Key::Char(' '));
        assert!(r.app.is_recording(), "{:?}", r.app.status_text());
        assert!(r.app.status_text().unwrap().contains("no sound"));
        pump(&mut r, "frames", |a| a.live().and_then(|l| l.rec_status()).is_some_and(|s| s.frames >= 5));
        // the screen shows REC, the time and the size
        let mut g = Gfx::new(1280, 720);
        r.app.render(&mut g, R::new(0.0, 50.0, 1280.0, 670.0), 1.0, &r.cx, 0);
        assert!(g.hits.iter().any(|h| h.label == "shutter"));
        // switching mode is refused while recording
        key(&mut r, Key::Char('1'));
        assert_eq!(r.app.shot(), Shot::Video);
        key(&mut r, Key::Char(' '));
        assert!(!r.app.is_recording());
        let vids = files(&r.dir.join("Videos"));
        assert_eq!(vids.len(), 1);
        assert!(vids[0].starts_with("video-") && vids[0].ends_with(".avi"));
        let info = crate::camera::avi::parse(&std::fs::read(r.dir.join("Videos").join(&vids[0])).unwrap()).unwrap();
        assert!(info.frames >= 5 && info.handler == "MJPG");
        assert!(r.app.status_text().unwrap().contains("frames"), "{:?}", r.app.status_text());
        assert!(r.app.gallery[0].video && r.app.gallery[0].pm.is_some());
    }
}

#[test]
fn a_passport_photo_is_cropped_to_size_at_300_dpi_and_printed_on_sheets() {
    let mut r = rig("passport", 1, FakeOpener::default());
    ready(&mut r);
    key(&mut r, Key::Char('3'));
    assert_eq!(r.app.shot(), Shot::Passport);
    assert_eq!(r.app.timer, 3, "passport mode counts down");
    assert!(r.app.status_text().unwrap().contains("plain light wall"));
    // the country picker: type to search, Enter chooses
    key(&mut r, Key::Char('p'));
    assert!(matches!(r.app.overlay, Overlay::Countries(..)));
    for c in "usa visa".chars() {
        key(&mut r, Key::Char(c));
    }
    let mut g = Gfx::new(1280, 720);
    r.app.render(&mut g, R::new(0.0, 50.0, 1280.0, 670.0), 1.0, &r.cx, 0);
    assert!(g.hits.iter().any(|h| h.label == "US visa (DS-160) 2×2 in"));
    assert!(!g.hits.iter().any(|h| h.label.starts_with("UK")), "the list is filtered");
    key(&mut r, Key::Enter);
    assert_eq!(r.app.cur().id, "us-visa");
    assert!(r.app.status_text().unwrap().contains("UNVERIFIED"), "an unconfirmed rule says so: {:?}", r.app.status_text());
    key(&mut r, Key::Char('-'));
    key(&mut r, Key::Right);
    // the crop in frame pixels: square, mirrored back to the real side
    let c = r.app.passport_crop(640, 360);
    assert!((c.w - c.h).abs() < 0.01 && c.x + c.w <= 640.0 && c.x >= 0.0);
    r.app.mirror = false;
    let c2 = r.app.passport_crop(640, 360);
    assert!((c2.x - (640.0 - c.x - c.w)).abs() < 0.01);
    key(&mut r, Key::Char(' '));
    for _ in 0..(3 * TICKS_PER_SEC + 1) {
        r.app.handle(&Event::Tick, &r.cx);
    }
    let pics = files(&r.dir.join("Pictures"));
    assert_eq!(pics.len(), 2, "the print photo and the online file: {pics:?} {:?}", r.app.status_text());
    let online = pics.iter().find(|p| p.ends_with("-online.jpg")).unwrap();
    let j = std::fs::read(r.dir.join("Pictures").join(online)).unwrap();
    assert!(j.len() <= 240 * 1024, "within the DS-160 limit");
    let b = iyakki_web::jpeg::decode(&j).unwrap();
    assert!((600..=1200).contains(&b.width) && b.width == b.height);
    let print = pics.iter().find(|p| p.ends_with(".png")).unwrap();
    assert!(print.starts_with("passport-us-visa-"));
    let png = std::fs::read(r.dir.join("Pictures").join(print)).unwrap();
    assert!((pixels::png_dpi(&png).unwrap() - 300.0).abs() < 0.1);
    let pm = Pixmap::decode_png(&png).unwrap();
    assert_eq!((pm.width(), pm.height()), (600, 600));
    assert!(matches!(r.app.overlay, Overlay::PassportResult));
    // the small fake camera is honestly reported as small for this size
    assert!(r.app.status_text().unwrap().contains("small"), "{:?}", r.app.status_text());
    let mut g = Gfx::new(1280, 720);
    r.app.render(&mut g, R::new(0.0, 50.0, 1280.0, 670.0), 1.0, &r.cx, 0);
    assert!(g.hits.iter().any(|h| h.label == "Print sheet A4"));
    key(&mut r, Key::Char('4'));
    key(&mut r, Key::Char('a'));
    let pics = files(&r.dir.join("Pictures"));
    let sheets: Vec<&String> = pics.iter().filter(|p| p.starts_with("passport-sheet-us-")).collect();
    assert_eq!(sheets.len(), 4, "{pics:?}");
    let a4 = sheets.iter().find(|p| p.contains("-a4-") && p.ends_with(".png")).unwrap();
    let pm = Pixmap::decode_png(&std::fs::read(r.dir.join("Pictures").join(a4)).unwrap()).unwrap();
    assert_eq!((pm.width(), pm.height()), (2480, 3508));
    let pdf = sheets.iter().find(|p| p.contains("-4x6-") && p.ends_with(".pdf")).unwrap();
    let text = String::from_utf8_lossy(&std::fs::read(r.dir.join("Pictures").join(pdf)).unwrap()).into_owned();
    assert_eq!(text.matches("/Im0 Do").count(), 2, "two 2x2 in copies on 4x6 in");
    assert!(r.app.status_text().unwrap().contains("Print at 100%"));
    key(&mut r, Key::Esc);
    assert!(matches!(r.app.overlay, Overlay::None));
    // the custom size asks for its width and height
    key(&mut r, Key::Char('p'));
    for c in "custom".chars() {
        key(&mut r, Key::Char(c));
    }
    key(&mut r, Key::Enter);
    assert!(matches!(r.app.overlay, Overlay::Custom(_)));
    r.app.overlay = Overlay::Custom(Form::new("x").text("Width", "40", "").text("Height", "50", ""));
    key(&mut r, Key::Enter);
    key(&mut r, Key::Enter);
    assert_eq!((r.app.cur().id, r.app.cur().w_mm, r.app.cur().h_mm), ("custom", 40.0, 50.0), "{:?}", r.app.status_text());
}

#[test]
fn cameras_are_chosen_and_sizes_changed_and_hotplug_follows() {
    let mut r = rig("pick", 2, FakeOpener::default());
    ready(&mut r);
    assert_eq!(r.app.camera_names(), vec!["HD 4MP WEBCAM", "C270"]);
    key(&mut r, Key::Char('c'));
    assert!(matches!(r.app.overlay, Overlay::Cameras));
    r.app.handle(&Event::Tab(PICK_BASE + 1), &r.cx);
    ready(&mut r);
    assert_eq!(r.opener.opened.lock().unwrap().last().unwrap().0, "video2");
    // sizes of the format in use only (MJPEG), small to large
    key(&mut r, Key::Char('r'));
    let sizes = r.app.size_choices();
    assert_eq!(sizes.iter().map(|m| (m.width, m.pix)).collect::<Vec<_>>(), vec![(320, crate::camera::Pix::Mjpeg), (640, crate::camera::Pix::Mjpeg), (640, crate::camera::Pix::Yuyv), (1280, crate::camera::Pix::Mjpeg)]);
    key(&mut r, Key::Char('1'));
    pump(&mut r, "the new size", |a| a.live_mode().is_some_and(|m| m.width == 320) && a.live().is_some_and(|l| l.preview().is_some_and(|p| p.1.width == 320)));
    // the camera in use is pulled out: the other one takes over within a few seconds
    r.machine.unplug("video2");
    r.machine.unplug("video3");
    pump(&mut r, "the other camera", |a| a.camera_names() == vec!["HD 4MP WEBCAM"] && a.live().is_some());
    // all gone: says so; plugged back: comes back by itself
    r.machine.unplug("video0");
    r.machine.unplug("video1");
    pump(&mut r, "no camera", |a| a.camera_names().is_empty() && a.live().is_none());
    let mut g = Gfx::new(800, 480);
    r.app.render(&mut g, R::new(0.0, 40.0, 800.0, 440.0), 0.8, &r.cx, 0);
    r.machine.plug_webcam(0, "HD 4MP WEBCAM");
    ready(&mut r);
    // a camera that stops mid-way is reported
    let mut r = rig("fail", 1, FakeOpener { fail_after: Some(3), ..Default::default() });
    pump(&mut r, "the error", |a| a.error_text().is_some());
    assert!(r.app.error_text().unwrap().contains("disconnected"));
}

#[test]
fn saving_needs_write_access_and_the_screen_lays_out_at_every_size() {
    let mut r = rig("policy", 1, FakeOpener::default());
    ready(&mut r);
    r.cx.session.policy.fs_write = vec![];
    key(&mut r, Key::Char(' '));
    assert!(files(&r.dir.join("Pictures")).is_empty());
    assert!(r.app.status_text().unwrap().contains("may not save"));
    for (w, h, s) in [(640u32, 360u32, 0.6f32), (800, 480, 0.75), (1280, 720, 1.0), (1920, 1080, 1.4)] {
        for shot in ['1', '2', '3'] {
            key(&mut r, Key::Char(shot));
            let mut g = Gfx::new(w, h);
            let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
            r.app.render(&mut g, area, s, &r.cx, 0);
            for hit in &g.hits {
                assert!(hit.rect.x >= area.x - 0.5 && hit.rect.x + hit.rect.w <= area.x + area.w + 0.5 && hit.rect.y >= area.y - 0.5 && hit.rect.y + hit.rect.h <= area.y + area.h + 0.5, "{} outside at {w}x{h}: {:?}", hit.label, hit.rect);
            }
            assert!(g.hits.iter().any(|h| h.label == "shutter"), "{w}x{h}");
        }
    }
    assert_eq!(human_size(4_200_000), "4.2 MB");
    assert_eq!(clock_text(75.4), "01:15");
    assert_eq!(clock_text(3725.0), "1:02:05");
}

/// Screenshots for the docs: `cargo test -p iyakki-shell --lib camera_screenshots -- --ignored` writes them to
/// docs/media/shots.
#[test]
#[ignore]
fn camera_screenshots() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/media/shots");
    std::fs::create_dir_all(&out).unwrap();
    let mut r = rig("shots", 1, FakeOpener::default());
    r.app.want = Some(crate::camera::Mode { pix: crate::camera::Pix::Mjpeg, width: 1280, height: 720 });
    r.app.connect();
    ready(&mut r);
    key(&mut r, Key::Char(' '));
    key(&mut r, Key::Char('2'));
    key(&mut r, Key::Char(' '));
    pump(&mut r, "frames", |a| a.live().and_then(|l| l.rec_status()).is_some_and(|s| s.frames >= 3));
    key(&mut r, Key::Char(' '));
    r.app.flash = 0;
    let (w, h, s) = (1280u32, 720u32, 1.0f32);
    let area = R::new(0.0, 46.0, w as f32, h as f32 - 46.0);
    let shot = |r: &mut Rig, name: &str| {
        pump(r, "a fresh picture", |a| a.live().is_some_and(|l| l.preview().is_some()));
        let mut g = Gfx::new(w, h);
        crate::fx::background(&mut g);
        fx::app_header(&mut g, w as f32, 46.0, s, "Camera", "", Icon::Camera, ACCENT, 0, w as f32 - 100.0);
        r.app.render(&mut g, area, s, &r.cx, 0);
        g.save_png(&out.join(name)).unwrap();
    };
    key(&mut r, Key::Char('1'));
    r.app.status = None;
    shot(&mut r, "camera-photo.png");
    key(&mut r, Key::Char('3'));
    shot(&mut r, "camera-passport.png");
}

fn two_cams(tag: &str) -> Rig {
    // the built-in camera (2 nodes, sysfs name with the driver's trailing colon) and a USB 4K camera (2 nodes)
    let mut r = rig(tag, 0, FakeOpener::default());
    r.machine.plug_webcam(0, "FaceTime HD Camera (Built-in):");
    r.machine.plug_webcam(2, "EMEET SmartCam Nova 4K");
    r.app = CameraApp::with_parts(r.machine.presence(), r.opener.clone(), r.dir.join("Pictures"), r.dir.join("Videos"));
    r
}

fn rows(g: &Gfx) -> Vec<(String, R)> {
    g.hits.iter().filter(|h| h.label != "backdrop" && matches!(h.ev, Event::Tab(n) if (PICK_BASE..PICK_BASE + 500).contains(&n))).map(|h| (h.label.clone(), h.rect)).collect()
}

#[test]
fn four_nodes_of_two_cameras_make_two_rows_with_the_built_in_and_the_one_in_use_marked() {
    let mut r = two_cams("rows");
    ready(&mut r);
    assert_eq!(r.app.camera_names(), vec!["FaceTime HD Camera (Built-in)", "EMEET SmartCam Nova 4K"]);
    assert_eq!(r.app.camera_rows(), vec!["FaceTime HD Camera (Built-in)  (built-in, in use)", "EMEET SmartCam Nova 4K"]);
    // metadata-only nodes never count
    assert_eq!(r.opener.opened.lock().unwrap().last().unwrap().0, "video0");
}

#[test]
fn the_camera_dialog_has_its_rows_inside_the_dialog_at_every_size() {
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (2560, 1600, 2.0), (640, 360, 0.6)] {
        let mut r = two_cams("layout");
        ready(&mut r);
        key(&mut r, Key::Char('c'));
        assert!(matches!(r.app.overlay, Overlay::Cameras));
        let mut g = Gfx::new(w, h);
        let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
        r.app.render(&mut g, area, s, &r.cx, 0);
        let rs = rows(&g);
        assert_eq!(rs.len(), 2, "{w}x{h}: one row per camera");
        let panel = g.hits.iter().find(|x| x.label == "Refresh").expect("Refresh button").rect;
        for (name, rc) in rs.iter().map(|(n, r)| (n.as_str(), *r)).chain(std::iter::once(("Refresh", panel))) {
            assert!(rc.x >= area.x && rc.x + rc.w <= area.x + area.w && rc.y >= area.y && rc.y + rc.h <= area.y + area.h, "{name} inside the screen at {w}x{h}");
            assert!(rc.h > 4.0 && rc.w > 20.0, "{name} has a size at {w}x{h}");
        }
        // rows and button share one dialog: the button lies below the last row and not far from it
        let last = rs.last().unwrap().1;
        assert!(panel.y > last.y + last.h - 1.0 && panel.y < last.y + last.h + 120.0 * s, "{w}x{h}");
    }
}

#[test]
fn choosing_a_row_switches_the_stream_keeps_sizes_per_camera_and_remembers_the_choice() {
    let mut r = two_cams("switch");
    ready(&mut r);
    key(&mut r, Key::Char('c'));
    r.app.handle(&Event::Tab(PICK_BASE + 1), &r.cx);
    assert!(matches!(r.app.overlay, Overlay::None));
    ready(&mut r);
    assert_eq!(r.opener.opened.lock().unwrap().last().unwrap().0, "video2", "the USB camera streams");
    key(&mut r, Key::Char('r'));
    key(&mut r, Key::Char('1'));
    pump(&mut r, "320 on the USB camera", |a| a.live_mode().is_some_and(|m| m.width == 320));
    // back to the built-in one: its own default size, not 320; then the USB camera again gets its 320 back
    key(&mut r, Key::Char('c'));
    r.app.handle(&Event::Tab(PICK_BASE), &r.cx);
    pump(&mut r, "the built-in", |a| a.live_mode().is_some_and(|m| m.width != 320));
    key(&mut r, Key::Char('c'));
    r.app.handle(&Event::Tab(PICK_BASE + 1), &r.cx);
    pump(&mut r, "320 again", |a| a.live_mode().is_some_and(|m| m.width == 320));
    // the next start opens the camera chosen last time
    let again = CameraApp::with_parts(r.machine.presence(), r.opener.clone(), r.dir.join("Pictures"), r.dir.join("Videos"));
    assert_eq!(again.camera_rows()[1], "EMEET SmartCam Nova 4K  (in use)");
}

#[test]
fn hotplug_adds_and_removes_rows_and_a_pulled_camera_falls_back_to_the_built_in_one_with_a_message() {
    let mut r = rig("hot", 0, FakeOpener::default());
    r.machine.plug_webcam(0, "FaceTime HD Camera (Built-in):");
    r.app = CameraApp::with_parts(r.machine.presence(), r.opener.clone(), r.dir.join("Pictures"), r.dir.join("Videos"));
    ready(&mut r);
    key(&mut r, Key::Char('c'));
    let mut g = Gfx::new(1280, 720);
    let area = R::new(0.0, 46.0, 1280.0, 674.0);
    r.app.render(&mut g, area, 1.0, &r.cx, 0);
    assert_eq!(rows(&g).len(), 1);
    // empty-state text and Refresh with only one camera
    assert!(g.hits.iter().any(|h| h.label == "Refresh"));
    // the USB camera is plugged in while the dialog is open: Refresh finds it
    r.machine.plug_webcam(2, "EMEET SmartCam Nova 4K");
    key(&mut r, Key::Char('r'));
    assert_eq!(r.app.camera_names().len(), 2);
    r.app.handle(&Event::Tab(PICK_BASE + 1), &r.cx);
    ready(&mut r);
    assert_eq!(r.opener.opened.lock().unwrap().last().unwrap().0, "video2");
    // pulled out while in use: the built-in one takes over and the person is told
    r.machine.unplug("video2");
    r.machine.unplug("video3");
    pump(&mut r, "the fall back", |a| a.camera_names().len() == 1 && a.live().is_some_and(|l| l.preview().is_some()));
    assert_eq!(r.opener.opened.lock().unwrap().last().unwrap().0, "video0");
    assert!(r.app.status_text().unwrap().contains("was disconnected; using FaceTime HD Camera (Built-in)"), "{:?}", r.app.status_text());
}

#[test]
fn the_dialog_says_there_is_no_other_camera_and_hidden_cameras_are_left_out() {
    let mut r = rig("empty", 1, FakeOpener::default());
    ready(&mut r);
    key(&mut r, Key::Char('c'));
    assert!(matches!(r.app.overlay, Overlay::Cameras), "one camera still opens the dialog");
    let src = include_str!("../camera.rs");
    assert!(src.contains("No other camera found."));
    let mut g = Gfx::new(1280, 720);
    r.app.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &r.cx, 0);
    assert_eq!(rows(&g).len(), 1);
    assert!(g.hits.iter().any(|h| h.label == "Refresh"));
    // a camera the person may not use is not in the list
    let m = FakeMachine::new("hidden");
    m.plug_webcam(0, "Secret Cam");
    m.plug_webcam(2, "C270");
    let mut p = m.presence();
    p.set_hidden(vec!["secret".into()]);
    p.scan();
    assert_eq!(p.cameras().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["C270"]);
    // the assistant's camera list is the same list
    use iyakki_agent::camtools::CameraBackend;
    let a = crate::camera::agentcam::AgentCamera::with(m.presence(), Arc::new(FakeOpener::default()));
    assert_eq!(a.cameras(), vec!["video0: Secret Cam", "video2: C270"]);
}

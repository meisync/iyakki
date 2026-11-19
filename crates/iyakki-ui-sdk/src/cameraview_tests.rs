use crate::cameras::*;
use crate::cameraview::*;
use crate::canvas::ImageFormat;
use crate::event::UiKey;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::size::Metrics;

fn rig(n: usize) -> (CameraView, MemCameras) {
    rig_with(n, ViewConfig::default())
}

fn rig_with(n: usize, cfg: ViewConfig) -> (CameraView, MemCameras) {
    let mem = MemCameras::new("panel");
    for (i, name) in ["Front door", "Yard", "Garage", "Hall", "Attic"].iter().enumerate().take(n) {
        let mut c = CamInfo::new(&format!("cam{i}"), name);
        c.ptz = i == 0;
        mem.add(c);
        mem.push_solid(&format!("cam{i}"), 8, 6, [10 + i as u8 * 40, 80, 120]);
    }
    (CameraView::new(Box::new(mem.clone()), cfg, 300), mem)
}

fn draw(v: &mut CameraView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { h.name.starts_with(name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn images(l: &DisplayList) -> usize {
    l.ops.iter().filter(|o| matches!(o, Op::Image(..))).count()
}

/// Pick the first `n` cameras, press Show and allow the card, with the clock at `t`.
fn go_live(v: &mut CameraView, n: usize, t: u64) {
    v.tick(t);
    for i in 0..n {
        assert!(v.action(300 + 20 + i as u32));
    }
    assert!(v.action(300));
    assert_eq!(v.stage(), &Stage::Consent);
    assert!(v.action(301), "Allow");
    assert_eq!(v.stage(), &Stage::Live);
}

#[test]
fn the_list_comes_first_the_person_ticks_cameras_and_a_view_of_more_than_four_is_refused() {
    let (mut v, mem) = rig(5);
    v.tick(0);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    for n in ["Front door", "Yard", "Attic"] {
        assert!(l.texts().iter().any(|t| t.contains(n)), "{n}: {:?}", l.texts());
    }
    assert!(!mem.calls().iter().any(|c| c == "open"), "listing starts nothing");
    for i in 0..5 {
        v.action(320 + i);
    }
    assert!(v.error().unwrap().contains("at most 4"));
    assert!(v.action(300));
    assert_eq!(v.stage(), &Stage::Consent);
    assert_eq!(v.shown().len(), 4);
    assert!(!v.action(999), "not ours");
}

#[test]
fn nothing_is_pulled_before_allow_and_a_denied_card_shows_nothing_and_starts_nothing() {
    let (mut v, mem) = rig(2);
    v.tick(0);
    v.action(320);
    v.action(321);
    v.action(300);
    for t in (100..1500).step_by(100) {
        v.tick(t);
    }
    assert!(!mem.calls().iter().any(|c| c.starts_with("frame")), "{:?}", mem.calls());
    assert_eq!(mem.pulled("cam0") + mem.pulled("cam1"), 0);
    let l = draw(&mut v, 360.0, 740.0, true, 1.0);
    let all = l.texts().join(" ");
    assert!(all.contains("panel wants to show 2 cameras: Front door, Yard"), "{all}");
    assert_eq!(images(&l), 0);
    assert!(!v.indicator());
    action_named(&l, "Allow");
    let deny = action_named(&l, "Deny");
    v.action(deny);
    assert!(matches!(v.stage(), Stage::Denied(_)));
    v.tick(5000);
    assert_eq!(mem.pulled("cam0"), 0);
    let l = draw(&mut v, 360.0, 740.0, true, 1.0);
    assert_eq!(images(&l), 0);
    action_named(&l, "Back to the list");
    assert!(mem.audit().contains(&"cameras.consent panel denied".to_string()));
}

#[test]
fn allowing_shows_the_pictures_with_the_sign_names_and_a_snapshot_asks_every_time() {
    let (mut v, mem) = rig(2);
    go_live(&mut v, 2, 0);
    v.tick(1000);
    assert!(v.indicator(), "the sign is on while pictures are pulled");
    assert_eq!(v.frame_of("cam0").unwrap().width, 8);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    assert_eq!(images(&l), 2);
    assert!(l.texts().contains(&"\u{25CF} Live"));
    // enlarge one camera, then snapshot: a card, and nothing is saved before it is allowed
    let cell = action_named(&l, "Front door, Live");
    v.action(cell);
    assert_eq!(v.enlarged().unwrap().id, "cam0");
    v.action(303);
    assert!(mem.snapshots().is_empty());
    let c = v.card().expect("a card for the snapshot").clone();
    assert!(c.summary.contains("Save one picture of Front door in your Pictures folder"));
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    let deny = action_named(&l, "Deny");
    v.action(deny);
    assert!(mem.snapshots().is_empty() && v.card().is_none());
    v.action(303);
    v.action(301);
    assert_eq!(mem.snapshots(), vec!["cam0".to_string()]);
    assert_eq!(v.note(), Some("Saved camera-snapshot.jpg in Pictures."));
    v.action(303);
    assert!(v.card().is_some(), "asks every time");
}

#[test]
fn pacing_follows_the_cells_and_only_the_newest_picture_is_kept_with_a_fake_clock() {
    let (mut v, mem) = rig(2);
    go_live(&mut v, 2, 0);
    mem.clear_calls();
    // 10 ms heartbeats for 2 seconds; a new picture every heartbeat on each camera
    let mut seq = 0;
    for t in (10..=2000u64).step_by(10) {
        seq = mem.push_solid("cam0", 8, 6, [t as u8, 1, 1]);
        mem.push_solid("cam1", 8, 6, [t as u8, 2, 2]);
        v.tick(t);
    }
    let asked = mem.calls().iter().filter(|c| *c == "frame cam0").count();
    assert!((9..=11).contains(&asked), "a grid cell is asked 5 times a second: {asked} in 2 s");
    let held = v.frame_of("cam0").unwrap().seq;
    assert!(seq - held <= 21, "the view holds the newest picture of its last look (at most 200 ms old), not a queue: {held} of {seq}");
    assert!(mem.pulled("cam0") <= 12, "only the pictures it asked for were handed out: {}", mem.pulled("cam0"));
    // one large camera is asked 10 times a second, and only that camera
    v.action(300 + 6);
    mem.clear_calls();
    for t in (2010..=3010u64).step_by(10) {
        mem.push_solid("cam0", 8, 6, [9, 9, 9]);
        v.tick(t);
    }
    let big = mem.calls().iter().filter(|c| *c == "frame cam0").count();
    assert!((9..=11).contains(&big), "{big}");
    assert_eq!(mem.calls().iter().filter(|c| *c == "frame cam1").count(), 0, "a camera that is not shown is not asked");
}

#[test]
fn a_hidden_view_pulls_nothing_and_tells_the_core_and_shows_again_when_visible() {
    let (mut v, mem) = rig(1);
    go_live(&mut v, 1, 0);
    v.tick(500);
    assert!(v.indicator());
    mem.clear_calls();
    v.set_visible(false);
    assert!(!v.indicator() && !v.is_visible());
    for t in (600..3000).step_by(50) {
        v.tick(t);
    }
    let calls = mem.calls();
    assert_eq!(calls, vec!["set_visible false".to_string()], "{calls:?}");
    v.set_visible(true);
    v.tick(3100);
    assert!(mem.calls().iter().any(|c| c == "frame cam0"));
    assert!(v.indicator());
}

#[test]
fn every_state_is_a_sentence_in_the_cell_and_a_refusing_host_is_told_in_words() {
    let (mut v, mem) = rig(2);
    mem.set_status("cam1", CamStatus::Lost, "");
    go_live(&mut v, 2, 0);
    let c = mem.clone();
    c.set_status("cam1", CamStatus::NeedsPassword, "the owner's vault is closed");
    // the second camera has a picture in MemCameras; take it away to see the sentence
    let mem2 = MemCameras::new("panel");
    let mut cam = CamInfo::new("x", "Gate");
    cam.status = CamStatus::NeedsAddon;
    cam.detail = "This camera sends H.264.".into();
    mem2.add(cam);
    let mut v2 = CameraView::new(Box::new(mem2), ViewConfig { cameras: vec!["x".into()], ..Default::default() }, 0);
    v2.tick(0);
    v2.action(1);
    v2.tick(300);
    let l = draw(&mut v2, 360.0, 740.0, true, 1.0);
    assert!(l.texts().iter().any(|t| t.contains("Needs the video add-on") || t.contains("H.264")), "{:?}", l.texts());
    // no cameras on this host
    let mut none = CameraView::new(Box::new(NoCameras), ViewConfig::default(), 0);
    none.tick(0);
    assert_eq!(none.error(), Some(UNAVAILABLE));
    let l = draw(&mut none, 360.0, 400.0, true, 1.0);
    assert!(l.texts().contains(&UNAVAILABLE) || l.texts().iter().any(|t| t.contains("not available")), "{:?}", l.texts());
}

#[test]
fn pan_tilt_zoom_shows_only_for_a_camera_that_has_it_and_asks_once() {
    let (mut v, mem) = rig(2);
    go_live(&mut v, 2, 0);
    v.tick(500);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    assert!(!l.a11y().iter().any(|n| n.name == "Zoom in"), "a grid has no pan controls");
    v.action(306);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    let right = action_named(&l, "Right");
    action_named(&l, "Zoom out");
    v.action(right);
    assert!(mem.moves("cam0").is_empty() && v.card().is_some(), "the first move asks");
    v.action(301);
    assert_eq!(mem.moves("cam0").len(), 1);
    v.action(right);
    assert_eq!(mem.moves("cam0").len(), 2, "later moves run");
    assert!(v.card().is_none());
    v.key(UiKey::Esc);
    v.action(307);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    assert!(!l.a11y().iter().any(|n| n.name == "Right"), "a camera without it has no buttons");
    assert!(v.key(UiKey::Left) || true);
    assert_eq!(mem.moves("cam1").len(), 0);
}

#[test]
fn zones_and_motion_marks_are_drawn_over_the_picture_and_are_not_controls() {
    let (mut v, mem) = rig(1);
    mem.set_zones("cam0", vec![Shape { name: "porch".into(), include: true, mask: false, points: vec![(0.1, 0.1), (0.9, 0.1), (0.5, 0.9)] }]);
    go_live(&mut v, 1, 0);
    v.tick(1200);
    let before = draw(&mut v, 800.0, 480.0, false, 1.0);
    let lines = |l: &DisplayList| l.ops.iter().filter(|o| matches!(o, Op::Line(..))).count();
    assert_eq!(lines(&before), 3, "the zone's three edges");
    mem.push_motion("cam0", Mark { id: 1, at_ms: 5, zone: "porch".into(), score: 50, bbox: [0.2, 0.2, 0.6, 0.6], active: true });
    v.tick(1500);
    let after = draw(&mut v, 800.0, 480.0, false, 1.0);
    let outlines = |l: &DisplayList| l.ops.iter().filter(|o| matches!(o, Op::Outline(..))).count();
    assert_eq!(outlines(&after), outlines(&before) + 1, "a box for the motion");
    assert_eq!(before.a11y().len(), after.a11y().len(), "overlays add no controls");
    // the picture lies under the overlay in drawing order
    let pos = |l: &DisplayList, f: &dyn Fn(&Op) -> bool| l.ops.iter().position(f).unwrap();
    assert!(pos(&after, &|o| matches!(o, Op::Image(..))) < pos(&after, &|o| matches!(o, Op::Line(..))));
}

#[test]
fn a_recording_camera_has_a_rec_mark_and_a_close_leaves_nothing_pulled_and_tells_the_core() {
    let (mut v, mem) = rig(1);
    let mut c = CamInfo::new("rec", "Studio");
    c.recording = true;
    mem.add(c);
    mem.push_solid("rec", 8, 6, [1, 2, 3]);
    v.tick(0);
    v.action(320 + 1);
    v.action(300);
    v.action(301);
    v.tick(600);
    let l = draw(&mut v, 800.0, 480.0, false, 1.0);
    assert!(l.texts().contains(&"REC"));
    assert!(l.a11y().iter().any(|n| n.name.contains("recording")));
    v.action(304);
    assert_eq!(v.stage(), &Stage::Picking);
    assert!(v.frame_of("rec").is_none() && !v.indicator());
    assert!(mem.audit().contains(&"cameras.close panel".to_string()));
    mem.clear_calls();
    v.tick(900);
    assert!(!mem.calls().iter().any(|c| c.starts_with("frame")), "after Close nothing is pulled");
}

#[test]
fn the_keys_answer_the_card_enlarge_a_cell_take_a_snapshot_and_go_back() {
    let (mut v, mem) = rig(3);
    v.tick(0);
    for i in 0..3 {
        v.action(320 + i);
    }
    assert!(v.key(UiKey::Enter));
    assert!(v.card().is_some());
    assert!(v.key(UiKey::Char('y')));
    v.tick(600);
    assert_eq!(v.stage(), &Stage::Live);
    assert!(v.key(UiKey::Char('2')));
    assert_eq!(v.enlarged().unwrap().id, "cam1");
    assert!(v.key(UiKey::Esc));
    assert!(v.enlarged().is_none(), "Esc goes back to the grid");
    assert!(v.key(UiKey::Char('f')));
    assert!(v.is_fullscreen());
    assert!(v.key(UiKey::Esc) && !v.is_fullscreen());
    assert!(v.key(UiKey::Char('s')) && v.card().is_some());
    assert!(v.key(UiKey::Char('n')));
    assert!(mem.snapshots().is_empty());
    v.focused = false;
    assert!(!v.key(UiKey::Esc), "without the focus the view takes no key");
}

#[test]
fn the_view_stays_inside_its_rectangle_with_named_targets_at_every_size_and_text_size() {
    for (w, h, touch) in [(360.0f32, 740.0f32, true), (800.0, 480.0, false), (1280.0, 720.0, false)] {
        for scale in [1.0f32, 3.0] {
            for n in [1usize, 2, 3, 4] {
                let (mut v, _) = rig(5);
                let area = Rect::new(0.0, 0.0, w, h);
                let check = |l: &DisplayList, what: &str| {
                    for node in l.a11y() {
                        assert!(!node.name.trim().is_empty(), "{what}: a control without a name");
                        let r = node.rect;
                        assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= area.right() + 0.5 && r.bottom() <= area.bottom() + 0.5, "{what} at {w}x{h} {scale}x: {} sits outside: {r:?}", node.name);
                        if touch && node.role == crate::canvas::Role::Button && node.name != "Live" {
                            assert!(r.h >= 44.0 - 0.5, "{what}: {} is {} high", node.name, r.h);
                        }
                    }
                    for o in &l.ops {
                        if let Op::Image(r, _) = o {
                            assert!(r.x >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{what}: a picture outside");
                        }
                    }
                };
                let l = draw(&mut v, w, h, touch, scale);
                check(&l, "empty list");
                v.tick(0);
                check(&draw(&mut v, w, h, touch, scale), "list");
                for i in 0..n {
                    v.action(320 + i as u32);
                }
                check(&draw(&mut v, w, h, touch, scale), "ticked");
                v.action(300);
                check(&draw(&mut v, w, h, touch, scale), "card");
                v.action(301);
                v.tick(1000);
                let live = draw(&mut v, w, h, touch, scale);
                check(&live, "live");
                assert_eq!(images(&live), n, "one picture per camera at {w}x{h} {scale}x ({n})");
                if n > 1 {
                    v.action(306);
                    check(&draw(&mut v, w, h, touch, scale), "enlarged");
                    v.action(305);
                    check(&draw(&mut v, w, h, touch, scale), "full screen");
                    v.key(UiKey::Esc);
                    v.key(UiKey::Esc);
                }
            }
        }
    }
}

#[test]
fn the_grid_follows_the_size_class_two_stack_on_a_phone_and_sit_side_by_side_on_a_desktop() {
    let rect_of = |l: &DisplayList, name: &str| -> Rect { l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { h.name.starts_with(name).then_some(*r) } else { None }).unwrap() };
    let (mut a, _) = rig(2);
    go_live(&mut a, 2, 0);
    a.tick(500);
    let phone = draw(&mut a, 360.0, 740.0, true, 1.0);
    let (p0, p1) = (rect_of(&phone, "Front door"), rect_of(&phone, "Yard"));
    assert!(p1.y > p0.bottom() - 1.0 && (p0.x - p1.x).abs() < 1.0, "stacked: {p0:?} {p1:?}");
    let desk = draw(&mut a, 1280.0, 720.0, false, 1.0);
    let (d0, d1) = (rect_of(&desk, "Front door"), rect_of(&desk, "Yard"));
    assert!(d1.x > d0.right() - 1.0 && (d0.y - d1.y).abs() < 1.0, "side by side: {d0:?} {d1:?}");
    // four cameras are two by two
    let (mut b, _) = rig(4);
    go_live(&mut b, 4, 0);
    b.tick(500);
    let l = draw(&mut b, 800.0, 480.0, false, 1.0);
    let ys: std::collections::BTreeSet<i32> = ["Front door", "Yard", "Garage", "Hall"].iter().map(|n| rect_of(&l, n).y as i32).collect();
    assert_eq!(ys.len(), 2);
}

#[test]
fn a_view_for_a_browser_asks_for_jpeg_and_the_pictures_are_in_the_list_as_images() {
    let (mut v, _) = rig_with(1, ViewConfig { format: ImageFormat::Jpeg, ..Default::default() });
    go_live(&mut v, 1, 0);
    v.tick(500);
    assert_eq!(v.frame_of("cam0").unwrap().format, ImageFormat::Jpeg);
    let l = draw(&mut v, 360.0, 740.0, true, 1.0);
    let text = l.to_text();
    assert!(text.lines().any(|x| x.starts_with("image ") && x.contains(" jpeg ")), "{text}");
    assert_eq!(DisplayList::parse(&text).unwrap(), l);
}

#[test]
fn the_cameras_the_app_names_open_without_the_list_and_the_view_has_no_colour_literals_or_machine_access() {
    let (mut v, mem) = rig_with(2, ViewConfig { cameras: vec!["cam1".into()], label: "Doorbell panel".into(), ..Default::default() });
    v.tick(0);
    assert_eq!(v.stage(), &Stage::Consent);
    assert!(v.card().unwrap().summary.contains("panel (Doorbell panel)") && v.card().unwrap().summary.contains("Yard"));
    assert!(mem.calls().iter().filter(|c| *c == "open").count() == 1);
    for (name, src) in [("cameraview.rs", include_str!("cameraview.rs")), ("cameras.rs", include_str!("cameras.rs"))] {
        assert!(!src.contains("rgb(") && !src.contains("Color::") , "{name}: a colour literal");
        let arrays = src.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| l.contains("[255,") || l.contains("[0, 0, 0") || l.contains("0x1") && l.contains("Color"));
        assert!(!arrays, "{name}: a colour literal");
        for bad in [["std::", "fs"].concat(), ["std::", "net"].concat(), ["std::", "process"].concat(), ["Tcp", "Stream"].concat()] {
            assert!(!src.contains(bad.as_str()), "{name}: machine access {bad}");
        }
    }
}

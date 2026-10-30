//! Touch screens on an extended desktop: the raw range onto the owner display's rectangle, the owner rules, the kept choice, the
//! calibration, rotation, several fingers, a touch that stays with its display, unplugging and the log line.

use super::*;
use crate::input::{InputState, Raw, EV_ABS, EV_KEY, EV_SYN};
use crate::ui::Event;

fn edp() -> OutRect {
    OutRect { name: "eDP-1".into(), x: 0, y: 0, w: 2560, h: 1600, internal: true, rot: Rot::R0 }
}
fn dp4() -> OutRect {
    OutRect { name: "DP-4".into(), x: 2560, y: 0, w: 1920, h: 1080, internal: false, rot: Rot::R0 }
}
fn usb_dev() -> TouchDev {
    TouchDev { key: "27c0:0859@usb-0000:00:14.0-3.3/input0".into(), id: "27c0:0859".into(), name: "TouchScreen".into(), usb: true, mt: true, x: (0, 32767), y: (0, 32767) }
}
fn ev(typ: u16, code: u16, value: i32) -> Raw {
    Raw { typ, code, value }
}
const SLOT: u16 = 0x2f;
const X: u16 = 0x35;
const Y: u16 = 0x36;
const TID: u16 = 0x39;

/// One finger frame on a multi-touch device: a finger down at (x, y) in `slot`.
fn down(st: &mut InputState, d: &TouchDev, t: Option<&OutRect>, slot: i32, id: i32, x: i32, y: i32) -> Vec<Event> {
    let mut v = Vec::new();
    for r in [ev(EV_ABS, SLOT, slot), ev(EV_ABS, TID, id), ev(EV_ABS, X, x), ev(EV_ABS, Y, y), ev(EV_SYN, 0, 0)] {
        v.extend(st.feed_touch(r, d, t).events);
    }
    v
}
fn up(st: &mut InputState, d: &TouchDev, t: Option<&OutRect>, slot: i32) -> Vec<Event> {
    let mut v = Vec::new();
    for r in [ev(EV_ABS, SLOT, slot), ev(EV_ABS, TID, -1), ev(EV_SYN, 0, 0)] {
        v.extend(st.feed_touch(r, d, t).events);
    }
    v
}

#[test]
fn the_raw_range_lands_in_the_rectangle_of_its_display() {
    let d = usb_dev();
    assert_eq!(map_point(&d, &dp4(), 0, 0), (2560, 0));
    assert_eq!(map_point(&d, &dp4(), 32767, 32767), (2560 + 1919, 1079));
    let mid = map_point(&d, &dp4(), 16384, 16384);
    assert!((mid.0 - (2560 + 960)).abs() <= 1 && (mid.1 - 540).abs() <= 1, "{mid:?}");
    // the built-in panel keeps the left part
    assert_eq!(map_point(&d, &edp(), 32767, 32767), (2559, 1599));
    // a range that does not start at 0 and values outside it
    let d2 = TouchDev { x: (100, 4195), y: (50, 2099), ..usb_dev() };
    assert_eq!(map_point(&d2, &dp4(), 100, 50), (2560, 0));
    assert_eq!(map_point(&d2, &dp4(), 4195, 2099), (2560 + 1919, 1079));
    assert_eq!(map_point(&d2, &dp4(), 9999, -5), (2560 + 1919, 0), "clamped to the display");
}

#[test]
fn rotation_turns_the_touch_with_the_picture() {
    let d = usb_dev();
    let r = |rot| OutRect { rot, w: 1080, h: 1920, ..dp4() };
    // the panel's own top-left corner
    assert_eq!(map_point(&d, &r(Rot::R0), 0, 0), (2560, 0));
    assert_eq!(map_point(&d, &r(Rot::R90), 0, 0), (2560 + 1079, 0), "top-left goes to top-right");
    assert_eq!(map_point(&d, &r(Rot::R180), 0, 0), (2560 + 1079, 1919));
    assert_eq!(map_point(&d, &r(Rot::R270), 0, 0), (2560, 1919), "top-left goes to bottom-left");
}

#[test]
fn the_owner_rules() {
    let both = [edp(), dp4()];
    let m = resolve(&usb_dev(), &both, &[]);
    assert_eq!((m.display, m.how.as_str()), (Some(1), "auto: only external display"));
    // a built-in (not USB) panel is the built-in display's
    let built_in = TouchDev { usb: false, ..usb_dev() };
    assert_eq!(resolve(&built_in, &both, &[]).display, Some(0));
    // one display: that one
    assert_eq!(resolve(&usb_dev(), &[edp()], &[]), Mapping { display: Some(0), how: "auto: only display".into() });
    // two external displays and an eDP: ask for the calibration, use the first external meanwhile
    let hdmi = OutRect { name: "HDMI-A-1".into(), x: 4480, ..dp4() };
    let three = [edp(), dp4(), hdmi.clone()];
    let m = resolve(&usb_dev(), &three, &[]);
    assert_eq!(m.display, Some(1));
    assert!(m.how.contains("calibration"), "{}", m.how);
    // nothing connected: no display, no panic
    assert_eq!(resolve(&usb_dev(), &[], &[]).display, None);
    // the saved choice wins, by device key; another device is not affected
    let saved = vec![(usb_dev().key, "HDMI-A-1".to_string())];
    assert_eq!(resolve(&usb_dev(), &three, &saved), Mapping { display: Some(2), how: "saved choice".into() });
    let other = TouchDev { key: "1111:2222@usb-x".into(), id: "1111:2222".into(), ..usb_dev() };
    assert_eq!(resolve(&other, &three, &saved).display, Some(1));
    // the saved display was unplugged: back to the rule, said so
    let m = resolve(&usb_dev(), &both, &saved);
    assert_eq!(m.display, Some(1));
    assert!(m.how.contains("HDMI-A-1 is not connected"), "{}", m.how);
}

#[test]
fn a_tap_on_the_external_monitor_reaches_the_external_monitor_not_the_main_screen() {
    // the bug: the raw range was spread over the whole 4480x1600 desktop, so the middle of the external monitor landed on eDP-1
    let d = usb_dev();
    let mut st = InputState::new(4480, 1600);
    let evs = down(&mut st, &d, Some(&dp4()), 0, 7, 16384, 16384);
    let pos = |e: &Event| match e {
        Event::MouseMove(x, y) | Event::MouseDown(x, y) | Event::MouseUp(x, y) => Some((*x, *y)),
        _ => None,
    };
    assert!(matches!(evs.first(), Some(Event::MouseMove(..))) && matches!(evs.last(), Some(Event::MouseDown(..))), "{evs:?}");
    for e in &evs {
        let (x, y) = pos(e).unwrap();
        assert!(x >= 2560 && y < 1080, "({x}, {y}) is not on DP-4");
    }
    // drag to the far left of the panel: still DP-4
    let moved = {
        let mut v = Vec::new();
        for r in [ev(EV_ABS, X, 0), ev(EV_SYN, 0, 0)] {
            v.extend(st.feed_touch(r, &d, Some(&dp4())).events);
        }
        v
    };
    assert_eq!(moved, vec![Event::MouseMove(2560, 540)]);
    let l = up(&mut st, &d, Some(&dp4()), 0);
    assert_eq!(l, vec![Event::MouseUp(2560, 540)]);
}

#[test]
fn a_touch_stays_with_the_display_it_started_on() {
    let d = usb_dev();
    let mut st = InputState::new(4480, 1600);
    down(&mut st, &d, Some(&dp4()), 0, 1, 0, 0);
    // the mapping changes while the finger is down (a choice made elsewhere): the move still goes to DP-4
    let mut v = Vec::new();
    for r in [ev(EV_ABS, X, 32767), ev(EV_SYN, 0, 0)] {
        v.extend(st.feed_touch(r, &d, Some(&edp())).events);
    }
    assert_eq!(v, vec![Event::MouseMove(2560 + 1919, 0)]);
    up(&mut st, &d, Some(&edp()), 0);
    // the next touch uses the new mapping
    let v = down(&mut st, &d, Some(&edp()), 0, 2, 0, 0);
    assert_eq!(v.last(), Some(&Event::MouseDown(0, 0)));
}

#[test]
fn several_fingers_the_first_drives_the_pointer() {
    let d = usb_dev();
    let mut st = InputState::new(4480, 1600);
    let t = Some(&dp4());
    let a = down(&mut st, &d, t, 0, 1, 0, 0);
    assert_eq!(a.last(), Some(&Event::MouseDown(2560, 0)));
    // a second finger in slot 1 moves nothing and clicks nothing
    assert!(down(&mut st, &d, t, 1, 2, 32767, 32767).is_empty());
    // the first finger moves
    let mut v = Vec::new();
    for r in [ev(EV_ABS, SLOT, 0), ev(EV_ABS, X, 32767), ev(EV_SYN, 0, 0)] {
        v.extend(st.feed_touch(r, &d, t).events);
    }
    assert_eq!(v, vec![Event::MouseMove(2560 + 1919, 0)]);
    // lifting the second finger is silent, lifting the first ends the touch
    assert!(up(&mut st, &d, t, 1).is_empty());
    assert_eq!(up(&mut st, &d, t, 0), vec![Event::MouseUp(2560 + 1919, 0)]);
}

#[test]
fn a_single_touch_device_uses_abs_x_y_and_btn_touch() {
    let d = TouchDev { mt: false, x: (0, 4095), y: (0, 4095), ..usb_dev() };
    let mut st = InputState::new(4480, 1600);
    let mut v = Vec::new();
    for r in [ev(EV_KEY, 0x14a, 1), ev(EV_ABS, 0, 4095), ev(EV_ABS, 1, 4095), ev(EV_SYN, 0, 0), ev(EV_KEY, 0x14a, 0), ev(EV_SYN, 0, 0)] {
        v.extend(st.feed_touch(r, &d, Some(&dp4())).events);
    }
    assert_eq!(v, vec![Event::MouseMove(4479, 1079), Event::MouseDown(4479, 1079), Event::MouseUp(4479, 1079)]);
}

#[test]
fn no_display_for_the_device_sends_nothing_and_unplugging_lifts_the_finger() {
    let d = usb_dev();
    let mut st = InputState::new(4480, 1600);
    assert!(down(&mut st, &d, None, 0, 1, 100, 100).is_empty(), "no owner display: no event, no panic");
    assert!(up(&mut st, &d, None, 0).is_empty());
    // a finger down, then the device is unplugged: the touch ends
    down(&mut st, &d, Some(&dp4()), 0, 3, 0, 0);
    assert_eq!(st.touch_gone(&d.key), vec![Event::MouseUp(2560, 0)]);
    assert!(st.touch_gone(&d.key).is_empty(), "gone twice is harmless");
}

#[test]
fn the_display_goes_away_and_the_device_falls_back() {
    let d = usb_dev();
    let m = resolve(&d, &[edp(), dp4()], &[]);
    assert_eq!(m.display, Some(1));
    // DP-4 unplugged: only the built-in display is left, the device follows it
    let m = resolve(&d, &[edp()], &[]);
    assert_eq!((m.display, m.how.as_str()), (Some(0), "auto: only display"));
}

#[test]
fn the_calibration_learns_a_display_per_device_and_keeps_it() {
    let dir = std::env::temp_dir().join(format!("iyakki-touchmap-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut m = TouchMap::default();
    m.load(&dir);
    m.start_calibration(vec!["eDP-1".into(), "DP-4".into()], 0);
    assert_eq!(m.calibrating(), Some("eDP-1"));
    // time passes with no tap: on to the next display
    m.tick(STEP_MS - 1);
    assert_eq!(m.calibrating(), Some("eDP-1"));
    m.tick(STEP_MS);
    assert_eq!(m.calibrating(), Some("DP-4"));
    // the finger of the external monitor taps while DP-4 is asked
    let key = usb_dev().key;
    assert_eq!(m.tap(&key, STEP_MS + 5), Some((key.clone(), "DP-4".to_string())));
    assert_eq!(m.calibrating(), None, "the last display was asked");
    // kept in the file and read back by a fresh map
    let mut again = TouchMap::default();
    again.load(&dir);
    assert_eq!(again.saved(), &[(key.clone(), "DP-4".to_string())]);
    assert_eq!(resolve(&usb_dev(), &[edp(), dp4()], again.saved()).how, "saved choice");
    // a device is placed once per run; Esc skips a display; a second run replaces the old answer
    m.start_calibration(vec!["eDP-1".into(), "DP-4".into()], 100);
    assert_eq!(m.tap(&key, 101), Some((key.clone(), "eDP-1".to_string())));
    assert_eq!(m.calibrating(), Some("DP-4"));
    assert_eq!(m.tap(&key, 102), None, "the same device does not also take the next display");
    m.skip_calibration(103);
    assert_eq!(m.calibrating(), None);
    assert_eq!(m.saved(), &[(key.clone(), "eDP-1".to_string())]);
    // an explicit choice from Settings, and "Match automatically" forgets it
    m.set_choice(&key, Some("DP-4"));
    assert_eq!(m.saved().len(), 1);
    m.set_choice(&key, None);
    assert!(m.saved().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_developer_log_line_names_the_device_the_display_and_the_reason() {
    let _guard = SHARED_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let line = DevLine { key: usb_dev().key, id: "27c0:0859".into(), name: "TouchScreen".into(), display: Some("DP-4".into()), how: "auto: only external display".into(), saved: None };
    assert_eq!(line.log_line(), "touch: 27c0:0859 TouchScreen -> DP-4 (auto: only external display)");
    // refresh() computes the same line from the real rule and publishes it for Settings
    let mut logged = Vec::new();
    let dev = std::sync::Arc::new(TouchDev { key: "test-refresh@x".into(), id: "27c0:0859".into(), name: "TouchScreen".into(), ..usb_dev() });
    let targets = refresh(&[dev], &[edp(), dp4()], &mut logged);
    assert_eq!(logged, vec![("test-refresh@x".to_string(), "touch: 27c0:0859 TouchScreen -> DP-4 (auto: only external display)".to_string())]);
    assert_eq!(targets[0].1.as_ref().map(|o| o.name.as_str()), Some("DP-4"));
    // the device is unplugged: forgotten
    let _ = refresh(&[], &[edp(), dp4()], &mut logged);
    assert!(logged.is_empty());
}

#[test]
fn the_plan_of_the_two_displays_gives_the_rectangles() {
    use crate::display::outputs::{Plan, Planned};
    let mode = |w: u16, h: u16| crate::display::kms::ModeInfo::make(w, h, 60, true);
    let plan = Plan {
        outputs: vec![
            Planned { name: "eDP-1".into(), connector: 1, crtc: 1, mode: mode(2560, 1600), x: 0, y: 0 },
            Planned { name: "DP-4".into(), connector: 2, crtc: 2, mode: mode(1920, 1080), x: 2560, y: 0 },
        ],
        ..Default::default()
    };
    let outs = outs_of_plan(&plan);
    assert_eq!(outs, vec![edp(), dp4()]);
    assert_eq!(resolve(&usb_dev(), &outs, &[]).display, Some(1));
}

use super::*;

/// The Apple SPI pad as the 14,1 reports it (ranges are typical, from the driver's defaults for this family).
const PAD: PadRange = PadRange { x_min: -5087, x_max: 8270, y_min: -182, y_max: 6749, major_max: 5000 };
const CFG: PadConfig = PadConfig { tap_to_click: true };
const W: i32 = 1920;

/// Replays frames onto a touchpad with a clock.
struct Rig {
    pad: Touchpad,
    now: u64,
    cfg: PadConfig,
    out: Vec<PadOut>,
}

impl Rig {
    fn new() -> Rig {
        Rig { pad: Touchpad::new(), now: 1000, cfg: CFG, out: Vec::new() }
    }
    fn ev(&mut self, typ: u16, code: u16, value: i32) {
        let o = self.pad.feed(Raw { typ, code, value }, self.now, &PAD, &self.cfg, W);
        self.out.extend(o);
    }
    /// One frame: (slot, tracking id (-1 lifts), x, y, major) per finger present, then SYN_REPORT, `ms` later.
    fn frame(&mut self, ms: u64, fingers: &[(i32, i32, i32, i32, i32)]) {
        self.now += ms;
        for &(slot, id, x, y, major) in fingers {
            self.ev(EV_ABS, ABS_MT_SLOT, slot);
            self.ev(EV_ABS, ABS_MT_TRACKING_ID, id);
            if id >= 0 {
                self.ev(EV_ABS, ABS_MT_POSITION_X, x);
                self.ev(EV_ABS, ABS_MT_POSITION_Y, y);
                self.ev(EV_ABS, ABS_MT_TOUCH_MAJOR, major);
            }
        }
        self.ev(EV_SYN, SYN_REPORT, 0);
    }
    fn lift(&mut self, ms: u64, slots: &[i32]) {
        let f: Vec<_> = slots.iter().map(|s| (*s, -1, 0, 0, 0)).collect();
        self.frame(ms, &f);
    }
    fn take(&mut self) -> Vec<PadOut> {
        std::mem::take(&mut self.out)
    }
}

fn moves(o: &[PadOut]) -> (i32, i32) {
    o.iter().fold((0, 0), |a, p| if let PadOut::Move(x, y) = p { (a.0 + x, a.1 + y) } else { a })
}

#[test]
fn a_short_single_finger_touch_is_a_left_click() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 1000, 3000, 600)]);
    r.frame(60, &[(0, 1, 1010, 3002, 600)]);
    r.lift(40, &[0]); // 100 ms in total
    let o = r.take();
    assert!(o.ends_with(&[PadOut::LeftDown, PadOut::LeftUp]), "{o:?}");
    assert!(moves(&o).0.abs() <= 2, "the finger's wobble moves the pointer by a pixel at most");
}

#[test]
fn a_two_finger_tap_is_a_right_click_and_makes_no_scroll() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
    r.frame(70, &[(0, 1, 1002, 3001, 600), (1, 2, 3001, 3000, 600)]);
    r.lift(40, &[0, 1]);
    assert_eq!(r.take(), vec![PadOut::RightClick]);
}

#[test]
fn a_touch_under_40_ms_or_over_the_tap_time_or_with_tap_to_click_off_is_no_click() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 1000, 3000, 600)]);
    r.lift(25, &[0]);
    assert!(r.take().is_empty(), "a 25 ms brush of the pad is ignored");

    r.frame(100, &[(0, 2, 1000, 3000, 600)]);
    r.frame(300, &[(0, 2, 1000, 3000, 600)]);
    r.lift(10, &[0]);
    assert!(r.take().is_empty(), "a long press is not a tap");

    r.cfg.tap_to_click = false;
    r.frame(100, &[(0, 3, 1000, 3000, 600)]);
    r.frame(60, &[(0, 3, 1000, 3000, 600)]);
    r.lift(30, &[0]);
    assert!(r.take().is_empty(), "tap-to-click off");
}

#[test]
fn a_touch_that_moved_is_not_a_tap_and_moves_the_pointer_instead() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 0, 3000, 600)]);
    for i in 1..=10 {
        r.frame(16, &[(0, 1, i * 300, 3000, 600)]); // 3000 units = 22% of the pad width in 160 ms
    }
    r.lift(16, &[0]);
    let o = r.take();
    assert!(!o.contains(&PadOut::LeftDown), "no click after a swipe: {o:?}");
    let (dx, dy) = moves(&o);
    assert!(dx > 100 && dx < 1500, "moved right by {dx}");
    assert_eq!(dy, 0);
}

#[test]
fn movement_starts_after_40_ms_and_a_faster_swipe_goes_farther_than_a_slow_one() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 0, 3000, 600)]);
    r.frame(16, &[(0, 1, 500, 3000, 600)]);
    r.frame(16, &[(0, 1, 1000, 3000, 600)]);
    assert!(r.take().is_empty(), "the first 32 ms only settle");
    r.frame(16, &[(0, 1, 1500, 3000, 600)]);
    assert!(moves(&r.take()).0 > 0, "48 ms: moving");
    r.lift(16, &[0]);
    r.take();

    // the same distance fast and slow
    let travel = |step: i32, frames: i32| {
        let mut r = Rig::new();
        r.frame(0, &[(0, 1, -3000, 3000, 600)]);
        r.frame(50, &[(0, 1, -3000, 3000, 600)]);
        r.take();
        for i in 1..=frames {
            r.frame(10, &[(0, 1, -3000 + i * step, 3000, 600)]);
        }
        moves(&r.take()).0
    };
    let slow = travel(40, 100); // 4000 units over 1 s
    let fast = travel(400, 10); // 4000 units over 100 ms
    assert!(fast > slow * 12 / 10, "acceleration: fast {fast} vs slow {slow}");
}

#[test]
fn small_movements_are_not_lost_to_rounding() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 0, 3000, 600)]);
    r.frame(50, &[(0, 1, 0, 3000, 600)]);
    r.take();
    for i in 1..=100 {
        r.frame(10, &[(0, 1, i * 3, 3000, 600)]); // 3 units per frame: a fraction of a pixel
    }
    assert!(moves(&r.take()).0 >= 20, "300 units of slow travel still moves the pointer");
}

#[test]
fn two_fingers_dragging_give_raw_travel_in_pad_widths_and_the_lift_ends_the_scroll() {
    let drag = |dir: i32| {
        let mut r = Rig::new();
        r.frame(0, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
        r.frame(50, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
        let first = r.take();
        assert!(first.is_empty(), "settling: nothing yet");
        for i in 1..=10 {
            r.frame(16, &[(0, 1, 1000, 3000 + dir * i * 100, 600), (1, 2, 3000, 3000 + dir * i * 100, 600)]);
        }
        let o = r.take();
        assert_eq!(moves(&o), (0, 0), "two fingers never move the pointer");
        assert!(!o.contains(&PadOut::ScrollEnd), "still scrolling");
        r.lift(16, &[0, 1]);
        assert!(r.take().contains(&PadOut::ScrollEnd), "the lift ends the scroll");
        o.iter().map(|p| if let PadOut::Scroll(n) = p { *n } else { 0.0 }).sum::<f32>()
    };
    // 1000 units of travel = 7.5% of the pad's width; the direction and speed are the scroll engine's, so the sign is the fingers'
    assert!((drag(1) - 0.075).abs() < 0.002, "{}", drag(1));
    assert!((drag(-1) + 0.075).abs() < 0.002);
}

#[test]
fn lifting_one_of_two_scrolling_fingers_ends_the_scroll_too() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
    r.frame(50, &[(0, 1, 1000, 3000, 600), (1, 2, 3000, 3000, 600)]);
    r.frame(16, &[(0, 1, 1000, 3200, 600), (1, 2, 3000, 3200, 600)]);
    r.take();
    r.lift(16, &[1]);
    assert!(r.take().contains(&PadOut::ScrollEnd));
}

#[test]
fn a_palm_is_ignored_but_a_finger_beside_it_still_works() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 0, 3000, 4000)]); // major 4000 of 5000: palm
    r.frame(80, &[(0, 1, 2000, 3000, 4000)]);
    r.lift(30, &[0]);
    assert!(r.take().is_empty(), "no movement, no tap from a palm");

    // palm at slot 1 and a finger at slot 0: one active contact moves the pointer
    r.frame(0, &[(0, 5, 0, 1000, 500), (1, 6, 0, 6000, 4500)]);
    r.frame(50, &[(0, 5, 0, 1000, 500), (1, 6, 0, 6000, 4500)]);
    r.take();
    for i in 1..=5 {
        r.frame(16, &[(0, 5, i * 300, 1000, 500), (1, 6, i * 50, 6000, 4500)]);
    }
    assert!(moves(&r.take()).0 > 0, "the finger moves the pointer while the palm rests");
}

#[test]
fn the_physical_click_presses_and_releases_the_left_button_and_with_two_fingers_it_is_a_right_click() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 1000, 3000, 600)]);
    r.ev(EV_KEY, BTN_LEFT, 1);
    r.ev(EV_KEY, BTN_LEFT, 0);
    assert_eq!(r.take(), vec![PadOut::LeftDown, PadOut::LeftUp]);
    r.lift(100, &[0]);
    assert!(r.take().is_empty(), "the touch that clicked physically does not tap again");

    r.frame(100, &[(0, 2, 1000, 3000, 600), (1, 3, 3000, 3000, 600)]);
    r.ev(EV_KEY, BTN_LEFT, 1);
    r.ev(EV_KEY, BTN_LEFT, 0);
    assert_eq!(r.take(), vec![PadOut::RightClick], "two fingers + click = right click, and no stray left release");
}

#[test]
fn a_finger_added_or_lifted_during_a_swipe_does_not_make_the_pointer_jump() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 0, 3000, 600)]);
    r.frame(50, &[(0, 1, 100, 3000, 600)]);
    r.take();
    r.frame(16, &[(0, 1, 200, 3000, 600), (1, 2, 6000, 3000, 600)]); // second finger lands far away
    r.frame(16, &[(0, 1, 200, 3000, 600)]); // and lifts again
    let (dx, _) = moves(&r.take());
    assert!(dx.abs() < 200, "no jump toward the landing point of the second finger: {dx}");
}

#[test]
fn a_pad_without_contact_size_has_no_palm_detection() {
    let range = PadRange { major_max: 0, ..PAD };
    let mut p = Touchpad::new();
    let mut out = Vec::new();
    for (code, v) in [(ABS_MT_SLOT, 0), (ABS_MT_TRACKING_ID, 1), (ABS_MT_POSITION_X, 0), (ABS_MT_POSITION_Y, 100), (ABS_MT_TOUCH_MAJOR, 9999)] {
        out.extend(p.feed(Raw { typ: EV_ABS, code, value: v }, 10, &range, &CFG, W));
    }
    out.extend(p.feed(Raw { typ: EV_SYN, code: SYN_REPORT, value: 0 }, 10, &range, &CFG, W));
    assert_eq!(p.active(&range), vec![0]);
}

#[test]
fn the_apple_spi_touchpad_of_the_14_1_is_a_touchpad_and_a_touch_screen_or_mouse_is_not() {
    // /proc/bus/input/devices of the owner's MacBookPro14,1: PROP=5 EV=f KEY=e520 10000 0 0 0 0 REL=3 ABS=27f800000000003
    let key = bits_from_proc("e520 10000 0 0 0 0");
    let prop = bits_from_proc("5");
    let abs = bits_from_proc("27f800000000003");
    assert!(bit(&key, 0x110) && bit(&key, 0x14a) && bit(&key, 0x145) && bit(&key, 0x14d), "BTN_LEFT, BTN_TOUCH, TOOL_FINGER, TOOL_QUINTTAP");
    assert!(bit(&prop, 0) && !bit(&prop, 1) && bit(&prop, 2), "POINTER + BUTTONPAD, not DIRECT");
    assert!(bit(&abs, 0) && bit(&abs, 1) && bit(&abs, 0x2f) && bit(&abs, 0x35) && bit(&abs, 0x36) && bit(&abs, 0x39));
    assert!(is_touchpad(&key, &prop, &abs));
    // the same with INPUT_PROP_DIRECT is a touch screen: old handling
    assert!(!is_touchpad(&key, &bits_from_proc("2"), &abs));
    // a mouse: no finger tool
    assert!(!is_touchpad(&bits_from_proc("1f0000 0 0 0 0 0"), &[], &[]));
    // single-touch only pad (no multi-touch axes) is not handled by this code
    assert!(!is_touchpad(&key, &prop, &bits_from_proc("3")));
}

#[test]
fn a_physical_click_with_one_finger_in_the_bottom_right_corner_is_a_right_click() {
    let mut r = Rig::new();
    r.frame(0, &[(0, 1, 7500, 5800, 600)]); // right and low
    r.ev(EV_KEY, BTN_LEFT, 1);
    r.ev(EV_KEY, BTN_LEFT, 0);
    assert_eq!(r.take(), vec![PadOut::RightClick]);
    r.lift(50, &[0]);
    r.frame(100, &[(0, 2, 0, 1000, 600)]); // top left
    r.ev(EV_KEY, BTN_LEFT, 1);
    r.ev(EV_KEY, BTN_LEFT, 0);
    assert_eq!(r.take(), vec![PadOut::LeftDown, PadOut::LeftUp]);
}

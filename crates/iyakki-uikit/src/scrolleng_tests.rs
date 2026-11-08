use super::*;

const CFG: ScrollConfig = ScrollConfig { speed_pct: 100, wheel_natural: false, pad_natural: true, smooth: true, momentum: true, wheel_lines: 3 };

/// Run the engine at 60 frames a second from `t0` until it settles (or 3 s); returns the steps and the time of the last one.
fn run(e: &mut ScrollEngine, t0: u64, raw: bool) -> (Vec<i32>, u64) {
    let (mut t, mut v) = (t0, Vec::new());
    while e.wants_frames() && t < t0 + 3000 {
        t += FRAME_MS;
        v.push(e.tick(t, raw));
    }
    (v, t)
}

#[test]
fn one_wheel_click_is_three_lines_and_the_steps_add_up_to_exactly_that() {
    let mut e = ScrollEngine::new();
    e.wheel(120, &CFG);
    let (steps, _) = run(&mut e, 0, false);
    assert_eq!(steps.iter().sum::<i32>(), 54, "3 lines of 18 px: {steps:?}");
    assert!(steps.iter().all(|s| *s >= 0), "never backwards");
}

#[test]
fn the_ease_takes_about_ninety_to_one_forty_milliseconds_and_never_overshoots() {
    let mut e = ScrollEngine::new();
    e.wheel(120 * 4, &CFG); // 216 px
    let (mut t, mut total) = (0u64, 0i32);
    let mut at_90 = 0;
    while e.wants_frames() {
        t += FRAME_MS;
        total += e.tick(t, false);
        assert!(total <= 216, "passed the target: {total} at {t} ms");
        if t == 96 {
            at_90 = total;
        }
        assert!(t < 400, "did not settle");
    }
    assert_eq!(total, 216);
    assert!(at_90 as f32 > 216.0 * 0.9, "about 90 percent after ~96 ms: {at_90}");
    assert!(t >= 96 && t <= 240, "settled at {t} ms");
}

#[test]
fn without_smoothing_the_wheel_goes_out_in_the_next_step() {
    let mut e = ScrollEngine::new();
    let c = ScrollConfig { smooth: false, ..CFG };
    e.wheel(120, &c);
    assert_eq!(e.tick(16, false), 54);
    assert!(!e.wants_frames());
}

#[test]
fn raw_mode_sends_the_wheel_at_once_for_the_browser() {
    let mut e = ScrollEngine::new();
    e.wheel(120, &CFG);
    assert_eq!(e.tick(16, true), 54, "no second easing on top of the page engine's own");
}

#[test]
fn speed_scales_the_distance_and_stays_inside_its_limits() {
    let total = |pct: u32| {
        let mut e = ScrollEngine::new();
        e.wheel(120, &ScrollConfig { speed_pct: pct, ..CFG });
        run(&mut e, 0, false).0.iter().sum::<i32>()
    };
    assert_eq!(total(100), 54);
    assert_eq!(total(200), 108);
    assert_eq!(total(50), 27);
    assert_eq!(total(10), 27, "below 50 is 50");
    assert_eq!(total(900), 162, "above 300 is 300");
}

#[test]
fn lines_per_click_and_direction_follow_the_settings() {
    let total = |c: ScrollConfig| {
        let mut e = ScrollEngine::new();
        e.wheel(120, &c);
        run(&mut e, 0, false).0.iter().sum::<i32>()
    };
    assert_eq!(total(ScrollConfig { wheel_lines: 5, ..CFG }), 90);
    assert_eq!(total(ScrollConfig { wheel_lines: 1, ..CFG }), 18);
    assert_eq!(total(ScrollConfig { wheel_lines: 99, ..CFG }), 180, "ten lines at most");
    assert_eq!(total(ScrollConfig { wheel_natural: true, ..CFG }), -54, "natural wheel: the other way");
}

#[test]
fn the_wheel_is_classic_and_the_trackpad_natural_by_default() {
    let d = ScrollConfig::default();
    assert!(!d.wheel_natural && d.pad_natural && d.smooth && d.momentum && d.speed_pct == 100 && d.wheel_lines == 3);
    let mut w = ScrollEngine::new();
    w.wheel(120, &d); // rolled away from the person: the page goes up the screen's start
    assert!(run(&mut w, 0, false).0.iter().sum::<i32>() > 0);
    let mut p = ScrollEngine::new();
    p.pad(PAD_STEP, 0, &d); // fingers moved down the pad: the content follows them down = scroll up
    assert!(p.tick(16, false) > 0);
    let mut q = ScrollEngine::new();
    q.pad(PAD_STEP, 0, &ScrollConfig { pad_natural: false, ..d });
    assert!(q.tick(16, false) < 0, "classic trackpad: the other way");
}

#[test]
fn a_hi_res_wheel_adds_up_its_fractions_and_loses_nothing() {
    let mut e = ScrollEngine::new();
    // 15 small turns of 8/120 of a click: one and a half clicks... exactly 120 units in all
    for _ in 0..15 {
        e.wheel(8, &CFG);
    }
    let (steps, _) = run(&mut e, 0, false);
    assert_eq!(steps.iter().sum::<i32>(), 54, "120/120 of a click");
}

#[test]
fn a_new_click_while_easing_adds_to_what_is_left() {
    let mut e = ScrollEngine::new();
    e.wheel(120, &CFG);
    let first = e.tick(16, false) + e.tick(32, false);
    e.wheel(120, &CFG);
    let (rest, _) = run(&mut e, 32, false);
    assert_eq!(first + rest.iter().sum::<i32>(), 108);
}

#[test]
fn an_opposite_click_cancels_part_of_what_was_queued() {
    let mut e = ScrollEngine::new();
    e.wheel(240, &CFG);
    e.wheel(-120, &CFG);
    assert_eq!(run(&mut e, 0, false).0.iter().sum::<i32>(), 54);
}

#[test]
fn a_runaway_wheel_does_not_scroll_for_seconds_after_it_stopped() {
    let mut e = ScrollEngine::new();
    for _ in 0..2000 {
        e.wheel(120, &CFG);
    }
    let (steps, t) = run(&mut e, 0, false);
    assert!(steps.iter().sum::<i32>() <= MAX_QUEUE_PX as i32 + 1);
    assert!(t < 1000, "settled in {t} ms");
}

#[test]
fn trackpad_movement_goes_straight_out_without_lag() {
    let mut e = ScrollEngine::new();
    e.pad(PAD_STEP * 2.0, 0, &CFG);
    assert_eq!(e.tick(16, false), 108, "two clicks' worth at once, at the next frame");
    assert_eq!(e.tick(32, false), 0);
}

fn fling(e: &mut ScrollEngine, cfg: &ScrollConfig, per_frame: f32) -> u64 {
    // 12 frames of 16 ms of steady two-finger travel
    let mut t = 0;
    for _ in 0..12 {
        t += FRAME_MS;
        e.pad(per_frame, t, cfg);
        e.tick(t, false);
    }
    e.pad_end(t, cfg);
    t
}

#[test]
fn momentum_continues_after_the_lift_and_decays_exponentially_to_a_stop() {
    let mut e = ScrollEngine::new();
    let t = fling(&mut e, &CFG, 0.01); // 216 px per frame step rate: fast
    assert!(e.wants_frames(), "momentum going");
    let (steps, end) = run(&mut e, t, false);
    assert!(steps.len() > 10 && steps.iter().all(|s| *s >= 0), "{steps:?}");
    let third = steps.len() / 3;
    let (a, b, c): (i32, i32, i32) = (steps[..third].iter().sum(), steps[third..2 * third].iter().sum(), steps[2 * third..].iter().sum());
    assert!(a > b && b > c, "slows down: {a} {b} {c} in {steps:?}");
    assert!(end - t < 3000, "stops");
    // exponential: the second half of a time constant moves about e^-1 of the first
    let early: i32 = steps[..20].iter().sum();
    let late: i32 = steps[20..40.min(steps.len())].iter().sum();
    assert!(late < early && late > 0, "{early} then {late}");
}

#[test]
fn momentum_is_off_when_the_setting_is_off_and_for_a_slow_lift() {
    let off = ScrollConfig { momentum: false, ..CFG };
    let mut e = ScrollEngine::new();
    fling(&mut e, &off, 0.01);
    e.tick(300, false);
    assert!(!e.wants_frames(), "no momentum without the setting");
    let mut s = ScrollEngine::new();
    fling(&mut s, &CFG, 0.00002); // a crawl
    s.tick(300, false);
    assert!(!s.wants_frames(), "a slow lift just stops");
}

#[test]
fn a_new_touch_and_the_end_of_the_list_stop_momentum_at_once() {
    let mut e = ScrollEngine::new();
    let t = fling(&mut e, &CFG, 0.01);
    assert!(e.tick(t + 16, false) > 0);
    e.touch();
    assert_eq!(e.tick(t + 32, false), 0);
    assert!(!e.wants_frames());
    let mut b = ScrollEngine::new();
    let t = fling(&mut b, &CFG, 0.01);
    b.bound();
    assert_eq!(b.tick(t + 16, false), 0);
    assert!(!b.wants_frames());
}

#[test]
fn bound_also_drops_what_the_wheel_had_queued() {
    let mut e = ScrollEngine::new();
    e.wheel(1200, &CFG);
    e.tick(16, false);
    e.bound();
    assert!(!e.wants_frames());
    assert_eq!(e.tick(32, false), 0);
}

#[test]
fn the_clock_is_the_callers_and_a_stalled_frame_does_not_jump() {
    let mut e = ScrollEngine::new();
    e.wheel(120, &CFG);
    let a = e.tick(0, false);
    let b = e.tick(10_000, false); // a stalled frame counts as at most 100 ms of easing
    assert!(a > 0 && b > 0 && a + b <= 54, "{a} {b}");
    // a clock that stands still (or goes back) makes a small step, never a negative one
    let mut f = ScrollEngine::new();
    f.wheel(120, &CFG);
    f.tick(100, false);
    assert!(f.tick(100, false) >= 0 && f.tick(50, false) >= 0);
}

#[test]
fn frames_are_wanted_while_scrolling_and_stop_when_it_settles() {
    let mut e = ScrollEngine::new();
    assert!(!e.wants_frames(), "idle: the shell stays at its low rate");
    e.wheel(120, &CFG);
    assert!(e.wants_frames());
    run(&mut e, 0, false);
    assert!(!e.wants_frames(), "settled: back to the low rate");
}

#[test]
fn the_shared_settings_are_stored_and_read_back() {
    let keep = config();
    request_config(ScrollConfig { speed_pct: 150, ..CFG });
    assert_eq!(config().speed_pct, 150);
    request_config(keep);
    set_raw(true);
    assert!(raw());
    set_raw(false);
    note_bound();
    assert!(take_bound() && !take_bound());
}

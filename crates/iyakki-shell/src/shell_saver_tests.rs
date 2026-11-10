//! The screen saver in the shell, with a fake clock: idle -> saver -> off -> wake, what keeps the screen on, the first key or click
//! only waking, injected input, battery halving, 'Never', the panel (fake /sys, a crash and the restart), the blank calls, the
//! sign-in screen and guests, doing less while off, and drawing at many sizes.

use super::tests::*;
use super::*;
use crate::saver::Stage;
use iyakki_agent::screensaver::{First, SaverConfig, Style};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MIN: u64 = 60_000;

fn cfg() -> SaverConfig {
    SaverConfig { first: First::Saver, after_min: 5, then_off_min: 5, battery_shorter: true, style: Style::Logo, dim_pct: 50 }
}

/// A signed-in shell on the launcher with a fake clock at 0 and the choices `c`.
fn saver_shell(tag: &str, c: SaverConfig) -> (Shell, Arc<AtomicU64>) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let clock = Arc::new(AtomicU64::new(1_000));
    sh.saver_clock_for_test(clock.clone());
    sh.saver_config_for_test(c);
    (sh, clock)
}

/// Move the clock on by `ms` in heartbeats of 250 ms (the real ones are 33 ms; the idle rules do not care).
fn run(sh: &mut Shell, clock: &AtomicU64, ms: u64) {
    step_run(sh, clock, ms, 250);
}

/// The same with the real 33 ms heartbeat, for what depends on counting heartbeats.
fn run_fine(sh: &mut Shell, clock: &AtomicU64, ms: u64) {
    step_run(sh, clock, ms, 33);
}

fn step_run(sh: &mut Shell, clock: &AtomicU64, ms: u64, every: u64) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(every);
        clock.fetch_add(step, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        left -= step;
    }
}

fn fresh(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-shsaver-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fake_sys(tag: &str, value: u32) -> (PathBuf, PathBuf) {
    let d = fresh(tag);
    let bl = d.join("class/backlight/intel_backlight");
    std::fs::create_dir_all(&bl).unwrap();
    std::fs::write(bl.join("brightness"), value.to_string()).unwrap();
    std::fs::write(bl.join("max_brightness"), "1000").unwrap();
    (d.clone(), d.join("run/iyakki/backlight-restore"))
}

fn backlight(sys: &Path) -> u32 {
    std::fs::read_to_string(sys.join("class/backlight/intel_backlight/brightness")).unwrap().trim().parse().unwrap()
}

#[test]
fn the_tests_start_with_a_clock_that_never_moves_so_no_saver_ever_shows() {
    let (mut sh, _) = shell_with_users("saver-frozen");
    login(&mut sh, "root", "rootpass12");
    for _ in 0..2000 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.screen_stage(), Stage::Awake);
}

#[test]
fn idle_goes_to_the_saver_then_off_and_any_input_wakes_it() {
    let (mut sh, clock) = saver_shell("saver-cycle", cfg());
    run(&mut sh, &clock, 4 * MIN + 50_000);
    assert_eq!(sh.screen_stage(), Stage::Awake);
    run(&mut sh, &clock, 12_000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    run(&mut sh, &clock, 5 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Off);
    sh.handle(&Event::MouseMove(10, 10));
    assert_eq!(sh.screen_stage(), Stage::Awake, "a pointer move wakes it");
    assert!(sh.take_stale(), "and the whole scene is drawn again");
}

#[test]
fn input_keeps_the_screen_on() {
    let (mut sh, clock) = saver_shell("saver-input", cfg());
    for _ in 0..8 {
        run(&mut sh, &clock, 3 * MIN);
        sh.handle(&Event::MouseMove(5, 5));
    }
    assert_eq!(sh.screen_stage(), Stage::Awake, "24 minutes with input every 3");
    for ev in [Event::Key(Key::Char('x')), Event::MouseDown(1, 1), Event::Scroll(1), Event::Fn(crate::fnkeys::FnKey::VolumeUp, true)] {
        run(&mut sh, &clock, 4 * MIN + 30_000);
        sh.handle(&ev);
        run(&mut sh, &clock, 1000);
        assert_eq!(sh.screen_stage(), Stage::Awake, "{ev:?} restarted the idle time");
    }
}

#[test]
fn the_first_key_only_wakes_and_the_next_reaches_the_screen() {
    let (mut sh, clock) = saver_shell("saver-key", cfg());
    assert_eq!(sh.screen_name(), "launcher");
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    sh.handle(&Event::Key(Key::Enter)); // would open the selected tile
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert_eq!(sh.screen_name(), "launcher", "the waking key was not delivered");
    sh.handle(&Event::Key(Key::Enter));
    assert_ne!(sh.screen_name(), "launcher", "the next one is");
}

#[test]
fn a_stray_click_wakes_without_pressing_anything_and_its_release_is_dropped_too() {
    let (mut sh, clock) = saver_shell("saver-click", cfg());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let chip = sh.hits().iter().find(|h| h.label == "group Build").map(|h| (h.rect.x as i32 + 5, h.rect.y as i32 + 5)).expect("the Build chip");
    assert_ne!(sh.group, crate::launcher::Group::Work);
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    sh.handle(&Event::MouseDown(chip.0, chip.1));
    sh.handle(&Event::MouseUp(chip.0, chip.1));
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert_ne!(sh.group, crate::launcher::Group::Work, "the click that woke it pressed nothing");
    sh.render(&mut g);
    sh.handle(&Event::MouseMove(chip.0, chip.1));
    sh.handle(&Event::MouseDown(chip.0, chip.1));
    sh.handle(&Event::MouseUp(chip.0, chip.1));
    assert_eq!(sh.group, crate::launcher::Group::Work, "the next click works");
}

#[test]
fn the_pointer_is_hidden_while_the_saver_or_black_screen_is_up() {
    let (mut sh, clock) = saver_shell("saver-pointer", cfg());
    assert_ne!(sh.pointer(), crate::ui::Pointer::Hidden);
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.pointer(), crate::ui::Pointer::Hidden, "a still arrow would burn in");
    run(&mut sh, &clock, 5 * MIN);
    assert_eq!(sh.pointer(), crate::ui::Pointer::Hidden);
    sh.handle(&Event::MouseMove(9, 9));
    assert_ne!(sh.pointer(), crate::ui::Pointer::Hidden);
}

#[test]
fn pointer_movement_only_wakes() {
    let (mut sh, clock) = saver_shell("saver-move", cfg());
    run(&mut sh, &clock, 5 * MIN + 1000);
    sh.handle(&Event::MouseMove(321, 123));
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert_eq!(sh.mouse, (321, 123), "the pointer is where it was moved to");
}

#[test]
fn input_from_a_paired_computer_wakes_the_screen_and_still_reaches_the_app() {
    let (mut sh, clock) = saver_shell("saver-inject", cfg());
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    sh.saver.injecting = true;
    sh.handle(&Event::Key(Key::Enter));
    sh.saver.injecting = false;
    assert_eq!(sh.screen_stage(), Stage::Awake);
    assert_ne!(sh.screen_name(), "launcher", "developer-mode input is delivered");
}

#[test]
fn never_and_do_nothing_leave_the_screen_alone() {
    for c in [SaverConfig { after_min: 0, ..cfg() }, SaverConfig { first: First::Nothing, ..cfg() }] {
        let (mut sh, clock) = saver_shell("saver-never", c);
        run(&mut sh, &clock, 120 * MIN);
        assert_eq!(sh.screen_stage(), Stage::Awake);
    }
}

#[test]
fn something_the_person_must_answer_or_is_watching_keeps_the_screen_on() {
    let (mut sh, clock) = saver_shell("saver-hold", cfg());
    assert_eq!(sh.saver_keep_on_for_test(), None);
    // each reason, looked at as soon as it is set (the heartbeat's own reading of the world would reset some of them)
    sh.badge.approvals = 1;
    assert_eq!(sh.saver_keep_on_for_test(), Some("a waiting approval"));
    sh.badge.approvals = 0;
    sh.battery.plug_card = true;
    assert_eq!(sh.saver_keep_on_for_test(), Some("a battery card"));
    sh.battery.plug_card = false;
    sh.loss.last = Some(iyakki_storage::loss::State::Lost);
    assert_eq!(sh.saver_keep_on_for_test(), Some("the drive banner"));
    sh.loss.last = Some(iyakki_storage::loss::State::Healthy);
    sh.ring = Some(Ring { items: vec![("tea".into(), true, "10:00".into())], since: 0, player: None, next_sound: 0 });
    assert_eq!(sh.saver_keep_on_for_test(), Some("an alarm"));
    sh.ring = None;
    sh.fnk.power_card = true;
    assert_eq!(sh.saver_keep_on_for_test(), Some("a dialog"));
    sh.fnk.power_card = false;
    assert_eq!(sh.saver_keep_on_for_test(), None);
    // held over time: the drive banner (the shell reads the drive's state itself) and a ringing alarm
    iyakki_storage::loss::set_thread_override(Some(iyakki_storage::loss::Snapshot { state: iyakki_storage::loss::State::Lost, since: 10, lost_at: Some(10), ..Default::default() }));
    run(&mut sh, &clock, 30 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Awake, "the drive banner holds the screen");
    iyakki_storage::loss::set_thread_override(None);
    run(&mut sh, &clock, 4 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Awake, "the idle time counts from the moment the banner went");
    run(&mut sh, &clock, 2 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Saver);
}

#[test]
fn something_that_appears_while_the_saver_runs_wakes_it() {
    let (mut sh, clock) = saver_shell("saver-appear", cfg());
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    iyakki_storage::loss::set_thread_override(Some(iyakki_storage::loss::Snapshot { state: iyakki_storage::loss::State::Lost, since: 10, lost_at: Some(10), ..Default::default() }));
    run_fine(&mut sh, &clock, 700);
    iyakki_storage::loss::set_thread_override(None);
    assert_eq!(sh.screen_stage(), Stage::Awake, "the drive banner appeared");
    assert!(sh.take_stale());
}

#[test]
fn the_inhibit_api_holds_the_screen_until_released_or_expired() {
    let (mut sh, clock) = saver_shell("saver-inhibit", cfg());
    let t = sh.inhibit_screen_off("a long download", 3600);
    assert_eq!(sh.saver_holds_for_test(), vec!["a long download".to_string()]);
    run(&mut sh, &clock, 30 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Awake);
    sh.release_screen_off(t);
    run(&mut sh, &clock, 5 * MIN + 5000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    sh.handle(&Event::Key(Key::Esc));
    sh.inhibit_screen_off("short", 60);
    run(&mut sh, &clock, 6 * MIN + 5000);
    assert_eq!(sh.screen_stage(), Stage::Saver, "the hold expired after a minute, then 5 idle minutes");
}

#[test]
fn on_battery_every_delay_is_halved() {
    let (mut sh, clock) = saver_shell("saver-battery", cfg());
    let sys = fresh("saver-batsys");
    for (k, v) in [("type", "Battery"), ("status", "Discharging"), ("capacity", "64"), ("energy_now", "32000000"), ("energy_full", "50000000"), ("energy_full_design", "60000000"), ("power_now", "8000000")] {
        let p = sys.join(format!("class/power_supply/BAT0/{k}"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{v}\n")).unwrap();
    }
    std::fs::create_dir_all(sys.join("class/power_supply/ADP1")).unwrap();
    std::fs::write(sys.join("class/power_supply/ADP1/type"), "Mains\n").unwrap();
    std::fs::write(sys.join("class/power_supply/ADP1/online"), "0\n").unwrap();
    sh.battery.set_env(iyakki_agent::battery::BatteryEnv::at(&sys));
    sh.battery_step(0);
    run(&mut sh, &clock, 1000);
    run(&mut sh, &clock, 150_000);
    assert_eq!(sh.screen_stage(), Stage::Saver, "2.5 minutes on battery");
    run(&mut sh, &clock, 150_000);
    assert_eq!(sh.screen_stage(), Stage::Off, "and 2.5 more");
    // on the adapter the normal times apply
    std::fs::write(sys.join("class/power_supply/ADP1/online"), "1\n").unwrap();
    sh.handle(&Event::Key(Key::Esc));
    sh.battery_step(1000); // the heartbeat reads the adapter (by its own clock, which a test does not move)
    run_fine(&mut sh, &clock, 700);
    run(&mut sh, &clock, 150_000);
    assert_eq!(sh.screen_stage(), Stage::Awake, "2.5 minutes on the adapter is not enough");
}

#[test]
fn the_saver_draws_only_when_a_frame_is_due_at_the_rate_of_the_power_source() {
    let (mut sh, clock) = saver_shell("saver-rate", cfg());
    let mut g = Gfx::new(1280, 720);
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    let mut frames = |sh: &mut Shell, ms: u64| -> u64 {
        let before = sh.saver_frames_for_test();
        let mut left = ms;
        while left > 0 {
            clock.fetch_add(33, Ordering::SeqCst);
            sh.handle(&Event::Tick);
            if sh.take_stale() {
                sh.render(&mut g);
            }
            left = left.saturating_sub(33);
        }
        sh.saver_frames_for_test() - before
    };
    let n = frames(&mut sh, 10_000);
    assert!((60..=160).contains(&n), "about 15 frames a second on the adapter: {n} in 10 s");
    assert_eq!(sh.redraw_interval_ms(), 1000, "the backend does not redraw the saver by itself every 100 ms");
}

#[test]
fn off_draws_black_once_stops_drawing_and_does_little_work() {
    let (mut sh, clock) = saver_shell("saver-off", SaverConfig { first: First::Off, after_min: 1, dim_pct: 0, ..cfg() });
    let mut g = Gfx::new(800, 480);
    sh.render(&mut g);
    run(&mut sh, &clock, MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Off);
    assert!(sh.take_stale(), "the black picture is drawn once");
    sh.render(&mut g);
    assert!(g.rgba().chunks(4).all(|p| p == [0, 0, 0, 255]), "all black");
    let t0 = sh.tick;
    let mut draws = 0;
    for _ in 0..300 {
        clock.fetch_add(33, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        if sh.take_stale() {
            draws += 1;
        }
    }
    assert_eq!(draws, 0, "nothing is drawn while the screen is off");
    assert_eq!(sh.tick - t0, 300, "the heartbeat count still follows the time");
    assert_eq!(sh.redraw_interval_ms(), 60_000);
    assert!(sh.saver_screen_off(), "the Touch Bar's strip follows");
}

#[test]
fn the_panel_is_dimmed_switched_off_and_restored_exactly_and_the_blank_calls_come_in_order() {
    let (sys, restore) = fake_sys("saver-panel", 777);
    let (mut sh, clock) = saver_shell("saver-panel-sh", cfg());
    sh.saver_panel_for_test(&sys, &restore);
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    assert_eq!(backlight(&sys), 388, "dimmed to 50 % when the saver starts");
    assert!(restore.is_file());
    assert!(sh.take_blank_requests().is_empty(), "the screen is still on");
    run(&mut sh, &clock, 5 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Off);
    assert_eq!(backlight(&sys), 0);
    assert_eq!(sh.take_blank_requests(), vec![true], "power the screen down once");
    sh.handle(&Event::Key(Key::Char('a')));
    assert_eq!(backlight(&sys), 777, "exactly the old brightness");
    assert!(!restore.exists());
    assert_eq!(sh.take_blank_requests(), vec![false], "and unblank");
    // saver then wake without going off: no blank call at all
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert_eq!(backlight(&sys), 388);
    sh.handle(&Event::MouseMove(3, 3));
    assert_eq!(backlight(&sys), 777);
    assert!(sh.take_blank_requests().is_empty());
}

#[test]
fn a_crash_while_the_screen_is_off_is_repaired_from_the_restore_file() {
    let (sys, restore) = fake_sys("saver-crash", 640);
    {
        let (mut sh, clock) = saver_shell("saver-crash-sh", SaverConfig { first: First::Off, after_min: 1, dim_pct: 0, ..cfg() });
        sh.saver_panel_for_test(&sys, &restore);
        run(&mut sh, &clock, MIN + 1000);
        assert_eq!(backlight(&sys), 0);
        // the shell dies here
    }
    assert!(restore.is_file());
    assert_eq!(crate::panel::restore_leftover(&restore), 1);
    assert_eq!(backlight(&sys), 640);
}

#[test]
fn leaving_the_shell_puts_the_panel_back() {
    let (sys, restore) = fake_sys("saver-leave", 500);
    let (mut sh, clock) = saver_shell("saver-leave-sh", SaverConfig { first: First::Off, after_min: 1, dim_pct: 0, ..cfg() });
    sh.saver_panel_for_test(&sys, &restore);
    run(&mut sh, &clock, MIN + 1000);
    assert_eq!(backlight(&sys), 0);
    sh.panel_shutdown();
    assert_eq!(backlight(&sys), 500);
    assert_eq!(sh.take_blank_requests(), vec![true, false]);
    assert_eq!(sh.screen_stage(), Stage::Awake);
}

#[test]
fn a_direct_off_dims_for_the_last_ten_seconds() {
    let (sys, restore) = fake_sys("saver-dim", 800);
    let (mut sh, clock) = saver_shell("saver-dim-sh", SaverConfig { first: First::Off, after_min: 2, dim_pct: 25, ..cfg() });
    sh.saver_panel_for_test(&sys, &restore);
    run(&mut sh, &clock, 100_000);
    assert_eq!(backlight(&sys), 800);
    run(&mut sh, &clock, 15_000);
    assert_eq!(sh.screen_stage(), Stage::Dim);
    assert_eq!(backlight(&sys), 200);
    assert!(sh.redraw_interval_ms() <= 100, "the desktop is still drawn while it is only dimmed");
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(backlight(&sys), 800, "any input brings the brightness back");
}

#[test]
fn a_machine_without_a_backlight_still_runs_the_saver_and_the_off_picture() {
    let (mut sh, clock) = saver_shell("saver-nobl", SaverConfig { first: First::Off, after_min: 1, ..cfg() });
    run(&mut sh, &clock, MIN + 1000);
    assert_eq!(sh.screen_stage(), Stage::Off);
    assert!(sh.take_blank_requests().is_empty(), "a window or a remote screen never blanks the host's display");
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.screen_stage(), Stage::Awake);
}

#[test]
fn a_preview_from_settings_runs_five_seconds_ignoring_the_click_that_started_it() {
    let (mut sh, clock) = saver_shell("saver-preview", SaverConfig { after_min: 0, ..cfg() });
    let dir = sh.env.services.dir().to_path_buf();
    iyakki_agent::screensaver::request(&dir, "root", iyakki_agent::screensaver::Request::Preview);
    run_fine(&mut sh, &clock, 600);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    sh.handle(&Event::MouseUp(5, 5)); // the release of the Preview button
    assert_eq!(sh.screen_stage(), Stage::Saver, "within the grace time");
    run(&mut sh, &clock, 5_500);
    assert_eq!(sh.screen_stage(), Stage::Awake, "five seconds");
    // and stopped by input after the grace time
    sh.start_screen_saver(Some(5));
    run(&mut sh, &clock, 1000);
    sh.handle(&Event::Key(Key::Char('x')));
    assert_eq!(sh.screen_stage(), Stage::Awake);
}

#[test]
fn the_sign_in_screen_uses_the_machines_choices_and_a_guest_the_standard_ones() {
    let (mut sh, _) = shell_with_users("saver-signin");
    let clock = Arc::new(AtomicU64::new(1_000));
    sh.saver_clock_for_test(clock.clone());
    assert!(sh.session.is_none());
    // default for the sign-in screen: saver after 5 minutes, off 10 minutes later
    run(&mut sh, &clock, 5 * MIN + 5000);
    assert_eq!(sh.screen_stage(), Stage::Saver);
    run(&mut sh, &clock, 10 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Off);
    sh.handle(&Event::Key(Key::Char('r')));
    assert_eq!(sh.screen_stage(), Stage::Awake);
    // an administrator's change applies
    sh.env.services.settings.lock().unwrap().update(|s| s.screensaver_signin = iyakki_agent::settings::SigninSaver(SaverConfig { after_min: 1, ..SaverConfig::signin_default() })).unwrap();
    run(&mut sh, &clock, 2000);
    run(&mut sh, &clock, MIN + 5000);
    assert_eq!(sh.screen_stage(), Stage::Saver, "after the one minute the administrator chose");
    // a guest has the standard ten minutes whatever file is there; a person has their own
    sh.handle(&Event::Key(Key::Esc));
    SaverConfig { after_min: 1, ..SaverConfig::default() }.save(sh.env.services.dir(), "gus").unwrap();
    SaverConfig { after_min: 2, ..SaverConfig::default() }.save(sh.env.services.dir(), "alice").unwrap();
    login(&mut sh, "gus", "guestpass1");
    let c = Arc::new(AtomicU64::new(clock.load(Ordering::SeqCst)));
    sh.saver_clock_for_test(c.clone());
    run(&mut sh, &c, 9 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Awake);
    run(&mut sh, &c, 2 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Saver, "ten minutes for a guest");
    sh.sign_out();
    login(&mut sh, "alice", "alicepass1");
    let c2 = Arc::new(AtomicU64::new(c.load(Ordering::SeqCst)));
    sh.saver_clock_for_test(c2.clone());
    run(&mut sh, &c2, 2 * MIN + 5000);
    assert_eq!(sh.screen_stage(), Stage::Saver, "alice chose two minutes");
}

#[test]
fn the_saver_draws_black_with_the_logo_and_the_desktop_comes_back_whole() {
    let (mut sh, clock) = saver_shell("saver-draw", cfg());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let desktop = g.rgba().to_vec();
    run(&mut sh, &clock, 5 * MIN + 1000);
    assert!(sh.take_stale());
    sh.render(&mut g);
    assert_eq!(&g.rgba()[0..4], &[0, 0, 0, 255], "pure black background");
    assert!(g.rgba().chunks(4).any(|p| p[0] > 60 || p[1] > 60 || p[2] > 60), "the logo is there");
    sh.handle(&Event::Key(Key::Char('w')));
    sh.render(&mut g);
    // the desktop shows the time: under load a minute can pass between the first picture and now, so the restored desktop is compared with
    // a desktop drawn completely, from nothing, at this moment (a patch of it would differ), not with the first picture
    let _ = &desktop;
    let mut fresh = Gfx::new(1280, 720);
    sh.render(&mut fresh);
    assert_eq!(g.rgba(), fresh.rgba(), "the whole desktop is back, not a patch of it");
}

#[test]
fn the_saver_and_off_work_at_every_size_and_in_every_style() {
    for (w, h) in [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (2880, 1800), (200, 150), (1, 1)] {
        for style in Style::ALL {
            let (mut sh, clock) = saver_shell("saver-sizes", SaverConfig { style, ..cfg() });
            let mut g = Gfx::new(w, h);
            sh.render(&mut g);
            run(&mut sh, &clock, 5 * MIN + 1000);
            for _ in 0..40 {
                clock.fetch_add(66, Ordering::SeqCst);
                sh.handle(&Event::Tick);
                sh.render(&mut g);
            }
            run(&mut sh, &clock, 5 * MIN);
            sh.render(&mut g);
            sh.handle(&Event::MouseMove(1, 1));
            sh.render(&mut g);
        }
    }
}

#[test]
fn a_frame_changes_a_small_part_of_a_big_screen() {
    let (mut sh, clock) = saver_shell("saver-budget", cfg());
    let mut g = Gfx::new(2880, 1800);
    run(&mut sh, &clock, 5 * MIN + 1000);
    sh.render(&mut g);
    let before = g.rgba().to_vec();
    clock.fetch_add(66, Ordering::SeqCst);
    sh.handle(&Event::Tick);
    clock.fetch_add(66, Ordering::SeqCst);
    sh.handle(&Event::Tick);
    assert!(sh.take_stale());
    sh.render(&mut g);
    let rows: usize = before.chunks(2880 * 4).zip(g.rgba().chunks(2880 * 4)).filter(|(a, b)| a != b).count();
    assert!(rows > 0 && rows < 900, "{rows} of 1800 rows changed: the logo's old and new boxes only");
}

/// Found on the Mac mini (2026-11-09): after ten idle minutes the saver and then the black screen drew only their own frames, which never
/// went through the capture, so `iyakki dev state` and `dev screenshot` both timed out until someone touched the machine.
#[test]
fn a_computer_asking_for_the_state_or_a_picture_ends_the_saver_instead_of_timing_out() {
    use iyakki_agent::devmode::state;
    let (mut sh, clock) = saver_shell("sv-dev", cfg());
    run(&mut sh, &clock, 11 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Off);
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    let dir = sh.env.services.dir().to_path_buf();
    // the state
    let _ = state::fetch(&dir, 0); // a computer asks: "wanted" for a few seconds
    run(&mut sh, &clock, 250);
    sh.render(&mut g);
    assert_eq!(sh.screen_stage(), Stage::Awake, "the request ended the black screen");
    let snap = state::fetch(&dir, 500).expect("a frame");
    assert_eq!(snap.app, "Home");
    // and the same after the saver came back
    run(&mut sh, &clock, 11 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Off);
    // a picture
    let svc = sh.env.services.clone();
    svc.screen.set_present(true);
    let asker = std::thread::spawn(move || svc.screen.request(iyakki_agent::devmode::SCREEN_ANY, std::time::Duration::from_secs(5)));
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        run(&mut sh, &clock, 33);
        sh.render(&mut g);
        if asker.is_finished() {
            break;
        }
    }
    let frame = asker.join().unwrap().expect("a picture");
    assert!(frame.w > 0 && frame.h > 0 && !frame.rgba.is_empty());
    assert_eq!(sh.screen_stage(), Stage::Awake);
}

#[test]
fn a_computer_asking_for_the_screen_makes_the_black_screen_draw_soon_instead_of_once_a_minute() {
    use iyakki_agent::devmode::state;
    // the backend draws when the time since its last frame passes redraw_interval_ms(): 60 s with the screen off, so a request that
    // arrived just after a frame waited up to a minute, and `iyakki dev state` gave up after ten seconds (found on the Mac mini)
    let (mut sh, clock) = saver_shell("sv-dev-interval", cfg());
    run(&mut sh, &clock, 11 * MIN);
    assert_eq!(sh.screen_stage(), Stage::Off);
    assert_eq!(sh.redraw_interval_ms(), 60_000, "nothing asks: the black screen stays quiet");
    let dir = sh.env.services.dir().to_path_buf();
    let _ = state::fetch(&dir, 0); // a computer asks for the state
    assert!(sh.redraw_interval_ms() <= 200, "it draws soon: {}", sh.redraw_interval_ms());
    // and for a picture, in the saver stage too
    let (mut sh2, clock2) = saver_shell("sv-dev-interval2", cfg());
    run(&mut sh2, &clock2, 6 * MIN);
    assert!(sh2.screen_stage().covers());
    assert!(sh2.redraw_interval_ms() >= 1000 || sh2.screen_stage() == Stage::Off);
    let svc = sh2.env.services.clone();
    svc.screen.set_present(true);
    let asker = std::thread::spawn(move || svc.screen.request(iyakki_agent::devmode::SCREEN_ANY, std::time::Duration::from_secs(2)));
    let t0 = std::time::Instant::now();
    while sh2.redraw_interval_ms() > 200 && t0.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(sh2.redraw_interval_ms() <= 200, "a request for a picture makes it draw soon too");
    let _ = asker.join();
}

#[test]
fn while_developer_mode_is_on_the_screen_stays_on_and_when_it_is_off_the_saver_works_as_before() {
    // asked by the owner 2026-11-10: the black screen of a test machine got in the way of the developer driving it
    use iyakki_agent::devmode::DevMode;
    let (mut sh, clock) = saver_shell("sv-dev-on", cfg());
    // developer mode on for real: the shell reads it every few seconds and would overwrite a value set by hand
    let dm = DevMode::open(sh.env.services.dir());
    dm.enable(&iyakki_sec::policy::Role::Admin.template(), "root", 0).unwrap();
    run(&mut sh, &clock, 30 * MIN);
    assert!(sh.dev_sign.as_deref().is_some_and(|s| s.starts_with("DEV")), "the sign: {:?}", sh.dev_sign);
    assert_eq!(sh.screen_stage(), Stage::Awake, "developer mode keeps the screen on");
    assert_eq!(sh.saver_keep_on(), Some("developer mode"));
    // developer mode off: the idle time counts again and the saver and the black screen come as before
    dm.disable(&iyakki_sec::policy::Role::Admin.template(), "root").unwrap();
    run(&mut sh, &clock, 12 * MIN);
    assert!(sh.dev_sign.is_none(), "{:?}", sh.dev_sign);
    assert_eq!(sh.screen_stage(), Stage::Off);
    // an unfinished system update is not developer mode
    let (mut sh2, clock2) = saver_shell("sv-dev-upd", cfg());
    sh2.dev_sign = Some("UPDATE INTERRUPTED \u{00b7} choose \"previous version\" at boot".to_string());
    assert_eq!(sh2.saver_keep_on(), None);
    run(&mut sh2, &clock2, 11 * MIN);
    assert_eq!(sh2.screen_stage(), Stage::Off);
}

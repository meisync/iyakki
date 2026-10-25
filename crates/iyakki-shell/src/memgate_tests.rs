use super::*;

fn gib(x: f64) -> u64 {
    (x * GIB as f64) as u64
}
fn mem(total: f64, free: f64) -> MemFacts {
    MemFacts { total: gib(total), available: gib(free) }
}
const HD: DisplayMode = DisplayMode { w: 1920, h: 1080 };
const QHD: DisplayMode = DisplayMode { w: 2560, h: 1440 };
const UHD: DisplayMode = DisplayMode { w: 3840, h: 2160 };

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-memgate-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_one_gigabyte_machine_gets_one_pane_on_one_display_whatever_is_free() {
    for free in [0.1, 0.4, 0.8, 0.9] {
        for disp in [vec![HD], vec![HD, HD], vec![UHD, UHD, UHD]] {
            let a = screen_features(mem(0.9, free), &disp, &[AppClass::Light; 4]);
            assert_eq!((a.max_panes, a.max_displays, a.multi_display), (1, 1, false), "free {free}");
            assert!(!a.reason.is_empty());
        }
    }
}

#[test]
fn under_two_to_three_gigabytes_means_two_panes_and_no_extra_display() {
    for total in [1.6, 1.9, 2.4] {
        let a = screen_features(mem(total, total * 0.8), &[HD, HD], &[AppClass::Light; 4]);
        assert_eq!((a.max_panes, a.max_displays, a.multi_display), (2, 1, false), "{total}");
        assert!(a.reason.contains("allows 2 panes"), "{}", a.reason);
    }
}

#[test]
fn three_gigabytes_gives_three_panes_and_two_displays() {
    let a = screen_features(mem(3.0, 2.4), &[HD, HD, HD], &[AppClass::Light; 4]);
    assert_eq!((a.max_panes, a.max_displays, a.multi_display), (3, 2, true));
}

#[test]
fn four_gigabytes_and_more_gives_four_panes_and_several_displays() {
    for total in [3.8, 7.8, 15.6] {
        let a = screen_features(mem(total, total * 0.75), &[QHD, HD, HD], &[AppClass::Medium; 4]);
        assert_eq!((a.max_panes, a.max_displays, a.multi_display), (4, 3, true), "{total}: {}", a.reason);
        assert!(a.reason.is_empty(), "{}", a.reason);
    }
    let a = screen_features(mem(3.8, 2.9), &[HD], &[AppClass::Light]);
    assert_eq!((a.max_panes, a.multi_display), (4, false), "one display only present");
}

#[test]
fn the_table_over_totals_free_levels_displays_and_resolutions_is_monotonic_and_safe() {
    let totals = [0.9, 1.9, 3.8, 7.8, 15.6];
    let fracs = [0.9, 0.6, 0.3, 0.15, 0.05];
    let res = [HD, QHD, UHD];
    let wanted = [AppClass::Medium, AppClass::Heavy, AppClass::Heavy, AppClass::Heavy];
    for total in totals {
        let mut last = usize::MAX;
        for f in fracs {
            for n in 1..=3usize {
                for r in res {
                    let ds = vec![r; n];
                    let m = mem(total, total * f);
                    let a = screen_features(m, &ds, &wanted);
                    assert!(a.max_panes >= 1 && a.max_panes <= MAX_PANES && a.max_displays >= 1 && a.max_displays <= n.max(1).min(MAX_DISPLAYS));
                    // never spend more than the budget, and never into the reserve
                    if a.max_panes > 1 || a.max_displays > 1 {
                        assert!(a.cost <= a.budget, "{total} {f} {n} {r:?}: cost {} budget {}", a.cost, a.budget);
                        assert!(m.available - a.cost >= MIN_FREE, "reserve broken");
                    }
                    assert_eq!(a.multi_display, a.max_displays >= 2);
                }
            }
            // less free memory never allows more panes (1 display, 1080p)
            let a = screen_features(mem(total, total * f), &[HD], &wanted);
            assert!(a.max_panes <= last, "{total} {f}: {} > {last}", a.max_panes);
            last = a.max_panes;
        }
    }
}

#[test]
fn the_reserve_rule_never_uses_more_than_a_quarter_and_leaves_512_mib() {
    assert_eq!(budget(gib(4.0)), gib(1.0));
    assert_eq!(budget(gib(0.6)), gib(0.6) - MIN_FREE, "a quarter of 0.6 GiB is 150 MiB, the floor leaves 100");
    assert_eq!(budget(MIN_FREE), 0);
    assert_eq!(budget(100 * MIB), 0);
    assert_eq!(budget(0), 0);
    // a machine with a lot of RAM but almost none available: single
    let a = screen_features(mem(15.6, 0.5), &[HD, HD], &[AppClass::Heavy; 4]);
    assert_eq!((a.max_panes, a.max_displays), (1, 1));
    // heavy apps cost more than light ones: the same memory allows fewer panes
    let heavy = screen_features(mem(3.8, 0.9), &[UHD], &[AppClass::Heavy; 4]);
    let light = screen_features(mem(3.8, 0.9), &[UHD], &[AppClass::Light; 4]);
    assert!(heavy.max_panes < light.max_panes, "{} vs {}", heavy.max_panes, light.max_panes);
}

#[test]
fn app_classes_put_the_engines_and_the_ide_in_the_heavy_class() {
    for a in [PApp::Browser, PApp::Ide, PApp::Camera, PApp::Data, PApp::Dash] {
        assert_eq!(class_of(a), AppClass::Heavy, "{a:?}");
    }
    for a in [PApp::Chat, PApp::Edit, PApp::Cli] {
        assert_eq!(class_of(a), AppClass::Medium);
    }
    for a in [PApp::Settings, PApp::Users, PApp::Inbox, PApp::Radio] {
        assert_eq!(class_of(a), AppClass::Light);
    }
    for a in PApp::ALL {
        let _ = class_of(a);
    }
    assert!(AppClass::Light.cost() < AppClass::Medium.cost() && AppClass::Medium.cost() < AppClass::Heavy.cost());
    assert!(AppClass::Light.min_size().0 < AppClass::Heavy.min_size().0);
}

#[test]
fn a_garbled_or_missing_meminfo_means_unknown_and_a_single_screen() {
    let d = tmp("garbled");
    assert_eq!(read_mem(&d), MemFacts::default(), "no proc at all");
    std::fs::create_dir_all(d.join("proc")).unwrap();
    assert_eq!(read_mem(&d), MemFacts::default(), "no meminfo");
    for text in ["", "\u{0}\u{1}garbage", "MemTotal: lots kB\nMemAvailable: many kB\n", "MemTotal:\nMemAvailable:", "MemTotal: 100 kB\nMemAvailable: 99999999 kB\n"] {
        std::fs::write(d.join("proc/meminfo"), text).unwrap();
        let m = read_mem(&d);
        assert!(!m.known(), "{text:?} gave {m:?}");
        let a = screen_features(m, &[HD, HD], &[AppClass::Light; 4]);
        assert_eq!((a.max_panes, a.max_displays), (1, 1));
        assert!(a.reason.contains("could not be read"));
        assert!(memory_line(&a, m).contains("single screen only"));
    }
    std::fs::write(d.join("proc/meminfo"), "MemTotal:        4000000 kB\nMemFree: 1 kB\nMemAvailable:    3000000 kB\nSwapTotal: 9999999 kB\nSwapFree: 9999999 kB\n").unwrap();
    let m = read_mem(&d);
    assert_eq!(m.total, 4_000_000 * 1024);
    assert_eq!(m.available, 3_000_000 * 1024, "swap is not memory");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_memory_line_says_what_is_allowed_and_why() {
    let m = mem(3.8, 2.9);
    let a = screen_features(m, &[HD], &[AppClass::Light]);
    assert_eq!(memory_line(&a, m), "Layouts: available (4 GB, 2.9 GB free)");
    let m = mem(1.8, 1.4);
    let a = screen_features(m, &[HD], &[AppClass::Light]);
    let line = memory_line(&a, m);
    assert!(line.starts_with("Layouts: limited to 2 panes (2 GB)"), "{line}");
    let m = mem(0.9, 0.6);
    let a = screen_features(m, &[HD], &[AppClass::Light]);
    let line = memory_line(&a, m);
    assert!(line.starts_with("Layouts: single screen only (1 GB)"), "{line}");
    let off = features_with(Switch::Off, m, &[HD], &[]);
    assert!(memory_line(&off, m).contains("off"));
    let on = features_with(Switch::On, m, &[HD], &[]);
    assert!(memory_line(&on, m).contains("always on"));
}

#[test]
fn the_off_and_always_on_switches() {
    let low = mem(0.9, 0.3);
    let big = mem(15.6, 12.0);
    let off = features_with(Switch::Off, big, &[HD, HD], &[AppClass::Light; 4]);
    assert_eq!((off.max_panes, off.max_displays, off.multi_display), (1, 1, false));
    let on = features_with(Switch::On, low, &[HD, HD], &[AppClass::Heavy; 4]);
    assert_eq!((on.max_panes, on.multi_display), (MAX_PANES, true), "on ignores tiers and budget");
    let auto = features_with(Switch::Auto, low, &[HD, HD], &[AppClass::Heavy; 4]);
    assert_eq!(auto.max_panes, 1);
    assert_eq!(Switch::Auto.next(), Switch::On);
    assert_eq!(Switch::On.next(), Switch::Off);
    assert_eq!(Switch::Off.next(), Switch::Auto);
    assert_eq!(Switch::from_id("garbage"), Switch::Auto);
}

#[test]
fn the_switch_is_stored_per_machine_and_defaults_to_automatic() {
    let d = tmp("switch");
    assert_eq!(read_switch(&d), Switch::Auto);
    write_switch(&d, Switch::Off).unwrap();
    assert_eq!(read_switch(&d), Switch::Off);
    write_switch(&d, Switch::On).unwrap();
    assert_eq!(read_switch(&d), Switch::On);
    std::fs::write(d.join("screen-features"), "\u{0}\u{0}").unwrap();
    assert_eq!(read_switch(&d), Switch::Auto);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn room_for_an_app_needs_its_cost_above_the_free_floor() {
    assert!(room_for_app(mem(3.8, 2.0), AppClass::Heavy));
    assert!(!room_for_app(mem(3.8, 0.6), AppClass::Heavy));
    assert!(room_for_app(mem(3.8, 0.6), AppClass::Light));
    assert!(!room_for_app(mem(3.8, 0.5), AppClass::Light));
    assert!(room_for_app(MemFacts::default(), AppClass::Heavy), "unknown does not block opening an app");
}

#[test]
fn what_each_extra_display_and_pane_costs_matches_the_formula() {
    let c = layout_cost(HD, 1, &[], &[]);
    assert_eq!(c, 0, "one pane on one display adds nothing");
    let two = layout_cost(HD, 2, &[AppClass::Light, AppClass::Heavy], &[]);
    assert_eq!(two, 1920 * 1080 * 4 + 2 * SURFACE_OVERHEAD + COST_HEAVY);
    let ext = layout_cost(HD, 1, &[], &[UHD]);
    assert_eq!(ext, 3840 * 2160 * 4 * 2 + SURFACE_OVERHEAD);
}

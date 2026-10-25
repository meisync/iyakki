//! Several displays at the shell level (made-up outputs only): a workspace per display, the working display, the pointer's
//! display, moving an app between displays, hot-plug, pop-ups only on the working display, identify, and saved layouts.

use super::tests::*;
use super::*;
use crate::display::outputs::{hub, now_ms, DispSpec, DisplayConfig};
use crate::memgate::{MemFacts, GIB};
use crate::panes::{LayoutKey, Side};
use std::sync::atomic::{AtomicUsize, Ordering};

fn spec(name: &str, w: u32, h: u32, x: i32, y: i32) -> DispSpec {
    DispSpec { name: name.into(), w, h, x, y }
}

fn two() -> Vec<DispSpec> {
    vec![spec("eDP-1", 1280, 720, 0, 0), spec("HDMI-A-1", 1024, 600, 1280, 0)]
}

struct Probe {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
    renders: Arc<AtomicUsize>,
}

impl App for Probe {
    fn title(&self) -> &'static str {
        self.name
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [200, 60, 60, 255]
    }
    fn hint(&self) -> String {
        "probe".into()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        if !matches!(ev, Event::Tick | Event::MouseMove(..)) {
            self.log.lock().unwrap().push(format!("{ev:?}"));
        }
        Flow::Stay
    }
    fn render(&mut self, g: &mut Gfx, area: R, _s: f32, _cx: &AppCx, _tick: u64) {
        self.renders.fetch_add(1, Ordering::SeqCst);
        g.rect(area.x, area.y, area.w, area.h, [60, 60, 200, 255]);
        g.hit(area, Event::Tab(77), &format!("probe {}", self.name));
    }
}

fn probe(name: &'static str) -> (Box<dyn App>, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    (Box::new(Probe { name, log: log.clone(), renders: Arc::new(AtomicUsize::new(0)) }), log)
}

fn setup(tag: &str) -> (Shell, Gfx, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    let mut g0 = Gfx::new(1280, 720);
    sh.render(&mut g0);
    login(&mut sh, "alice", "alicepass1");
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    sh.set_displays(&two(), &DisplayConfig::default());
    let mut g1 = Gfx::new(1024, 600);
    frame(&mut sh, &mut g0, &mut g1);
    (sh, g0, g1)
}

/// What the backend does: the other displays first, the working one last.
fn frame(sh: &mut Shell, g0: &mut Gfx, g1: &mut Gfx) {
    let a = sh.displays_active_for_test();
    if a == 0 {
        sh.render_display(1, g1);
        sh.render_display(0, g0);
    } else {
        sh.render_display(0, g0);
        sh.render_display(1, g1);
    }
}

fn center(r: &R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

#[test]
fn every_display_gets_its_own_panes_bar_and_strip_and_one_display_is_as_before() {
    let (mut sh, mut g0, mut g1) = setup("pd1");
    assert_eq!(sh.display_count(), 2);
    assert_eq!(sh.display_layouts_for_test(), vec!["single", "single"]);
    for k in 0..2 {
        let labels: Vec<String> = sh.display_hits_for_test(k).iter().map(|h| h.label.clone()).collect();
        assert!(labels.iter().any(|l| l == "pane 1 move") && labels.iter().any(|l| l == "layout"), "display {k}: {labels:?}");
    }
    // each display is drawn at its own size and the pane fills its display
    assert_eq!(sh.display_rects_for_test(0).len(), 1);
    let (r0, r1) = (sh.display_rects_for_test(0)[0], sh.display_rects_for_test(1)[0]);
    assert!(r0.w > 1200.0 && r1.w > 960.0 && r1.w < 1024.0, "{r0:?} {r1:?}");
    // one display again: the shell is the single screen it was
    sh.set_displays(&two()[..1], &DisplayConfig::default());
    sh.render_display(0, &mut g0);
    assert_eq!(sh.display_count(), 1);
    assert!(sh.panes_focus_for_test().is_none(), "no workspace with one display and one pane");
    sh.render(&mut g1);
}

#[test]
fn a_click_on_the_other_display_makes_it_the_working_one_and_keys_go_to_its_app() {
    let (mut sh, mut g0, mut g1) = setup("pd2");
    let (a, la) = probe("ALPHA");
    let (b, lb) = probe("BRAVO");
    sh.display_put_for_test(0, 0, a, PApp::Settings);
    sh.display_put_for_test(1, 0, b, PApp::Settings);
    frame(&mut sh, &mut g0, &mut g1);
    assert_eq!(sh.displays_active_for_test(), 0);
    sh.handle(&Event::Key(Key::Char('x')));
    assert_eq!(la.lock().unwrap().len(), 1);
    // the pointer goes to display 2 and clicks the app there
    sh.set_pointer_display(1);
    sh.handle(&Event::MouseMove(500, 300));
    let r = sh.display_rects_for_test(1)[0];
    let (x, y) = center(&r);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    assert_eq!(sh.displays_active_for_test(), 1);
    frame(&mut sh, &mut g0, &mut g1);
    sh.handle(&Event::Key(Key::Char('y')));
    assert_eq!(la.lock().unwrap().len(), 1, "the first display's app did not get it");
    assert!(lb.lock().unwrap().iter().any(|l| l.contains("Char('y')")), "{:?}", lb.lock().unwrap());
    // the click reached the app under the pointer on display 2 in its own coordinates (a region event, not a position)
    assert!(lb.lock().unwrap().iter().any(|l| l == "Tab(77)"), "{:?}", lb.lock().unwrap());
    assert_eq!(sh.panes_titles_for_test(), vec!["BRAVO"]);
    assert_eq!(sh.display_titles_for_test(0), vec!["ALPHA"]);
    // the pointer on display 1 while display 2 works: moves are ignored there, they do not reach display 2's app
    let n = lb.lock().unwrap().len();
    sh.set_pointer_display(0);
    sh.handle(&Event::MouseMove(100, 100));
    assert_eq!(lb.lock().unwrap().len(), n);
}

#[test]
fn move_picks_an_app_up_and_a_click_on_a_pane_of_the_other_display_puts_it_there() {
    let (mut sh, mut g0, mut g1) = setup("pd3");
    let (a, _) = probe("ALPHA");
    sh.display_put_for_test(0, 0, a, PApp::Settings);
    frame(&mut sh, &mut g0, &mut g1);
    // Move on display 1's strip
    let h = sh.display_hits_for_test(0).iter().find(|h| h.label == "pane 1 move").unwrap().rect;
    let (x, y) = center(&h);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    frame(&mut sh, &mut g0, &mut g1);
    // display 2 offers itself as the target
    let r = sh.display_rects_for_test(1)[0];
    let txt: Vec<String> = sh.display_hits_for_test(1).iter().map(|h| h.label.clone()).collect();
    assert!(txt.iter().any(|l| l == "put here 1"), "{txt:?}");
    sh.set_pointer_display(1);
    let (x, y) = center(&r);
    sh.handle(&Event::MouseDown(x, y));
    assert_eq!(sh.display_titles_for_test(1), vec!["ALPHA"]);
    assert_eq!(sh.display_titles_for_test(0), vec!["Home"]);
    // Esc cancels a move
    frame(&mut sh, &mut g0, &mut g1);
    let h = sh.display_hits_for_test(sh.displays_active_for_test()).iter().find(|h| h.label == "pane 1 move").map(|h| h.rect);
    assert!(h.is_some());
}

#[test]
fn ctrl_alt_m_and_the_arrow_toward_a_display_move_the_working_app_there_and_the_keyboard_follows() {
    let (mut sh, mut g0, mut g1) = setup("pd4");
    let (a, la) = probe("ALPHA");
    sh.display_put_for_test(0, 0, a, PApp::Settings);
    frame(&mut sh, &mut g0, &mut g1);
    sh.handle(&Event::Layout(LayoutKey::NextDisplay));
    assert_eq!(sh.displays_active_for_test(), 1);
    assert_eq!(sh.display_titles_for_test(1), vec!["ALPHA"]);
    assert_eq!(sh.display_titles_for_test(0), vec!["Home"]);
    frame(&mut sh, &mut g0, &mut g1);
    sh.handle(&Event::Key(Key::Char('k')));
    assert_eq!(la.lock().unwrap().len(), 1);
    // back with the arrow toward display 1 (it is on the left of display 2)
    sh.handle(&Event::Layout(LayoutKey::Snap(Side::Left)));
    assert_eq!(sh.displays_active_for_test(), 0);
    assert_eq!(sh.display_titles_for_test(0), vec!["ALPHA"]);
    // no display above: the single layout makes a 50:50 pane layout instead
    sh.handle(&Event::Layout(LayoutKey::Snap(Side::Up)));
    assert_eq!(sh.panes_layout_for_test(), "rows-50-50");
    // with a pane that way, the arrow swaps panes and does not leave the display
    sh.handle(&Event::Layout(LayoutKey::Apply(2)));
    sh.handle(&Event::Layout(LayoutKey::Snap(Side::Right)));
    assert_eq!(sh.displays_active_for_test(), 0);
}

#[test]
fn pop_ups_cover_only_the_working_display() {
    let (mut sh, mut g0, mut g1) = setup("pd5");
    sh.handle(&Event::Layout(LayoutKey::Picker));
    frame(&mut sh, &mut g0, &mut g1);
    let on = |k: usize| sh.display_hits_for_test(k).iter().any(|h| h.label.starts_with("layout 50:50"));
    assert!(on(0), "the picker is on the working display");
    assert!(!on(1), "and not on the other");
    // the other display's bar and strip are still there and not dimmed by a backdrop
    assert!(sh.display_hits_for_test(1).iter().any(|h| h.label == "pane 1 move"));
    // F1 help covers the working display only: the other display's picture does not change
    sh.handle(&Event::Key(Key::Esc));
    frame(&mut sh, &mut g0, &mut g1);
    let before = g1.pm.data().to_vec();
    sh.handle(&Event::Key(Key::F(1)));
    frame(&mut sh, &mut g0, &mut g1);
    assert!(g1.pm.data()[1024 * 60 * 4..] == before[1024 * 60 * 4..], "the other display is untouched by the help popup (below its bar, whose clock may move on)");
}

#[test]
fn identify_puts_a_big_number_on_every_display_for_a_few_seconds() {
    let (mut sh, mut g0, mut g1) = setup("pd6");
    let before = (g0.pm.data().to_vec(), g1.pm.data().to_vec());
    hub().identify(now_ms() + 3000);
    frame(&mut sh, &mut g0, &mut g1);
    assert_ne!(g0.pm.data(), &before.0[..]);
    assert_ne!(g1.pm.data(), &before.1[..]);
    hub().identify(0);
    frame(&mut sh, &mut g0, &mut g1);
    // below the bar (its clock may have moved on a minute)
    let skip = 1024 * 60;
    let diff = g1.pm.data().chunks(4).zip(before.1.chunks(4)).skip(skip).filter(|(a, b)| a != b).count();
    let first = g1.pm.data().chunks(4).zip(before.1.chunks(4)).skip(skip).position(|(a, b)| a != b);
    assert_eq!(diff, 0, "back to the normal picture; {diff} pixels differ, the first at {first:?}");
}

#[test]
fn each_displays_layout_is_remembered_and_comes_back_with_the_display() {
    let (mut sh, mut g0, mut g1) = setup("pd7");
    // 50:50 on display 1, 2x2 on display 2
    sh.handle(&Event::Layout(LayoutKey::Apply(2)));
    sh.set_pointer_display(1);
    sh.handle(&Event::MouseDown(300, 300));
    assert_eq!(sh.displays_active_for_test(), 1);
    sh.handle(&Event::Layout(LayoutKey::Apply(7)));
    frame(&mut sh, &mut g0, &mut g1);
    assert_eq!(sh.display_layouts_for_test(), vec!["cols-50-50", "grid"]);
    let dir = sh.env.services.dir().to_path_buf();
    let saved = crate::display::outputs::load(&dir, "alice");
    assert_eq!(saved.template_of("eDP-1"), Some("cols-50-50"));
    assert_eq!(saved.template_of("HDMI-A-1"), Some("grid"));
    // unplug the monitor: its apps keep a place on the first display, then plug it in again: its layout is back
    let (b, _) = probe("BRAVO");
    sh.display_put_for_test(1, 0, b, PApp::Settings);
    sh.set_displays(&two()[..1], &saved);
    assert_eq!(sh.display_count(), 1);
    assert!(sh.panes_titles_for_test().contains(&"BRAVO".to_string()), "{:?}", sh.panes_titles_for_test());
    sh.set_displays(&two(), &saved);
    frame(&mut sh, &mut g0, &mut g1);
    assert_eq!(sh.display_layouts_for_test(), vec!["cols-50-50", "grid"]);
}

#[test]
fn a_display_that_goes_away_gives_its_apps_to_launcher_panes_and_the_working_display_follows() {
    let (mut sh, mut g0, mut g1) = setup("pd8");
    let three = vec![spec("eDP-1", 1280, 720, 0, 0), spec("HDMI-A-1", 1024, 600, 1280, 0), spec("DP-2", 1024, 600, 2304, 0)];
    sh.set_displays(&three, &DisplayConfig::default());
    let (b, _) = probe("BRAVO");
    let (c, _) = probe("CHARLIE");
    sh.display_put_for_test(1, 0, b, PApp::Settings);
    sh.display_put_for_test(2, 0, c, PApp::Settings);
    sh.set_pointer_display(1);
    sh.handle(&Event::MouseDown(300, 300));
    assert_eq!(sh.displays_active_for_test(), 1);
    // display 2 (the working one) is unplugged
    sh.set_displays(&[three[0].clone(), three[2].clone()], &DisplayConfig::default());
    assert_eq!(sh.display_count(), 2);
    assert_eq!(sh.displays_active_for_test(), 0, "the first display takes the keyboard");
    let all: Vec<String> = (0..2).flat_map(|k| sh.display_titles_for_test(k)).collect();
    assert!(all.contains(&"CHARLIE".to_string()), "{all:?}");
    assert!(all.contains(&"BRAVO".to_string()) || sh.toast.as_ref().is_some_and(|t| t.0.contains("no room")), "{all:?} {:?}", sh.toast);
    frame(&mut sh, &mut g0, &mut g1);
}

#[test]
fn apps_keep_their_heartbeat_on_every_display() {
    let (mut sh, mut g0, mut g1) = setup("pd9");
    struct Ticker(Arc<AtomicUsize>);
    impl App for Ticker {
        fn title(&self) -> &'static str {
            "TICK"
        }
        fn icon(&self) -> Icon {
            Icon::Chat
        }
        fn accent(&self) -> [u8; 4] {
            [1, 2, 3, 255]
        }
        fn hint(&self) -> String {
            String::new()
        }
        fn handle(&mut self, ev: &Event, _: &AppCx) -> Flow {
            if matches!(ev, Event::Tick) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            Flow::Stay
        }
        fn render(&mut self, _: &mut Gfx, _: R, _: f32, _: &AppCx, _: u64) {}
    }
    let n = Arc::new(AtomicUsize::new(0));
    sh.display_put_for_test(1, 0, Box::new(Ticker(n.clone())), PApp::Settings);
    frame(&mut sh, &mut g0, &mut g1);
    for _ in 0..4 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(n.load(Ordering::SeqCst), 4);
}

#[test]
fn displays_known_before_sign_in_get_their_workspaces_and_saved_layouts_at_sign_in() {
    let (mut sh, _) = shell_with_users("pd10");
    let (mut g0, mut g1) = (Gfx::new(1280, 720), Gfx::new(1024, 600));
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    // two displays at the sign-in screen: the same picture on both (one shell, no panes yet)
    sh.set_displays(&two(), &DisplayConfig::default());
    sh.render_display(1, &mut g1);
    sh.render_display(0, &mut g0);
    assert!(sh.panes_focus_for_test().is_none());
    // the person had a layout on the second display last time
    let mut cfg = DisplayConfig::default();
    cfg.set_template("HDMI-A-1", "cols-30-70");
    crate::display::outputs::save(sh.env.services.dir(), "alice", &cfg).unwrap();
    login(&mut sh, "alice", "alicepass1");
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    sh.render_display(1, &mut g1);
    sh.render_display(0, &mut g0);
    assert_eq!(sh.display_layouts_for_test(), vec!["single", "cols-30-70"]);
    assert!(sh.display_hits_for_test(1).iter().any(|h| h.label == "pane 2 move"));
    // sign out and in again: workspaces are made again, nothing is left over
    sh.sign_out();
    sh.render_display(1, &mut g1);
    sh.render_display(0, &mut g0);
    assert_eq!(sh.screen_name(), "login");
    login(&mut sh, "alice", "alicepass1");
    sh.set_mem_for_test(Some(MemFacts { total: 16 * GIB, available: 12 * GIB }));
    sh.render_display(1, &mut g1);
    sh.render_display(0, &mut g0);
    assert_eq!(sh.display_layouts_for_test(), vec!["single", "cols-30-70"]);
}

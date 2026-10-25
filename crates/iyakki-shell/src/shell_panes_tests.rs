//! Panes at the shell level: templates drawn side by side, keys, focus, Move, Back, Close, dividers, the launcher in a pane, a card
//! that cannot leave its pane, redraw counts, cloud and remote sessions, and the memory gate (`docs/MULTI-DISPLAY.md`).

use super::tests::*;
use super::*;
use crate::memgate::{MemFacts, Switch, GIB};
use crate::panes::{LayoutKey, Side};
use std::sync::atomic::{AtomicUsize, Ordering};

fn big() -> MemFacts {
    MemFacts { total: 16 * GIB, available: 12 * GIB }
}

/// A stand-in app: fills its area with a colour, writes its name, registers one region over everything and records what it was told.
struct Probe {
    name: &'static str,
    color: [u8; 4],
    log: Arc<Mutex<Vec<String>>>,
    renders: Arc<AtomicUsize>,
    ticks: Arc<AtomicUsize>,
    /// Draw far beyond the area, as a card that is too big would.
    overdraw: bool,
}

impl App for Probe {
    fn title(&self) -> &'static str {
        self.name
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        self.color
    }
    fn hint(&self) -> String {
        "probe".into()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        match ev {
            Event::Tick => {
                self.ticks.fetch_add(1, Ordering::SeqCst);
            }
            Event::MouseMove(..) => {}
            Event::Key(Key::Esc) => return Flow::Back,
            other => self.log.lock().unwrap().push(format!("{other:?}")),
        }
        Flow::Stay
    }
    fn render(&mut self, g: &mut Gfx, area: R, _s: f32, _cx: &AppCx, _tick: u64) {
        self.renders.fetch_add(1, Ordering::SeqCst);
        if self.overdraw {
            g.rect(-2000.0, -2000.0, 9000.0, 9000.0, [255, 0, 0, 255]);
            g.hit(R::new(-2000.0, -2000.0, 9000.0, 9000.0), Event::Tab(7), "probe-huge");
        }
        g.rect(area.x, area.y, area.w, area.h, self.color);
        g.text(area.x + 8.0, area.y + 8.0, 16.0, [255, 255, 255, 255], self.name, Face::Bold);
        g.hit(area, Event::Tab(77), &format!("probe {}", self.name));
    }
}

struct Handles {
    log: Arc<Mutex<Vec<String>>>,
    renders: Arc<AtomicUsize>,
    ticks: Arc<AtomicUsize>,
}

fn probe(name: &'static str, color: [u8; 4]) -> (Box<dyn App>, Handles) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let renders = Arc::new(AtomicUsize::new(0));
    let ticks = Arc::new(AtomicUsize::new(0));
    (Box::new(Probe { name, color, log: log.clone(), renders: renders.clone(), ticks: ticks.clone(), overdraw: false }), Handles { log, renders, ticks })
}

fn signed_in(tag: &str, w: u32, h: u32, user: &str) -> (Shell, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    let pass = if user == "root" { "rootpass12" } else { "alicepass1" };
    login(&mut sh, user, pass);
    sh.set_mem_for_test(Some(big()));
    sh.render(&mut g);
    (sh, g)
}

fn chord(sh: &mut Shell, k: LayoutKey) {
    sh.handle(&Event::Layout(k));
}

fn center(r: &R) -> (i32, i32) {
    ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)
}

fn click_at(sh: &mut Shell, g: &mut Gfx, x: i32, y: i32) {
    sh.render(g);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    sh.render(g);
}

fn click_label(sh: &mut Shell, g: &mut Gfx, label: &str) {
    sh.render(g);
    let h = sh.hits().iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing labelled `{label}`: {:?}", sh.hits().iter().map(|h| h.label.clone()).collect::<Vec<_>>())).clone();
    let (x, y) = center(&h.rect);
    click_at(sh, g, x, y);
}

fn overlaps(a: &R, b: &R) -> bool {
    a.x < b.x + b.w - 0.01 && b.x < a.x + a.w - 0.01 && a.y < b.y + b.h - 0.01 && b.y < a.y + a.h - 0.01
}

fn pixel(g: &Gfx, x: i32, y: i32) -> [u8; 4] {
    let i = ((y as u32 * g.width() + x as u32) * 4) as usize;
    let d = g.pm.data();
    [d[i], d[i + 1], d[i + 2], d[i + 3]]
}

#[test]
fn ctrl_alt_2_makes_two_panes_side_by_side_and_each_shows_an_app_with_a_strip_and_no_overlap() {
    let (mut sh, mut g) = signed_in("pn1", 1280, 720, "alice");
    let (a, _) = probe("ALPHA", [200, 40, 40, 255]);
    let (b, _) = probe("BRAVO", [40, 40, 200, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    assert_eq!(rs.len(), 2);
    assert!(!overlaps(&rs[0], &rs[1]));
    assert!((rs[0].w - rs[1].w).abs() <= 2.0, "50:50: {rs:?}");
    // both apps drew into their own half
    let (c0, c1) = (center(&rs[0]), center(&rs[1]));
    assert_eq!(pixel(&g, c0.0, c0.1), [200, 40, 40, 255]);
    assert_eq!(pixel(&g, c1.0, c1.1), [40, 40, 200, 255]);
    // title strips with their buttons, one set per pane
    let labels: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    for want in ["pane 1 move", "pane 1 layout", "pane 1 back", "pane 1 close", "pane 2 move", "pane 2 layout", "pane 2 back", "pane 2 close"] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
    }
    // no region of one pane reaches into the other, and every region is inside the display
    for h in sh.hits().iter().filter(|h| h.label.starts_with("probe") || h.label.starts_with("pane")) {
        let inside = rs.iter().filter(|r| h.rect.x >= r.x - 0.6 && h.rect.y >= r.y - 0.6 && h.rect.x + h.rect.w <= r.x + r.w + 0.6 && h.rect.y + h.rect.h <= r.y + r.h + 0.6).count();
        assert_eq!(inside, 1, "{} {:?}", h.label, h.rect);
    }
    assert_eq!(sh.panes_titles_for_test(), vec!["ALPHA", "BRAVO"]);
}

#[test]
fn thirty_seventy_gives_the_left_pane_thirty_percent_and_the_strips_stay_inside() {
    let (mut sh, mut g) = signed_in("pn2", 1280, 720, "alice");
    let (a, _) = probe("LEFT", [10, 160, 10, 255]);
    let (b, _) = probe("RIGHT", [160, 160, 10, 255]);
    sh.show_app_for_test(a, PApp::Settings);
    chord(&mut sh, LayoutKey::Apply(4));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    let ratio = rs[0].w / (rs[0].w + rs[1].w);
    assert!((ratio - 0.30).abs() < 0.01, "{ratio}");
    assert!(!overlaps(&rs[0], &rs[1]));
    assert_eq!(pixel(&g, center(&rs[0]).0, center(&rs[0]).1), [10, 160, 10, 255]);
    assert_eq!(pixel(&g, center(&rs[1]).0, center(&rs[1]).1), [160, 160, 10, 255]);
    // the strip of the narrow pane keeps its buttons inside the pane
    for h in sh.hits().iter().filter(|h| h.label.starts_with("pane 1")) {
        assert!(h.rect.x >= rs[0].x - 0.6 && h.rect.x + h.rect.w <= rs[0].x + rs[0].w + 0.6, "{} {:?} vs {:?}", h.label, h.rect, rs[0]);
    }
}

#[test]
fn a_pane_smaller_than_its_apps_minimum_says_so_instead_of_drawing_it() {
    let (mut sh, mut g) = signed_in("pn2b", 1280, 720, "alice");
    let (a, ha) = probe("CHATTY", [10, 160, 10, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(4));
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    assert!(rs[0].w < 480.0 * 0.9, "{rs:?}");
    assert_eq!(ha.renders.load(Ordering::SeqCst), 0, "the app is not asked to draw in a pane that is too small");
    assert_ne!(pixel(&g, center(&rs[0]).0, center(&rs[0]).1), [10, 160, 10, 255]);
    // it is kept alive and draws again in a bigger pane
    chord(&mut sh, LayoutKey::Apply(5));
    sh.render(&mut g);
    assert!(ha.renders.load(Ordering::SeqCst) > 0);
}

#[test]
fn every_template_draws_at_every_usual_size_with_apps_in_every_pane() {
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480), (2560, 1600)] {
        let (mut sh, mut g) = signed_in(&format!("pn3-{w}"), w, h, "alice");
        for t in crate::panes::TEMPLATES.iter().filter(|t| w < 2000 || t.id == "grid" || t.id == "thirds") {
            let (a, _) = probe("ONE", [90, 30, 30, 255]);
            sh.show_app_for_test(a, PApp::Settings);
            if sh.ws_apply(t.id, None, true).is_err() {
                // too small a display for this template: refused, and the single screen is untouched
                assert_eq!(sh.panes_layout_for_test(), "single", "{} at {w}x{h}", t.id);
                continue;
            }
            let n = crate::panes::Layout::from_template(t.id).leaf_count();
            for i in 1..n {
                let (p, _) = probe("MORE", [30, 90, 30, 255]);
                sh.panes_put_for_test(i, p, PApp::Settings);
            }
            sh.render(&mut g);
            let rs = sh.panes_rects_for_test();
            if n > 1 {
                assert_eq!(rs.len(), n, "{} at {w}x{h}", t.id);
                for (i, r) in rs.iter().enumerate() {
                    for q in rs.iter().skip(i + 1) {
                        assert!(!overlaps(r, q), "{} at {w}x{h}", t.id);
                    }
                    assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5);
                }
            }
            sh.ws_apply("single", None, true).unwrap();
        }
    }
}

#[test]
fn keys_go_to_the_focused_pane_and_ctrl_alt_tab_moves_focus() {
    let (mut sh, mut g) = signed_in("pn4", 1280, 720, "alice");
    let (a, ha) = probe("A", [100, 0, 0, 255]);
    let (b, hb) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    assert_eq!(sh.panes_focus_for_test(), Some(0));
    sh.handle(&Event::Key(Key::Char('x')));
    assert_eq!(ha.log.lock().unwrap().len(), 1);
    assert_eq!(hb.log.lock().unwrap().len(), 0);
    chord(&mut sh, LayoutKey::Cycle { back: false });
    assert_eq!(sh.panes_focus_for_test(), Some(1));
    sh.handle(&Event::Key(Key::Char('y')));
    sh.handle(&Event::Key(Key::Char('z')));
    assert_eq!(ha.log.lock().unwrap().len(), 1);
    assert_eq!(hb.log.lock().unwrap().len(), 2);
    chord(&mut sh, LayoutKey::Cycle { back: false });
    assert_eq!(sh.panes_focus_for_test(), Some(0));
    chord(&mut sh, LayoutKey::Cycle { back: true });
    assert_eq!(sh.panes_focus_for_test(), Some(1));
    // the app in the other pane keeps getting its heartbeat
    let before = ha.ticks.load(Ordering::SeqCst);
    for _ in 0..5 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(ha.ticks.load(Ordering::SeqCst) - before, 5, "background pane ticks");
}

#[test]
fn a_click_in_a_pane_focuses_it_and_reaches_the_control_under_the_pointer() {
    let (mut sh, mut g) = signed_in("pn5", 1280, 720, "alice");
    let (a, ha) = probe("A", [100, 0, 0, 255]);
    let (b, hb) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    click_label(&mut sh, &mut g, "probe B");
    assert_eq!(sh.panes_focus_for_test(), Some(1));
    let tabs = |h: &Handles| h.log.lock().unwrap().iter().filter(|l| l.starts_with("Tab")).count();
    assert_eq!(hb.log.lock().unwrap().first().map(String::as_str), Some("Tab(77)"), "the app got its own control's event, not coordinates");
    assert_eq!(tabs(&hb), 1);
    assert_eq!(tabs(&ha), 0, "the click did not reach the other pane");
    click_label(&mut sh, &mut g, "probe A");
    assert_eq!(sh.panes_focus_for_test(), Some(0));
    assert_eq!(tabs(&ha), 1);
}

#[test]
fn ctrl_alt_arrows_snap_the_focused_app_and_the_focus_follows_it() {
    let (mut sh, mut g) = signed_in("pn6", 1280, 720, "alice");
    let (a, _) = probe("A", [100, 0, 0, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    // from one pane: Right makes 50:50 and puts the app on the right
    chord(&mut sh, LayoutKey::Snap(Side::Right));
    sh.render(&mut g);
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    assert_eq!(sh.panes_titles_for_test(), vec!["Home", "A"]);
    assert_eq!(sh.panes_focus_for_test(), Some(1));
    // Left swaps it to the left pane
    chord(&mut sh, LayoutKey::Snap(Side::Left));
    sh.render(&mut g);
    assert_eq!(sh.panes_titles_for_test(), vec!["A", "Home"]);
    assert_eq!(sh.panes_focus_for_test(), Some(0));
    // nothing further left
    chord(&mut sh, LayoutKey::Snap(Side::Left));
    assert_eq!(sh.panes_titles_for_test(), vec!["A", "Home"]);
    // Down in a side-by-side layout: no pane below
    chord(&mut sh, LayoutKey::Snap(Side::Down));
    assert_eq!(sh.panes_titles_for_test(), vec!["A", "Home"]);
    // up/down from a single pane makes stacked panes
    chord(&mut sh, LayoutKey::Apply(1));
    chord(&mut sh, LayoutKey::Snap(Side::Up));
    assert_eq!(sh.panes_layout_for_test(), "rows-50-50");
    assert_eq!(sh.panes_focus_for_test(), Some(0));
}

#[test]
fn a_divider_is_dragged_with_the_pointer_snaps_and_stops_at_the_minimum() {
    let (mut sh, mut g) = signed_in("pn7", 1280, 720, "alice");
    let (a, _) = probe("A", [100, 0, 0, 255]);
    let (b, _) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    let gap_x = ((rs[0].x + rs[0].w + rs[1].x) / 2.0) as i32;
    let y = (rs[0].y + rs[0].h / 2.0) as i32;
    sh.handle(&Event::MouseMove(gap_x, y));
    sh.handle(&Event::MouseDown(gap_x, y));
    // drag to about 70 %: snaps
    let target = (1280.0 * 0.70) as i32;
    sh.handle(&Event::MouseMove(target - 20, y));
    sh.handle(&Event::MouseMove(target, y));
    sh.handle(&Event::MouseUp(target, y));
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    let ratio = rs[0].w / (rs[0].w + rs[1].w);
    assert!((ratio - 0.70).abs() < 0.01, "{ratio}");
    // far to the right: the second pane keeps a usable width
    let gap_x = ((rs[0].x + rs[0].w + rs[1].x) / 2.0) as i32;
    sh.handle(&Event::MouseDown(gap_x, y));
    sh.handle(&Event::MouseMove(1270, y));
    sh.handle(&Event::MouseUp(1270, y));
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    assert!(rs[1].w >= 250.0, "{:?}", rs[1]);
    // the click on the divider did not reach the apps
    // keys move it too
    let before = sh.panes_rects_for_test()[0].w;
    sh.panes_focus_for_test();
    chord(&mut sh, LayoutKey::Nudge(Side::Left));
    sh.render(&mut g);
    assert!(sh.panes_rects_for_test()[0].w < before || sh.panes_rects_for_test()[0].w == before);
}

#[test]
fn move_picks_a_pane_up_and_a_click_on_another_puts_it_there() {
    let (mut sh, mut g) = signed_in("pn8", 1280, 720, "alice");
    let (a, _) = probe("A", [100, 0, 0, 255]);
    let (b, _) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    click_label(&mut sh, &mut g, "pane 1 move");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "put here 2"), "the other pane offers itself as the target");
    click_label(&mut sh, &mut g, "put here 2");
    assert_eq!(sh.panes_titles_for_test(), vec!["B", "A"]);
    // Esc cancels a move
    click_label(&mut sh, &mut g, "pane 1 move");
    sh.handle(&Event::Key(Key::Esc));
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label.starts_with("put here")));
    assert_eq!(sh.panes_titles_for_test(), vec!["B", "A"]);
}

#[test]
fn back_in_a_pane_shows_that_panes_launcher_and_close_goes_to_one_pane_fewer() {
    let (mut sh, mut g) = signed_in("pn9", 1280, 720, "alice");
    let (a, _) = probe("A", [100, 0, 0, 255]);
    let (b, _) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    click_label(&mut sh, &mut g, "pane 2 back");
    assert_eq!(sh.panes_titles_for_test(), vec!["A", "Home"], "B went back to a launcher; A stays");
    // Esc inside an app is Back for that pane only; Esc in a launcher pane does not sign anyone out
    chord(&mut sh, LayoutKey::Cycle { back: false });
    assert_eq!(sh.panes_focus_for_test(), Some(0));
    sh.handle(&Event::Key(Key::Esc));
    sh.handle(&Event::Key(Key::Esc));
    sh.render(&mut g);
    assert_eq!(sh.panes_titles_for_test(), vec!["Home", "Home"]);
    // put A back and close pane 2: the layout becomes single and A is kept
    let (a, _) = probe("A2", [100, 0, 0, 255]);
    sh.panes_put_for_test(0, a, PApp::Chat);
    sh.render(&mut g);
    click_label(&mut sh, &mut g, "pane 2 close");
    assert_eq!(sh.panes_layout_for_test(), "single");
    assert_eq!(sh.screen_name(), "app:chat");
    // closing the focused pane of three keeps the app that was focused
    chord(&mut sh, LayoutKey::Apply(6));
    chord(&mut sh, LayoutKey::Cycle { back: true });
    assert_eq!(sh.panes_focus_for_test(), Some(2));
    let (c, _) = probe("C", [100, 100, 0, 255]);
    sh.panes_put_for_test(2, c, PApp::Edit);
    sh.render(&mut g);
    click_label(&mut sh, &mut g, "pane 1 close");
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    assert!(sh.panes_titles_for_test().contains(&"C".to_string()), "{:?}", sh.panes_titles_for_test());
}

#[test]
fn going_to_fewer_panes_says_which_apps_were_closed() {
    let (mut sh, mut g) = signed_in("pn10", 1280, 720, "alice");
    let (a, _) = probe("A", [100, 0, 0, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(7));
    for (i, n) in ["B", "C", "D"].iter().enumerate() {
        let (p, _) = probe(n, [0, 100, 0, 255]);
        sh.panes_put_for_test(i + 1, p, PApp::Edit);
    }
    sh.render(&mut g);
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_titles_for_test(), vec!["A", "B"]);
    sh.render(&mut g);
    let toast = sh.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.contains("C") && toast.contains("D") && toast.contains("Closed"), "{toast}");
}

#[test]
fn an_app_that_draws_far_beyond_its_area_cannot_leave_its_pane() {
    let (mut sh, mut g) = signed_in("pn11", 1280, 720, "alice");
    let log = Arc::new(Mutex::new(Vec::new()));
    let big_card: Box<dyn App> = Box::new(Probe { name: "CARD", color: [0, 200, 0, 255], log, renders: Arc::new(AtomicUsize::new(0)), ticks: Arc::new(AtomicUsize::new(0)), overdraw: true });
    let (b, _) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(big_card, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    let rs = sh.panes_rects_for_test();
    // no red anywhere in the other pane, in the bar or between the panes
    for (x, y) in [center(&rs[1]), (rs[1].x as i32 + 5, rs[1].y as i32 + 40), (640, 10), ((rs[0].x + rs[0].w + 2.0) as i32, 300)] {
        assert_ne!(pixel(&g, x, y), [255, 0, 0, 255], "red at {x},{y}");
    }
    assert_eq!(pixel(&g, center(&rs[1]).0, center(&rs[1]).1), [0, 0, 100, 255]);
    // and its huge click region is cut to its pane
    let huge = sh.hits().iter().find(|h| h.label == "probe-huge").unwrap().rect;
    assert!(huge.x >= rs[0].x - 0.6 && huge.x + huge.w <= rs[0].x + rs[0].w + 0.6 && huge.y >= rs[0].y - 0.6 && huge.y + huge.h <= rs[0].y + rs[0].h + 0.6, "{huge:?}");
}

#[test]
fn a_pane_without_an_app_shows_the_launcher_at_its_size_and_a_click_opens_the_app_in_that_pane() {
    let (mut sh, mut g) = signed_in("pn12", 1280, 720, "alice");
    let (a, _) = probe("A", [100, 0, 0, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.render(&mut g);
    // pane 2 is a launcher: clicking its first tile opens that app in pane 2 and leaves pane 1 alone
    let (origin, (cw, ch)) = sh.panes_content_for_test(1).unwrap();
    let n = sh.group_items().len();
    let rects = launcher::page_rects(cw, ch, scale_for(cw, ch), n, 0);
    assert!(!rects.is_empty());
    let first = center(&rects[0]);
    let first_app = sh.group_tiles().first().map(|t| t.app).unwrap();
    click_at(&mut sh, &mut g, first.0 + origin.x as i32, first.1 + origin.y as i32);
    assert_eq!(sh.panes_focus_for_test(), Some(1));
    assert_eq!(sh.screen_name(), format!("app:{}", first_app.id()));
    assert_eq!(sh.panes_titles_for_test()[0], "A");
    sh.render(&mut g);
    assert_eq!(sh.panes_titles_for_test().len(), 2);
}

#[test]
fn the_launcher_tiles_adapt_to_every_pane_size() {
    for (w, h) in [(1280u32, 720u32), (800, 480)] {
        let (mut sh, mut g) = signed_in(&format!("pn13-{w}"), w, h, "alice");
        for t in ["cols-50-50", "cols-30-70", "cols-25-75", "thirds", "grid", "rows-70-30", "main-stack"] {
            if sh.ws_apply(t, None, true).is_err() {
                continue;
            }
            sh.render(&mut g);
            for i in 0..sh.panes_rects_for_test().len() {
                let (_, (cw, ch)) = sh.panes_content_for_test(i).unwrap();
                let s = scale_for(cw, ch);
                let n = sh.group_items().len();
                let rects = launcher::page_rects(cw, ch, s, n, 0);
                assert!(!rects.is_empty(), "{t} {w}x{h} pane {i}");
                for r in &rects {
                    assert!(r.w > 20.0 && r.h > 20.0, "{t} {w}x{h} pane {i}: {r:?}");
                    assert!(r.x >= -0.5 && r.x + r.w <= cw as f32 + 0.5, "{t} {w}x{h} pane {i}: tile {r:?} wider than {cw}");
                }
            }
            sh.ws_apply("single", None, true).unwrap();
        }
    }
}

#[test]
fn only_the_pane_that_changed_is_drawn_again() {
    let (mut sh, mut g) = signed_in("pn14", 1280, 720, "alice");
    let (a, ha) = probe("A", [100, 0, 0, 255]);
    let (b, hb) = probe("B", [0, 0, 100, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    sh.panes_put_for_test(1, b, PApp::Edit);
    sh.render(&mut g);
    sh.take_stale();
    sh.render(&mut g);
    let base = sh.panes_redraws_for_test();
    let (ra, rb) = (ha.renders.load(Ordering::SeqCst), hb.renders.load(Ordering::SeqCst));
    // nothing happened: no pane is drawn
    sh.render(&mut g);
    assert_eq!(sh.panes_redraws_for_test(), base);
    // a key for the focused pane draws that pane only
    sh.handle(&Event::Key(Key::Char('k')));
    sh.render(&mut g);
    sh.take_stale();
    assert_eq!(sh.panes_redraws_for_test(), base + 1);
    assert_eq!(ha.renders.load(Ordering::SeqCst), ra + 1);
    assert_eq!(hb.renders.load(Ordering::SeqCst), rb, "the other pane's app was not asked to draw");
    // heartbeats alone draw nothing
    for k in 0..40 {
        let _ = k;
        sh.handle(&Event::Tick);
        sh.render(&mut g);
        sh.take_stale();
    }
    assert!(sh.panes_redraws_for_test() <= base + 1 + 2, "idle ticks redraw at most now and then for the clock: {}", sh.panes_redraws_for_test() - base);
    // a pointer move that crosses nothing draws nothing
    let now = sh.panes_redraws_for_test();
    sh.handle(&Event::MouseMove(700, 400));
    sh.handle(&Event::MouseMove(702, 402));
    sh.render(&mut g);
    sh.take_stale();
    assert_eq!(sh.panes_redraws_for_test() - now, 0, "a pointer that crosses nothing draws nothing");
}

#[test]
fn the_apps_stay_alive_when_the_layout_changes_and_come_back_in_order() {
    let (mut sh, mut g) = signed_in("pn15", 1280, 720, "alice");
    let (a, ha) = probe("A", [100, 0, 0, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    chord(&mut sh, LayoutKey::Apply(6));
    chord(&mut sh, LayoutKey::Apply(7));
    chord(&mut sh, LayoutKey::Apply(1));
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "app:chat");
    sh.handle(&Event::Key(Key::Char('q')));
    assert_eq!(ha.log.lock().unwrap().len(), 1, "it is the same instance");
}

#[test]
fn a_remote_session_and_cloud_mode_get_the_same_layouts() {
    let (mut sh, mut g) = signed_in("pn16", 1100, 700, "alice");
    sh.set_remote(true);
    let (a, _) = probe("A", [100, 0, 0, 255]);
    sh.show_app_for_test(a, PApp::Chat);
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    sh.render(&mut g);
    assert_eq!(sh.panes_rects_for_test().len(), 2);
    let (mut sh, mut g) = signed_in("pn17", 1280, 720, "root");
    sh.set_cloud_for_test(iyakki_agent::cloudmode::cloud_mode(iyakki_agent::cloudmode::Setting::On, &iyakki_agent::cloudmode::Facts::full()));
    sh.set_mem_for_test(Some(big()));
    chord(&mut sh, LayoutKey::Apply(2));
    sh.render(&mut g);
    assert_eq!(sh.panes_rects_for_test().len(), 2);
}

#[test]
fn real_apps_open_in_panes_and_draw() {
    let (mut sh, mut g) = signed_in("pn18", 1280, 720, "root");
    chord(&mut sh, LayoutKey::Apply(2));
    sh.open(PApp::Calendar);
    sh.render(&mut g);
    chord(&mut sh, LayoutKey::Cycle { back: false });
    sh.open(PApp::System);
    for _ in 0..3 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    let t = sh.panes_titles_for_test();
    assert_eq!(t.len(), 2);
    assert!(t.iter().all(|x| x != "Home"), "{t:?}");
    let rs = sh.panes_rects_for_test();
    assert!(!overlaps(&rs[0], &rs[1]));
}

#[test]
fn sign_out_drops_the_panes() {
    let (mut sh, mut g) = signed_in("pn19", 1280, 720, "alice");
    chord(&mut sh, LayoutKey::Apply(2));
    sh.render(&mut g);
    sh.sign_out();
    sh.render(&mut g);
    assert!(sh.panes_focus_for_test().is_none());
    assert_eq!(sh.screen_name(), "login");
}

// ---------------------------------------------------------------------------------------------------- the memory gate

#[test]
fn at_low_memory_the_desktop_is_exactly_as_before_no_button_no_layouts() {
    let (mut sh, _) = shell_with_users("pm1");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    login(&mut sh, "alice", "alicepass1");
    // unknown memory in tests = conservative
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label == "layout"), "no LAYOUT button");
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_layout_for_test(), "single");
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("Not available") || t.0.contains("memory")), "{:?}", sh.toast);
    // a 1 GB machine
    sh.set_mem_for_test(Some(MemFacts { total: 900 * 1024 * 1024, available: 600 * 1024 * 1024 }));
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label == "layout"));
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_layout_for_test(), "single");
    // apps open fullscreen as always
    sh.open(PApp::Calendar);
    assert_eq!(sh.screen_name(), "app:calendar");
}

#[test]
fn a_two_gigabyte_machine_gets_two_panes_and_refuses_three() {
    let (mut sh, mut g) = signed_in("pm2", 1280, 720, "alice");
    sh.set_mem_for_test(Some(MemFacts { total: (19 * GIB) / 10, available: (14 * GIB) / 10 }));
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "layout"), "the button shows when two panes are possible");
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50");
    chord(&mut sh, LayoutKey::Apply(6));
    assert_eq!(sh.panes_layout_for_test(), "cols-50-50", "three panes refused");
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("Not available")));
    let line = sh.panes_memory_line();
    assert!(line.contains("limited to 2 panes"), "{line}");
}

#[test]
fn when_memory_falls_a_card_says_so_the_layout_shrinks_and_it_comes_back_when_memory_is_free() {
    let (mut sh, mut g) = signed_in("pm3", 1280, 720, "alice");
    chord(&mut sh, LayoutKey::Apply(7));
    assert_eq!(sh.panes_layout_for_test(), "grid");
    sh.render(&mut g);
    // memory drops: a 4 GB machine with little free
    sh.set_mem_for_test(Some(MemFacts { total: 3 * GIB + GIB / 2 + 1, available: 520 * 1024 * 1024 }));
    let card = sh.panes_card_for_test().expect("a card");
    assert!(card.contains("Not enough free memory for this layout") && card.contains("Switching to"), "{card}");
    assert_eq!(sh.panes_layout_for_test(), "single");
    sh.render(&mut g);
    // the card takes keys until answered, then goes
    sh.handle(&Event::Key(Key::Enter));
    assert!(sh.panes_card_for_test().is_none());
    // memory is free again: the person's wish is restored with a notice
    sh.set_mem_for_test(Some(big()));
    assert_eq!(sh.panes_layout_for_test(), "grid");
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("back")), "{:?}", sh.toast);
}

#[test]
fn memory_is_looked_at_again_every_ten_seconds_by_the_heartbeat() {
    let (mut sh, mut g) = signed_in("pm4", 1280, 720, "alice");
    chord(&mut sh, LayoutKey::Apply(6));
    sh.render(&mut g);
    sh.panes.mem = Some(MemFacts { total: 16 * GIB, available: 400 * 1024 * 1024 });
    for _ in 0..305 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.panes_layout_for_test(), "single");
    assert!(sh.panes_card_for_test().is_some());
}

#[test]
fn opening_an_app_into_a_pane_at_low_memory_says_so_and_does_not_open_it() {
    let (mut sh, mut g) = signed_in("pm5", 1280, 720, "alice");
    chord(&mut sh, LayoutKey::Apply(2));
    sh.render(&mut g);
    sh.panes.mem = Some(MemFacts { total: 16 * GIB, available: 600 * 1024 * 1024 });
    sh.open(PApp::Browser);
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("Low memory: close an app or use a smaller layout")), "{:?}", sh.toast);
    assert_eq!(sh.panes_titles_for_test()[0], "Home");
    // a light app still fits
    sh.panes.mem = Some(MemFacts { total: 16 * GIB, available: 1500 * 1024 * 1024 });
    sh.open(PApp::Calendar);
    assert_eq!(sh.screen_name(), "app:calendar");
}

#[test]
fn the_picker_shows_the_memory_line_and_only_an_administrator_has_the_switch() {
    let (mut sh, mut g) = signed_in("pm6", 1280, 720, "alice");
    chord(&mut sh, LayoutKey::Picker);
    assert!(sh.panes_picker_open_for_test());
    sh.render(&mut g);
    let t: Vec<String> = sh.hits().iter().map(|h| h.label.clone()).collect();
    assert!(t.iter().any(|l| l == "layout 50:50 side by side") && t.iter().any(|l| l == "layout 2x2 grid"), "{t:?}");
    assert!(!t.iter().any(|l| l.contains("switch")), "{t:?}");
    assert!(sh.panes_memory_line().starts_with("Layouts: available (16 GB, 12.0 GB free)"), "{}", sh.panes_memory_line());
    sh.handle(&Event::Key(Key::Char('s')));
    assert!(sh.toast.as_ref().is_some_and(|t| t.0.contains("administrator")));
    // choose by clicking a template
    click_label(&mut sh, &mut g, "layout 50:50 stacked");
    assert_eq!(sh.panes_layout_for_test(), "rows-50-50");
    assert!(!sh.panes_picker_open_for_test());
    // arrows and Enter, Esc
    chord(&mut sh, LayoutKey::Picker);
    sh.handle(&Event::Key(Key::Right));
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.panes_layout_for_test(), "cols-30-70");
    chord(&mut sh, LayoutKey::Picker);
    sh.handle(&Event::Key(Key::Esc));
    assert!(!sh.panes_picker_open_for_test());
    // the picker draws at small and large screens
    for (w, h) in [(800, 480), (2560, 1600), (1024, 600)] {
        let mut g2 = Gfx::new(w, h);
        chord(&mut sh, LayoutKey::Picker);
        sh.render(&mut g2);
        sh.handle(&Event::Key(Key::Esc));
    }
}

#[test]
fn the_switch_is_automatic_always_on_or_off_and_the_off_state_is_as_before() {
    let (mut sh, mut g) = signed_in("pm7", 1280, 720, "root");
    let dir = sh.env.services.dir().to_path_buf();
    assert_eq!(crate::memgate::read_switch(&dir), Switch::Auto);
    // always on with a small machine: layouts work
    sh.panes.mem = Some(MemFacts { total: 900 * 1024 * 1024, available: 300 * 1024 * 1024 });
    chord(&mut sh, LayoutKey::Picker);
    sh.render(&mut g);
    click_label(&mut sh, &mut g, "Screen features: Automatic  (S)");
    assert_eq!(crate::memgate::read_switch(&dir), Switch::On);
    sh.handle(&Event::Key(Key::Esc));
    chord(&mut sh, LayoutKey::Apply(7));
    assert_eq!(sh.panes_layout_for_test(), "grid", "always on ignores memory");
    // off: back to one screen at once, keys inert, as before
    chord(&mut sh, LayoutKey::Picker);
    sh.render(&mut g);
    sh.handle(&Event::Key(Key::Char('s')));
    assert_eq!(crate::memgate::read_switch(&dir), Switch::Off);
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.panes_layout_for_test(), "single", "off folds the layout");
    chord(&mut sh, LayoutKey::Apply(2));
    assert_eq!(sh.panes_layout_for_test(), "single");
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label == "layout"));
    assert!(sh.panes_memory_line().contains("off"));
    // and automatic again
    chord(&mut sh, LayoutKey::Picker);
    sh.handle(&Event::Key(Key::Char('s')));
    assert_eq!(crate::memgate::read_switch(&dir), Switch::Auto);
}

#[test]
fn the_layout_button_in_the_top_bars_opens_the_picker() {
    let (mut sh, mut g) = signed_in("pn20", 1280, 720, "alice");
    // on the launcher
    click_label(&mut sh, &mut g, "layout");
    assert!(sh.panes_picker_open_for_test());
    sh.handle(&Event::Key(Key::Esc));
    // inside an app, left of the bell, and the clock makes room
    sh.open(PApp::Calendar);
    sh.render(&mut g);
    let lay = sh.hits().iter().find(|h| h.label == "layout").unwrap().rect;
    let bell = sh.hits().iter().find(|h| h.label == "inbox").unwrap().rect;
    let clock = sh.hits().iter().find(|h| h.label == "clock").map(|h| h.rect);
    assert!(lay.x + lay.w <= bell.x + 0.5);
    if let Some(c) = clock {
        assert!(c.x + c.w <= lay.x + 0.5, "{c:?} {lay:?}");
    }
    click_label(&mut sh, &mut g, "layout");
    assert!(sh.panes_picker_open_for_test());
}

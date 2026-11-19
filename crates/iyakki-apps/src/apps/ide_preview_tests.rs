//! F-IDE-25: the docked Preview. The embedding facility with a made-up app (clicks, keys, ticks, Esc, going back), the split of the main
//! area at four sizes (room, no room, compact), the header buttons, and Run on a tile app docking the real app beside the editor.

use std::sync::{Arc, Mutex};

use super::super::embed::{self, Embedded, Out};
use super::super::ide_preview::{CLOSE, POP_OUT};
use super::super::{App, Flow};
use iyakki_shared::target::{Target as AppTarget};
use iyakki_agent::target::{self as tg};

use super::*;

#[derive(Default)]
struct Seen {
    keys: Vec<char>,
    ticks: u32,
    clicks: Vec<u32>,
    moves: u32,
    drew_at: Option<R>,
}

struct Fake {
    seen: Arc<Mutex<Seen>>,
    back_on: Option<char>,
}

impl App for Fake {
    fn title(&self) -> &'static str {
        "Fake tile"
    }
    fn icon(&self) -> crate::gfx::Icon {
        crate::gfx::Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [120, 200, 255, 255]
    }
    fn hint(&self) -> String {
        "x · y".into()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        let mut s = self.seen.lock().unwrap();
        match ev {
            Event::Key(Key::Char(c)) => {
                s.keys.push(*c);
                if Some(*c) == self.back_on {
                    return Flow::Back;
                }
            }
            Event::Tick => s.ticks += 1,
            Event::Row(n) => s.clicks.push(*n as u32),
            Event::MouseMove(..) => s.moves += 1,
            _ => {}
        }
        Flow::Stay
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
        self.seen.lock().unwrap().drew_at = Some(area);
        g.rect(area.x, area.y, area.w, area.h, [30, 40, 60, 255]);
        // two buttons inside the area and one half outside it
        g.hit(R::new(area.x + 10.0 * s, area.y + 10.0 * s, 60.0 * s, 24.0 * s), Event::Row(1), "one");
        g.hit(R::new(area.x + 80.0 * s, area.y + 10.0 * s, 60.0 * s, 24.0 * s), Event::Row(2), "two");
        g.hit(R::new(area.x + area.w - 20.0 * s, area.y + 40.0 * s, 100.0 * s, 24.0 * s), Event::Row(3), "sticks out");
        g.hit(R::new(area.x - 500.0, area.y, 10.0, 10.0), Event::Row(4), "far outside");
    }
}

fn fake(back_on: Option<char>) -> (Box<dyn App>, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    (Box::new(Fake { seen: seen.clone(), back_on }), seen)
}

fn click_hit(g: &Gfx, label: &str) -> (R, Event) {
    let h = g.hits.iter().find(|h| h.label == label).unwrap_or_else(|| panic!("no hit {label}"));
    (h.rect, h.ev.clone())
}

#[test]
fn an_embedded_app_is_drawn_in_its_rectangle_and_its_controls_come_back_as_its_own_events() {
    let w = world("embed-a");
    let (app, seen) = fake(None);
    let mut e = Embedded::new(app);
    let mut g = Gfx::new(800, 480);
    g.hit(R::new(0.0, 0.0, 50.0, 20.0), Event::Row(9), "host button");
    let area = R::new(400.0, 100.0, 300.0, 300.0);
    e.render(&mut g, area, 1.0, &w.cx, 1);
    assert_eq!(seen.lock().unwrap().drew_at, Some(area), "the child sees the rectangle as its whole screen");
    // the host's own hit stays as it was; the child's are re-registered as AreaAt over the part inside the rectangle
    assert_eq!(click_hit(&g, "host button").1, Event::Row(9));
    let (r1, ev1) = click_hit(&g, "one");
    assert!(matches!(ev1, Event::AreaAt(id, ..) if id >= embed::BASE) && Embedded::owns(&ev1));
    assert!(r1.x >= area.x && r1.y >= area.y);
    let (r3, _) = click_hit(&g, "sticks out");
    assert!(r3.x + r3.w <= area.x + area.w + 0.01, "cut to the rectangle: {r3:?}");
    assert!(g.hits.iter().all(|h| h.label != "far outside"), "a control outside the rectangle is dropped");
    // a click on "two" reaches the child as the event it registered, and gives it the keyboard
    e.focused = false;
    let (_, ev2) = click_hit(&g, "two");
    assert_eq!(e.handle(&ev2, &w.cx), Some(Out::Stay));
    assert_eq!(seen.lock().unwrap().clicks, vec![2]);
    assert!(e.focused);
    // a render again does not pile up regions
    let n = g.hits.len();
    e.render(&mut g, area, 1.0, &w.cx, 2);
    assert_eq!(g.hits.len(), n + 3, "three regions again, the old ones were the old frame's");
}

#[test]
fn keys_go_to_the_child_only_with_the_keyboard_esc_takes_it_back_and_the_child_can_close_itself() {
    let w = world("embed-b");
    let (app, seen) = fake(Some('q'));
    let mut e = Embedded::new(app);
    let mut g = Gfx::new(800, 480);
    e.render(&mut g, R::new(400.0, 100.0, 300.0, 300.0), 1.0, &w.cx, 1);
    assert_eq!(e.handle(&Event::Key(Key::Char('a')), &w.cx), Some(Out::Stay));
    // Esc is not seen by the child; the keyboard goes back to the host
    assert_eq!(e.handle(&Event::Key(Key::Esc), &w.cx), Some(Out::GaveKeyboard));
    assert!(!e.focused);
    assert_eq!(e.handle(&Event::Key(Key::Char('b')), &w.cx), None, "the host handles it now");
    // a press inside takes the keyboard again; a press outside does not concern the child
    assert_eq!(e.handle(&Event::MouseDown(10, 10), &w.cx), None);
    assert!(!e.focused);
    assert_eq!(e.handle(&Event::MouseDown(450, 150), &w.cx), Some(Out::Stay));
    assert!(e.focused);
    // pointer moves inside go in, outside do not; ticks always do
    e.handle(&Event::MouseMove(450, 150), &w.cx);
    e.handle(&Event::MouseMove(5, 5), &w.cx);
    e.focused = false;
    e.handle(&Event::Tick, &w.cx);
    // the child going back means closed
    e.focused = true;
    assert_eq!(e.handle(&Event::Key(Key::Char('q')), &w.cx), Some(Out::Closed));
    let s = seen.lock().unwrap();
    assert_eq!((s.keys.clone(), s.moves, s.ticks), (vec!['a', 'q'], 1, 1));
}

#[test]
fn the_preview_sits_beside_the_editor_at_four_sizes_and_a_narrow_or_compact_window_has_no_room() {
    let w = world("embed-c");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let _ = render(&mut app, &w.cx, wd, ht);
        let room = app.can_dock();
        let (fk, seen) = fake(None);
        app.dock_preview(fk);
        let g = render(&mut app, &w.cx, wd, ht);
        let l = app.layout.clone().unwrap();
        if wd >= 1100 {
            assert!(room, "{wd}x{ht}: room for a preview");
            let pv = app.preview_area.expect("docked");
            assert!(pv.x >= l.main.x + l.main.w - 0.5 && pv.x + pv.w <= wd as f32 + 0.5 && pv.y + pv.h <= ht as f32, "{wd}x{ht}: {pv:?} beside {:?}", l.main);
            assert!(pv.w >= 300.0 * scale(wd, ht) - 1.0 || pv.w > 250.0, "{wd}x{ht}: {pv:?}");
            assert!(l.main.w >= lay_min_main(wd, ht) - 1.0, "{wd}x{ht}: the editor keeps its width: {:?}", l.main);
            assert!(seen.lock().unwrap().drew_at.is_some_and(|a| a.w > 100.0), "{wd}x{ht}: the child was drawn");
            // no two header buttons on top of each other, and the child's regions are inside the preview
            let close = g.hits.iter().find(|h| h.ev == Event::AreaAt(CLOSE, 0.0, 0.0)).unwrap().rect;
            let pop = g.hits.iter().find(|h| h.ev == Event::AreaAt(POP_OUT, 0.0, 0.0)).unwrap().rect;
            assert!(close.x >= pop.x + pop.w - 0.5 || pop.x >= close.x + close.w - 0.5, "{wd}x{ht}: {close:?} {pop:?}");
            for h in g.hits.iter().filter(|h| Embedded::owns(&h.ev)) {
                assert!(h.rect.x >= pv.x - 0.5 && h.rect.x + h.rect.w <= pv.x + pv.w + 0.5, "{wd}x{ht}: {:?} not inside {pv:?}", h.rect);
            }
        } else {
            assert!(app.preview_area.is_none(), "{wd}x{ht}: a compact window draws no preview beside the editor");
        }
        app.close_preview();
    }
}

fn lay_min_main(wd: u32, ht: u32) -> f32 {
    super::super::ide_layout::MIN_MAIN * scale(wd, ht)
}

#[test]
fn close_and_pop_out_buttons_and_the_editor_keys_still_work_while_the_preview_is_open() {
    let w = world("embed-d");
    let d = project(&w, "alpha", &[]);
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    let _ = render(&mut app, &w.cx, 1920, 1080);
    let (fk, seen) = fake(None);
    app.dock_preview(fk);
    let g = render(&mut app, &w.cx, 1920, 1080);
    // typing goes to the preview while it has the keyboard, to the editor once Esc gave it back
    key(&mut app, &w.cx, Key::Char('z'));
    assert_eq!(seen.lock().unwrap().keys, vec!['z']);
    key(&mut app, &w.cx, Key::Esc);
    assert!(app.has_preview());
    key(&mut app, &w.cx, Key::Char('y'));
    assert_eq!(seen.lock().unwrap().keys, vec!['z'], "the editor got that one");
    // ticks reach the preview all the time
    ticks(&mut app, &w.cx, 3);
    assert!(seen.lock().unwrap().ticks >= 3);
    // a click on the child's control
    let (_, ev) = click_hit(&g, "one");
    app.handle(&ev, &w.cx);
    assert_eq!(seen.lock().unwrap().clicks, vec![1]);
    // pop out hands the folder to the shell and closes the preview
    app.preview_dir = Some(d.clone());
    app.handle(&Event::AreaAt(POP_OUT, 0.0, 0.0), &w.cx);
    assert!(!app.has_preview());
    assert_eq!(app.request.take().map(|r| r.0), Some(format!("tile-preview:{}", d.display())));
    // close
    let (fk, _) = fake(None);
    app.dock_preview(fk);
    app.handle(&Event::AreaAt(CLOSE, 0.0, 0.0), &w.cx);
    assert!(!app.has_preview() && app.request.is_none());
}

#[test]
fn run_docks_the_real_tile_app_when_there_is_room_and_opens_it_full_screen_when_there_is_not() {
    let w = world("embed-e");
    let d = project(&w, "alpha", &[]);
    iyakki_agent::store::author::scaffold("tile", "clock", &d, "").unwrap();
    tg::write(&d, "Alpha", AppTarget::IyakkiTile).unwrap();
    let mut app = IdeApp::new(&w.cx);
    open(&mut app, &w.cx, "Alpha", &d);
    // a compact window: the old behaviour
    let _ = render(&mut app, &w.cx, 800, 480);
    assert!(!app.can_dock());
    key(&mut app, &w.cx, Key::F(5));
    assert!(app.request.take().is_some_and(|r| r.0.starts_with("tile-preview:")) && !app.has_preview());
    // a wide window: the app is docked and drawn
    for (wd, ht) in [(1280u32, 720u32), (1920, 1080)] {
        let _ = render(&mut app, &w.cx, wd, ht);
        assert!(app.can_dock(), "{wd}x{ht}");
        key(&mut app, &w.cx, Key::F(5));
        assert!(app.request.is_none() && app.has_preview(), "{wd}x{ht}: {:?}", app.notice);
        let g = render(&mut app, &w.cx, wd, ht);
        assert!(app.preview_area.is_some_and(|r| r.w > 100.0 && r.x + r.w <= wd as f32 + 0.5), "{wd}x{ht}");
        assert!(g.text_log.iter().any(|t| t.2.starts_with("Preview: ")), "{wd}x{ht}: the header names it");
        app.close_preview();
    }
}

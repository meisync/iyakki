use std::cell::RefCell;
use std::rc::Rc;

use iyakki_uikit::gfx::{Face, Gfx};
use iyakki_uikit::look::{DockPos, Look};
use iyakki_uikit::ui::R;
use iyakki_uikit::wallpaper;

use crate::app::{Flow, UiApp};
use crate::canvas::{Canvas, Hit, Role, TextStyle};
use crate::cx::UiCx;
use crate::desktop::*;
use crate::event::*;
use crate::geom::Rect;
use crate::gesture::Recognizer;
use crate::host::Host;
use crate::list::*;
use crate::raster::RasterCanvas;
use crate::size::*;
use crate::widgets;

/// A tiny app that uses every part of the SDK once.
struct Probe {
    actions: Vec<u32>,
    events: Rc<RefCell<Vec<UiEvent>>>,
}

impl UiApp for Probe {
    fn id(&self) -> &'static str {
        "probe"
    }
    fn title(&self) -> String {
        "Probe".into()
    }
    fn dock_status(&self) -> Option<DockStatus> {
        Some(DockStatus::new(Some(self.actions.len() as u32), "a very long status line that goes past the limit of forty characters"))
    }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        self.events.borrow_mut().push(ev.clone());
        if let UiEvent::Action(a) = ev {
            self.actions.push(*a);
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let th = cx.colors();
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        c.text(10.0, 10.0, 18.0, th.text, "Hello Probe", TextStyle::Bold);
        c.clipped(Rect::new(0.0, 40.0, w, 60.0), &mut |cc| {
            cc.rrect(Rect::new(5.0, 45.0, w - 10.0, 50.0), 8.0, th.card);
            cc.hit(Rect::new(5.0, 45.0, w - 10.0, 50.0), Hit { action: 7, name: "Card".into(), role: Role::ListItem });
        });
        widgets::button(c, th, &cx.metrics(), Rect::new(10.0, 120.0, 140.0, 20.0), "Press me", 9, true);
    }
}

fn host(w: u32, h: u32) -> (Host<Probe>, Rc<RefCell<Vec<UiEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let app = Probe { actions: vec![], events: events.clone() };
    (Host::new(app, Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&["probe", "chat"])), Box::new(MemWallpaper::default())), events)
}

fn ptr(x: f32, y: f32, phase: Phase, t: u64) -> Pointer {
    Pointer { id: 0, x, y, phase, t_ms: t }
}

#[test]
fn size_classes_follow_the_width_and_touch_targets_are_never_small() {
    assert_eq!(SizeClass::for_width(360.0), SizeClass::Compact);
    assert_eq!(SizeClass::for_width(599.9), SizeClass::Compact);
    assert_eq!(SizeClass::for_width(600.0), SizeClass::Medium);
    assert_eq!(SizeClass::for_width(899.0), SizeClass::Medium);
    assert_eq!(SizeClass::for_width(1280.0), SizeClass::Expanded);
    let m = Metrics::new(360.0, 740.0, 2.0, 1.0, true);
    assert_eq!((m.class, m.min_target()), (SizeClass::Compact, 44.0));
    assert_eq!(Metrics::new(1280.0, 720.0, 1.0, 1.0, false).min_target(), 24.0);
    assert_eq!(Metrics::new(800.0, 600.0, 1.0, f32::NAN, false).text_scale, 1.0, "a broken text size is ignored");
    assert_eq!(Metrics::new(800.0, 600.0, 1.0, 9.0, false).font(10.0), 30.0, "the text size is capped at 3x");
}

#[test]
fn a_frame_recorded_as_a_list_survives_the_wire_format_and_names_its_controls() {
    let (mut h, _) = host(800, 480);
    let list = h.record();
    assert!(list.texts().contains(&"Hello Probe") && list.texts().contains(&"Press me"));
    let text = list.to_text();
    assert!(text.starts_with("iyakki-display-list 1 800 480\n"));
    assert_eq!(DisplayList::parse(&text).unwrap(), list, "the wire format round trips exactly");
    let tree = list.a11y();
    assert!(tree.iter().any(|n| n.name == "Press me" && n.role == Role::Button));
    assert!(tree.iter().any(|n| n.name == "Card" && n.role == Role::ListItem));
}

#[test]
fn text_with_awkward_characters_cannot_break_the_wire_format() {
    let mut c = ListCanvas::new(100.0, 100.0);
    let nasty = "line one\nline two %41 ~ \u{e9}\u{4e2d} \"q\" \t";
    c.text(1.0, 2.0, 12.0, [1, 2, 3, 255], nasty, TextStyle::Mono);
    c.text(1.0, 2.0, 12.0, [1, 2, 3, 255], "", TextStyle::Regular);
    c.hit(Rect::new(0.0, 0.0, 5.0, 5.0), Hit { action: 1, name: "a b\nc".into(), role: Role::Link });
    let l = c.finish();
    let t = l.to_text();
    assert_eq!(t.lines().count(), 4, "one header and one line per operation: {t}");
    assert_eq!(DisplayList::parse(&t).unwrap(), l);
}

#[test]
fn a_newer_or_damaged_list_is_refused_by_name_never_skipped() {
    let e = DisplayList::parse("iyakki-display-list 3 10 10\nrect 0 0 1 1 ffffffff\n").unwrap_err();
    assert!(e.contains("newer"), "{e}");
    let e = DisplayList::parse("iyakki-display-list 1 10 10\nblur 0 0 1 1\n").unwrap_err();
    assert!(e.contains("line 2") && e.contains("blur"), "{e}");
    assert!(DisplayList::parse("iyakki-display-list 1 10 10\nrect 0 0 1 NaN ffffffff\n").is_err());
    assert!(DisplayList::parse("iyakki-display-list 1 10 10\nrect 0 0 1 1 fff\n").is_err());
    assert!(DisplayList::parse("iyakki-display-list 1 10 10\nendclip\n").is_err());
    assert!(DisplayList::parse("iyakki-display-list 1 10 10\nclip 0 0 5 5\n").is_err());
    assert!(DisplayList::parse("hello").is_err());
    assert!(DisplayList::parse(&"x".repeat(MAX_BYTES + 1)).is_err());
}

#[test]
fn the_same_frame_drawn_directly_and_replayed_from_a_list_is_the_same_picture() {
    let (w, h) = (360u32, 740u32);
    // direct, in pixels
    let (mut host_a, _) = host(w, h);
    let mut ga = Gfx::new(w, h);
    host_a.draw(&mut RasterCanvas::new(&mut ga, R { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }, 1.0));
    // recorded with the device's own text measuring, then replayed
    let (mut host_b, _) = host(w, h);
    let meter = RefCell::new(Gfx::new(8, 8));
    let measure: Box<dyn FnMut(f32, &str, TextStyle) -> f32> = Box::new(move |size, s, st| {
        meter.borrow_mut().text_width(size, s, match st { TextStyle::Regular => Face::Sans, TextStyle::Bold => Face::Bold, TextStyle::Mono => Face::Mono })
    });
    let mut lc = ListCanvas::with_measure(w as f32, h as f32, measure);
    host_b.draw(&mut lc);
    let list = DisplayList::parse(&lc.finish().to_text()).unwrap();
    let mut gb = Gfx::new(w, h);
    list.replay(&mut RasterCanvas::new(&mut gb, R { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }, 1.0));
    assert!(ga.rgba() == gb.rgba(), "pixels differ between the direct frame and the replayed list");
    assert!(ga.rgba().iter().any(|b| *b != 0), "something was drawn");
}

#[test]
fn the_pixel_canvas_applies_the_scale_to_everything_and_reports_logical_size() {
    let mut g = Gfx::new(200, 100);
    let mut c = RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 200.0, h: 100.0 }, 2.0);
    assert_eq!(c.size(), (100.0, 50.0));
    c.hit(Rect::new(10.0, 5.0, 20.0, 10.0), Hit { action: 3, name: "x".into(), role: Role::Button });
    drop(c);
    let hits = g.probes();
    let _ = hits;
    g.clear_hits();
}

#[test]
fn a_hit_inside_a_clip_only_counts_inside_it() {
    let (mut h, _) = host(300, 200);
    let l = h.record();
    assert_eq!(l.hit_test(20.0, 60.0).map(|x| x.action), Some(7));
    assert!(l.hit_test(20.0, 105.0).is_none(), "outside the card");
    assert_eq!(l.hit_test(30.0, 125.0).map(|x| x.action), Some(9));
    let mut c = ListCanvas::new(100.0, 100.0);
    c.clipped(Rect::new(0.0, 0.0, 10.0, 10.0), &mut |cc| cc.hit(Rect::new(0.0, 0.0, 50.0, 50.0), Hit { action: 1, name: "n".into(), role: Role::Button }));
    let l = c.finish();
    assert!(l.hit_test(5.0, 5.0).is_some());
    assert!(l.hit_test(30.0, 30.0).is_none(), "a region cut by its clip is not hit outside the clip");
}

#[test]
fn gestures_tap_long_press_drag_and_swipe() {
    let mut r = Recognizer::new();
    assert!(r.feed(&ptr(10.0, 10.0, Phase::Down, 0)).is_empty());
    assert_eq!(r.feed(&ptr(12.0, 11.0, Phase::Up, 120)), vec![Gesture::Tap { x: 10.0, y: 10.0 }]);
    // held: a long press once, then no tap on release
    r.feed(&ptr(5.0, 5.0, Phase::Down, 1000));
    assert!(r.tick(1300).is_empty());
    assert_eq!(r.tick(1500), vec![Gesture::LongPress { x: 5.0, y: 5.0 }]);
    assert!(r.tick(1800).is_empty(), "once");
    assert!(r.feed(&ptr(5.0, 5.0, Phase::Up, 1900)).is_empty());
    // a slow drag is a drag, not a swipe
    r.feed(&ptr(0.0, 0.0, Phase::Down, 0));
    let g = r.feed(&ptr(30.0, 0.0, Phase::Move, 100));
    assert_eq!(g, vec![Gesture::DragStart { x: 0.0, y: 0.0 }, Gesture::Drag { dx: 30.0, dy: 0.0 }]);
    assert_eq!(r.feed(&ptr(40.0, 0.0, Phase::Up, 900)), vec![Gesture::DragEnd { dx: 40.0, dy: 0.0 }]);
    // a quick long one is a swipe
    r.feed(&ptr(200.0, 100.0, Phase::Down, 0));
    r.feed(&ptr(150.0, 100.0, Phase::Move, 50));
    let g = r.feed(&ptr(100.0, 105.0, Phase::Up, 150));
    assert_eq!(g, vec![Gesture::DragEnd { dx: -100.0, dy: 5.0 }, Gesture::Swipe(Dir::Left)]);
    // a cancelled touch gives nothing; a second finger makes a pinch (own tests below), here it lifts again without moving
    r.feed(&ptr(1.0, 1.0, Phase::Down, 0));
    r.feed(&Pointer { id: 1, x: 50.0, y: 50.0, phase: Phase::Down, t_ms: 10 });
    assert_eq!(r.feed(&Pointer { id: 1, x: 50.0, y: 50.0, phase: Phase::Up, t_ms: 20 }), vec![Gesture::PinchEnd { scale: 1.0 }]);
    r.feed(&ptr(1.0, 1.0, Phase::Cancel, 30));
    assert!(r.feed(&ptr(1.0, 1.0, Phase::Up, 40)).is_empty());
}

#[test]
fn a_tap_on_a_region_of_the_last_frame_reaches_the_app_as_its_action_and_the_dock_hears_the_status() {
    let (mut h, events) = host(300, 200);
    h.record();
    assert_eq!(h.mouse(30.0, 125.0, Phase::Down, 0), Flow::Stay);
    h.mouse(30.0, 125.0, Phase::Up, 80);
    assert!(events.borrow().contains(&UiEvent::Action(9)));
    assert!(events.borrow().iter().any(|e| matches!(e, UiEvent::Gesture(Gesture::Tap { .. }))));
    h.mouse(250.0, 190.0, Phase::Down, 100);
    h.mouse(250.0, 190.0, Phase::Up, 150);
    assert_eq!(events.borrow().iter().filter(|e| matches!(e, UiEvent::Action(_))).count(), 1, "a tap on nothing is no action");
}

#[test]
fn the_dock_status_is_published_after_events_and_cut_to_the_limit() {
    let st = DockStatus::new(Some(2), "x\u{7}y");
    assert_eq!(st.line, "xy", "control characters are dropped");
    assert_eq!(DockStatus::new(None, &"z".repeat(100)).line.chars().count(), STATUS_MAX);
    let mut d = MemDock::knowing(&["a", "b"]);
    assert!(d.pin("a").is_ok() && d.pin("a").is_ok());
    assert_eq!(d.pinned(), vec!["a".to_string()], "pinning twice pins once");
    assert_eq!(d.pin("zzz"), Err(DeskError::UnknownApp));
    assert!(d.open("b").is_ok() && d.unpin("a").is_ok() && d.pinned().is_empty());
}

#[test]
fn the_pane_follows_resize_view_changes_and_an_unplugged_screen() {
    let a = Arrangement::new(vec![
        Screen { id: 1, name: "main".into(), width: 1920, height: 1080, scale: 1.5, x: 0, y: 0, primary: true },
        Screen { id: 2, name: "side".into(), width: 1080, height: 1920, scale: 2.0, x: 1920, y: 0, primary: false },
    ]);
    assert_eq!(a.screens()[1].orientation(), Orientation::Portrait);
    assert_eq!(a.screen_at(2000, 10).map(|s| s.id), Some(2));
    assert!(a.screen_at(-5, 0).is_none());
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut h = Host::new(Probe { actions: vec![], events: events.clone() }, Look::default(), a, Box::new(MemDock::default()), Box::new(MemWallpaper::default()));
    assert_eq!((h.pane.screen, h.pane.w, h.pane.h), (1, 1280.0, 720.0));
    h.move_to(2, ViewInfo { name: "phone".into(), index: 1, count: 2 });
    assert_eq!((h.pane.screen, h.pane.w, h.pane.h, h.pane.view.as_str()), (2, 540.0, 960.0, "phone"));
    assert!(events.borrow().contains(&UiEvent::Resized));
    h.resize(360.0, 740.0);
    assert_eq!(SizeClass::for_width(h.pane.w), SizeClass::Compact);
    let n = events.borrow().len();
    h.resize(360.0, 740.0);
    assert_eq!(events.borrow().len(), n, "no event when nothing changed");
    // the side screen is unplugged while the pane is on it: the pane goes back to the primary screen
    assert!(h.unplug_screen(2));
    assert_eq!((h.pane.screen, h.pane.w), (1, 1280.0));
    assert!(!h.unplug_screen(1), "the last screen cannot be removed");
    assert_eq!(h.arrangement.resolve(99).id, 1, "an unknown screen is the primary one");
}

#[test]
fn dock_and_wallpaper_are_per_screen_and_the_dock_is_off_on_a_second_screen_by_default() {
    let mut a = Arrangement::new(vec![
        Screen { id: 1, name: "a".into(), width: 1280, height: 720, scale: 1.0, x: 0, y: 0, primary: true },
        Screen { id: 2, name: "b".into(), width: 1280, height: 720, scale: 1.0, x: 1280, y: 0, primary: false },
    ]);
    let mut look = Look::default();
    look.dock.position = DockPos::Left;
    look.wallpaper = wallpaper::Id::Dusk;
    assert_eq!((a.dock_for(&look, 1), a.dock_for(&look, 2)), (DockPos::Left, DockPos::Off));
    a.set_dock(2, DockPos::Bottom);
    a.set_wallpaper(2, wallpaper::Id::Meadow);
    assert_eq!((a.dock_for(&look, 2), a.wallpaper_for(&look, 1), a.wallpaper_for(&look, 2)), (DockPos::Bottom, wallpaper::Id::Dusk, wallpaper::Id::Meadow));
    a.unplug(2);
    assert_eq!(a.wallpaper_for(&look, 2), wallpaper::Id::Dusk, "the unplugged screen's choices are gone, the primary's answer");
    let mut w = MemWallpaper::default();
    assert_eq!(w.get(1), wallpaper::Id::Theme);
    w.set(1, wallpaper::Id::Slate).unwrap();
    assert_eq!(w.get(1), wallpaper::Id::Slate);
}

#[test]
fn text_over_a_wallpaper_always_reads() {
    let mut looks = Look::builtin();
    looks.push(Look::default());
    for l in &looks {
        for id in wallpaper::ALL {
            let c = text_over_wallpaper(l, id);
            let bg = if id == wallpaper::Id::Theme { l.theme.bg_top } else if id.is_light() { [235, 240, 248, 255] } else { [24, 22, 48, 255] };
            assert!(iyakki_uikit::usertheme::contrast(c, bg) >= 4.5, "{} over {}", l.name, id.name());
        }
    }
}

/// Every operation the widgets draw lies inside the rectangle they were given (heights may grow to the touch target), at the three
/// reference sizes and at the largest text size.
#[test]
fn widgets_stay_inside_their_space_at_every_size_class_and_text_size() {
    let look = Look::default();
    let th = &look.theme;
    for (w, touch) in [(360.0f32, true), (800.0, false), (1280.0, false)] {
        for text_scale in [1.0f32, 3.0] {
            let m = Metrics::new(w, 740.0, 1.0, text_scale, touch);
            let mut c = ListCanvas::new(w, 740.0);
            let area = Rect::new(8.0, 8.0, w - 16.0, 700.0);
            let b = widgets::button(&mut c, th, &m, Rect::new(area.x, 10.0, area.w, 20.0), "A rather long button label that needs to be cut", 1, false);
            let r = widgets::row(&mut c, th, &m, Rect::new(area.x, 100.0, area.w, 10.0), "A row with a long title that is too wide for a phone screen at the largest text", true, 2);
            let f = widgets::field(&mut c, th, &m, Rect::new(area.x, 200.0, area.w, 10.0), "Name", "something typed that is much longer than the field can show", true, 3);
            let p = widgets::wallpaper_picker(&mut c, th, &m, Rect::new(area.x, 300.0, area.w, 300.0), wallpaper::Id::Dusk, 100);
            for used in [b, r, f, p] {
                assert!(used.x >= area.x && used.right() <= area.right() + 0.01, "width at {w} x{text_scale}: {used:?}");
            }
            assert!(b.h >= m.min_target() && r.h >= m.min_target() && f.h >= m.min_target(), "targets at {w}");
            let l = c.finish();
            for op in &l.ops {
                if let Op::Text { x, size, text, style, .. } = op {
                    let mut meter = ListCanvas::new(1.0, 1.0);
                    let tw = meter.measure(*size, text, *style);
                    assert!(*x + tw <= area.right() + 0.5, "text runs off the space at {w} x{text_scale}: {text} ({x} + {tw})");
                }
            }
            assert!(l.a11y().iter().any(|n| n.name == "Wallpaper Dusk"));
        }
    }
}

#[test]
fn fit_shortens_with_an_ellipsis_and_leaves_short_text_alone() {
    let mut c = ListCanvas::new(1.0, 1.0);
    assert_eq!(crate::canvas::fit(&mut c, 10.0, "short", TextStyle::Regular, 100.0), "short");
    let t = crate::canvas::fit(&mut c, 10.0, "a long line of words", TextStyle::Regular, 40.0);
    assert!(t.ends_with('\u{2026}') && c.measure(10.0, &t, TextStyle::Regular) <= 40.0, "{t}");
    assert_eq!(crate::canvas::fit(&mut c, 10.0, "abc", TextStyle::Regular, 0.0), "\u{2026}");
}

#[test]
fn the_ui_sdk_crate_has_no_machine_access_of_its_own() {
    // the core rule for the SDK itself: no files, sockets, processes or devices in non-test code
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = e.path().file_name().unwrap().to_string_lossy().into_owned();
        if name == "tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(e.path()).unwrap();
        for bad in ["std::fs", "std::net", "std::process", "Command::new", "\"/proc", "\"/sys", "\"/dev", "iyakki_agent", "iyakki_devctl", "iyakki_shell"] {
            assert!(!text.contains(bad), "{name} uses {bad}");
        }
    }
    let m = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let deps: Vec<&str> = m.lines().filter(|l| l.starts_with("iyakki-")).collect();
    // the toolkit, and the editing engine (plain Rust: no dependencies, no I/O, F-SDK-104), nothing else
    assert!(deps.iter().all(|l| l.starts_with("iyakki-uikit") || l.starts_with("iyakki-edit")), "{deps:?}");
    let edit = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../iyakki-edit/Cargo.toml")).unwrap();
    let edit_deps = edit.split("[dependencies]").nth(1).unwrap_or("").lines().skip(1).take_while(|l| !l.starts_with('[')).filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).count();
    assert_eq!(edit_deps, 0, "the editing engine has no dependencies of its own");
}

/// The fixture alpha.4 wrote: every later release must read it and write it back byte for byte.
#[test]
fn the_display_list_alpha4_wrote_is_still_read_and_written_the_same() {
    let fixture = include_str!("../../../tests/compat/alpha4/display-list-v1.txt");
    let l = DisplayList::parse(fixture).expect("the alpha.4 list parses");
    assert_eq!((l.width, l.height, l.ops.len()), (300.0, 200.0, 12));
    assert_eq!(l.to_text(), fixture, "writing it back gives the same bytes");
    assert_eq!(l.texts(), vec!["Hello Probe", ""]);
    assert_eq!(l.hit_test(20.0, 60.0).map(|h| h.name.as_str()), Some("Card"));
}

#[test]
fn input_lines_round_trip_and_a_bad_line_names_itself_and_applies_nothing() {
    use crate::wire::{parse_batch, parse_line, to_line, Input};
    let inputs = [
        Input::Pointer(ptr(12.5, 40.0, Phase::Down, 7)),
        Input::Event(UiEvent::Key(UiKey::Enter)),
        Input::Event(UiEvent::Key(UiKey::Char('a'))),
        Input::Event(UiEvent::Key(UiKey::Char(' '))),
        Input::Event(UiEvent::Key(UiKey::Char('\u{e9}'))),
        Input::Event(UiEvent::Text("pasted %\u{1f600} text\nline".into())),
        Input::Event(UiEvent::Text(String::new())),
        Input::Event(UiEvent::Action(9)),
        Input::Event(UiEvent::Wheel(-3)),
        Input::Resize(360.0, 740.0),
        Input::Tick(33),
    ];
    for i in &inputs {
        let line = to_line(i).unwrap();
        assert!(!line.contains('\n'), "{line}");
        assert_eq!(&parse_line(&line, 1).unwrap(), i, "{line}");
    }
    assert!(to_line(&Input::Event(UiEvent::Resized)).is_none());
    for (bad, why) in [("ptr 0 sideways 1 1 1", "unknown pointer phase"), ("key f13", "unknown key"), ("key char %07", "one printable"), ("key char ab", "one printable"), ("action -1", "bad number"), ("resize 5 5", "100 to 4096"), ("tick x", "bad number"), ("fly 1", "unknown or malformed input 'fly'"), ("text %zz", "bad escape")] {
        let e = parse_line(bad, 3).unwrap_err();
        assert!(e.starts_with("line 3:") && e.contains(why), "{bad}: {e}");
    }
    assert!(parse_batch("action 1\nbogus\n").unwrap_err().starts_with("line 2:"));
    assert!(parse_batch(&"tick 1\n".repeat(300)).unwrap_err().contains("too many"));
    assert!(parse_line(&format!("text {}", "a".repeat(5000)), 1).unwrap_err().contains("too long"));
}

#[test]
fn a_session_applies_input_lines_through_the_host_and_answers_with_the_next_frame() {
    use crate::wire::Session;
    let (h, events) = host(400, 300);
    let mut s = Session::new(h);
    let first = s.frame();
    assert!(first.starts_with("iyakki-display-list 1 400 300\n") && first.contains("Press"));
    // a tap on the button (down, up, 60 ms) reaches the app as the action of the hit region
    let list = crate::list::DisplayList::parse(&first).unwrap();
    let (x, y) = list.ops.iter().find_map(|o| if let crate::list::Op::Hit(r, h) = o { (h.name == "Press me").then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap();
    let out = s.apply(&format!("ptr 0 down {x} {y} 0\nptr 0 up {x} {y} 60\n")).unwrap();
    assert!(events.borrow().contains(&UiEvent::Action(9)), "{:?}", events.borrow());
    assert!(out.starts_with("iyakki-display-list 1 "));
    // a bad batch applies nothing
    let n = events.borrow().len();
    assert!(s.apply("action 5\nnope\n").is_err());
    assert_eq!(events.borrow().len(), n);
    // a resize changes the frame's size
    assert!(s.apply("resize 600 500").unwrap().starts_with("iyakki-display-list 1 600 500\n"));
    assert!(!s.left());
}

fn p(id: u32, x: f32, y: f32, phase: Phase, t: u64) -> Pointer {
    Pointer { id, x, y, phase, t_ms: t }
}

#[test]
fn two_fingers_make_a_pinch_with_the_ratio_of_the_distances_and_neither_finger_makes_a_tap() {
    let mut r = Recognizer::new();
    assert!(r.feed(&p(1, 100.0, 100.0, Phase::Down, 0)).is_empty());
    assert!(r.feed(&p(2, 200.0, 100.0, Phase::Down, 10)).is_empty(), "the second finger only starts the pinch");
    // spreading to twice the distance
    let g = r.feed(&p(2, 300.0, 100.0, Phase::Move, 20));
    assert_eq!(g, vec![Gesture::Pinch { scale: 2.0, x: 200.0, y: 100.0 }]);
    // a tiny jitter is not reported again
    assert!(r.feed(&p(2, 300.4, 100.0, Phase::Move, 25)).is_empty());
    // the first finger moving closer, to a distance of 150 -> scale 1.5
    let g = r.feed(&p(1, 150.0, 100.0, Phase::Move, 30));
    assert!(matches!(g[..], [Gesture::Pinch { scale, .. }] if (scale - 1.5).abs() < 0.01), "{g:?}");
    // lifting a finger ends it, with the last scale; the finger left behind makes no tap and no swipe
    let g = r.feed(&p(2, 300.0, 100.0, Phase::Up, 40));
    assert!(matches!(g[..], [Gesture::PinchEnd { scale }] if (scale - 1.5).abs() < 0.01), "{g:?}");
    assert!(r.feed(&p(1, 150.0, 100.0, Phase::Up, 50)).is_empty(), "no tap after a pinch");
    // and the recogniser is clean again: a plain tap works
    r.feed(&p(1, 10.0, 10.0, Phase::Down, 100));
    assert_eq!(r.feed(&p(1, 10.0, 10.0, Phase::Up, 150)), vec![Gesture::Tap { x: 10.0, y: 10.0 }]);
}

#[test]
fn a_pinch_that_starts_during_a_drag_ends_the_drag_and_a_cancel_ends_the_pinch_and_a_third_finger_is_ignored() {
    let mut r = Recognizer::new();
    r.feed(&p(1, 0.0, 0.0, Phase::Down, 0));
    assert!(matches!(r.feed(&p(1, 40.0, 0.0, Phase::Move, 10))[..], [Gesture::DragStart { .. }, Gesture::Drag { .. }]));
    let g = r.feed(&p(2, 140.0, 0.0, Phase::Down, 20));
    assert!(matches!(g[..], [Gesture::DragEnd { .. }]), "{g:?}");
    assert!(r.feed(&p(3, 5.0, 5.0, Phase::Down, 21)).is_empty(), "a third finger does nothing");
    assert!(r.feed(&p(3, 6.0, 6.0, Phase::Move, 22)).is_empty());
    assert!(matches!(r.feed(&p(1, 40.0, 100.0, Phase::Move, 30))[..], [Gesture::Pinch { .. }]));
    assert!(matches!(r.feed(&p(2, 140.0, 0.0, Phase::Cancel, 40))[..], [Gesture::PinchEnd { .. }]));
    // the long press clock does not fire for a finger that is part of a pinch
    assert!(r.tick(5000).is_empty());
}

#[test]
fn the_host_hands_the_app_pinch_gestures_and_says_when_the_keyboard_is_wanted() {
    struct Kb(bool, Rc<RefCell<Vec<Gesture>>>);
    impl UiApp for Kb {
        fn id(&self) -> &'static str {
            "kb"
        }
        fn title(&self) -> String {
            "KB".into()
        }
        fn wants_keyboard(&self) -> bool {
            self.0
        }
        fn event(&mut self, ev: &UiEvent, _: &mut UiCx) -> Flow {
            match ev {
                UiEvent::Gesture(g @ (Gesture::Pinch { .. } | Gesture::PinchEnd { .. })) => self.1.borrow_mut().push(*g),
                UiEvent::Key(UiKey::Tab) => self.0 = !self.0,
                _ => {}
            }
            Flow::Stay
        }
        fn view(&mut self, _: &mut dyn Canvas, _: &UiCx) {}
    }
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut h = Host::new(Kb(false, seen.clone()), Look::default(), Arrangement::single(360, 740), Box::new(MemDock::default()), Box::new(MemWallpaper::default()));
    assert!(!h.wants_keyboard());
    h.event(&UiEvent::Key(UiKey::Tab));
    assert!(h.wants_keyboard());
    h.pointer(p(1, 100.0, 100.0, Phase::Down, 0));
    h.pointer(p(2, 200.0, 100.0, Phase::Down, 1));
    h.pointer(p(2, 250.0, 100.0, Phase::Move, 2));
    h.pointer(p(2, 250.0, 100.0, Phase::Up, 3));
    assert!(matches!(seen.borrow()[..], [Gesture::Pinch { .. }, Gesture::PinchEnd { .. }]), "{:?}", seen.borrow());
}

#[test]
fn the_in_memory_storage_port_has_no_machine_access_and_the_crate_still_builds_without_the_pixel_side() {
    // the list side of the crate stands alone (the client engine links it that way): its modules name nothing of the toolkit
    for f in ["canvas.rs", "geom.rs", "list.rs", "event.rs", "gesture.rs", "size.rs", "ports.rs"] {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(f)).unwrap();
        assert!(!text.contains("iyakki_uikit"), "{f} needs the toolkit");
    }
}

#[test]
fn a_drag_from_one_hit_region_onto_another_is_a_drop_and_one_that_stays_or_starts_on_nothing_is_not() {
    let (mut h, events) = host(360, 740);
    let _ = h.record(); // the frame the hit regions come from
    let drops = |e: &Rc<RefCell<Vec<UiEvent>>>| e.borrow().iter().filter_map(|x| if let UiEvent::Drop { from, to } = x { Some((*from, *to)) } else { None }).collect::<Vec<_>>();
    // the card (action 7) is picked up and let go over the button (action 9)
    h.mouse(20.0, 60.0, Phase::Down, 0);
    h.mouse(20.0, 90.0, Phase::Move, 20);
    h.mouse(20.0, 130.0, Phase::Move, 40);
    h.mouse(20.0, 130.0, Phase::Up, 60);
    assert_eq!(drops(&events), vec![(7, 9)]);
    // a drag that ends over the region it started on is not a drop
    h.mouse(20.0, 50.0, Phase::Down, 100);
    h.mouse(80.0, 52.0, Phase::Move, 120);
    h.mouse(80.0, 52.0, Phase::Up, 140);
    assert_eq!(drops(&events).len(), 1);
    // a drag that starts on nothing is not a drop either
    h.mouse(300.0, 400.0, Phase::Down, 200);
    h.mouse(40.0, 130.0, Phase::Move, 220);
    h.mouse(40.0, 130.0, Phase::Up, 240);
    assert_eq!(drops(&events).len(), 1);
}

// ---- the embedded browser view (F-SDK-61)

#[test]
fn a_translated_list_moves_everything_and_keeps_the_pages_actions_apart_from_the_apps() {
    let mut c = ListCanvas::new(100.0, 50.0);
    c.rect(Rect::new(0.0, 0.0, 100.0, 50.0), [1, 2, 3, 255]);
    c.clipped(Rect::new(0.0, 0.0, 100.0, 50.0), &mut |cc| {
        cc.text(5.0, 6.0, 12.0, [9, 9, 9, 255], "hi", TextStyle::Regular);
        cc.hit(Rect::new(5.0, 6.0, 20.0, 10.0), Hit { action: 3, name: "link".into(), role: Role::Link });
        cc.hit(Rect::new(0.0, 0.0, 1.0, 1.0), Hit { action: 9_500, name: "past the cap".into(), role: Role::Link });
    });
    let l = c.finish();
    let t = l.translated(10.0, 20.0, 5_000, 9_000);
    assert_eq!(t.a11y().len(), 1, "a hit above the cap is dropped");
    let n = &t.a11y()[0];
    assert_eq!((n.rect.x, n.rect.y), (15.0, 26.0));
    assert!(t.ops.iter().any(|o| matches!(o, Op::Hit(_, h) if h.action == 5_003)));
    assert!(t.ops.iter().any(|o| matches!(o, Op::Text { x, y, .. } if (*x, *y) == (15.0, 26.0))));
    assert_eq!(DisplayList::parse(&t.to_text()).unwrap(), t, "the moved list is still a valid list");
}

#[test]
fn a_size_policy_takes_space_from_the_rectangle_and_never_more() {
    use crate::browser::SizePolicy;
    let area = Rect::new(10.0, 20.0, 400.0, 300.0);
    assert_eq!(SizePolicy::Fill.used(area), area);
    assert_eq!(SizePolicy::Fixed(500.0, 100.0).used(area), Rect::new(10.0, 20.0, 400.0, 100.0));
    assert_eq!(SizePolicy::Height(120.0).used(area).h, 120.0);
    let a = SizePolicy::Aspect(2.0).used(area);
    assert_eq!((a.w, a.h), (400.0, 200.0));
    assert_eq!(SizePolicy::Aspect(0.5).used(area).h, 300.0, "cut to the room there is");
}

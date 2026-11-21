//! Design tokens beyond colour (F-SDK-120, F-SDK-121): the numbers come from the Look's skin parts, the widgets read them and have none
//! of their own, and a skin changes what is drawn.

use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::{Edge, Elevation, Look, Motion, Shape, Spacing};
use crate::size::{Metrics, TypeRole};
use crate::tokens::{Line, Radius, Space, Speed, Tokens};
use crate::widgets;

fn skin(f: impl FnOnce(&mut Look)) -> Look {
    let mut l = Look::default();
    f(&mut l);
    l
}

fn metrics(l: &Look) -> Metrics {
    Metrics::new(800.0, 600.0, 1.0, 1.0, false).with_tokens(Tokens::of(l))
}

fn button_list(l: &Look) -> DisplayList {
    let m = metrics(l);
    let mut c = ListCanvas::new(800.0, 600.0);
    widgets::button(&mut c, &l.theme, &m, Rect::new(10.0, 10.0, 200.0, 44.0), "Go", 1, true);
    c.finish()
}

fn radii(list: &DisplayList) -> Vec<f32> {
    list.ops.iter().filter_map(|o| if let Op::RoundRect(_, r, _) = o { Some(*r) } else { None }).collect()
}

fn shadows(list: &DisplayList) -> Vec<f32> {
    list.ops.iter().filter_map(|o| if let Op::Shadow(_, _, s, _) = o { Some(*s) } else { None }).collect()
}

#[test]
fn the_plain_skin_is_what_the_widgets_drew_before_skins_existed() {
    let t = Tokens::default();
    assert_eq!([t.radius(Radius::Sm), t.radius(Radius::Md), t.radius(Radius::Lg)], [6.0, 8.0, 10.0]);
    assert_eq!([t.line(Line::Hair), t.line(Line::Normal), t.line(Line::Thick)], [1.0, 1.0, 2.0]);
    assert_eq!([t.space(Space::Xxs), t.space(Space::Xs), t.space(Space::Sm), t.space(Space::Md), t.space(Space::Lg), t.space(Space::Xl)], [2.0, 4.0, 6.0, 8.0, 12.0, 16.0]);
    assert_eq!((t.shadow_spread(), t.glass, t.glyph(), t.focus_width()), (0.0, false, 20.0, 2.0));
    assert_eq!((t.duration_ms(Speed::Fast), t.duration_ms(Speed::Normal), t.duration_ms(Speed::Slow)), (120, 200, 320));
    assert_eq!(Tokens::of(&Look::default()), t);
    // a plain button: one rounded rectangle with the medium radius, no shadow
    let l = button_list(&Look::default());
    assert_eq!(radii(&l), vec![8.0]);
    assert!(shadows(&l).is_empty());
}

#[test]
fn the_shape_of_the_skin_changes_the_corners_and_a_pill_never_bulges() {
    let sharp = button_list(&skin(|l| l.surface.shape = Shape::Sharp));
    let round = button_list(&skin(|l| l.surface.shape = Shape::Round));
    let pill = button_list(&skin(|l| l.surface.shape = Shape::Pill));
    assert_eq!(radii(&sharp), vec![2.0]);
    assert_eq!(radii(&round), vec![12.0]);
    // 20 is the medium pill radius; the button is 44 high so 20 fits; a 24-high thing is clamped to 12
    assert_eq!(radii(&pill), vec![20.0]);
    let t = Tokens::of(&skin(|l| l.surface.shape = Shape::Pill));
    assert_eq!(t.radius_for(Radius::Lg, 24.0), 12.0);
    assert_eq!(t.radius_for(Radius::Lg, 0.0), 0.0);
}

#[test]
fn elevation_adds_a_shadow_to_a_raised_primary_button_and_flat_adds_none() {
    assert!(shadows(&button_list(&Look::default())).is_empty());
    assert_eq!(shadows(&button_list(&skin(|l| l.surface.elevation = Elevation::Low))), vec![6.0]);
    assert_eq!(shadows(&button_list(&skin(|l| l.surface.elevation = Elevation::High))), vec![14.0]);
    // a button that is not primary is not raised
    let l = skin(|l| l.surface.elevation = Elevation::High);
    let m = metrics(&l);
    let mut c = ListCanvas::new(800.0, 600.0);
    widgets::button(&mut c, &l.theme, &m, Rect::new(10.0, 10.0, 200.0, 44.0), "Go", 1, false);
    assert!(shadows(&c.finish()).is_empty());
}

#[test]
fn glass_makes_fills_translucent_and_adds_a_hairline_and_off_is_opaque() {
    let plain = button_list(&Look::default());
    let glass = button_list(&skin(|l| l.surface.glass = true));
    let alpha = |l: &DisplayList| l.ops.iter().find_map(|o| if let Op::RoundRect(_, _, c) = o { Some(c[3]) } else { None }).unwrap();
    assert_eq!(alpha(&plain), 255);
    assert!(alpha(&glass) < 255);
    assert!(glass.ops.iter().any(|o| matches!(o, Op::Outline(..))), "a glass surface has a hairline edge");
    assert_eq!(Tokens::of(&skin(|l| { l.surface.glass = true; l.surface.blur = 20 })).blur, 20.0);
    assert_eq!(Tokens::of(&skin(|l| l.surface.blur = 20)).blur, 0.0, "no blur without glass");
}

#[test]
fn spacing_and_edges_scale_and_reduced_motion_stops_every_animation() {
    let g = |s: Spacing| metrics(&skin(|l| l.surface.spacing = s)).gutter();
    assert!(g(Spacing::Tight) < g(Spacing::Normal) && g(Spacing::Normal) < g(Spacing::Airy));
    let m = metrics(&skin(|l| l.surface.spacing = Spacing::Airy));
    assert_eq!(m.gap(Space::Lg), 15.0);
    let e = |s: Edge| Tokens::of(&skin(|l| l.surface.edge = s)).line(Line::Normal);
    assert_eq!((e(Edge::Thin), e(Edge::Regular), e(Edge::Heavy)), (0.5, 1.0, 2.0));
    let calm = Tokens::of(&skin(|l| l.motion = Motion::Reduced));
    assert!(!calm.animates());
    assert_eq!([Speed::Fast, Speed::Normal, Speed::Slow].map(|s| calm.duration_ms(s)), [0, 0, 0]);
    assert!(Tokens::default().animates());
    // High contrast draws the focus ring thick
    let hc = Tokens::of(&skin(|l| l.theme = *iyakki_uikit::theme::ThemeId::HighContrast.theme()));
    assert!(hc.focus_width() >= Tokens::default().focus_width());
}

#[test]
fn the_text_roles_grow_with_the_text_size_and_leading_follows_the_size() {
    let m1 = Metrics::new(800.0, 600.0, 1.0, 1.0, false);
    let m3 = Metrics::new(800.0, 600.0, 1.0, 3.0, false);
    for r in [TypeRole::Caption, TypeRole::Small, TypeRole::Body, TypeRole::Label, TypeRole::Large, TypeRole::Title, TypeRole::Heading, TypeRole::Display] {
        assert_eq!(m3.size(r), 3.0 * m1.size(r));
        assert!(m1.leading(m1.size(r)) > m1.size(r));
    }
    assert!(m1.size(TypeRole::Caption) < m1.size(TypeRole::Body) && m1.size(TypeRole::Body) < m1.size(TypeRole::Display));
}

#[test]
fn a_look_file_carries_the_skin_parts_and_a_bad_one_is_refused_by_line() {
    let l = skin(|l| {
        l.name = "Soft glass".into();
        l.surface.shape = Shape::Round;
        l.surface.edge = Edge::Heavy;
        l.surface.spacing = Spacing::Airy;
        l.surface.elevation = Elevation::High;
        l.surface.glass = true;
        l.surface.blur = 18;
        l.surface.glyph = 24;
    });
    let text = l.to_text();
    for k in ["[shape]", "shape = round", "edge = heavy", "[spacing]", "spacing = airy", "[effects]", "elevation = high", "glass = on", "blur = 18", "glyph_size = 24", "parallel_draw = off"] {
        assert!(text.contains(k), "missing {k}");
    }
    assert_eq!(Look::from_text(&text).unwrap(), l);
    // a file from before skins has none of these and reads as the plain skin
    let old = Look::default().to_text().replace("[shape]\nshape = soft\nedge = regular\n\n[spacing]\nspacing = normal\n\n[effects]\nelevation = flat\nglass = off\nblur = 12\nglyph_size = 20\nparallel_draw = off\n", "");
    assert!(!old.contains("[shape]"));
    assert_eq!(Look::from_text(&old).unwrap().surface, Look::default().surface);
    let bad = text.replace("shape = round", "shape = hexagon").replace("blur = 18", "blur = 99");
    let p = Look::from_text(&bad).unwrap_err();
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(p.iter().any(|x| x.message.contains("hexagon") && x.line > 0));
    assert!(p.iter().any(|x| x.message.contains("0 to 24")));
    // a key in the wrong section is refused
    assert!(Look::from_text(&text.replace("glass = on", "glass = on\nshape = pill")).is_err());
}

#[test]
fn widget_code_has_no_styling_numbers_of_its_own() {
    for (name, src) in [
        ("widgets.rs", include_str!("widgets.rs")),
        ("chatview.rs", include_str!("chatview.rs")),
        // the view; the in-memory test backend after it is not widget code
        ("browser.rs", include_str!("browser.rs").split("pub struct MemPage").next().unwrap()),
        ("editorview.rs", include_str!("editorview.rs")),
        // the views; the size text helper after the view is not widget code
        ("filesview.rs", include_str!("filesview.rs").split("/// \"12 B\"").next().unwrap()),
        ("terminalview.rs", include_str!("terminalview.rs")),
        ("cameraview.rs", include_str!("cameraview.rs")),
        ("audioview.rs", include_str!("audioview.rs")),
        ("barcodeview.rs", include_str!("barcodeview.rs")),
        ("mediaview.rs", include_str!("mediaview.rs")),
    ] {
        let bad = crate::lint::style_literals(src);
        assert!(bad.is_empty(), "{name} has styling numbers (use the tokens: m.gap, m.size, m.radius, m.line):\n{}", bad.join("\n"));
    }
}

#[test]
fn the_context_gives_an_app_the_tokens_of_the_look_in_use() {
    use crate::desktop::{Arrangement, MemDock, MemWallpaper};
    use crate::host::Host;
    struct Nothing;
    impl crate::app::UiApp for Nothing {
        fn id(&self) -> &'static str { "nothing" }
        fn title(&self) -> String { "N".into() }
        fn event(&mut self, _: &crate::event::UiEvent, _: &mut crate::cx::UiCx) -> crate::app::Flow { crate::app::Flow::Stay }
        fn view(&mut self, c: &mut dyn crate::canvas::Canvas, cx: &crate::cx::UiCx) {
            let m = cx.metrics();
            c.rrect(Rect::new(0.0, 0.0, 10.0, 10.0), m.radius(Radius::Lg), cx.colors().card);
        }
    }
    let look = skin(|l| l.surface.shape = Shape::Round);
    let mut h = Host::new(Nothing, look, Arrangement::single(300, 300), Box::new(MemDock::knowing(&[])), Box::new(MemWallpaper::default()));
    assert_eq!(radii(&h.record()), vec![16.0]);
}

// ---- skins (F-SDK-122)

use crate::app::{Flow, UiApp};
use crate::canvas::Canvas;
use crate::cx::UiCx;
use crate::desktop::{Arrangement, MemDock, MemWallpaper};
use crate::event::UiEvent;
use crate::host::Host;
use iyakki_uikit::skins;

/// An app with state that draws one primary button in the colours its `cx` gives, and with an accent of its own when it has one.
struct Counter {
    n: u32,
    own_accent: Option<[u8; 4]>,
    seen: Vec<skins::SkinInfo>,
}

impl UiApp for Counter {
    fn id(&self) -> &'static str {
        "counter"
    }
    fn title(&self) -> String {
        "COUNTER".into()
    }
    fn event(&mut self, ev: &UiEvent, _: &mut UiCx) -> Flow {
        if matches!(ev, UiEvent::Action(1)) {
            self.n += 1;
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let m = cx.metrics();
        let th = match self.own_accent {
            Some(a) => cx.colors_with_accent(a),
            None => *cx.colors(),
        };
        self.seen.push(cx.skin());
        widgets::button(c, &th, &m, Rect::new(10.0, 10.0, 200.0, 44.0), &format!("Count {}", self.n), 1, true);
    }
}

fn counter_host(own_accent: Option<[u8; 4]>, look: Look) -> Host<Counter> {
    Host::new(Counter { n: 0, own_accent, seen: vec![] }, look, Arrangement::single(400, 300), Box::new(MemDock::knowing(&[])), Box::new(MemWallpaper::default()))
}

fn shipped_look(key: &str) -> Look {
    skins::shipped().into_iter().find(|(k, _)| k == key).unwrap().1.unwrap().look
}

#[test]
fn switching_the_skin_is_live_the_app_keeps_its_state_and_asks_cx_for_the_new_one() {
    let mut h = counter_host(None, Look::default());
    assert!(h.record().texts().contains(&"Count 0"));
    h.event(&UiEvent::Action(1));
    h.event(&UiEvent::Action(1));
    let before = h.record();
    assert_eq!(radii(&before), vec![8.0]);
    assert!(shadows(&before).is_empty());
    // the person picks Glass night in Settings: one call, no new app
    h.set_look(shipped_look("glass-night"));
    let after = h.record();
    assert!(after.texts().contains(&"Count 2"), "the count survived the switch");
    assert_eq!(radii(&after)[0], 12.0, "round corners now");
    assert_eq!(shadows(&after), vec![6.0], "low elevation now");
    // and to Blueprint
    h.set_look(shipped_look("blueprint"));
    let bp = h.record();
    assert_eq!(radii(&bp)[0], 2.0);
    assert!(shadows(&bp).is_empty());
    let last = h.app.seen.last().unwrap();
    assert_eq!((last.key.as_str(), last.shape, last.dark), ("blueprint", "sharp", true));
    assert_eq!(h.app.seen.first().unwrap().key, "default");
}

#[test]
fn an_app_with_its_own_accent_is_corrected_when_the_skin_would_make_it_unreadable() {
    let fill = |l: &DisplayList| l.ops.iter().find_map(|o| if let Op::RoundRect(_, _, c) = o { Some(*c) } else { None }).unwrap();
    // a pale yellow accent: fine as a wish, unreadable as a button on the dark skin and on the pale skin
    let wanted = [250, 250, 150, 255];
    for look in [Look::default(), shipped_look("paper"), skin(|l| l.theme = *iyakki_uikit::theme::ThemeId::HighContrast.theme())] {
        let mut h = counter_host(Some(wanted), look.clone());
        let drawn = fill(&h.record());
        let th = &look.theme;
        assert!(skins::accent_ok(th, drawn), "the drawn accent passes on {}", look.name);
        assert!(iyakki_uikit::usertheme::contrast(iyakki_uikit::skins::label_on(th, drawn), drawn) >= 4.5, "label contrast on {}", look.name);
        assert!(iyakki_uikit::usertheme::contrast(drawn, th.card) >= 3.0);
        if skins::accent_ok(th, wanted) {
            assert_eq!(drawn[..3], wanted[..3]);
        } else {
            assert_ne!(drawn[..3], wanted[..3], "corrected on {}", look.name);
        }
    }
    // an accent that already reads is left alone
    let good = Look::default().theme.accent;
    let mut h = counter_host(Some(good), Look::default());
    assert_eq!(fill(&h.record())[..3], good[..3]);
}

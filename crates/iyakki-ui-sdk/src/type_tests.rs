//! Font abilities, the display list version 2 and the theme x size class x text scale x family matrix (F-SKIN-04 to F-SKIN-07).

use std::cell::RefCell;

use iyakki_uikit::gfx::{Face, Gfx};
use iyakki_uikit::look::Look;
use iyakki_uikit::theme::{ThemeId, ALL};
use iyakki_uikit::ui::R;
use iyakki_uikit::usertheme::contrast;

use crate::assistant::*;
use crate::canvas::{Canvas, TextStyle};
use crate::chatview::ChatView;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::raster::RasterCanvas;
use crate::size::{Metrics, TypeRole};
use crate::tokens::Tokens;
use crate::typeface::*;
use crate::widgets;

#[test]
fn a_role_is_a_style_and_the_families_and_scripts_are_named() {
    assert_eq!([FontRole::Body, FontRole::Ui, FontRole::Mono, FontRole::Display].map(FontRole::style), [TextStyle::Regular, TextStyle::Regular, TextStyle::Mono, TextStyle::Bold]);
    for f in Family::ALL {
        assert_eq!(Family::from_key(f.key()), Some(f));
        assert!(!f.css().is_empty());
    }
    assert!(Family::from_key("comic-sans").is_none());
    // what is bundled: Latin, Greek, Cyrillic, the Noto scripts of the licence file; CJK is not
    let t = TypeStyle::default();
    assert!(t.uncovered("Hello, \u{3b1}\u{3b2} \u{43f}\u{440}\u{438} \u{928}\u{92e}\u{938}\u{94d}\u{924}\u{947} \u{ba4}\u{bae}\u{bbf}\u{bb4}\u{bcd} \u{639}\u{631}\u{628}\u{64a} \u{5e9}\u{5dc}\u{5d5}\u{5dd} 123").is_empty());
    assert_eq!(t.uncovered("abc \u{6f22}\u{5b57} \u{3053}\u{3093} \u{d55c}\u{ae00}"), vec![Script::Cjk]);
    for s in [Script::Devanagari, Script::Tamil, Script::Thai, Script::Arabic] {
        assert!(s.noto().unwrap().starts_with("Noto Sans "));
    }
    assert!(Script::Latin.noto().is_none() && !Script::Cjk.bundled());
    assert_eq!((script_of('\u{b95}'), script_of('\u{905}'), script_of('a')), (Script::Tamil, Script::Devanagari, Script::Latin));
}

#[test]
fn the_type_style_is_clamped_and_line_height_moves_the_leading() {
    let t = TypeStyle::new(Family::Liberation, 9.0, 5.0, true);
    assert_eq!((t.line_height, t.tracking), (1.8, TextOpts::MAX_TRACKING));
    assert_eq!(TypeStyle::new(Family::Default, f32::NAN, f32::NAN, false).line_height, TypeStyle::NORMAL_LINE);
    let plain = Metrics::new(800.0, 600.0, 1.0, 1.0, false);
    let loose = plain.with_type(TypeStyle::new(Family::Default, 1.6, 0.0, false));
    assert_eq!(plain.leading(10.0), 13.5);
    assert_eq!(loose.leading(10.0), 16.0);
    let spec = loose.text_style(FontRole::Mono, TypeRole::Body);
    assert_eq!((spec.style, spec.size, spec.leading), (TextStyle::Mono, 14.0, 14.0 * 1.6));
}

#[test]
fn letter_spacing_widens_the_measure_and_the_run_carries_the_family_into_the_list() {
    let mut plain = ListCanvas::new(100.0, 100.0);
    let w0 = plain.measure(10.0, "abcd", TextStyle::Regular);
    let mut c = ListCanvas::new(100.0, 100.0);
    c.set_type(TextOpts { family: Family::Liberation, tracking: 0.1, tabular: true });
    assert!((c.measure(10.0, "abcd", TextStyle::Regular) - (w0 + 4.0)).abs() < 0.01);
    let adv = c.text(1.0, 2.0, 10.0, [1, 2, 3, 255], "a b", TextStyle::Bold);
    assert!((adv - (ListCanvas::new(1.0, 1.0).measure(10.0, "a b", TextStyle::Bold) + 3.0)).abs() < 0.01);
    c.set_type(TextOpts::default());
    c.text(1.0, 2.0, 10.0, [1, 2, 3, 255], "plain", TextStyle::Regular);
    let l = c.finish();
    assert!(matches!(&l.ops[0], Op::TextEx { opts, .. } if opts.family == Family::Liberation && opts.tabular));
    assert!(matches!(&l.ops[1], Op::Text { .. }), "a default run stays a plain `text`");
    // the wire: version 2 because of the run, and it reads back the same
    let text = l.to_text();
    assert!(text.starts_with("iyakki-display-list 2 "), "{text}");
    assert!(text.contains("textx 1 2 10 010203ff bold liberation 0.1 1 a%20b"), "{text}");
    assert_eq!(DisplayList::parse(&text).unwrap(), l);
    // replaying it on another canvas keeps the run's options and puts the canvas's own back afterwards
    let mut other = ListCanvas::new(100.0, 100.0);
    other.set_type(TextOpts { family: Family::DejaVu, tracking: 0.0, tabular: false });
    l.replay(&mut other);
    assert_eq!(other.type_opts().family, Family::DejaVu);
    let again = other.finish();
    assert!(matches!(&again.ops[0], Op::TextEx { opts, .. } if opts.family == Family::Liberation));
    assert!(matches!(&again.ops[1], Op::TextEx { opts, .. } if opts.family == Family::DejaVu), "the second run takes the canvas's own family again");
}

#[test]
fn a_list_without_version_2_parts_is_still_written_as_version_1_and_a_newer_one_is_refused() {
    let mut c = ListCanvas::new(50.0, 50.0);
    c.text(0.0, 0.0, 10.0, [0, 0, 0, 255], "x", TextStyle::Regular);
    assert!(c.finish().to_text().starts_with("iyakki-display-list 1 "));
    let e = DisplayList::parse("iyakki-display-list 3 10 10\n").unwrap_err();
    assert!(e.contains("newer"), "{e}");
    assert!(DisplayList::parse("iyakki-display-list 2 10 10\ntextx 0 0 10 000000ff bold comic 0 0 x\n").unwrap_err().contains("family"));
    assert!(DisplayList::parse("iyakki-display-list 2 10 10\ntextx 0 0 10 000000ff bold dejavu 0 2 x\n").unwrap_err().contains("tabular"));
    // a tracking out of range is clamped, never trusted
    let l = DisplayList::parse("iyakki-display-list 2 10 10\ntextx 0 0 10 000000ff bold dejavu 9 0 x\n").unwrap();
    assert!(matches!(&l.ops[0], Op::TextEx { opts, .. } if opts.tracking == TextOpts::MAX_TRACKING));
}

#[test]
fn the_display_list_alpha5_wrote_is_read_and_written_back_byte_for_byte() {
    let fixture = include_str!("../../../tests/compat/alpha5/display-list-v2.txt");
    let l = DisplayList::parse(fixture).expect("the alpha.5 list parses");
    assert_eq!(l.to_text(), fixture);
    assert!(l.ops.iter().any(|o| matches!(o, Op::Blur(..))) && l.ops.iter().any(|o| matches!(o, Op::TextEx { .. })));
    assert_eq!(l.texts(), vec!["Total 1,234.50", "Plain"]);
}

fn real_measure() -> Box<dyn FnMut(f32, &str, TextStyle) -> f32> {
    let meter = RefCell::new(Gfx::new(8, 8));
    Box::new(move |size, s, st| meter.borrow_mut().text_width(size, s, match st { TextStyle::Regular => Face::Sans, TextStyle::Bold => Face::Bold, TextStyle::Mono => Face::Mono }))
}

#[test]
fn the_pixel_canvas_spaces_letters_and_blurs_what_lies_behind_glass() {
    let draw = |tracking: f32| {
        let mut g = Gfx::new(200, 40);
        let mut c = RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 200.0, h: 40.0 }, 1.0);
        c.set_type(TextOpts { family: Family::Default, tracking, tabular: false });
        let adv = c.text(2.0, 2.0, 14.0, [255, 255, 255, 255], "Spacing", TextStyle::Regular);
        assert!((adv - c.measure(14.0, "Spacing", TextStyle::Regular)).abs() < 0.5, "text and measure agree");
        (adv, g.rgba().to_vec())
    };
    let (a0, p0) = draw(0.0);
    let (a1, p1) = draw(0.1);
    assert!(a1 > a0 + 6.0 * 1.0, "seven letters, each followed by 1.4 units: {a0} then {a1}");
    assert!(p0 != p1);
    // blur: a hard black and white edge becomes a ramp
    let mut g = Gfx::new(40, 10);
    {
        let mut c = RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 40.0, h: 10.0 }, 1.0);
        c.rect(Rect::new(0.0, 0.0, 40.0, 10.0), [0, 0, 0, 255]);
        c.rect(Rect::new(20.0, 0.0, 20.0, 10.0), [255, 255, 255, 255]);
    }
    let at = |g: &Gfx, x: usize| g.rgba()[(5 * 40 + x) * 4];
    assert_eq!((at(&g, 18), at(&g, 22)), (0, 255));
    {
        let mut c = RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 40.0, h: 10.0 }, 1.0);
        c.blur(Rect::new(0.0, 0.0, 40.0, 10.0), 8.0);
    }
    assert!(at(&g, 18) > 0 && at(&g, 22) < 255, "the edge is soft now: {} {}", at(&g, 18), at(&g, 22));
}

// ---- the matrix: 3 themes x 3 size classes x text 1x, 2x, 3x x 2 families

fn themed(id: ThemeId) -> Look {
    let mut l = Look::default();
    l.theme = *id.theme();
    l
}

fn pale() -> Look {
    // a pale skin: the shipped one called Paper
    iyakki_uikit::skins::shipped().into_iter().find(|(k, _)| k == "paper").unwrap().1.unwrap().look
}

fn assistant_view() -> ChatView {
    let (port, _h) = SharedAssistant::new(vec![]);
    let mut v = ChatView::new(Box::new(port), SessionConfig { conversation: "m".into(), title: "M".into(), ..Default::default() }, 900);
    v.send("a rather long first message that has to wrap on a narrow phone at the largest text size");
    v
}

#[test]
fn every_widget_fits_reads_and_is_big_enough_at_three_themes_three_size_classes_three_text_sizes_and_two_families() {
    let mut looks: Vec<Look> = ALL.iter().map(|t| themed(*t)).collect();
    looks.push(pale());
    let mut cases = 0;
    for look in &looks {
        let th = &look.theme;
        // contrast: the pairs the widgets draw
        assert!(contrast(th.text, th.card) >= 4.5 && contrast(th.text, th.panel) >= 4.5 && contrast(th.text, th.field) >= 4.5, "{}: text on surfaces", look.name);
        assert!(contrast(iyakki_uikit::skins::label_on(th, th.accent), th.accent) >= 4.5, "{}: label on accent", look.name);
        assert!(contrast(th.accent, th.card) >= 3.0, "{}: accent against the card", look.name);
        for (w, touch) in [(360.0f32, true), (800.0, false), (1280.0, false)] {
            for scale in [1.0f32, 2.0, 3.0] {
                for family in [Family::DejaVu, Family::Liberation] {
                    iyakki_uikit::fonts::set_ui(match family {
                        Family::Liberation => iyakki_uikit::fonts::UiFont::Liberation,
                        _ => iyakki_uikit::fonts::UiFont::DejaVu,
                    });
                    for tracking in [0.0f32, 0.08] {
                        let ty = TypeStyle::new(family, 1.35, tracking, true);
                        let m = Metrics::new(w, 900.0, 1.0, scale, touch).with_tokens(Tokens::of(look)).with_type(ty);
                        let mut c = ListCanvas::with_measure(w, 3000.0, real_measure());
                        c.set_type(ty.opts());
                        let area = Rect::new(8.0, 8.0, w - 16.0, 2900.0);
                        let b = widgets::button(&mut c, th, &m, Rect::new(area.x, 10.0, area.w, 20.0), "A rather long button label that needs to be cut 1,234.50", 1, true);
                        let r = widgets::row(&mut c, th, &m, Rect::new(area.x, 100.0, area.w, 10.0), "A row with a long title that is too wide for a phone screen at the largest text", true, 2);
                        let f = widgets::field(&mut c, th, &m, Rect::new(area.x, 200.0, area.w, 10.0), "Name", "something typed that is much longer than the field can show", true, 3);
                        let p = widgets::wallpaper_picker(&mut c, th, &m, Rect::new(area.x, 300.0, area.w, 400.0), iyakki_uikit::wallpaper::Id::Dusk, 100);
                        for used in [b, r, f, p] {
                            assert!(used.x >= area.x - 0.01 && used.right() <= area.right() + 0.01, "{} {w} x{scale} {family:?}: {used:?}", look.name);
                        }
                        assert!(b.h >= m.min_target() && r.h >= m.min_target() && f.h >= m.min_target(), "targets at {w} x{scale}");
                        let mut chat = assistant_view();
                        chat.draw(&mut c, th, &m, Rect::new(area.x, 800.0, area.w, 1500.0));
                        let l = c.finish();
                        let mut meter = ListCanvas::with_measure(1.0, 1.0, real_measure());
                        for op in &l.ops {
                            let (x, size, text, style, opts) = match op {
                                Op::Text { x, size, text, style, .. } => (*x, *size, text, *style, TextOpts::default()),
                                Op::TextEx { x, size, text, style, opts, .. } => (*x, *size, text, *style, *opts),
                                _ => continue,
                            };
                            meter.set_type(opts);
                            let tw = meter.measure(size, text, style);
                            assert!(x + tw <= area.right() + 0.5, "{} {w} x{scale} {family:?} tracking {tracking}: text runs off the space: {text} ({x} + {tw})", look.name);
                        }
                        // every control is named and at least as big as the smallest target
                        for op in &l.ops {
                            if let Op::Hit(r, h) = op {
                                assert!(!h.name.is_empty());
                                if matches!(h.role, crate::canvas::Role::Button | crate::canvas::Role::ListItem | crate::canvas::Role::Field | crate::canvas::Role::Tab) {
                                    assert!(r.w > 0.0 && r.h + 0.01 >= m.min_target(), "{} is {} high, the smallest target is {}", h.name, r.h, m.min_target());
                                }
                            }
                        }
                        assert!(l.a11y().iter().any(|n| n.name == "Wallpaper Dusk"));
                        cases += 1;
                    }
                }
            }
        }
    }
    iyakki_uikit::fonts::set_ui(iyakki_uikit::fonts::UiFont::DejaVu);
    assert_eq!(cases, 4 * 3 * 3 * 2 * 2);
}

#[test]
fn every_shipped_skin_draws_only_radii_its_tokens_allow() {
    for (key, r) in iyakki_uikit::skins::shipped() {
        let look = r.unwrap().look;
        let t = Tokens::of(&look);
        let m = Metrics::new(800.0, 600.0, 1.0, 1.0, false).with_tokens(t);
        let mut c = ListCanvas::new(800.0, 600.0);
        widgets::button(&mut c, &look.theme, &m, Rect::new(10.0, 10.0, 200.0, 44.0), "Go", 1, true);
        widgets::row(&mut c, &look.theme, &m, Rect::new(10.0, 70.0, 200.0, 44.0), "Row", false, 2);
        widgets::field(&mut c, &look.theme, &m, Rect::new(10.0, 130.0, 200.0, 44.0), "F", "", false, 3);
        widgets::panel(&mut c, &look.theme, &m, Rect::new(10.0, 200.0, 300.0, 200.0));
        let l = c.finish();
        let max = t.radius(crate::tokens::Radius::Lg);
        for o in &l.ops {
            match o {
                Op::RoundRect(_, rad, _) | Op::Outline(_, rad, _, _) | Op::Shadow(_, rad, _, _) => assert!(*rad <= max + 0.01, "{key}: radius {rad} over {max}"),
                _ => {}
            }
        }
        assert_eq!(l.ops.iter().any(|o| matches!(o, Op::Shadow(..))), t.shadow_spread() > 0.0, "{key}: shadows follow the elevation");
        assert_eq!(l.ops.iter().any(|o| matches!(o, Op::Blur(..))), t.glass && t.blur > 0.0, "{key}: blur follows glass");
    }
}

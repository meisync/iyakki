//! Display list version 3: paths, layers and cluster runs (F-SDK-35).

use crate::canvas::{Canvas, TextStyle};
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::path::Seg;

fn tri() -> Vec<Seg> {
    vec![Seg::Move(0.0, 0.0), Seg::Line(20.0, 0.0), Seg::Line(0.0, 20.0), Seg::Close]
}

#[test]
fn the_display_list_alpha7_wrote_is_read_and_written_back_byte_for_byte() {
    let fixture = include_str!("../../../tests/compat/alpha7/display-list-v3.txt");
    let l = DisplayList::parse(fixture).expect("the alpha.7 list parses");
    assert_eq!(l.to_text(), fixture);
    assert!(l.ops.iter().any(|o| matches!(o, Op::Path { fill: Some(_), stroke: Some(_), .. })));
    assert!(l.ops.iter().any(|o| matches!(o, Op::PushLayer(a) if *a == 0.5)));
    assert!(l.ops.iter().any(|o| matches!(o, Op::TextC { advances, .. } if advances.len() == 3)));
    assert_eq!(l.texts(), vec!["e\u{0301}xy"]);
}

#[test]
fn a_list_without_version_3_parts_is_still_written_as_before_and_a_newer_one_is_refused() {
    let mut c = ListCanvas::new(50.0, 50.0);
    c.rect(Rect::new(0.0, 0.0, 5.0, 5.0), [0, 0, 0, 255]);
    assert!(c.finish().to_text().starts_with("iyakki-display-list 1 "));
    let mut c = ListCanvas::new(50.0, 50.0);
    c.path(Some([1, 2, 3, 255]), None, &tri());
    assert!(c.finish().to_text().starts_with("iyakki-display-list 3 "));
    assert!(DisplayList::parse("iyakki-display-list 4 10 10\n").unwrap_err().contains("newer"));
}

#[test]
fn a_canvas_records_a_path_a_layer_and_a_cluster_run_and_the_list_replays_the_same() {
    let mut c = ListCanvas::new(100.0, 100.0);
    c.layer(0.4, &mut |cc| {
        cc.path(Some([9, 9, 9, 255]), Some(([1, 1, 1, 255], 2.0)), &tri());
        cc.clipped(Rect::new(0.0, 0.0, 10.0, 10.0), &mut |c3| c3.rect(Rect::new(0.0, 0.0, 4.0, 4.0), [5, 5, 5, 255]));
    });
    c.text(5.0, 5.0, 12.0, [0, 0, 0, 255], "ne\u{0301}", TextStyle::Regular);
    c.text(5.0, 25.0, 12.0, [0, 0, 0, 255], "plain", TextStyle::Regular);
    let l = c.finish();
    assert!(matches!(l.ops[0], Op::PushLayer(a) if a == 0.4));
    assert!(l.ops.iter().filter(|o| matches!(o, Op::TextC { .. })).count() == 1, "only the run with a mark is a cluster run");
    assert!(l.ops.iter().any(|o| matches!(o, Op::Text { text, .. } if text == "plain")));
    let back = DisplayList::parse(&l.to_text()).unwrap();
    assert_eq!(back, l);
    let mut again = ListCanvas::new(100.0, 100.0);
    l.replay(&mut again);
    assert_eq!(again.finish(), l);
    // a moved list moves the path and the run
    let moved = l.translated(3.0, 4.0, 0, 10);
    assert!(matches!(&moved.ops[1], Op::Path { segs, .. } if segs[0] == Seg::Move(3.0, 4.0)));
}

#[test]
fn a_layer_fades_the_colours_of_every_operation_inside_and_nests() {
    let mut inner = ListCanvas::new(20.0, 20.0);
    {
        let mut c = crate::canvas::AlphaCanvas { inner: &mut inner, alpha: 0.5 };
        c.rect(Rect::new(0.0, 0.0, 2.0, 2.0), [10, 20, 30, 200]);
        c.layer(0.5, &mut |cc| cc.rect(Rect::new(0.0, 0.0, 2.0, 2.0), [10, 20, 30, 200]));
        c.path(Some([1, 1, 1, 100]), Some(([2, 2, 2, 100], 1.0)), &tri());
    }
    let l = inner.finish();
    assert!(matches!(l.ops[0], Op::Rect(_, c) if c[3] == 100));
    assert!(matches!(l.ops[1], Op::PushLayer(a) if a == 0.25));
    assert!(matches!(&l.ops[4], Op::Path { fill: Some(f), stroke: Some((s, _)), .. } if f[3] == 50 && s[3] == 50));
}

#[test]
fn a_damaged_version_3_line_is_refused_by_line_and_never_skipped() {
    let head = "iyakki-display-list 3 10 10\n";
    let bad = |body: &str| DisplayList::parse(&format!("{head}{body}")).unwrap_err();
    assert!(bad("path - - 0 M0:0;L1:1\n").contains("fill or a stroke"));
    assert!(bad("path ff0000ff - 0 L1:1\n").contains("starts with a move"));
    assert!(bad("path ff0000ff - 0 M0:0;X\n").contains("unknown path segment"));
    assert!(bad("path ff0000ff - 0\n").contains("malformed"));
    assert!(bad("layer 0.5\nclip 0 0 1 1\nendlayer\nendclip\n").contains("endlayer without layer"));
    assert!(bad("endlayer\n").contains("without layer"));
    assert!(bad("layer 0.5\n").contains("never closed"));
    assert!(bad("textc 0 0 10 000000ff regular default 0 0 5,5 abc\n").contains("one advance for every cluster"));
    assert!(bad("textc 0 0 10 000000ff regular default 0 0 5 ab\n").contains("one advance"));
    let deep = "layer 1\n".repeat(65);
    assert!(bad(&deep).contains("too deep"));
    // an opacity out of range is clamped
    let l = DisplayList::parse(&format!("{head}layer 7\nendlayer\n")).unwrap();
    assert!(matches!(l.ops[0], Op::PushLayer(a) if a == 1.0));
}

#[test]
fn a_canvas_that_knows_no_paths_still_shows_the_shape_as_bands_and_lines() {
    // the default `path` of the trait, through a canvas that only records rectangles and lines
    struct Bare(Vec<String>);
    impl Canvas for Bare {
        fn size(&self) -> (f32, f32) {
            (50.0, 50.0)
        }
        fn rect(&mut self, r: Rect, _: crate::geom::Color) {
            self.0.push(format!("rect {} {} {}", r.x, r.y, r.w));
        }
        fn rrect(&mut self, _: Rect, _: f32, _: crate::geom::Color) {}
        fn outline(&mut self, _: Rect, _: f32, _: f32, _: crate::geom::Color) {}
        fn line(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: crate::geom::Color) {
            self.0.push("line".into());
        }
        fn gradient(&mut self, _: Rect, _: crate::geom::Color, _: crate::geom::Color) {}
        fn shadow(&mut self, _: Rect, _: f32, _: f32, _: crate::geom::Color) {}
        fn text(&mut self, _: f32, _: f32, _: f32, _: crate::geom::Color, _: &str, _: TextStyle) -> f32 {
            0.0
        }
        fn measure(&mut self, _: f32, _: &str, _: TextStyle) -> f32 {
            0.0
        }
        fn clipped(&mut self, _: Rect, f: &mut dyn FnMut(&mut dyn Canvas)) {
            f(self)
        }
        fn layer(&mut self, _: f32, f: &mut dyn FnMut(&mut dyn Canvas)) {
            f(self)
        }
        fn hit(&mut self, _: Rect, _: crate::canvas::Hit) {}
    }
    let mut b = Bare(Vec::new());
    b.path(Some([0, 0, 0, 255]), Some(([0, 0, 0, 255], 1.0)), &tri());
    assert_eq!(b.0.iter().filter(|s| s.starts_with("rect")).count(), 20, "one band per row of the 20 unit triangle");
    assert_eq!(b.0.iter().filter(|s| *s == "line").count(), 3);
    assert!(b.0[0].starts_with("rect 0 0 ") && b.0[19].starts_with("rect 0 19 "));
}

#[test]
fn the_pixel_canvas_fills_a_path_with_a_hole_and_fades_a_layer() {
    use iyakki_uikit::gfx::Gfx;
    use iyakki_uikit::ui::R;
    let ring = vec![
        Seg::Move(1.0, 1.0),
        Seg::Line(19.0, 1.0),
        Seg::Line(19.0, 19.0),
        Seg::Line(1.0, 19.0),
        Seg::Close,
        Seg::Move(6.0, 6.0),
        Seg::Line(6.0, 14.0),
        Seg::Line(14.0, 14.0),
        Seg::Line(14.0, 6.0),
        Seg::Close,
    ];
    let mut g = Gfx::new(20, 20);
    {
        let mut c = crate::raster::RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 20.0, h: 20.0 }, 1.0);
        c.rect(Rect::new(0.0, 0.0, 20.0, 20.0), [0, 0, 0, 255]);
        c.layer(0.5, &mut |cc| cc.path(Some([255, 255, 255, 255]), None, &ring));
    }
    let px = |x: usize, y: usize| g.rgba()[(y * 20 + x) * 4];
    assert!((px(3, 3) as i32 - 128).abs() <= 2, "half white on black: {}", px(3, 3));
    assert_eq!(px(10, 10), 0, "the hole stays black");
    assert_eq!(px(0, 0), 0);
}

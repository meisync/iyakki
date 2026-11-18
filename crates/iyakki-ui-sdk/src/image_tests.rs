//! The picture operation of the display list (`Canvas::image`, the `image` line; F-SDK-164): wire format, limits, replay, pixels.

use crate::canvas::{contain, Canvas, Image, ImageFormat, MAX_IMAGE_BYTES};
use crate::geom::Rect;
use crate::list::{b64_decode, b64_encode, DisplayList, ListCanvas, Op};
use crate::raster::RasterCanvas;
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::R;
use std::sync::Arc;

fn rgba(w: u32, h: u32, c: [u8; 4], seq: u64) -> Image {
    Image { format: ImageFormat::Rgba, width: w, height: h, seq, data: Arc::new(c.iter().copied().cycle().take((w * h * 4) as usize).collect()) }
}

fn jpeg(seq: u64) -> Image {
    Image { format: ImageFormat::Jpeg, width: 48, height: 32, seq, data: Arc::new(vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9]) }
}

#[test]
fn base64_round_trips_every_length_and_refuses_what_is_not_base64() {
    for n in 0..40usize {
        let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
        assert_eq!(b64_decode(&b64_encode(&b)).unwrap(), b, "length {n}");
    }
    assert_eq!(b64_encode(b"Man"), "TWFu");
    assert_eq!(b64_encode(b"Ma"), "TWE=");
    assert!(b64_decode("TWE").is_none() && b64_decode("T*Fu").is_none() && b64_decode("TW==TWFu").is_none() && b64_decode("=WFu").is_none());
}

#[test]
fn a_picture_survives_the_wire_replays_the_same_and_moves_with_a_translated_list() {
    let mut lc = ListCanvas::new(200.0, 100.0);
    lc.image(Rect::new(10.0, 20.0, 60.0, 40.0), &rgba(3, 2, [9, 8, 7, 255], 41));
    lc.image(Rect::new(80.0, 20.0, 60.0, 40.0), &jpeg(5));
    let l = lc.finish();
    let text = l.to_text();
    assert!(text.lines().nth(1).unwrap().starts_with("image 10 20 60 40 rgba 3 2 41 "), "{text}");
    assert!(text.lines().nth(2).unwrap().starts_with("image 80 20 60 40 jpeg 48 32 5 "));
    let back = DisplayList::parse(&text).unwrap();
    assert_eq!(back, l);
    assert_eq!(back.to_text(), text);
    let mut again = ListCanvas::new(200.0, 100.0);
    back.replay(&mut again);
    assert_eq!(again.finish(), l);
    let moved = back.translated(5.0, 6.0, 100, 200);
    assert!(matches!(&moved.ops[0], Op::Image(r, i) if *r == Rect::new(15.0, 26.0, 60.0, 40.0) && i.seq == 41));
}

#[test]
fn a_damaged_or_oversized_picture_is_refused_by_line_and_a_canvas_never_records_one() {
    let ok = {
        let mut lc = ListCanvas::new(100.0, 100.0);
        lc.image(Rect::new(0.0, 0.0, 10.0, 10.0), &rgba(2, 2, [1, 2, 3, 255], 1));
        lc.finish().to_text()
    };
    let line = ok.lines().nth(1).unwrap().to_string();
    let with = |l: &str| format!("iyakki-display-list 1 100 100\n{l}\n");
    assert!(DisplayList::parse(&with(&line)).is_ok());
    let cases = [
        (line.replace(" rgba ", " png "), "unknown image format"),
        (line.replace(" 2 2 1 ", " 3 2 1 "), "width x height x 4"),
        (line.replace(" 2 2 1 ", " 0 2 1 "), "pixels each way"),
        (line.replace(" 2 2 1 ", " 9999 2 1 "), "pixels each way"),
        (line.rsplit_once(' ').unwrap().0.to_string() + " not*base64!", "bad image data"),
        (line.replace(" rgba 2 2 1 ", " jpeg 2 2 1 "), "starts with FF D8"),
        (line.rsplit_once(' ').unwrap().0.to_string(), "unknown or malformed"),
    ];
    for (l, want) in cases {
        let e = DisplayList::parse(&with(&l)).unwrap_err();
        assert!(e.contains("line 2") && e.contains(want), "{want}: {e}");
    }
    let big = format!("image 0 0 1 1 jpeg 2 2 1 {}", "A".repeat(MAX_IMAGE_BYTES / 3 * 4 + 16));
    assert!(DisplayList::parse(&with(&big)).unwrap_err().contains("too big"));
    // a canvas drops a picture the reader would refuse
    let mut lc = ListCanvas::new(10.0, 10.0);
    lc.image(Rect::new(0.0, 0.0, 5.0, 5.0), &Image { format: ImageFormat::Rgba, width: 2, height: 2, seq: 1, data: Arc::new(vec![0; 3]) });
    assert!(lc.finish().ops.is_empty());
    // an old reader (this one before the operation) would say unknown operation 'image', by name: the rule for every additive operation
    assert!(DisplayList::parse("iyakki-display-list 1 10 10\nimage 1\n").unwrap_err().contains("'image'"));
}

#[test]
fn the_pixel_canvas_stretches_rgba_into_the_rectangle_only_and_draws_nothing_for_a_jpeg() {
    let mut g = Gfx::new(40, 20);
    {
        let mut c = RasterCanvas::new(&mut g, R { x: 0.0, y: 0.0, w: 40.0, h: 20.0 }, 1.0);
        c.image(Rect::new(10.0, 5.0, 20.0, 10.0), &rgba(2, 2, [200, 40, 40, 255], 1));
        c.image(Rect::new(0.0, 0.0, 5.0, 5.0), &jpeg(2));
    }
    let px = |x: usize, y: usize| {
        let i = (y * 40 + x) * 4;
        [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2], g.rgba()[i + 3]]
    };
    assert_eq!(px(20, 10), [200, 40, 40, 255], "inside");
    assert_eq!(px(11, 6), [200, 40, 40, 255], "the corner is covered");
    assert_eq!(px(5, 10)[3], 0, "outside is untouched");
    assert_eq!(px(2, 2)[3], 0, "a JPEG draws nothing on pixels: the core is asked for RGBA there");
}

#[test]
fn contain_letterboxes_inside_the_rectangle_and_keeps_the_aspect() {
    let r = contain(Rect::new(0.0, 0.0, 200.0, 100.0), 48, 32);
    assert!((r.w / r.h - 1.5).abs() < 1e-4 && r.h == 100.0 && (r.x - (200.0 - r.w) / 2.0).abs() < 1e-3 && r.y == 0.0);
    let t = contain(Rect::new(10.0, 10.0, 90.0, 300.0), 48, 32);
    assert!(t.w == 90.0 && (t.h - 60.0).abs() < 1e-3 && t.y > 10.0);
    assert_eq!(contain(Rect::new(0.0, 0.0, 0.0, 10.0), 4, 4).w, 0.0);
}

/// The picture list the first release with the operation wrote: every later release reads it and writes it back byte for byte.
#[test]
fn the_picture_list_alpha5_wrote_is_still_read_and_written_the_same() {
    let fixture = include_str!("../../../tests/compat/alpha5/display-list-v1-image.txt");
    let l = DisplayList::parse(fixture).expect("the alpha.5 picture list parses");
    assert_eq!(l.ops.len(), 2);
    assert!(matches!(&l.ops[1], Op::Image(_, i) if i.seq == 7 && i.width == 2 && i.data.len() == 16 && i.data[3] == 255));
    assert_eq!(l.to_text(), fixture);
}

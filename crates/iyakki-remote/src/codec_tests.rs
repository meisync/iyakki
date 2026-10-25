//! Round trips for every tile type, odd sizes, damage rectangles, lossy bounds, refinement, budgets and hostile data.

use crate::codec::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn blank(w: usize, h: usize, c: [u8; 3]) -> Vec<u8> {
    (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect()
}

fn set(img: &mut [u8], w: usize, x: usize, y: usize, c: [u8; 3]) {
    img[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&c);
}

fn opts(level: Level, deflate: bool, now_ms: u64) -> Opts {
    Opts { level, caps: Caps { deflate }, budget: usize::MAX, now_ms }
}

/// Send a picture until nothing more is due; returns the canvas the page ends up with and the messages sent.
fn deliver(st: &mut Streamer, canvas: &mut Canvas, level: Level, deflate: bool, now: u64) -> Vec<Encoded> {
    let mut sent = Vec::new();
    for _ in 0..50 {
        match st.encode(&opts(level, deflate, now)) {
            Some(e) => {
                decode_frame(&e.msg, canvas).expect("decodes");
                sent.push(e);
            }
            None => break,
        }
        if !st.backlog_more() {
            break;
        }
    }
    sent
}

impl Streamer {
    fn backlog_more(&self) -> bool {
        self.backlog()
    }
}

fn same(canvas: &Canvas, img: &[u8], w: usize, h: usize) {
    assert_eq!((canvas.w, canvas.h), (w, h));
    assert!(canvas.rgba == img, "the page's picture differs from the source");
}

fn photo(w: usize, h: usize, rng: &mut Rng) -> Vec<u8> {
    let mut img = blank(w, h, [0, 0, 0]);
    for y in 0..h {
        for x in 0..w {
            let n = rng.below(9) as i32 - 4;
            let c = |v: i32| (v + n).clamp(0, 255) as u8;
            set(&mut img, w, x, y, [c((x * 255 / w.max(1)) as i32), c((y * 255 / h.max(1)) as i32), c(((x + y) * 128 / (w + h).max(1)) as i32 + 60)]);
        }
    }
    img
}

#[test]
fn filters_round_trip_on_random_rows() {
    let mut rng = Rng(7);
    for (row, rows, bpp) in [(3, 1, 3), (30, 5, 3), (17, 9, 1), (1, 1, 1), (192, 64, 3)] {
        let data: Vec<u8> = (0..row * rows).map(|_| rng.next() as u8).collect();
        let f = filter_rows(&data, row, rows, bpp);
        assert_eq!(unfilter_rows(&f, row, rows, bpp).unwrap(), data);
    }
    assert!(unfilter_rows(&[9, 0, 0, 0], 3, 1, 3).is_none(), "unknown filter");
    assert!(unfilter_rows(&[0, 0], 3, 1, 3).is_none(), "wrong length");
}

#[test]
fn every_tile_type_round_trips_exactly_with_and_without_deflate() {
    let mut rng = Rng(11);
    let (w, h) = (70usize, 66usize); // an edge tile on both sides
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push(("solid", blank(w, h, [12, 34, 56])));
    let mut two = blank(w, h, [255, 255, 255]);
    for x in 0..w {
        set(&mut two, w, x, 5, [0, 0, 0]);
    }
    cases.push(("two colours", two));
    let mut sixteen = blank(w, h, [0, 0, 0]);
    for i in 0..w * h {
        let v = (rng.below(16) * 16) as u8;
        sixteen[i * 4..i * 4 + 3].copy_from_slice(&[v, v, v]);
    }
    cases.push(("sixteen", sixteen));
    let mut fifty = blank(w, h, [0, 0, 0]);
    for i in 0..w * h {
        let v = rng.below(50) as u8 * 5;
        fifty[i * 4..i * 4 + 3].copy_from_slice(&[v, 255 - v, 7]);
    }
    cases.push(("fifty", fifty));
    cases.push(("gradient and noise", photo(w, h, &mut rng)));
    let mut noise = blank(w, h, [0, 0, 0]);
    for i in 0..w * h {
        noise[i * 4..i * 4 + 3].copy_from_slice(&[rng.next() as u8, rng.next() as u8, rng.next() as u8]);
    }
    cases.push(("pure noise", noise));
    for (name, img) in &cases {
        for deflate in [true, false] {
            let mut st = Streamer::new();
            st.set_source(w, h, img.clone());
            let mut canvas = Canvas::default();
            let sent = deliver(&mut st, &mut canvas, Level::Lossless, deflate, 0);
            assert!(!sent.is_empty(), "{name}");
            same(&canvas, img, w, h);
            assert!(st.encode(&opts(Level::Lossless, deflate, 10)).is_none(), "{name}: nothing more to send");
        }
    }
}

#[test]
fn tiny_odd_and_huge_pictures_round_trip() {
    let mut rng = Rng(3);
    for (w, h) in [(1usize, 1usize), (2, 1), (1, 9), (63, 63), (64, 64), (65, 65), (3, 200), (129, 7)] {
        let img = photo(w, h, &mut rng);
        for level in [Level::Lossless] {
            let mut st = Streamer::new();
            st.set_source(w, h, img.clone());
            let mut canvas = Canvas::default();
            deliver(&mut st, &mut canvas, level, true, 0);
            same(&canvas, &img, w, h);
        }
    }
    // a full 4K screen of UI-like content with a photo in it
    let (w, h) = (3840usize, 2160usize);
    let mut img = blank(w, h, [20, 24, 40]);
    let ph = photo(640, 360, &mut rng);
    for y in 0..360 {
        let dst = ((y + 100) * w + 200) * 4;
        img[dst..dst + 640 * 4].copy_from_slice(&ph[y * 640 * 4..(y + 1) * 640 * 4]);
    }
    let mut st = Streamer::new();
    st.set_source(w, h, img.clone());
    let mut canvas = Canvas::default();
    let sent = deliver(&mut st, &mut canvas, Level::Lossless, true, 0);
    same(&canvas, &img, w, h);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].tiles, 60 * 34);
    assert!(sent[0].msg.len() < 1_200_000, "a 4K screen with one photo should be about a megabyte at most: {}", sent[0].msg.len());
}

#[test]
fn only_the_tiles_that_changed_are_sent_and_an_unchanged_picture_sends_nothing() {
    let (w, h) = (200usize, 150usize);
    let mut img = blank(w, h, [10, 20, 30]);
    let mut st = Streamer::new();
    st.set_source(w, h, img.clone());
    let mut canvas = Canvas::default();
    let first = deliver(&mut st, &mut canvas, Level::Lossless, true, 0);
    assert_eq!(first[0].tiles, 4 * 3);
    // the same picture again: nothing
    st.set_source(w, h, img.clone());
    assert!(st.encode(&opts(Level::Lossless, true, 5)).is_none());
    // one pixel in the tile (1,1)
    set(&mut img, w, 70, 80, [255, 0, 0]);
    st.set_source(w, h, img.clone());
    let e = st.encode(&opts(Level::Lossless, true, 10)).unwrap();
    assert_eq!(e.tiles, 1);
    decode_frame(&e.msg, &mut canvas).unwrap();
    same(&canvas, &img, w, h);
    // a pixel in the last (partial) tile and one on a tile border: two and then two tiles
    set(&mut img, w, 199, 149, [1, 2, 3]);
    set(&mut img, w, 63, 0, [9, 9, 9]);
    set(&mut img, w, 64, 0, [9, 9, 9]);
    st.set_source(w, h, img.clone());
    let e = st.encode(&opts(Level::Lossless, true, 20)).unwrap();
    assert_eq!(e.tiles, 3, "last tile, and the two tiles either side of the border");
    decode_frame(&e.msg, &mut canvas).unwrap();
    same(&canvas, &img, w, h);
    // a resize sends everything at the new size
    let small = blank(100, 100, [1, 1, 1]);
    st.set_source(100, 100, small.clone());
    let e = st.encode(&opts(Level::Lossless, true, 30)).unwrap();
    assert_eq!(e.tiles, 4);
    decode_frame(&e.msg, &mut canvas).unwrap();
    same(&canvas, &small, 100, 100);
    // reset() (the page asked for everything) resends everything
    st.reset();
    assert!(st.wants_encode(31));
    assert_eq!(st.encode(&opts(Level::Lossless, true, 31)).unwrap().tiles, 4);
}

#[test]
fn random_edits_over_many_frames_always_converge_to_the_source_when_lossless() {
    let mut rng = Rng(99);
    let (w, h) = (257usize, 131usize);
    let mut img = photo(w, h, &mut rng);
    let mut st = Streamer::new();
    let mut canvas = Canvas::default();
    for step in 0..40u64 {
        for _ in 0..rng.below(30) {
            let (x, y) = (rng.below(w), rng.below(h));
            let rw = 1 + rng.below(40);
            for xx in x..(x + rw).min(w) {
                set(&mut img, w, xx, y, [rng.next() as u8, rng.next() as u8, rng.next() as u8]);
            }
        }
        st.set_source(w, h, img.clone());
        // a small budget makes some updates take several messages
        let o = Opts { level: Level::Lossless, caps: Caps { deflate: step % 2 == 0 }, budget: 3000, now_ms: step * 10 };
        let mut guard = 0;
        while let Some(e) = st.encode(&o) {
            decode_frame(&e.msg, &mut canvas).unwrap();
            guard += 1;
            assert!(guard < 200);
        }
        same(&canvas, &img, w, h);
    }
}

#[test]
fn the_byte_budget_splits_a_big_update_into_messages_and_nothing_is_lost() {
    let mut rng = Rng(5);
    let (w, h) = (640usize, 360usize);
    let img = photo(w, h, &mut rng);
    let mut st = Streamer::new();
    st.set_source(w, h, img.clone());
    let mut canvas = Canvas::default();
    let o = Opts { level: Level::Lossless, caps: Caps { deflate: true }, budget: 20_000, now_ms: 0 };
    let mut messages = 0;
    while let Some(e) = st.encode(&o) {
        assert!(e.body_len < 20_000 + 12_300, "a message stays near the budget: {}", e.body_len);
        decode_frame(&e.msg, &mut canvas).unwrap();
        messages += 1;
        assert!(st.backlog() || canvas.rgba == img);
    }
    assert!(messages > 3, "{messages}");
    same(&canvas, &img, w, h);
}

#[test]
fn text_and_ui_tiles_stay_exact_at_every_level_and_photo_tiles_stay_within_bounds() {
    let mut rng = Rng(21);
    let (w, h) = (192usize, 128usize);
    // left: UI/text-like (few colours); right: a photo
    let mut img = blank(w, h, [30, 34, 60]);
    for y in (4..h).step_by(12) {
        for x in 4..60 {
            if (x + y) % 3 != 0 {
                set(&mut img, w, x, y, [230, 230, 240]);
                set(&mut img, w, x, y + 1, [120, 130, 160]);
            }
        }
    }
    let ph = photo(128, 128, &mut rng);
    for y in 0..h {
        for x in 64..192 {
            let s = (y * 128 + (x - 64)) * 4;
            set(&mut img, w, x, y, [ph[s], ph[s + 1], ph[s + 2]]);
        }
    }
    for level in [Level::Balanced, Level::Low] {
        let mut st = Streamer::new();
        st.set_source(w, h, img.clone());
        let mut canvas = Canvas::default();
        let sent = deliver(&mut st, &mut canvas, level, true, 0);
        assert!(sent[0].lossy_tiles > 0, "{level:?}: the photo tiles are coded lossily");
        let mut worst = 0i32;
        for y in 0..h {
            for x in 0..w {
                let (a, b) = ((y * w + x) * 4, (y * w + x) * 4);
                let in_photo = x >= 64;
                for c in 0..3 {
                    let d = (canvas.rgba[a + c] as i32 - img[b + c] as i32).abs();
                    if !in_photo {
                        assert_eq!(d, 0, "{level:?}: UI pixel ({x},{y}) must be exact");
                    } else {
                        worst = worst.max(d);
                    }
                }
            }
        }
        let bound = if level == Level::Balanced { 5 } else { 60 };
        assert!(worst <= bound, "{level:?}: worst photo error {worst}");
    }
}

#[test]
fn lossy_tiles_are_sent_again_exactly_once_the_picture_is_still() {
    let mut rng = Rng(8);
    let (w, h) = (128usize, 64usize);
    let img = photo(w, h, &mut rng);
    let mut st = Streamer::new();
    st.set_source(w, h, img.clone());
    let mut canvas = Canvas::default();
    let first = deliver(&mut st, &mut canvas, Level::Low, true, 0);
    assert_eq!(first[0].lossy_tiles, 2);
    assert!(canvas.rgba != img);
    assert!(st.backlog(), "a refinement is waiting");
    assert!(!st.wants_encode(100), "not yet: the picture has not been still for long enough");
    assert!(st.encode(&opts(Level::Low, true, 100)).is_none());
    assert!(st.wants_encode(450));
    let e = st.encode(&opts(Level::Low, true, 450)).expect("refinement");
    assert_eq!((e.tiles, e.refined, e.lossy_tiles), (2, 2, 0));
    decode_frame(&e.msg, &mut canvas).unwrap();
    same(&canvas, &img, w, h);
    assert!(!st.backlog());
    assert!(st.encode(&opts(Level::Low, true, 5000)).is_none());
    // changing a tile makes it lossy again and restarts its clock
    let mut img2 = img.clone();
    set(&mut img2, w, 3, 3, [0, 0, 0]);
    st.set_source(w, h, img2.clone());
    let e = st.encode(&opts(Level::Low, true, 6000)).unwrap();
    assert_eq!((e.tiles, e.lossy_tiles), (1, 1));
}

#[test]
fn the_lowest_level_halves_the_picture_and_odd_sizes_work() {
    let mut rng = Rng(2);
    for (w, h) in [(101usize, 77usize), (641, 401), (2, 2)] {
        let img = photo(w, h, &mut rng);
        let mut st = Streamer::new();
        st.set_source(w, h, img.clone());
        let mut canvas = Canvas::default();
        deliver(&mut st, &mut canvas, Level::VeryLow, true, 0);
        let (half, hw, hh) = halve(&img, w, h);
        assert_eq!((canvas.w, canvas.h), (hw, hh));
        assert_eq!((hw, hh), (w.div_ceil(2), h.div_ceil(2)));
        // after refinement the page has exactly the halved picture
        let mut guard = 0;
        while let Some(e) = st.encode(&opts(Level::VeryLow, true, 10_000 + guard)) {
            decode_frame(&e.msg, &mut canvas).unwrap();
            guard += 1;
        }
        same(&canvas, &half, hw, hh);
    }
}

#[test]
fn a_level_change_from_lossy_to_lossless_refines_at_once() {
    let mut rng = Rng(4);
    let (w, h) = (64usize, 64usize);
    let img = photo(w, h, &mut rng);
    let mut st = Streamer::new();
    st.set_source(w, h, img.clone());
    let mut canvas = Canvas::default();
    deliver(&mut st, &mut canvas, Level::Balanced, true, 0);
    assert!(canvas.rgba != img);
    let e = st.encode(&opts(Level::Lossless, true, 1)).unwrap();
    decode_frame(&e.msg, &mut canvas).unwrap();
    same(&canvas, &img, w, h);
}

#[test]
fn compression_is_far_better_than_the_old_png_on_ui_and_text() {
    // a text-like screen
    let (w, h) = (1280usize, 720usize);
    let mut img = blank(w, h, [18, 22, 40]);
    for y in (20..h - 20).step_by(16) {
        for x in 40..w - 40 {
            if (x * 7 + y * 3) % 5 < 2 {
                set(&mut img, w, x, y, [225, 228, 240]);
                set(&mut img, w, x, y + 1, [90, 100, 140]);
            }
        }
    }
    let mut st = Streamer::new();
    st.set_source(w, h, img.clone());
    let e = st.encode(&opts(Level::Lossless, true, 0)).unwrap();
    let old = crate::screen::encode_region(&img, w, (0, 0, w, h)).unwrap().png.len();
    assert!(e.msg.len() < old, "tiles+deflate {} vs png {}", e.msg.len(), old);
}

#[test]
fn hostile_frame_data_is_refused_without_panics_or_big_allocations() {
    let mut rng = Rng(77);
    let mut st = Streamer::new();
    let (w, h) = (130usize, 70usize);
    let img = photo(w, h, &mut rng);
    st.set_source(w, h, img);
    let good = st.encode(&opts(Level::Lossless, true, 0)).unwrap().msg;
    let mut canvas = Canvas::default();
    assert!(decode_frame(&good, &mut canvas).is_ok());
    // every truncation
    for n in 0..good.len().min(400) {
        let _ = decode_frame(&good[..n], &mut Canvas::default());
    }
    // flipped bytes
    for _ in 0..3000 {
        let mut m = good.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(m.len());
            m[i] = rng.next() as u8;
        }
        let _ = decode_frame(&m, &mut Canvas::default());
    }
    // random bytes behind a plausible header
    for _ in 0..500 {
        let mut m = good[..HEADER].to_vec();
        m[1] = rng.below(2) as u8;
        let extra = rng.below(300);
        m.extend((0..extra).map(|_| rng.next() as u8));
        let _ = decode_frame(&m, &mut Canvas::default());
    }
    // sizes
    let mut m = good.clone();
    m[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
    m[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(decode_frame(&m, &mut Canvas::default()), Err(DecodeError::Size), "65535 x 65535 is refused before anything is allocated");
    let mut m = good.clone();
    m[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode_frame(&m, &mut Canvas::default()), Err(DecodeError::Size), "a 4 GiB body is refused");
    // a body that inflates to more than it says
    let mut m = good.clone();
    m[14..18].copy_from_slice(&10u32.to_le_bytes());
    assert!(decode_frame(&m, &mut Canvas::default()).is_err());
    let mut m = good.clone();
    m[0] = 1;
    assert_eq!(decode_frame(&m, &mut Canvas::default()), Err(DecodeError::Kind));
    assert_eq!(decode_frame(&[], &mut Canvas::default()), Err(DecodeError::Short));
}

#[test]
fn levels_have_names_and_steps() {
    for l in Level::ALL {
        assert_eq!(Level::parse(l.name()), Some(l));
        assert_eq!(Level::from_u8(l as u8), Some(l));
    }
    assert_eq!(Level::parse("ultra"), None);
    assert_eq!(Level::Lossless.higher(), Level::Lossless);
    assert_eq!(Level::VeryLow.lower(), Level::VeryLow);
    assert_eq!(Level::Low.lower(), Level::VeryLow);
    assert_eq!(Level::Low.higher(), Level::Balanced);
    assert_eq!(Level::VeryLow.scale(), 2);
    assert!(Level::Lossless.min_interval_ms() < Level::VeryLow.min_interval_ms());
}

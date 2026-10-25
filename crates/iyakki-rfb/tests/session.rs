//! A fake RFB server in-process: it sends raw, CopyRect, Hextile and ZRLE frames and malformed data to the client state machine.

use iyakki_rfb::inflate::Inflater;
use iyakki_rfb::{Error, Event, Link, Rect, Session};
use miniz_oxide::deflate::core::{compress, create_comp_flags_from_zip_params, CompressorOxide, TDEFLFlush};

fn init_msg(w: u16, h: u16, name: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    v.extend_from_slice(&(name.len() as u32).to_be_bytes());
    v.extend_from_slice(name.as_bytes());
    v
}

fn handshake(w: u16, h: u16) -> Vec<u8> {
    let mut v = b"RFB 003.008\n".to_vec();
    v.extend_from_slice(&[1, 1]); // one security type: None
    v.extend_from_slice(&0u32.to_be_bytes()); // result ok
    v.extend(init_msg(w, h, "test"));
    v
}

fn ready(w: u16, h: u16) -> Session {
    let mut s = Session::new();
    s.feed(&handshake(w, h)).unwrap();
    assert!(s.ready());
    s.take_output();
    s.take_events();
    s
}

fn rect_hdr(x: u16, y: u16, w: u16, h: u16, enc: i32) -> Vec<u8> {
    let mut v = Vec::new();
    for n in [x, y, w, h] {
        v.extend_from_slice(&n.to_be_bytes());
    }
    v.extend_from_slice(&enc.to_be_bytes());
    v
}

fn update(rects: Vec<Vec<u8>>) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend_from_slice(&(rects.len() as u16).to_be_bytes());
    for r in rects {
        v.extend(r);
    }
    v
}

fn px4(rgb: u32) -> [u8; 4] {
    [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8, 0]
}

fn px3(rgb: u32) -> [u8; 3] {
    [rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8]
}

struct Zlib {
    c: Box<CompressorOxide>,
}

impl Zlib {
    fn new() -> Zlib {
        Zlib { c: Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(6, 1, 0))) }
    }
    /// One chunk ending with a sync flush, as a real server does after each rectangle.
    fn chunk(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; data.len() * 2 + 1024];
        let (_, _, n) = compress(&mut self.c, data, &mut out, TDEFLFlush::Sync);
        out.truncate(n);
        out
    }
}

fn zrle_rect(x: u16, y: u16, w: u16, h: u16, z: &mut Zlib, tiles: &[u8]) -> Vec<u8> {
    let d = z.chunk(tiles);
    let mut v = rect_hdr(x, y, w, h, 16);
    v.extend_from_slice(&(d.len() as u32).to_be_bytes());
    v.extend(d);
    v
}

#[test]
fn the_handshake_asks_for_our_pixel_format_and_encodings() {
    let mut s = Session::new();
    s.feed(&handshake(64, 48)).unwrap();
    assert!(s.ready());
    assert_eq!((s.fb().w, s.fb().h, s.name()), (64, 48, "test"));
    let out = s.take_output();
    assert!(out.starts_with(b"RFB 003.008\n"));
    assert_eq!(out[12], 1, "security type None");
    assert_eq!(out[13], 1, "shared");
    assert_eq!(&out[14..18], &[0, 0, 0, 0], "SetPixelFormat");
    assert_eq!(&out[18..22], &[32, 24, 0, 1]);
    let enc = &out[14 + 20..];
    assert_eq!(&enc[..4], &[2, 0, 0, 6]);
    assert_eq!(&enc[4..8], &16i32.to_be_bytes(), "ZRLE first");
    let req = &out[out.len() - 10..];
    assert_eq!(req, &[3, 0, 0, 0, 0, 0, 0, 64, 0, 48], "a full update request");
    assert_eq!(s.take_events(), vec![Event::Ready { w: 64, h: 48 }]);
}

#[test]
fn version_3_3_and_3_7_work_and_a_password_is_refused() {
    let mut s = Session::new();
    let mut v = b"RFB 003.003\n".to_vec();
    v.extend_from_slice(&1u32.to_be_bytes());
    v.extend(init_msg(10, 10, "x"));
    s.feed(&v).unwrap();
    assert!(s.ready());
    assert!(s.take_output().starts_with(b"RFB 003.003\n\x01"), "3.3 has no security reply, just ClientInit");

    let mut s = Session::new();
    let mut v = b"RFB 003.007\n".to_vec();
    v.extend_from_slice(&[1, 1]);
    v.extend(init_msg(10, 10, "x"));
    s.feed(&v).unwrap();
    assert!(s.ready(), "3.7 has no security result for None");

    let mut s = Session::new();
    let mut v = b"RFB 003.008\n".to_vec();
    v.extend_from_slice(&[1, 2]);
    let e = s.feed(&v).unwrap_err();
    assert!(matches!(&e, Error::Refused(m) if m.contains("password")), "{e}");
    assert_eq!(s.feed(b"x"), Err(e), "a failed session stays failed");

    let mut s = Session::new();
    let mut v = b"RFB 003.008\n".to_vec();
    v.push(0);
    v.extend_from_slice(&4u32.to_be_bytes());
    v.extend_from_slice(b"busy");
    assert!(matches!(s.feed(&v), Err(Error::Refused(m)) if m.contains("busy")));

    let mut s = Session::new();
    assert!(matches!(s.feed(b"HTTP/1.1 200\r\n"), Err(Error::Protocol(_))));
}

#[test]
fn raw_and_copyrect_rectangles_draw() {
    let mut s = ready(8, 4);
    let mut raw = rect_hdr(0, 0, 2, 2, 0);
    for c in [0xff0000, 0x00ff00, 0x0000ff, 0x123456] {
        raw.extend_from_slice(&px4(c));
    }
    let mut copy = rect_hdr(4, 2, 2, 2, 1);
    copy.extend_from_slice(&[0, 0, 0, 0]);
    s.feed(&update(vec![raw, copy])).unwrap();
    assert_eq!(s.fb().get(0, 0), Some(0xff0000));
    assert_eq!(s.fb().get(1, 0), Some(0x00ff00));
    assert_eq!(s.fb().get(0, 1), Some(0x0000ff));
    assert_eq!(s.fb().get(5, 3), Some(0x123456));
    assert_eq!(s.take_events(), vec![Event::Updated(Rect { x: 0, y: 0, w: 6, h: 4 })], "changes are merged");
    let out = s.take_output();
    assert_eq!(out, vec![3, 1, 0, 0, 0, 0, 0, 8, 0, 4], "an incremental request follows an update");
}

#[test]
fn hextile_tiles_with_background_foreground_subrects_and_raw() {
    let mut s = ready(32, 16);
    let mut body = Vec::new();
    // tile 0: background + foreground + two coloured... use: bg, fg, 1 plain subrect at (1,2) size 3x2
    body.push(2 | 4 | 8);
    body.extend_from_slice(&px4(0x101010));
    body.extend_from_slice(&px4(0xff00ff));
    body.push(1);
    body.push(0x12);
    body.push(0x21);
    // tile 1: raw 16x16
    body.push(1);
    for i in 0..256u32 {
        body.extend_from_slice(&px4(i));
    }
    let mut r = rect_hdr(0, 0, 32, 16, 5);
    r.extend(body);
    s.feed(&update(vec![r])).unwrap();
    assert_eq!(s.fb().get(0, 0), Some(0x101010));
    assert_eq!(s.fb().get(1, 2), Some(0xff00ff));
    assert_eq!(s.fb().get(3, 3), Some(0xff00ff));
    assert_eq!(s.fb().get(4, 2), Some(0x101010));
    assert_eq!(s.fb().get(16, 0), Some(0));
    assert_eq!(s.fb().get(17, 0), Some(1));
    assert_eq!(s.fb().get(31, 15), Some(255));
    // a coloured subrect with the background kept from the earlier tile
    let mut s = ready(16, 16);
    let mut r = rect_hdr(0, 0, 16, 16, 5);
    r.push(2 | 8 | 16);
    r.extend_from_slice(&px4(0x222222));
    r.push(1);
    r.extend_from_slice(&px4(0xabcdef));
    r.push(0xff);
    r.push(0x00);
    s.feed(&update(vec![r])).unwrap();
    assert_eq!((s.fb().get(15, 15), s.fb().get(0, 0), s.fb().get(1, 1)), (Some(0xabcdef), Some(0x222222), Some(0x222222)));
}

#[test]
fn a_hextile_subrect_outside_its_tile_is_an_error() {
    let mut s = ready(16, 16);
    let mut r = rect_hdr(0, 0, 16, 16, 5);
    r.extend_from_slice(&[8, 1, 0xf0, 0x11]);
    assert!(matches!(s.feed(&update(vec![r])), Err(Error::Protocol(_))));
}

fn zrle_all_kinds() -> (Vec<Vec<u8>>, Zlib) {
    // two rectangles on one zlib stream
    let mut z = Zlib::new();
    let mut t1 = Vec::new();
    t1.push(1); // solid
    t1.extend_from_slice(&px3(0x336699));
    let r1 = zrle_rect(0, 0, 64, 64, &mut z, &t1);
    // 70 x 3: two tiles: palette-packed (3 colours) 64x3 and raw 6x3
    let mut t2 = vec![3];
    for c in [0xff0000, 0x00ff00, 0x0000ff] {
        t2.extend_from_slice(&px3(c));
    }
    for _row in 0..3 {
        // 64 pixels at 2 bits = 16 bytes: indices 0,1,2,0,1,2...
        let mut bits = Vec::new();
        for c in 0..64 {
            bits.push((c % 3) as u8);
        }
        for ch in bits.chunks(4) {
            t2.push(ch[0] << 6 | ch[1] << 4 | ch[2] << 2 | ch[3]);
        }
    }
    t2.push(0); // raw
    for i in 0..18u32 {
        t2.extend_from_slice(&px3(0x010000 * (i + 1)));
    }
    let r2 = zrle_rect(0, 64, 70, 3, &mut z, &t2);
    // 5 x 2: plain RLE: 4 red then 6 blue (length bytes: n-1), then palette RLE tile in another rect
    let mut t3 = vec![128];
    t3.extend_from_slice(&px3(0xff0000));
    t3.push(3);
    t3.extend_from_slice(&px3(0x0000ff));
    t3.push(5);
    let r3 = zrle_rect(0, 80, 5, 2, &mut z, &t3);
    let mut t4 = vec![130 + 1]; // palette of 3
    for c in [0x111111, 0x222222, 0x333333] {
        t4.extend_from_slice(&px3(c));
    }
    t4.push(0x80 | 2); // colour 2, run
    t4.push(1); // 2 pixels
    t4.push(0); // colour 0 once
    t4.push(1); // colour 1 once
    let r4 = zrle_rect(8, 80, 2, 2, &mut z, &t4);
    (vec![r1, r2, r3, r4], z)
}

#[test]
fn zrle_decodes_every_tile_kind_over_one_stream() {
    let (rects, _) = zrle_all_kinds();
    let mut s = ready(100, 100);
    s.feed(&update(rects)).unwrap();
    assert_eq!(s.fb().get(10, 10), Some(0x336699));
    assert_eq!(s.fb().get(63, 63), Some(0x336699));
    assert_eq!(s.fb().get(64, 10), Some(0));
    assert_eq!((s.fb().get(0, 64), s.fb().get(1, 64), s.fb().get(2, 64), s.fb().get(3, 65)), (Some(0xff0000), Some(0x00ff00), Some(0x0000ff), Some(0xff0000)));
    assert_eq!((s.fb().get(64, 64), s.fb().get(65, 64), s.fb().get(69, 66)), (Some(0x010000), Some(0x020000), Some(0x120000)));
    assert_eq!((s.fb().get(3, 80), s.fb().get(4, 80), s.fb().get(4, 81)), (Some(0xff0000), Some(0x0000ff), Some(0x0000ff)));
    assert_eq!((s.fb().get(8, 80), s.fb().get(9, 80), s.fb().get(8, 81), s.fb().get(9, 81)), (Some(0x333333), Some(0x333333), Some(0x111111), Some(0x222222)));
}

#[test]
fn feeding_one_byte_at_a_time_gives_the_same_picture() {
    let (rects, _) = zrle_all_kinds();
    let mut data = update(rects);
    let mut raw = rect_hdr(90, 90, 3, 1, 0);
    for c in [1u32, 2, 3] {
        raw.extend_from_slice(&px4(c));
    }
    data.extend(update(vec![raw]));
    let mut a = ready(100, 100);
    a.feed(&data).unwrap();
    let mut b = ready(100, 100);
    for byte in &data {
        b.feed(std::slice::from_ref(byte)).unwrap();
    }
    assert_eq!(a.fb().px, b.fb().px);
    assert_eq!(b.fb().get(91, 90), Some(2));
}

#[test]
fn rectangles_outside_the_screen_unknown_encodings_and_bad_messages_are_errors() {
    for (name, data) in [
        ("outside", update(vec![rect_hdr(8, 0, 4, 1, 0)])),
        ("overflow", update(vec![rect_hdr(65535, 65535, 65535, 65535, 0)])),
        ("copy source", {
            let mut r = rect_hdr(0, 0, 4, 4, 1);
            r.extend_from_slice(&[0, 6, 0, 0]);
            update(vec![r])
        }),
        ("encoding", update(vec![rect_hdr(0, 0, 1, 1, 7)])),
        ("message", vec![99]),
        ("cut text size", vec![3, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]),
    ] {
        let mut s = ready(8, 8);
        assert!(s.feed(&data).is_err(), "{name}");
        assert!(!s.ready());
    }
}

#[test]
fn bad_zrle_data_is_an_error_and_a_huge_screen_is_refused() {
    let mut s = ready(64, 64);
    let mut r = rect_hdr(0, 0, 64, 64, 16);
    r.extend_from_slice(&4u32.to_be_bytes());
    r.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    assert!(s.feed(&update(vec![r])).is_err());
    let mut s = ready(64, 64);
    let mut r = rect_hdr(0, 0, 64, 64, 16);
    r.extend_from_slice(&u32::MAX.to_be_bytes());
    assert!(s.feed(&update(vec![r])).is_err(), "a payload size beyond the bound");
    for (w, h) in [(0u16, 10u16), (10, 0), (9000, 100), (8000, 8000)] {
        let mut s = Session::new();
        assert!(s.feed(&handshake(w, h)).is_err(), "{w}x{h}");
    }
}

#[test]
fn a_zlib_bomb_cannot_grow_past_the_rectangle() {
    // a stream that decodes to far more than a 4x4 rectangle can hold
    let mut z = Zlib::new();
    let big = vec![0u8; 5_000_000];
    let d = z.chunk(&big);
    let mut s = ready(8, 8);
    let mut r = rect_hdr(0, 0, 4, 4, 16);
    r.extend_from_slice(&(d.len() as u32).to_be_bytes());
    r.extend(d);
    assert!(s.feed(&update(vec![r])).is_err());
}

#[test]
fn desktop_size_and_extended_desktop_size_resize_the_picture() {
    let mut s = ready(16, 16);
    assert!(!s.can_resize());
    assert!(!s.request_resize(100, 100), "not before the server said it can");
    // the server announces Extended Desktop Size (reason 0, status 0) with one screen
    let mut r = rect_hdr(0, 0, 16, 16, -308);
    r.extend_from_slice(&[1, 0, 0, 0]);
    r.extend_from_slice(&7u32.to_be_bytes());
    r.extend_from_slice(&[0; 8]);
    r.extend_from_slice(&5u32.to_be_bytes());
    s.feed(&update(vec![r])).unwrap();
    assert!(s.can_resize());
    s.take_output();
    assert!(s.request_resize(320, 200));
    let out = s.take_output();
    assert_eq!(&out[..8], &[251, 0, 1, 64, 0, 200, 1, 0]);
    assert_eq!(&out[8..12], &7u32.to_be_bytes(), "the screen id the server used");
    assert_eq!(&out[20..24], &5u32.to_be_bytes(), "and its flags");
    // the answer: reason 1 (our request), status 0, the new size
    let mut r = rect_hdr(1, 0, 320, 200, -308);
    r.extend_from_slice(&[1, 0, 0, 0]);
    r.extend_from_slice(&7u32.to_be_bytes());
    r.extend_from_slice(&[0, 0, 0, 0, 1, 64, 0, 200]);
    r.extend_from_slice(&5u32.to_be_bytes());
    s.take_events();
    s.feed(&update(vec![r])).unwrap();
    assert_eq!((s.fb().w, s.fb().h), (320, 200));
    assert!(s.take_events().contains(&Event::Resized { w: 320, h: 200 }));
    let out = s.take_output();
    assert_eq!(out[1], 0, "a full (not incremental) request after a resize");
    // a refusal (reason 1, status 3)
    let mut r = rect_hdr(1, 3, 320, 200, -308);
    r.extend_from_slice(&[1, 0, 0, 0]);
    r.extend_from_slice(&[0; 16]);
    s.feed(&update(vec![r])).unwrap();
    assert!(s.take_events().contains(&Event::ResizeRefused(3)));
    assert_eq!((s.fb().w, s.fb().h), (320, 200));
    // the old pseudo encoding
    s.feed(&update(vec![rect_hdr(0, 0, 50, 40, -223)])).unwrap();
    assert_eq!((s.fb().w, s.fb().h), (50, 40));
    // beyond the bounds
    assert!(s.feed(&update(vec![rect_hdr(0, 0, 60000, 60000, -223)])).is_err());
}

#[test]
fn resize_requests_are_bounded() {
    let mut s = ready(16, 16);
    let mut r = rect_hdr(0, 0, 16, 16, -308);
    r.extend_from_slice(&[1, 0, 0, 0]);
    r.extend_from_slice(&[0; 16]);
    s.feed(&update(vec![r])).unwrap();
    s.take_output();
    for (w, h) in [(0, 0), (1, 1), (100_000, 100_000), (8192, 8192), (u32::MAX, u32::MAX)] {
        assert!(s.request_resize(w, h));
        let o = s.take_output();
        let (rw, rh) = (u32::from(o[2]) << 8 | u32::from(o[3]), u32::from(o[4]) << 8 | u32::from(o[5]));
        assert!((64..=8192).contains(&rw) && (64..=8192).contains(&rh) && u64::from(rw) * u64::from(rh) <= 4096 * 4096, "{w}x{h} -> {rw}x{rh}");
    }
}

#[test]
fn input_messages_have_the_wire_format() {
    let mut s = ready(100, 50);
    s.key_event(true, 0xff0d);
    s.key_event(false, 0x61);
    s.pointer_event(10, 20, 1);
    s.pointer_event(-5, 9999, 8);
    s.cut_text("héllo\u{20ac}\u{7}\n");
    let o = s.take_output();
    assert_eq!(&o[..8], &[4, 1, 0, 0, 0, 0, 0xff, 0x0d]);
    assert_eq!(&o[8..16], &[4, 0, 0, 0, 0, 0, 0, 0x61]);
    assert_eq!(&o[16..22], &[5, 1, 0, 10, 0, 20]);
    assert_eq!(&o[22..28], &[5, 8, 0, 0, 0, 49], "the pointer stays on the screen");
    assert_eq!(&o[28..32], &[6, 0, 0, 0]);
    assert_eq!(&o[32..36], &[0, 0, 0, 7]);
    assert_eq!(&o[36..], &[b'h', 0xe9, b'l', b'l', b'o', b'?', b'\n'], "Latin-1, others become ?, controls dropped");
}

#[test]
fn bell_clipboard_and_colour_maps_are_taken() {
    let mut s = ready(8, 8);
    let mut d = vec![2];
    d.extend_from_slice(&[3, 0, 0, 0, 0, 0, 0, 3, b'a', 0xe9, 0x07]);
    d.extend_from_slice(&[1, 0, 0, 0, 0, 1, 1, 2, 3, 4, 5, 6]);
    s.feed(&d).unwrap();
    assert_eq!(s.take_events(), vec![Event::Bell, Event::CutText("a\u{e9}".into())]);
}

#[test]
fn inflate_matches_a_real_compressor_and_keeps_its_window_between_chunks() {
    let mut z = Zlib::new();
    let mut inf = Inflater::new();
    let mut seed = 12345u32;
    let mut data = Vec::new();
    for round in 0..40 {
        let mut chunk = Vec::new();
        for _ in 0..(round * 500 + 10) {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            chunk.push(if round % 3 == 0 { (seed >> 24) as u8 } else { (seed >> 28) as u8 });
        }
        // repeat an earlier piece so back-references reach into previous chunks
        if data.len() > 5000 {
            let from = data.len() - 4000;
            let piece: Vec<u8> = data[from..from + 3000].to_vec();
            chunk.extend(piece);
        }
        let packed = z.chunk(&chunk);
        let got = inf.inflate(&packed, 1 << 24).unwrap();
        assert_eq!(got, chunk, "round {round}");
        data.extend(chunk);
    }
}

#[test]
fn inflate_never_panics_on_garbage() {
    let mut seed = 99u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut z = Zlib::new();
    let good = z.chunk(&(0..4000u32).map(|i| (i * 7 % 251) as u8).collect::<Vec<_>>());
    for i in 0..3000 {
        let mut d = if i % 2 == 0 { good.clone() } else { (0..(rnd() % 300) as usize).map(|_| rnd() as u8).collect() };
        if i % 2 == 0 && !d.is_empty() {
            for _ in 0..(1 + rnd() % 4) {
                let k = (rnd() as usize) % d.len();
                d[k] ^= 1 << (rnd() % 8);
            }
            if rnd() % 3 == 0 {
                d.truncate((rnd() as usize) % d.len());
            }
        }
        if i % 5 == 0 && d.len() >= 2 {
            d[0] = 0x78;
            d[1] = 0x9c;
        }
        let mut inf = Inflater::new();
        let _ = inf.inflate(&d, 1 << 16);
        let _ = inf.inflate(&d, 1 << 16);
    }
}

#[test]
fn the_session_never_panics_on_corrupted_or_random_streams() {
    let mut seed = 7u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (rects, _) = zrle_all_kinds();
    let mut stream = update(rects);
    let mut hex = rect_hdr(0, 0, 32, 16, 5);
    hex.extend_from_slice(&[2 | 4 | 8, 1, 2, 3, 0, 4, 5, 6, 0, 1, 0x12, 0x21, 1]);
    hex.extend(vec![0u8; 16 * 16 * 4 + 1]);
    stream.extend(update(vec![hex]));
    for i in 0..4000 {
        let mut d = stream.clone();
        match i % 4 {
            0 => {
                for _ in 0..(1 + rnd() % 6) {
                    let k = (rnd() as usize) % d.len();
                    d[k] = rnd() as u8;
                }
            }
            1 => d.truncate((rnd() as usize) % d.len()),
            2 => d = (0..(rnd() % 400) as usize).map(|_| rnd() as u8).collect(),
            _ => {
                let k = (rnd() as usize) % d.len();
                d.splice(k..k, (0..(rnd() % 20)).map(|_| rnd() as u8));
            }
        }
        let mut s = ready(100, 100);
        // in random pieces
        let mut p = 0;
        while p < d.len() {
            let n = 1 + (rnd() as usize) % 64;
            let e = (p + n).min(d.len());
            if s.feed(&d[p..e]).is_err() {
                break;
            }
            p = e;
            s.take_events();
            s.take_output();
        }
    }
    // random bytes from the very start
    for _ in 0..2000 {
        let mut s = Session::new();
        let d: Vec<u8> = (0..(rnd() % 200) as usize).map(|_| rnd() as u8).collect();
        let _ = s.feed(&d);
        let mut s = Session::new();
        let mut d = handshake(20, 20);
        d.extend((0..(rnd() % 200) as usize).map(|_| rnd() as u8));
        let _ = s.feed(&d);
    }
}

#[test]
fn the_link_runs_a_session_over_a_unix_socket() {
    use std::io::{Read, Write};
    let dir = std::env::temp_dir().join(format!("iyakki-rfb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("d.sock");
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut c, _) = listener.accept().unwrap();
        c.write_all(b"RFB 003.008\n").unwrap();
        let mut v = [0u8; 12];
        c.read_exact(&mut v).unwrap();
        assert_eq!(&v, b"RFB 003.008\n");
        c.write_all(&[1, 1]).unwrap();
        let mut sel = [0u8; 1];
        c.read_exact(&mut sel).unwrap();
        c.write_all(&0u32.to_be_bytes()).unwrap();
        let mut shared = [0u8; 1];
        c.read_exact(&mut shared).unwrap();
        c.write_all(&init_msg(32, 32, "fake")).unwrap();
        let mut req = vec![0u8; 20 + 4 + 24 + 10];
        c.read_exact(&mut req).unwrap();
        let mut raw = rect_hdr(1, 1, 1, 1, 0);
        raw.extend_from_slice(&px4(0xaabbcc));
        c.write_all(&update(vec![raw])).unwrap();
        // wait for the key event the client sends
        let mut key = [0u8; 8 + 10];
        c.read_exact(&mut key).unwrap();
        key
    });
    let link = Link::connect_unix(&path, std::time::Duration::from_secs(5)).unwrap();
    let t0 = std::time::Instant::now();
    loop {
        let px = link.with(|s| s.fb().get(1, 1));
        if px == Some(0xaabbcc) {
            break;
        }
        assert!(t0.elapsed().as_secs() < 5, "no picture");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    link.with(|s| s.key_event(true, 0x61));
    let key = server.join().unwrap();
    assert_eq!(&key[10..], &[4, 1, 0, 0, 0, 0, 0, 0x61], "after the incremental update request");
    drop(link);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_link_that_loses_the_server_says_so() {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let w = a.try_clone().unwrap();
    let link = Link::start(a, w);
    drop(b);
    let t0 = std::time::Instant::now();
    while link.ended().is_none() {
        assert!(t0.elapsed().as_secs() < 5);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(link.ended().unwrap().contains("closed"));
}

use crate::*;
use alloc::{string::String, vec, vec::Vec};

fn sample() -> DisplayList {
    let c = Color(10, 20, 30, 255);
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(0, 0, 320, 200), color: c });
    l.push(Op::RoundRect { r: Rect::new(-5, 7, 100, 30), radius: 6, color: c });
    l.push(Op::Outline { r: Rect::new(1, 2, 3, 4), thickness: 1, color: c });
    l.push(Op::Line { x1: -3, y1: 4, x2: 300, y2: 190, thickness: 2, color: c });
    l.push(Op::PushClip { r: Rect::new(0, 0, 50, 50) });
    l.push(Op::Text { x: 4, y: 5, face: 1, size: 14, color: c, text: String::from("héllo") });
    l.push(Op::Image { handle: 70000, r: Rect::new(0, 0, 16, 16) });
    l.push(Op::PopClip);
    l.push(Op::Hit { id: 300, r: Rect::new(10, 10, 20, 20) });
    l
}

fn events() -> EventBatch {
    let c = Color(1, 2, 3, 4);
    EventBatch::new(vec![
        Event::PointerDown { x: -1, y: 200, button: 0 },
        Event::PointerUp { x: 5, y: 6, button: 1 },
        Event::PointerMove { x: 7, y: 8 },
        Event::Key { code: 300, mods: 3, down: true },
        Event::TextInput { text: String::from("ab\u{1F600}") },
        Event::Tick { dt_ms: 16 },
        Event::Resize { w: 640, h: 480, scale_pct: 200 },
        Event::Theme { dark: true, accent: c, background: c, foreground: c },
        Event::Hit { id: 9 },
    ])
}

#[test]
fn display_list_roundtrip() {
    let l = sample();
    let b = l.encode().unwrap();
    assert_eq!(DisplayList::decode(&b).unwrap(), l);
}

#[test]
fn event_batch_roundtrip() {
    let e = events();
    assert_eq!(EventBatch::decode(&e.encode().unwrap()).unwrap(), e);
}

#[test]
fn encoding_is_compact() {
    // a typical small frame stays far below a kilobyte
    assert!(sample().encode().unwrap().len() < 120);
}

#[test]
fn every_truncation_is_an_error_not_a_panic() {
    for full in [sample().encode().unwrap(), events().encode().unwrap()] {
        for n in 0..full.len() {
            let cut = &full[..n];
            assert!(DisplayList::decode(cut).is_err() || cut.len() == full.len());
            assert!(EventBatch::decode(cut).is_err());
        }
    }
}

#[test]
fn hostile_bytes_never_panic() {
    // a simple xorshift fuzz, plus every single-byte mutation of a valid list
    let mut s = 0x1234_5678_9abc_def1u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for _ in 0..4000 {
        let len = (next() % 80) as usize;
        let v: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let _ = DisplayList::decode(&v);
        let _ = EventBatch::decode(&v);
    }
    let base = sample().encode().unwrap();
    for i in 0..base.len() {
        for m in [0x00u8, 0xff, 0x80, 0x7f, 0x01] {
            let mut b = base.clone();
            b[i] = m;
            let _ = DisplayList::decode(&b);
        }
    }
}

#[test]
fn huge_counts_are_refused_without_allocating() {
    let mut b = Vec::new();
    b.extend_from_slice(&LIST_MAGIC);
    b.extend_from_slice(&(ABI_VERSION as u16).to_le_bytes());
    b.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x0f]); // 4 billion ops
    assert_eq!(DisplayList::decode(&b), Err(DecodeError::TooManyItems));
}

#[test]
fn size_limits() {
    let mut l = DisplayList::new();
    for _ in 0..(limits::MAX_OPS + 1) {
        l.push(Op::PopClip);
    }
    assert_eq!(l.encode(), Err(DecodeError::TooManyItems));
    let mut l = DisplayList::new();
    l.push(Op::Text { x: 0, y: 0, face: 0, size: 10, color: Color(0, 0, 0, 255), text: "x".repeat(limits::MAX_TEXT + 1) });
    assert_eq!(l.encode(), Err(DecodeError::TooLarge));
    assert_eq!(DisplayList::decode(&vec![0u8; limits::MAX_LIST_BYTES + 1]), Err(DecodeError::TooLarge));
    let big = EventBatch::new(vec![Event::Tick { dt_ms: 1 }; limits::MAX_EVENTS + 1]);
    assert_eq!(big.encode(), Err(DecodeError::TooManyItems));
}

#[test]
fn coordinates_out_of_range_are_refused() {
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(limits::MAX_COORD + 1, 0, 1, 1), color: Color(0, 0, 0, 255) });
    let b = l.encode().unwrap();
    assert_eq!(DisplayList::decode(&b), Err(DecodeError::OutOfRange));
}

#[test]
fn clip_must_balance_and_depth_is_capped() {
    let mut l = DisplayList::new();
    l.push(Op::PopClip);
    assert_eq!(DisplayList::decode(&l.encode().unwrap()), Err(DecodeError::UnbalancedClip));
    let mut l = DisplayList::new();
    l.push(Op::PushClip { r: Rect::new(0, 0, 1, 1) });
    assert_eq!(DisplayList::decode(&l.encode().unwrap()), Err(DecodeError::UnbalancedClip));
    let mut l = DisplayList::new();
    for _ in 0..=limits::MAX_CLIP_DEPTH {
        l.push(Op::PushClip { r: Rect::new(0, 0, 1, 1) });
    }
    assert_eq!(DisplayList::decode(&l.encode().unwrap()), Err(DecodeError::TooManyItems));
}

#[test]
fn version_mismatch_is_named() {
    let mut b = sample().encode().unwrap();
    b[4] = 9;
    assert_eq!(DisplayList::decode(&b), Err(DecodeError::BadVersion(9)));
    b[0] = b'X';
    assert_eq!(DisplayList::decode(&b), Err(DecodeError::BadMagic));
}

#[test]
fn bounds_check_and_hit_test() {
    let l = sample();
    assert!(l.check_bounds(320, 200).is_ok());
    assert_eq!(l.hit_test(15, 15), Some(300));
    assert_eq!(l.hit_test(50, 50), None);
    let mut bad = DisplayList::new();
    bad.push(Op::Rect { r: Rect::new(5000, 0, 10, 10), color: Color(0, 0, 0, 255) });
    assert_eq!(bad.check_bounds(320, 200), Err(0));
}

#[test]
fn text_metrics_are_integer_and_stable() {
    assert_eq!(measure_text(0, 14, "abc"), 36);
    assert_eq!(measure_text(0, 14, ""), 0);
    assert_eq!(measure_text(0, 7, "é"), 6);
    assert_eq!(line_height(16), 20);
}

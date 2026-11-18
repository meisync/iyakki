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

#[test]
fn event_batches_name_a_wrong_version_marker_tag_text_and_trailing_bytes() {
    let good = events().encode().unwrap();
    let mut b = good.clone();
    b[4] = 9;
    assert_eq!(EventBatch::decode(&b), Err(DecodeError::BadVersion(9)));
    let mut b = good.clone();
    b[0] = b'X';
    assert_eq!(EventBatch::decode(&b), Err(DecodeError::BadMagic));
    // an unknown event tag
    let mut b = Vec::new();
    b.extend_from_slice(&EVENT_MAGIC);
    b.extend_from_slice(&(ABI_VERSION as u16).to_le_bytes());
    b.extend_from_slice(&[1, 200]);
    assert_eq!(EventBatch::decode(&b), Err(DecodeError::BadTag(200)));
    // text that is not UTF-8
    let mut b = Vec::new();
    b.extend_from_slice(&EVENT_MAGIC);
    b.extend_from_slice(&(ABI_VERSION as u16).to_le_bytes());
    b.extend_from_slice(&[1, 5, 2, 0xff, 0xfe]);
    assert_eq!(EventBatch::decode(&b), Err(DecodeError::BadUtf8));
    // bytes after the last event
    let mut b = good.clone();
    b.push(0);
    assert_eq!(EventBatch::decode(&b), Err(DecodeError::TrailingBytes));
    // too many events named in the header, and an oversized buffer
    let mut b = Vec::new();
    b.extend_from_slice(&EVENT_MAGIC);
    b.extend_from_slice(&(ABI_VERSION as u16).to_le_bytes());
    b.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
    assert_eq!(EventBatch::decode(&b), Err(DecodeError::TooManyItems));
    assert_eq!(EventBatch::decode(&vec![0u8; limits::MAX_EVENT_BYTES + 1]), Err(DecodeError::TooLarge));
    let long = EventBatch::new(vec![Event::TextInput { text: "x".repeat(limits::MAX_TEXT + 1) }]);
    assert_eq!(long.encode(), Err(DecodeError::TooLarge));
}

#[test]
fn a_display_list_with_an_unknown_tag_bad_text_or_trailing_bytes_is_refused() {
    let good = sample().encode().unwrap();
    let mut b = good.clone();
    b.push(0);
    assert_eq!(DisplayList::decode(&b), Err(DecodeError::TrailingBytes));
    let mut b = Vec::new();
    b.extend_from_slice(&LIST_MAGIC);
    b.extend_from_slice(&(ABI_VERSION as u16).to_le_bytes());
    b.extend_from_slice(&[1, 200]);
    assert_eq!(DisplayList::decode(&b), Err(DecodeError::BadTag(200)));
}

// ------------------------------------------------------------------------------------------------------ ABI 2: Access (F-A11Y-06)

fn acc(id: u32, role: Role, name: &str, value: &str, fl: u8) -> Op {
    Op::Access { id, role, r: Rect::new(1, 2, 30, 20), name: String::from(name), value: String::from(value), flags: fl }
}

#[test]
fn access_round_trips_in_list_order_and_is_version_2_only_when_used() {
    let plain = sample().encode().unwrap();
    assert_eq!(u16::from_le_bytes([plain[4], plain[5]]), 1, "a list without Access stays ABI 1 on the wire");
    let mut l = sample();
    l.push(acc(300, Role::Button, "Save", "", 0));
    l.push(acc(0, Role::Heading, "Settings", "", 0));
    l.push(acc(7, Role::Toggle, "Wi-Fi", "on", access_flags::CHECKED | access_flags::SELECTED));
    let bytes = l.encode().unwrap();
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2);
    let back = DisplayList::decode(&bytes).unwrap();
    assert_eq!(back, l);
    let tree = back.access_tree();
    let names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["Save", "Settings", "Wi-Fi"]);
    assert_eq!(tree[2].flags, 3);
}

#[test]
fn an_abi_1_list_cannot_carry_access_and_hostile_input_is_refused() {
    let mut l = DisplayList::new();
    l.push(acc(1, Role::Button, "x", "", 0));
    let mut bytes = l.encode().unwrap();
    bytes[4] = 1; // claim ABI 1: tag 10 is unknown there
    assert_eq!(DisplayList::decode(&bytes), Err(DecodeError::BadTag(10)));
    // truncated at every length: an error, never a panic
    let good = l.encode().unwrap();
    for n in 0..good.len() {
        assert!(DisplayList::decode(&good[..n]).is_err());
    }
    // a bad role byte and an unknown flag bit
    let mut raw = alloc::vec::Vec::new();
    raw.extend_from_slice(&LIST_MAGIC);
    raw.extend_from_slice(&2u16.to_le_bytes());
    raw.push(1); // one op
    raw.push(10); // access
    raw.push(0); // id
    raw.push(99); // role: invalid
    assert_eq!(DisplayList::decode(&raw), Err(DecodeError::OutOfRange));
    // a name over the limit is refused by the encoder and the decoder alike
    let mut big = DisplayList::new();
    big.push(acc(1, Role::Button, &"n".repeat(limits::MAX_ACCESS_TEXT + 1), "", 0));
    assert_eq!(big.encode(), Err(DecodeError::TooLarge));
    let mut flags = DisplayList::new();
    flags.push(acc(1, Role::Button, "x", "", 32));
    assert!(flags.encode().is_err());
}

#[test]
fn too_many_access_operations_are_refused() {
    let mut l = DisplayList::new();
    for i in 0..=limits::MAX_ACCESS as u32 {
        l.push(acc(i, Role::Text, "t", "", 0));
    }
    assert_eq!(l.encode(), Err(DecodeError::TooManyItems));
}

#[test]
fn a_secret_field_never_carries_its_text_and_names_are_cut_on_a_character_boundary() {
    // a hand-made list that carries a value for a secret field: the decoder drops it
    let mut l = DisplayList::new();
    l.push(acc(1, Role::Field, "Password", "hunter2", access_flags::SECRET));
    let back = DisplayList::decode(&l.encode().unwrap()).unwrap();
    assert_eq!(back.access_tree()[0].value, "");
    assert!(!alloc::format!("{:?}", back).contains("hunter2"));
    // the tree cuts an over-long name from an unchecked list on a boundary
    let mut raw = DisplayList::new();
    raw.ops.push(acc(2, Role::Text, &"é".repeat(100), "", 0)); // 200 bytes, not encoded
    let t = raw.access_tree();
    assert!(t[0].name.len() <= limits::MAX_ACCESS_TEXT && t[0].name.chars().all(|c| c == 'é'));
}

#[test]
fn hits_without_access_are_listed_so_the_store_review_can_see_them() {
    let mut l = DisplayList::new();
    l.push(Op::Hit { id: 1, r: Rect::new(0, 0, 10, 10) });
    l.push(Op::Hit { id: 2, r: Rect::new(0, 10, 10, 10) });
    assert_eq!(l.unnamed_hits(), [1, 2], "an ABI 1 tile has every hit unnamed");
    l.push(acc(2, Role::Button, "Open", "", 0));
    assert_eq!(l.unnamed_hits(), [1]);
}

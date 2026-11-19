// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

use super::*;
use std::collections::VecDeque;

struct Script {
    kind: RecKind,
    params: Vec<u8>,
    units: VecDeque<Result<Unit, FeedErr>>,
}

impl RecFeed for Script {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        self.units.pop_front().unwrap_or(Err(FeedErr::Lost("end".into())))
    }
    fn kind(&self) -> RecKind {
        self.kind
    }
    fn params(&self) -> Vec<u8> {
        self.params.clone()
    }
}

fn h264(key: bool, tag: u8) -> Unit {
    Unit::H264 { annex_b: vec![0, 0, 0, 1, if key { 0x65 } else { 0x41 }, tag], key }
}
fn tag(u: &Unit) -> u8 {
    match u {
        Unit::H264 { annex_b, .. } => *annex_b.last().unwrap(),
        Unit::Jpeg(j) => j[0],
    }
}

#[test]
fn a_viewer_gets_what_the_recorder_reads_without_a_second_connection() {
    let share = Arc::new(Share::default());
    assert!(share.subscribe("cam").is_none() && !share.held("cam"));
    let feed = Script { kind: RecKind::Jpeg, params: vec![], units: (1..=3).map(|i| Ok(Unit::Jpeg(vec![i]))).collect() };
    let mut rec = share.tee("cam", Box::new(feed));
    assert!(share.held("cam"));
    let mut view = share.subscribe("cam").expect("the recorder holds the camera");
    for i in 1..=3u8 {
        assert_eq!(tag(&rec.next().unwrap()), i, "the recorder still reads everything");
        assert_eq!(tag(&view.next().unwrap()), i, "and the viewer sees the same");
    }
}

#[test]
fn an_h264_viewer_starts_at_a_key_frame_with_the_parameter_sets_in_front() {
    let share = Arc::new(Share::default());
    let units = vec![Ok(h264(true, 1)), Ok(h264(false, 2)), Ok(h264(false, 3)), Ok(h264(true, 4)), Ok(h264(false, 5))];
    let mut rec = share.tee("c", Box::new(Script { kind: RecKind::H264, params: vec![0, 0, 0, 1, 0x67, 9, 0, 0, 0, 1, 0x68, 8], units: units.into() }));
    rec.next().unwrap(); // before anybody looks
    let mut view = share.subscribe("c").unwrap();
    rec.next().unwrap(); // delta frame 2: not a place to start
    rec.next().unwrap(); // delta frame 3
    rec.next().unwrap(); // key frame 4
    rec.next().unwrap(); // delta frame 5
    let first = view.next().unwrap();
    let Unit::H264 { annex_b, key } = &first else { panic!("h264") };
    assert!(*key && annex_b.starts_with(&[0, 0, 0, 1, 0x67, 9]) && annex_b.ends_with(&[0x65, 4]), "{annex_b:?}");
    assert_eq!(tag(&view.next().unwrap()), 5);
    let nals = nals_from_annex_b(annex_b);
    assert_eq!(nals, vec![vec![0x67, 9], vec![0x68, 8], vec![0x65, 4]]);
}

#[test]
fn a_viewer_that_falls_behind_is_cut_loose_and_an_h264_one_waits_for_the_next_key_frame() {
    let share = Arc::new(Share::default());
    let mut units: Vec<Result<Unit, FeedErr>> = vec![Ok(h264(true, 0))];
    for i in 1..=(QUEUE_MAX as u8 + 5) {
        units.push(Ok(h264(false, i)));
    }
    units.push(Ok(h264(true, 200)));
    let n = units.len();
    let mut rec = share.tee("c", Box::new(Script { kind: RecKind::H264, params: vec![], units: units.into() }));
    let mut view = share.subscribe("c").unwrap();
    for _ in 0..n {
        rec.next().unwrap();
    }
    // the first key frame was queued, then the queue overflowed and was dropped; what is left starts at a key frame
    let mut seen = vec![];
    loop {
        match view.next() {
            Ok(u) => seen.push((matches!(u, Unit::H264 { key: true, .. }), tag(&u))),
            Err(_) => break,
        }
        if seen.len() > QUEUE_MAX + 2 {
            break;
        }
    }
    assert!(seen.len() <= QUEUE_MAX, "the queue is bounded: {}", seen.len());
    assert_eq!(seen.first().map(|s| s.0), Some(true), "it starts at a key frame: {seen:?}");
    assert_eq!(seen.last(), Some(&(true, 200)), "and ends with the newest key frame");
}

#[test]
fn when_the_recorder_lets_go_the_viewer_is_told_and_the_entry_goes() {
    let share = Arc::new(Share::default());
    let rec = share.tee("c", Box::new(Script { kind: RecKind::Jpeg, params: vec![], units: VecDeque::new() }));
    let mut view = share.subscribe("c").unwrap();
    drop(rec);
    assert!(!share.held("c") && share.subscribe("c").is_none());
    assert!(matches!(view.next(), Err(FeedErr::Lost(m)) if m.contains("let go")));
}

#[test]
fn a_viewer_that_goes_away_costs_the_recorder_nothing_and_a_newer_connection_is_not_removed_by_an_older_one() {
    let share = Arc::new(Share::default());
    let mut rec = share.tee("c", Box::new(Script { kind: RecKind::Jpeg, params: vec![], units: (1..=4).map(|i| Ok(Unit::Jpeg(vec![i]))).collect() }));
    let view = share.subscribe("c").unwrap();
    rec.next().unwrap();
    drop(view);
    rec.next().unwrap();
    rec.next().unwrap();
    // a second tee for the same camera replaces the first; dropping the first leaves the second in place
    let newer = share.tee("c", Box::new(Script { kind: RecKind::Jpeg, params: vec![], units: VecDeque::new() }));
    drop(rec);
    assert!(share.held("c"));
    drop(newer);
    assert!(!share.held("c"));
}

#[test]
fn start_codes_of_three_and_four_bytes_split_into_units() {
    assert_eq!(nals_from_annex_b(&[0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65]), vec![vec![0x67, 1], vec![0x68, 2], vec![0x65]]);
    assert!(nals_from_annex_b(&[]).is_empty() && nals_from_annex_b(&[1, 2, 3]).is_empty());
}

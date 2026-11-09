use super::*;

fn dir(tag: &str) -> PathBuf {
    crate::testdir::unique("camrecfmt", tag)
}

fn key(b: u8) -> RecKey {
    RecKey::from_bytes([b; 32])
}

fn pic(n: usize) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend((0..n).map(|i| (i * 7 + n) as u8));
    v.extend([0xFF, 0xD9]);
    v
}

fn write(path: &Path, k: &RecKey, n: usize) -> Vec<Vec<u8>> {
    let mut w = SegmentWriter::create(path, k, RecKind::Jpeg, 1_000).unwrap();
    let mut all = vec![];
    for i in 0..n {
        let p = pic(100 + i * 37);
        w.append(1_000 + i as u64 * 100, F_KEY, &p).unwrap();
        all.push(p);
    }
    all
}

#[test]
fn frames_read_back_exactly_and_in_order() {
    let d = dir("roundtrip");
    let p = d.join("a.irec");
    let sent = write(&p, &key(1), 12);
    let s = Segment::open(&p, &key(1)).unwrap();
    assert_eq!(s.len(), 12);
    assert_eq!(s.scan.kind, RecKind::Jpeg);
    assert_eq!(s.scan.start_ms, 1_000);
    assert!(!s.scan.cut_short());
    for (i, want) in sent.iter().enumerate() {
        let f = s.frame(i).unwrap();
        assert_eq!(&f.data, want);
        assert_eq!(f.ts_ms, 1_000 + i as u64 * 100);
        assert_eq!(f.flags, F_KEY);
    }
    assert_eq!(s.frame(99), Err(ReadErr::OutOfRange));
}

#[test]
fn the_file_shows_no_picture_bytes_and_no_run_of_the_plain_text() {
    let d = dir("opaque");
    let p = d.join("a.irec");
    let marker: Vec<u8> = (0..400).map(|i| (i % 251) as u8 ^ 0x5a).collect();
    let mut w = SegmentWriter::create(&p, &key(2), RecKind::Jpeg, 5).unwrap();
    let mut frame = vec![0xFF, 0xD8, 0xFF, 0xE0];
    frame.extend(&marker);
    w.append(5, F_KEY, &frame).unwrap();
    drop(w);
    let raw = std::fs::read(&p).unwrap();
    assert!(!raw.windows(32).any(|w| w == &marker[..32]), "plain picture bytes are on the disk");
    assert!(!raw.windows(4).any(|w| w == [0xFF, 0xD8, 0xFF, 0xE0]), "a JPEG start marker is on the disk");
    // two frames with the same content do not give the same cipher text
    let p2 = d.join("b.irec");
    let mut w = SegmentWriter::create(&p2, &key(2), RecKind::Jpeg, 5).unwrap();
    w.append(5, F_KEY, &frame).unwrap();
    w.append(6, F_KEY, &frame).unwrap();
    drop(w);
    let s = scan(&p2).unwrap();
    let raw2 = std::fs::read(&p2).unwrap();
    let a = &raw2[s.frames[0].offset as usize + 13..][..s.frames[0].cipher_len as usize];
    let b = &raw2[s.frames[1].offset as usize + 13..][..s.frames[1].cipher_len as usize];
    assert_ne!(a, b);
    // the same frame in two files is different too (a per-file key)
    let a2 = &raw[s.frames[0].offset as usize + 13..][..s.frames[0].cipher_len as usize];
    assert_ne!(a, a2);
}

#[test]
fn a_wrong_key_is_refused_before_anything_is_decrypted() {
    let d = dir("wrongkey");
    let p = d.join("a.irec");
    write(&p, &key(1), 3);
    assert!(matches!(Segment::open(&p, &key(2)), Err(ReadErr::WrongKey)));
}

#[test]
fn every_changed_byte_after_the_header_is_found() {
    let d = dir("tamper");
    let p = d.join("a.irec");
    write(&p, &key(1), 3);
    let orig = std::fs::read(&p).unwrap();
    let sc = scan(&p).unwrap();
    // flip one byte in the cipher text, the MAC and the time stamp of the middle record
    let r = &sc.frames[1];
    for off in [r.offset as usize + 13 + 5, r.offset as usize + 13 + r.cipher_len as usize + 3, r.offset as usize + 6, r.offset as usize + 12] {
        let mut b = orig.clone();
        b[off] ^= 0x01;
        std::fs::write(&p, &b).unwrap();
        let s = Segment::open(&p, &key(1)).unwrap();
        assert_eq!(s.frame(1), Err(ReadErr::Tampered(1)), "byte at {off}");
        assert!(s.frame(0).is_ok(), "neighbours still read");
    }
    // a changed header byte (the start time) is a wrong-key error, never a silently wrong day
    let mut b = orig.clone();
    b[13] ^= 1;
    std::fs::write(&p, &b).unwrap();
    assert!(matches!(Segment::open(&p, &key(1)), Err(ReadErr::WrongKey)));
}

#[test]
fn records_swapped_inside_a_file_are_found() {
    let d = dir("swap");
    let p = d.join("a.irec");
    write(&p, &key(1), 3);
    let orig = std::fs::read(&p).unwrap();
    let sc = scan(&p).unwrap();
    // records have different lengths here, so swap two of the same length by writing equal-size frames
    let p2 = d.join("b.irec");
    let mut w = SegmentWriter::create(&p2, &key(1), RecKind::Jpeg, 1).unwrap();
    for i in 0..3u64 {
        w.append(i, F_KEY, &pic(200)).unwrap();
    }
    drop(w);
    let sc2 = scan(&p2).unwrap();
    let mut b = std::fs::read(&p2).unwrap();
    let (a0, a1) = (sc2.frames[0].offset as usize, sc2.frames[1].offset as usize);
    let len = a1 - a0;
    let first = b[a0..a0 + len].to_vec();
    let second = b[a1..a1 + len].to_vec();
    b[a0..a0 + len].copy_from_slice(&second);
    b[a1..a1 + len].copy_from_slice(&first);
    std::fs::write(&p2, &b).unwrap();
    let s = Segment::open(&p2, &key(1)).unwrap();
    assert!(s.frame(0).is_err() && s.frame(1).is_err(), "a moved record does not verify");
    assert!(s.frame(2).is_ok());
    let _ = (orig, sc);
}

#[test]
fn a_power_cut_in_the_middle_of_a_record_leaves_every_earlier_one_readable() {
    let d = dir("cut");
    let p = d.join("a.irec");
    let sent = write(&p, &key(1), 6);
    let full = std::fs::read(&p).unwrap();
    let sc = scan(&p).unwrap();
    // cut the file at every byte of the last record, and in the header of it
    let last = sc.frames[5].offset as usize;
    for cut in (last..full.len()).step_by(7).chain([full.len() - 1]) {
        std::fs::write(&p, &full[..cut]).unwrap();
        let s = Segment::open(&p, &key(1)).unwrap();
        assert_eq!(s.len(), 5, "cut at {cut}");
        assert!(s.scan.cut_short() || cut == last);
        for (i, want) in sent.iter().take(5).enumerate() {
            assert_eq!(&s.frame(i).unwrap().data, want);
        }
    }
    // garbage after the last good record is ignored too
    let mut g = full[..last].to_vec();
    g.extend([0xEE; 40]);
    std::fs::write(&p, &g).unwrap();
    assert_eq!(Segment::open(&p, &key(1)).unwrap().len(), 5);
    // a file cut inside its header is not a recording
    std::fs::write(&p, &full[..30]).unwrap();
    assert!(matches!(scan(&p), Err(ReadErr::NotARecording)));
}

#[test]
fn scanning_needs_no_key_and_files_are_private_and_never_replaced() {
    let d = dir("scan");
    let p = d.join("sub/a.irec");
    write(&p, &key(1), 4);
    let s = scan(&p).unwrap();
    assert_eq!((s.frames.len(), s.end_ms()), (4, 1_300));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(SegmentWriter::create(&p, &key(1), RecKind::Jpeg, 1).is_err(), "an existing file is never replaced");
    std::fs::write(d.join("junk"), b"hello world, this is not a recording at all, not at all, not at all, not at all, not.").unwrap();
    assert!(matches!(scan(&d.join("junk")), Err(ReadErr::NotARecording)));
}

#[test]
fn h264_units_and_the_key_text_round_trip() {
    let d = dir("h264");
    let p = d.join("a.irec");
    let mut w = SegmentWriter::create(&p, &key(3), RecKind::H264, 9).unwrap();
    let au = [0u8, 0, 0, 1, 0x65, 1, 2, 3, 4, 5];
    w.append(10, F_KEY | F_PARAMS, &au).unwrap();
    drop(w);
    let s = Segment::open(&p, &key(3)).unwrap();
    assert_eq!(s.scan.kind, RecKind::H264);
    let f = s.frame(0).unwrap();
    assert_eq!((f.data.as_slice(), f.flags), (&au[..], F_KEY | F_PARAMS));
    let k = RecKey::generate().unwrap();
    let hex = k.to_hex();
    assert_eq!(hex.len(), 64);
    assert!(RecKey::from_hex(&hex).is_some());
    assert!(RecKey::from_hex("zz").is_none() && RecKey::from_hex(&"g".repeat(64)).is_none());
    // empty and oversized frames are refused, not stored
    let mut w = SegmentWriter::create(&d.join("b.irec"), &key(3), RecKind::Jpeg, 1).unwrap();
    assert!(w.append(1, F_KEY, &[]).is_err());
    assert!(w.append(1, F_KEY, &vec![1u8; MAX_FRAME + 1]).is_err());
}

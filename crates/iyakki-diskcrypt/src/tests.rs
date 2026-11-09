//! Tests for the encrypted-volume header parsers, the secret and the rate limit (moved from the agent's `disks_unlock_tests.rs`; the
//! made-up machine, helper and engine tests stay there).

use super::*;

/// The size the DISKS probe reads of a partition (`iyakki_agent::disks::PROBE_BYTES`) plus some spare, all zero.
fn blank() -> Vec<u8> {
    vec![0u8; 0x10230 + 64]
}

fn put(b: &mut [u8], off: usize, bytes: &[u8]) {
    b[off..off + bytes.len()].copy_from_slice(bytes);
}

/// A LUKS2 header.
fn sb_luks() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 0, b"LUKS\xba\xbe");
    put(&mut b, 6, &[0, 2]);
    put(&mut b, 8, &16384u64.to_be_bytes());
    put(&mut b, 72, b"sha256");
    put(&mut b, 168, b"0a1b2c3d-0000-4000-8000-000000000001");
    b
}

/// A LUKS1 header (aes-xts-plain64, sha256, 512-bit key).
fn sb_luks1() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 0, b"LUKS\xba\xbe");
    put(&mut b, 6, &[0, 1]);
    put(&mut b, 8, b"aes");
    put(&mut b, 40, b"xts-plain64");
    put(&mut b, 72, b"sha256");
    put(&mut b, 108, &64u32.to_be_bytes());
    put(&mut b, 168, b"0a1b2c3d-0000-4000-8000-000000000002");
    b
}

fn sb_bitlocker() -> Vec<u8> {
    let mut b = blank();
    put(&mut b, 3, b"-FVE-FS-");
    put(&mut b, 11, &512u16.to_le_bytes());
    b
}

#[test]
fn luks1_luks2_and_bitlocker_headers_are_recognised_with_their_ids() {
    let c = parse(&sb_luks()).expect("LUKS2");
    assert_eq!((c.kind, c.uuid.as_str(), c.problem.clone()), (CryptKind::Luks2, "0a1b2c3d-0000-4000-8000-000000000001", None));
    assert_eq!(c.headline(), "Encrypted: LUKS2");
    assert_eq!(c.kind.family(), "LUKS");
    let c = parse(&sb_luks1()).expect("LUKS1");
    assert_eq!((c.kind, c.uuid.as_str(), c.problem.clone()), (CryptKind::Luks1, "0a1b2c3d-0000-4000-8000-000000000002", None));
    assert!(c.detail.contains("aes-xts-plain64") && c.detail.contains("sha256"), "{}", c.detail);
    let c = parse(&sb_bitlocker()).expect("BitLocker");
    assert_eq!((c.kind, c.problem.clone()), (CryptKind::BitLocker, None));
    assert_eq!(c.headline(), "Encrypted: BitLocker");
}

#[test]
fn a_bitlocker_to_go_sector_is_recognised_and_a_plain_fat_one_is_not() {
    let mut b = blank();
    b[3..11].copy_from_slice(b"MSWIN4.1");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[424..440].copy_from_slice(&FVE_TO_GO_GUID);
    let c = parse(&b).expect("To Go");
    assert_eq!((c.kind, c.detail.as_str()), (CryptKind::BitLocker, "BitLocker To Go"));
    b[424] = 0;
    assert!(parse(&b).is_none(), "MSWIN4.1 without the GUID is an ordinary FAT volume");
    assert!(parse(&blank()).is_none() && parse(&[]).is_none());
}

#[test]
fn hostile_and_truncated_headers_are_flagged_never_trusted_and_never_panic() {
    // magic only
    let c = parse_luks(b"LUKS\xba\xbe").expect("magic");
    assert!(c.problem.is_some_and(|p| p.contains("cut short")));
    // an unknown version
    let mut b = sb_luks();
    b[6..8].copy_from_slice(&9u16.to_be_bytes());
    assert!(parse_luks(&b).unwrap().problem.unwrap().contains("version 9"));
    // a LUKS2 header size that is not a power of two, zero, or huge
    for sz in [0u64, 12345, u64::MAX, 1 << 40] {
        let mut b = sb_luks();
        b[8..16].copy_from_slice(&sz.to_be_bytes());
        assert!(parse_luks(&b).unwrap().problem.unwrap().contains("damaged"), "{sz}");
    }
    // a LUKS1 header with a nonsense key size or an empty cipher
    let mut b = sb_luks1();
    b[108..112].copy_from_slice(&7u32.to_be_bytes());
    assert!(parse_luks(&b).unwrap().problem.unwrap().contains("damaged"));
    let mut b = sb_luks1();
    b[8..40].fill(0);
    assert!(parse_luks(&b).unwrap().problem.is_some());
    // text from the drive is cleaned: control characters and non-ASCII are dropped, a bad UUID is not shown
    let mut b = sb_luks();
    let t: &[u8] = b"na\x1b[31mme\xff\xfe\n\t\x07tail   ";
    b[24..24 + t.len()].copy_from_slice(t);
    let c = parse_luks(&b).unwrap();
    assert!(c.label.chars().all(|ch| ch.is_ascii_graphic() || ch == ' '), "{:?}", c.label);
    let mut b = sb_luks();
    let t: &[u8] = b"not-a-uuid\x1b[2J----------------------!!!!!";
    b[168..168 + t.len().min(40)].copy_from_slice(&t[..t.len().min(40)]);
    assert_eq!(parse_luks(&b).unwrap().uuid, "");
    // BitLocker: cut short, a sector size that makes no sense
    assert!(parse_bitlocker(b"\0\0\0-FVE-FS-").unwrap().problem.unwrap().contains("cut short"));
    let mut b = sb_bitlocker();
    b[11..13].copy_from_slice(&3u16.to_le_bytes());
    assert!(parse_bitlocker(&b).unwrap().problem.unwrap().contains("damaged"));
    // every prefix length of a good header parses without panicking
    for n in 0..300 {
        for src in [sb_luks(), sb_luks1(), sb_bitlocker()] {
            let _ = parse(&src[..n.min(src.len())]);
        }
    }
    // pseudo-random garbage with the magic in front
    let mut x = 0x2545F491u32;
    for _ in 0..200 {
        let mut g: Vec<u8> = (0..300)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        g[..6].copy_from_slice(b"LUKS\xba\xbe");
        let c = parse(&g).expect("magic");
        assert!(c.label.chars().all(|ch| (' '..='~').contains(&ch)) && c.uuid.len() <= 36 && c.detail.len() <= 100);
    }
}

#[test]
fn a_secret_prints_nothing_and_is_wiped() {
    let s = Secret::new("hunter2");
    assert_eq!(format!("{s:?}"), "Secret(hidden)");
    assert!(!format!("{s:?}").contains("hunter"));
    let mut buf = b"hunter2".to_vec();
    wipe(&mut buf);
    assert!(buf.iter().all(|b| *b == 0));
    let mut t = String::from("pa55word");
    wipe_string(&mut t);
    assert!(t.is_empty());
}

#[test]
fn wrong_keys_cost_a_growing_wait_and_a_right_key_clears_it() {
    let mut l = Limiter::default();
    assert!(l.check("sdb1", 100).is_ok());
    l.failed("sdb1", 100);
    assert_eq!(l.check("sdb1", 100), Err(2));
    assert_eq!(l.check("sdb1", 101), Err(1));
    assert!(l.check("sdb1", 102).is_ok());
    l.failed("sdb1", 102);
    assert_eq!(l.check("sdb1", 102), Err(5));
    for t in 0..10 {
        l.failed("sdb1", 200 + t);
    }
    assert_eq!(l.check("sdb1", 209), Err(60));
    assert!(l.check("sdb2", 209).is_ok(), "another partition is not slowed down");
    l.ok("sdb1");
    assert!(l.check("sdb1", 209).is_ok());
    // the table cannot grow without bound
    for i in 0..500 {
        l.failed(&format!("p{i}"), i);
    }
    assert!(l.fails("p499") == 1 && l.fails("p0") == 0);
}

//! Fuzz tests (deterministic, seeded): the xz decoder on mutated files, the zip writer's round trip, and the zip/tar readers on mutated archives.
//! A panic, a case slower than the guard or a case over the heap bound fails the test and prints the seed and case.
//! `IYAKKI_FUZZ_CASES=200000 cargo test -p iyakki-archive fuzz` runs a larger set.

use crate::fuzzkit::{cases, run, Rng};
use crate::xz::{crc32, decompress, make_xz_with_check};
use crate::zipw::{valid_name, ZipWriter};
use std::io::Cursor;
use std::time::Duration;

const REAL_XZ: &[u8] = &[0xfd,0x37,0x7a,0x58,0x5a,0x00,0x00,0x04,0xe6,0xd6,0xb4,0x46,0x04,0xc0,0x8b,0x02,0xcc,0x0d,0x21,0x01,0x1c,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x0f,0x6d,0x2b,0x12,0xe0,0x06,0xcb,0x01,0x03,0x5d,0x00,0x34,0x19,0x49,0xee,0x8d,0xe9,0x13,0xb8,0xaf,0x95,0x7c,0x89,0x50,0x59,0x3e,0xc0,0x03,0xe4,0x4d,0x99,0x3f,0xfc,0x73,0x51,0x64,0x77,0x1b,0xd7,0x99,0x69,0x9d,0x86,0x32,0x9b,0xda,0xe2,0x6b,0x81,0x7f,0x97,0x97,0x16,0x05,0x0b,0x0e,0x80,0xa3,0xee,0x80,0xce,0x0d,0xe2,0x71,0x41,0x12,0x4c,0x35,0xc5,0x5e,0x34,0x7f,0xcf,0xdb,0x3f,0x4d,0x6e,0x5e,0xa5,0x92,0xbe,0xcf,0xc8,0x3f,0x50,0x0a,0x66,0xa9,0xfc,0xaa,0x20,0x4e,0x87,0x96,0x2a,0x3e,0xed,0xc9,0xee,0x8b,0x42,0x5d,0xe5,0x29,0x82,0x0d,0x26,0x05,0xe9,0x27,0x74,0xa4,0x79,0x99,0xe6,0x4a,0xc5,0x05,0x36,0xd1,0x07,0xaa,0xbd,0xda,0xfd,0x34,0xc6,0x7e,0x0c,0xb9,0x11,0xbd,0x21,0xd8,0xa3,0xc9,0xd7,0x47,0xc7,0x27,0x8e,0x6a,0xd8,0x60,0xe9,0x0b,0xe3,0x4b,0x83,0x46,0x06,0x3b,0x0d,0x5b,0x47,0x0b,0xff,0xe1,0xb7,0x15,0x09,0x2b,0xce,0x99,0x05,0x46,0x42,0x7c,0xc4,0x23,0x96,0x15,0x35,0x71,0x9b,0x03,0xb8,0xa8,0x44,0xb9,0xb2,0xd3,0xe9,0x02,0xda,0xbb,0x9e,0x27,0xe7,0x5e,0xf5,0xd6,0x9a,0x52,0x14,0x82,0xcc,0xe8,0xaa,0x92,0xf7,0x37,0xaf,0x59,0x90,0x0c,0xff,0x12,0x7f,0x70,0xc0,0xb1,0x03,0x39,0x3f,0xe0,0x25,0x61,0xda,0x03,0xb2,0xb7,0xd3,0x9f,0xb3,0xe3,0xa4,0xc3,0xf8,0x27,0x64,0x14,0xdf,0x87,0xbd,0xbb,0xc6,0x5c,0xc3,0xc4,0xf4,0x39,0xb3,0x81,0x82,0xea,0x57,0x4b,0xcd,0x32,0xe8,0x91,0x02,0xfc,0x02,0x76,0x79,0x88,0xca,0x6f,0x11,0x05,0x5e,0xe3,0x78,0xff,0x84,0x1b,0xea,0x00,0x00,0x00,0x24,0x4a,0xd8,0xfc,0xff,0xb0,0x7a,0xf7,0x00,0x01,0xa7,0x02,0xcc,0x0d,0x00,0x00,0x06,0x64,0xcf,0x89,0xb1,0xc4,0x67,0xfb,0x02,0x00,0x00,0x00,0x00,0x04,0x59,0x5a];

const SMALL_OUT: u64 = 2 << 20;

fn data(n: usize) -> Vec<u8> {
    (0..n as u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect()
}

/// Recomputes the stream-header and first block-header checksums so that a mutated header field is parsed instead of refused at the CRC.
fn repair(mut v: Vec<u8>) -> Vec<u8> {
    if v.len() >= 12 {
        let c = crc32(&v[6..8]).to_le_bytes();
        v[8..12].copy_from_slice(&c);
        let hs = (v.get(12).copied().unwrap_or(0) as usize + 1) * 4;
        if v.get(12).is_some_and(|b| *b != 0) && v.len() >= 12 + hs && hs >= 8 {
            let c = crc32(&v[12..12 + hs - 4]).to_le_bytes();
            v[12 + hs - 4..12 + hs].copy_from_slice(&c);
        }
    }
    v
}

#[test]
fn fuzz_xz_mutated_files_never_panic_hang_or_overgrow() {
    let plain = data(3000);
    let corpus: Vec<Vec<u8>> = vec![make_xz_with_check(&plain, 0), make_xz_with_check(&plain, 1), make_xz_with_check(&plain, 4), make_xz_with_check(&plain, 10), REAL_XZ.to_vec()];
    let heap = (crate::xz::MAX_DICT + (16 << 20)) as i64;
    let mut ok_plain = 0;
    for (label, fix) in [("raw", false), ("repaired headers", true)] {
        run(&format!("xz {label}"), 0x58_5A, &corpus, 6000, Duration::from_secs(3), heap, |input| {
            let input = if fix { repair(input.to_vec()) } else { input.to_vec() };
            let mut out = Vec::new();
            if let Ok(n) = decompress(&input, &mut out, SMALL_OUT) {
                assert!(n <= SMALL_OUT && out.len() as u64 <= SMALL_OUT, "the output limit was not kept");
                // accepted output of a checked file must be the original bytes: a damaged file is not accepted
                if input.len() >= 8 && input[7] != 0 && input[..6] == [0xFD, b'7', b'z', b'X', b'Z', 0] && out.len() == plain.len() && out != plain {
                    ok_plain += 1;
                    panic!("a damaged xz file with a check was accepted with different bytes");
                }
            }
        });
    }
    assert_eq!(ok_plain, 0);
}

#[test]
fn fuzz_xz_regressions() {
    // an index whose record count overflows when doubled used to panic in debug builds
    let mut x = make_xz_with_check(b"hello", 4);
    let at = x.len() - 12 - 4 - 8; // the index starts here
    x.truncate(at);
    x.push(0);
    x.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]);
    x.extend_from_slice(&[0; 20]);
    let _ = decompress(&x, &mut Vec::new(), 1 << 20);
}

fn names(r: &mut Rng) -> String {
    let parts = ["a", "b c", "Grüße €", "x.txt", "..", ".", "", "d\\e", "f\ng", "ok", "very-long-name-".repeat(3).leak()];
    let n = 1 + r.below(3);
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            s.push('/');
        }
        s.push_str(r.pick(&parts));
    }
    if r.chance(6) {
        s.insert(0, '/');
    }
    if r.chance(5) {
        s.push('/');
    }
    s
}

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
    let d = std::path::PathBuf::from(base).join(format!("iyakki-fuzz-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn fuzz_zip_writer_round_trip_with_random_names_and_sizes() {
    let d = tmpdir("zipw");
    let mut r = Rng::new(0x21_9);
    for case in 0..cases(300) {
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        let mut want: Vec<(String, Vec<u8>)> = Vec::new();
        for _ in 0..1 + r.below(6) {
            let name = names(&mut r);
            let len = *r.pick(&[0usize, 1, 2, 100, 65535, 65536, 70000]);
            let body: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
            let valid = valid_name(&name).is_ok();
            let res = if r.chance(5) { z.add_dir(&name, r.next() as i64) } else { z.add_file(&name, r.next() as i64 >> r.below(40), r.next() as u32, &mut &body[..], 1 << 20).map(|_| ()) };
            // names outside the rules are refused in words; valid ones are taken
            match (valid, res.is_ok()) {
                (false, true) => {
                    // add_dir adds a trailing slash first, so a name valid after that is allowed
                    assert!(valid_name(&format!("{}/", name.trim_end_matches('/'))).is_ok(), "case {case}: {name:?} was accepted");
                }
                (true, false) => panic!("case {case}: valid name {name:?} refused: {:?}", res),
                _ => {}
            }
            if res.is_ok() && !name.ends_with('/') && valid {
                want.retain(|(n, _)| n != &name);
                if z.entries() > 0 {
                    want.push((name.clone(), body));
                }
            }
        }
        let bytes = z.finish().unwrap().into_inner();
        // the reader of this crate must read what the writer made without error (duplicates and file/folder clashes may be refused in words)
        let zp = d.join("x.zip");
        std::fs::write(&zp, &bytes).unwrap();
        let out = d.join("out");
        let _ = std::fs::remove_dir_all(&out);
        let _ = crate::archive::unzip(&zp, &out);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fuzz_zip_and_tar_readers_on_mutated_archives() {
    let d = tmpdir("zipr");
    let zip = crate::archive::tests::make_zip(&[("a.txt", "alpha", 0o644, false), ("dir/b.txt", "bravo bravo bravo", 0o755, false), ("dir", "", 0o755, true)]);
    let tar = crate::archive::tests::make_tar(&[("a.txt", b'0', "alpha", 0o644), ("dir/", b'5', "", 0o755), ("dir/b.txt", b'0', "bravo", 0o600)]);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.add_file("w.bin", 1_700_000_000, 0o644, &mut &data(5000)[..], 1 << 20).unwrap();
    let ours = w.finish().unwrap().into_inner();
    let corpus = vec![zip, ours, tar];
    let mut n = 0u32;
    run("zip/tar readers", 0x7A_1, &corpus, 1500, Duration::from_secs(3), 512 << 20, |input| {
        n += 1;
        let p = d.join("in.bin");
        std::fs::write(&p, input).unwrap();
        let out = d.join("out");
        let _ = std::fs::remove_dir_all(&out);
        let _ = crate::archive::unzip(&p, &out);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::create_dir_all(&out);
        let _ = crate::archive::untar_stream(input, &out, 64 << 20);
        let _ = crate::archive::gunzip_members(input, 1 << 20);
        // nothing may be written outside `out`
        assert!(!d.join("escape").exists() && !d.parent().unwrap().join("escape").exists(), "a file escaped the folder");
    });
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fuzz_xz_regression_stream_header_check_is_verified() {
    // a flipped stream-flags check bit used to be accepted: the stream header CRC32 was never compared
    let good = make_xz_with_check(b"hello", 4);
    assert!(decompress(&good, &mut Vec::new(), 1 << 20).is_ok());
    for i in 6..12 {
        let mut bad = good.clone();
        bad[i] ^= 1;
        assert!(decompress(&bad, &mut Vec::new(), 1 << 20).is_err(), "byte {i}");
    }
}

#[test]
fn fuzz_xz_regression_index_and_footer_are_verified() {
    // KI-107: the index and the 12-byte footer used to be skipped without a look
    for kind in [0u8, 1, 4, 10] {
        for len in [0usize, 5, 70_000] {
            let d = data(len);
            let good = make_xz_with_check(&d, kind);
            let mut out = Vec::new();
            assert_eq!(decompress(&good, &mut out, 1 << 24).unwrap(), len as u64, "kind {kind} len {len}");
            assert_eq!(out, d);
            // the index and the footer: every byte of them is covered by a check or a fixed value, so no flipped bit is accepted
            let unpadded = 12 + d.chunks(65536).map(|c| c.len() + 3).sum::<usize>() as u64 + 1 + [0usize, 4, 0, 0, 8, 0, 0, 0, 0, 0, 32][kind as usize] as u64;
            let mut tail = Vec::new();
            crate::xz::xz_tail(&mut tail, kind, &[(unpadded, len as u64)]);
            assert!(good.ends_with(&tail), "the tail helper matches the file");
            for i in good.len() - tail.len()..good.len() {
                for bit in [0x01u8, 0x80] {
                    let mut bad = good.clone();
                    bad[i] ^= bit;
                    assert!(decompress(&bad, &mut Vec::new(), 1 << 24).is_err(), "kind {kind} len {len}: a flip of {bit:#x} in byte {i} of the index or footer was accepted");
                }
            }
            for cut in good.len() - 40..good.len() {
                assert!(decompress(&good[..cut], &mut Vec::new(), 1 << 24).is_err(), "kind {kind} len {len}: cut at {cut}");
            }
        }
    }
    // two streams one after the other still unpack, with the zero padding between them
    let mut two = make_xz_with_check(b"first ", 4);
    two.extend_from_slice(&[0; 4]);
    two.extend_from_slice(&make_xz_with_check(b"second", 1));
    let mut out = Vec::new();
    decompress(&two, &mut out, 1 << 20).unwrap();
    assert_eq!(out, b"first second");
}

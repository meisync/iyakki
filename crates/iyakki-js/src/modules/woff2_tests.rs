// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

use super::super::webfont::tests::font;
use super::super::webfont::FontMetrics;
use super::*;

fn base128_bytes(mut v: u32) -> Vec<u8> {
    let mut groups = vec![(v & 0x7f) as u8];
    v >>= 7;
    while v > 0 {
        groups.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    groups.reverse();
    groups
}

/// A Brotli stream that stores `data` as uncompressed meta-blocks (what the encoder writes for bytes it cannot shrink).
fn store(data: &[u8]) -> Vec<u8> {
    let mut bits: Vec<bool> = vec![false]; // window 16
    let push = |bits: &mut Vec<bool>, v: u32, n: u32| (0..n).for_each(|i| bits.push((v >> i) & 1 == 1));
    let mut chunks = data.chunks(60000).peekable();
    while let Some(c) = chunks.next() {
        push(&mut bits, 0, 1); // not last
        push(&mut bits, 0, 2); // four nibbles
        push(&mut bits, c.len() as u32 - 1, 16);
        push(&mut bits, 1, 1); // uncompressed
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let mut bytes: Vec<u8> = bits.chunks(8).map(|b| b.iter().enumerate().fold(0u8, |a, (i, &x)| a | (u8::from(x) << i))).collect();
        bytes.extend_from_slice(c);
        bits = bytes.iter().flat_map(|b| (0..8).map(move |i| (b >> i) & 1 == 1)).collect();
    }
    push(&mut bits, 3, 2); // last and empty
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    bits.chunks(8).map(|b| b.iter().enumerate().fold(0u8, |a, (i, &x)| a | (u8::from(x) << i))).collect()
}

/// A WOFF2 file with these tables as (tag, bytes, transform version, transformed bytes): `None` for the version-0 default of a table that
/// is not transformed by default.
fn woff2(flavor: &[u8], tables: &[([u8; 4], Vec<u8>, u8, Option<Vec<u8>>)]) -> Vec<u8> {
    let mut dir = Vec::new();
    let mut stream = Vec::new();
    for (tag, orig, version, transformed) in tables {
        let idx = KNOWN.iter().position(|k| *k == tag).map_or(63, |i| i as u8);
        dir.push(idx | (version << 6));
        if idx == 63 {
            dir.extend_from_slice(tag);
        }
        dir.extend(base128_bytes(orig.len() as u32));
        match transformed {
            Some(t) => {
                dir.extend(base128_bytes(t.len() as u32));
                stream.extend_from_slice(t);
            }
            None => stream.extend_from_slice(orig),
        }
    }
    let packed = store(&stream);
    let mut out = b"wOF2".to_vec();
    out.extend_from_slice(flavor);
    out.extend_from_slice(&[0; 4]); // length, not read
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0; 2]);
    out.extend_from_slice(&[0; 4]); // totalSfntSize
    out.extend_from_slice(&(packed.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0; 24]);
    out.extend(dir);
    out.extend(packed);
    out
}

/// The tables of the test font as they sit in its sfnt file.
fn tables_of(sfnt: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let n = u16_at(sfnt, 4).unwrap() as usize;
    (0..n)
        .map(|i| {
            let r = 12 + i * 16;
            let (off, len) = (u32_at(sfnt, r + 8).unwrap() as usize, u32_at(sfnt, r + 12).unwrap() as usize);
            (sfnt[r..r + 4].try_into().unwrap(), sfnt[off..off + len].to_vec())
        })
        .collect()
}

#[test]
fn the_table_names_are_the_63_of_the_standard_in_its_order() {
    assert_eq!(KNOWN.len(), 63);
    assert_eq!((KNOWN[0], KNOWN[3], KNOWN[10], KNOWN[11], KNOWN[27], KNOWN[62]), (b"cmap", b"hmtx", b"glyf", b"loca", b"GPOS", b"Sill"));
}

#[test]
fn a_woff2_file_with_plain_tables_is_measured_like_the_plain_font() {
    let plain = font();
    let tables: Vec<_> = tables_of(&plain).into_iter().map(|(t, b)| (t, b, 0, None)).collect();
    let f = FontMetrics::parse(&woff2(&plain[..4], &tables)).expect("the WOFF2 file is read");
    assert!((f.text_em("AV") - 1.26).abs() < 1e-6, "kerned like the plain file: {}", f.text_em("AV"));
    assert!((f.text_em("A V") - FontMetrics::parse(&plain).unwrap().text_em("A V")).abs() < 1e-6);
}

#[test]
fn a_transformed_hmtx_gives_its_advances_and_a_transformed_glyf_is_left_out() {
    let plain = font();
    let mut tables = Vec::new();
    for (tag, bytes) in tables_of(&plain) {
        if &tag == b"hmtx" {
            // the transform: flags 3 (both bearing lists left out), then the four advances of the font
            let mut t = vec![3u8];
            for a in [500u16, 700, 700, 250] {
                t.extend_from_slice(&a.to_be_bytes());
            }
            tables.push((tag, bytes, 1, Some(t)));
        } else {
            tables.push((tag, bytes, 0, None));
        }
    }
    // a glyf and a loca with the default transform: their transformed bytes are not looked at
    tables.push((*b"glyf", vec![0; 40], 0, Some(vec![1, 2, 3, 4, 5])));
    tables.push((*b"loca", vec![0; 10], 0, Some(vec![])));
    let sfnt = unwoff2(&woff2(&plain[..4], &tables)).expect("unpacked");
    assert!(table_of(&sfnt, b"glyf").is_none() && table_of(&sfnt, b"loca").is_none());
    let f = FontMetrics::parse(&woff2(&plain[..4], &tables)).expect("read");
    assert!((f.text_em("AV") - 1.26).abs() < 1e-6);
    // an hmtx flag that is reserved, or one that is cut short, is refused
    assert!(rebuild_hmtx(&[0x10, 0, 0], 1, 1).is_none());
    assert!(rebuild_hmtx(&[0, 0], 1, 1).is_none());
    assert!(rebuild_hmtx(&[3, 1, 244], 2, 2).is_none(), "fewer advances than metrics");
    assert_eq!(rebuild_hmtx(&[1, 1, 244, 0, 7], 1, 2).unwrap(), vec![1, 244, 0, 0, 0, 7], "bearings of the rest are kept when present");
}

fn table_of<'a>(sfnt: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    tables_of_ref(sfnt).into_iter().find(|(t, _)| t == tag).map(|(_, b)| b)
}

fn tables_of_ref(sfnt: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let n = u16_at(sfnt, 4).unwrap() as usize;
    (0..n)
        .map(|i| {
            let r = 12 + i * 16;
            let (off, len) = (u32_at(sfnt, r + 8).unwrap() as usize, u32_at(sfnt, r + 12).unwrap() as usize);
            (sfnt[r..r + 4].try_into().unwrap(), &sfnt[off..off + len])
        })
        .collect()
}

#[test]
fn base128_numbers_follow_the_standard_and_refuse_a_leading_zero_group_and_overflow() {
    for v in [0u32, 1, 127, 128, 300, 16383, 16384, 2_097_151, 0x0fff_ffff, 0xffff_ffff] {
        let b = base128_bytes(v);
        let mut at = 0;
        assert_eq!(base128(&b, &mut at), Some(v), "{v}");
        assert_eq!(at, b.len());
    }
    assert_eq!(base128(&[0x80, 0x01], &mut 0), None, "a leading 0x80 is not allowed");
    assert_eq!(base128(&[0xff, 0xff, 0xff, 0xff, 0xff, 0x7f], &mut 0), None, "more than five bytes");
    assert_eq!(base128(&[0x81], &mut 0), None, "cut short");
}

#[test]
fn collections_damaged_and_oversized_files_are_refused_and_nothing_panics() {
    let plain = font();
    let tables: Vec<_> = tables_of(&plain).into_iter().map(|(t, b)| (t, b, 0, None)).collect();
    let good = woff2(&plain[..4], &tables);
    assert!(FontMetrics::parse(&good).is_some());
    let mut ttc = good.clone();
    ttc[4..8].copy_from_slice(b"ttcf");
    assert!(FontMetrics::parse(&ttc).is_none(), "a collection is not read");
    for cut in (0..good.len()).step_by(3) {
        let _ = FontMetrics::parse(&good[..cut]);
    }
    for i in (0..good.len()).step_by(2) {
        let mut v = good.clone();
        v[i] ^= 0xa5;
        let _ = FontMetrics::parse(&v).map(|f| f.text_em("AV"));
    }
    // a table that says it is bigger than the cap
    let big = woff2(&plain[..4], &[(*b"head", vec![0; 54], 1, Some(vec![0; 54]))]);
    let mut huge = big.clone();
    huge[48 + 1 + 1] = 0xff; // the declared length gets a high group
    let _ = FontMetrics::parse(&huge);
    let toomany = woff2(&plain[..4], &(0..65).map(|i| ([b'x', b'x', b'x', i as u8 + 32], vec![0u8; 4], 0, None)).collect::<Vec<_>>());
    assert!(unwoff2(&toomany).is_none(), "more than 64 tables");
}

/// The Debian package of the glyph font ships the same font as WOFF and as WOFF2 (the WOFF2 one has a transformed glyf and hmtx): both must
/// give the same measurements. The files are on the development machines that have the package, not in this repository; where they are not
/// there the test says so and passes (the hermetic tests above cover the container).
#[test]
fn a_real_woff2_font_measures_like_the_woff_of_the_same_font_when_the_files_are_on_this_machine() {
    let dir = std::path::Path::new("/usr/share/fonts-glyphicons");
    let (Ok(a), Ok(b)) = (std::fs::read(dir.join("glyphicons-halflings-regular.woff")), std::fs::read(dir.join("glyphicons-halflings-regular.woff2"))) else {
        eprintln!("no glyph font files here: the real-font comparison is skipped");
        return;
    };
    let (w1, w2) = (FontMetrics::parse(&a).expect("woff"), FontMetrics::parse(&b).expect("woff2"));
    for text in ["\u{e001}", "\u{e002}\u{e003}\u{e010}", "\u{e105}x \u{e120}"] {
        assert!((w1.text_em(text) - w2.text_em(text)).abs() < 1e-6, "{text:?}: {} against {}", w1.text_em(text), w2.text_em(text));
    }
    assert!(w2.text_em("\u{e001}\u{e002}") > 0.0);
}

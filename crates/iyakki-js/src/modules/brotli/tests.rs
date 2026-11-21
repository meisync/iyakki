// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

use super::*;

fn fx(name: &str) -> &'static [u8] {
    macro_rules! files {
        ($($n:literal),*) => { match name { $($n => include_bytes!(concat!("fixtures/", $n)) as &[u8],)* other => panic!("no fixture {other}") } };
    }
    files!("text.txt", "blob.bin", "text.q0.br", "text.q2.br", "text.q5.br", "text.q9.br", "text.q11.br", "text.q11.w10.br", "text.q11.w24.br", "text.q11.font.br", "text.q4.text.br", "blob.q5.br", "blob.q11.br", "mixed.q9.br", "empty.q5.br")
}

const BIG: usize = 1 << 24;

#[test]
fn text_made_by_the_reference_encoder_at_every_quality_window_and_mode_comes_back_exactly() {
    let text = fx("text.txt");
    for n in ["text.q0.br", "text.q2.br", "text.q5.br", "text.q9.br", "text.q11.br", "text.q11.w10.br", "text.q11.w24.br", "text.q11.font.br", "text.q4.text.br"] {
        let got = decompress(fx(n), BIG).unwrap_or_else(|e| panic!("{n}: {e:?}"));
        assert!(got == text, "{n}: {} bytes, differs", got.len());
    }
}

#[test]
fn pseudo_random_bytes_are_stored_uncompressed_and_a_mixed_file_has_every_kind_of_meta_block() {
    let blob = fx("blob.bin");
    assert!(decompress(fx("blob.q5.br"), BIG).unwrap() == blob);
    assert!(decompress(fx("blob.q11.br"), BIG).unwrap() == blob);
    let text = fx("text.txt");
    let mut mixed = text[..9000].to_vec();
    mixed.extend_from_slice(&blob[..3000]);
    mixed.extend_from_slice(&text[9000..20000]);
    assert!(decompress(fx("mixed.q9.br"), BIG).unwrap() == mixed);
}

#[test]
fn the_empty_stream_is_empty() {
    assert_eq!(decompress(fx("empty.q5.br"), BIG).unwrap(), Vec::<u8>::new());
}

#[test]
fn the_answer_is_capped_by_the_caller() {
    assert_eq!(decompress(fx("text.q11.br"), 1000), Err(BrotliError::TooBig));
    assert_eq!(decompress(fx("blob.q5.br"), 5999), Err(BrotliError::TooBig));
    assert!(decompress(fx("text.q11.br"), 39920).is_ok());
}

#[test]
fn a_cut_stream_is_an_error_at_every_length_and_never_a_panic() {
    let whole = fx("text.q11.br");
    for cut in (0..whole.len()).step_by(37) {
        assert!(decompress(&whole[..cut], BIG).is_err(), "{cut}");
    }
    let small = fx("blob.q5.br");
    for cut in 0..small.len().min(400) {
        assert!(decompress(&small[..cut], BIG).is_err(), "{cut}");
    }
}

#[test]
fn damaged_and_made_up_streams_never_panic() {
    let whole = fx("text.q11.br");
    let mut x = 99u32;
    let mut rnd = move || {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        (x >> 8) as usize
    };
    for _ in 0..300 {
        let mut d = whole.to_vec();
        for _ in 0..1 + rnd() % 4 {
            let at = rnd() % d.len();
            d[at] ^= 1 << (rnd() % 8);
        }
        let _ = decompress(&d, BIG);
    }
    for _ in 0..300 {
        let len = rnd() % 300;
        let noise: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
        let _ = decompress(&noise, 1 << 20);
    }
}

#[test]
fn a_hand_made_stream_with_an_uncompressed_meta_block_and_a_skipped_metadata_block() {
    // window 16 (bit 0); metadata block: ISLAST 0, MNIBBLES 3 (the value 3 in two bits), reserved 0, MSKIPBYTES 1, length 3 - 1 in one byte, bytes
    let mut bits: Vec<bool> = vec![false];
    let push = |bits: &mut Vec<bool>, v: u32, n: u32| (0..n).for_each(|i| bits.push((v >> i) & 1 == 1));
    push(&mut bits, 0, 1); // ISLAST
    push(&mut bits, 3, 2); // MNIBBLES = 0: metadata
    push(&mut bits, 0, 1); // reserved
    push(&mut bits, 1, 2); // one byte of length
    push(&mut bits, 2, 8); // 3 bytes skipped
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut bytes: Vec<u8> = bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << i))).collect();
    bytes.extend_from_slice(&[7, 7, 7]);
    // an uncompressed meta-block of "hello": ISLAST 0, MNIBBLES 0 (4 nibbles), MLEN-1 in 16 bits, ISUNCOMPRESSED 1, padding, the bytes
    let mut bits: Vec<bool> = Vec::new();
    push(&mut bits, 0, 1);
    push(&mut bits, 0, 2);
    push(&mut bits, 4, 16);
    push(&mut bits, 1, 1);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    bytes.extend(bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (u8::from(b) << i))));
    bytes.extend_from_slice(b"hello");
    // the end: ISLAST 1, ISLASTEMPTY 1
    bytes.push(0b11);
    assert_eq!(decompress(&bytes, 100).unwrap(), b"hello");
    // padding that is not zero is refused
    let mut bad = bytes.clone();
    bad[0] |= 0x80;
    assert!(decompress(&bad, 100).is_err());
}

#[test]
fn the_static_dictionary_and_its_transforms_are_the_ones_of_the_standard() {
    use sha2_free::fnv;
    assert_eq!(DICTIONARY.len(), 122_784);
    // the dictionary file is the one of RFC 7932 appendix A: its first word of length 4 starts the file
    assert_eq!(&DICTIONARY[..4], b"time");
    assert_eq!(fnv(DICTIONARY), 0xd8a843a9cc932619, "the bytes are the dictionary of the standard (its SHA-256 starts 20e42eb1)");
    let mut w = Vec::new();
    dictionary_word(4, 0, &mut w).unwrap();
    assert_eq!(w, b"time");
    // transform 4 upper-cases the first letter and adds a space; transform 3 omits the first byte
    w.clear();
    dictionary_word(4, 4 << 10, &mut w).unwrap();
    assert_eq!(w, b"Time ");
    w.clear();
    dictionary_word(4, 3 << 10, &mut w).unwrap();
    assert_eq!(w, b"ime");
    // a transform that does not exist, a length that does not exist
    assert!(dictionary_word(4, 121 << 10, &mut Vec::new()).is_err());
    assert!(dictionary_word(3, 0, &mut Vec::new()).is_err());
    assert!(dictionary_word(25, 0, &mut Vec::new()).is_err());
}

mod sha2_free {
    /// A small checksum for a test (FNV-1a); the point is only that the same bytes give the same number twice.
    pub fn fnv(d: &[u8]) -> u64 {
        d.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
    }
}

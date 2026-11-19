//! The in-house FLAC decoder against what ffmpeg's own decoder gives for the same files (committed, see
//! `fixtures/flac/make-fixtures.sh`): every sample must match. The files cover independent, left/side, right/side and mid/side
//! stereo, fixed and LPC predictors, verbatim blocks, both Rice codings and 24 bits per sample.

use iyakki_audiodec::{flac, StreamDecoder};
use std::path::Path;

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flac")
}

fn reference(name: &str) -> Vec<i16> {
    if let Ok(b) = std::fs::read(dir().join(format!("{name}.s16"))) {
        return b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    }
    let b = std::fs::read(dir().join(format!("{name}.s32"))).unwrap();
    b.chunks_exact(4).map(|c| (i32::from_le_bytes([c[0], c[1], c[2], c[3]]) >> 16) as i16).collect()
}

fn decode(bytes: &[u8], chunk: usize) -> Vec<i16> {
    let mut d = StreamDecoder::new();
    let mut out = Vec::new();
    for c in bytes.chunks(chunk) {
        out.extend(d.push(c).unwrap());
    }
    out
}

#[test]
fn every_fixture_decodes_to_the_same_samples_as_ffmpeg_whole_and_in_small_pieces() {
    let mut n = 0;
    for e in std::fs::read_dir(dir()).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "flac") {
            continue;
        }
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(&p).unwrap();
        let want = reference(&name);
        for chunk in [bytes.len(), 997, 13] {
            let got = decode(&bytes, chunk);
            assert_eq!(got.len(), want.len(), "{name} in pieces of {chunk}: sample count");
            if let Some(i) = got.iter().zip(&want).position(|(a, b)| a != b) {
                panic!("{name} in pieces of {chunk}: sample {i} is {} but ffmpeg says {}", got[i], want[i]);
            }
        }
        n += 1;
    }
    assert!(n >= 9, "the fixtures are there: {n}");
}

#[test]
fn the_stream_says_its_format_and_a_mono_file_has_one_channel() {
    let mut d = StreamDecoder::new();
    d.push(&std::fs::read(dir().join("stereo-ms.flac")).unwrap()).unwrap();
    assert!(d.ready() && d.rate == 8000 && d.channels == 2);
    let mut d = StreamDecoder::new();
    d.push(&std::fs::read(dir().join("mono-lpc.flac")).unwrap()).unwrap();
    assert!(d.ready() && d.channels == 1);
}

#[test]
fn damaged_input_is_skipped_or_refused_and_never_panics() {
    let good = std::fs::read(dir().join("stereo-ms.flac")).unwrap();
    // a flipped byte inside a frame: that frame is dropped (its CRC fails), the decoder goes on and does not panic
    let mut bad = good.clone();
    let mid = bad.len() - 100; // the file ends with the frame (a padding block comes before it)
    bad[mid] ^= 0xFF;
    let got = decode(&bad, 500);
    assert!(got.len() < 3200 && got.len() % 2 == 0);
    // truncated at every length, and noise: no panic
    for cut in (0..good.len()).step_by(37) {
        let mut d = StreamDecoder::new();
        let _ = d.push(&good[..cut]);
    }
    let mut x = 0x1234_5678u32;
    let noise: Vec<u8> = (0..4000).map(|_| { x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (x >> 24) as u8 }).collect();
    let mut d = StreamDecoder::new();
    let _ = d.push(b"fLaC");
    let _ = d.push(&noise);
    // a stream that is not FLAC and not Ogg yields nothing
    let mut d = StreamDecoder::new();
    assert!(d.push(&noise).map(|v| v.is_empty()).unwrap_or(true));
    // the checksums used are the real ones
    assert_eq!(flac::crc8(b"123456789"), 0xF4);
    assert_eq!(flac::crc16(b"123456789"), 0xFEE8);
}

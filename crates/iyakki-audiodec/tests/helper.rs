//! The program itself: its two pipes, its header and its way of saying why it cannot play a stream.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn run(input: &[u8]) -> (Vec<u8>, String, Option<i32>) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_iyakki-audiodec")).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdin = c.stdin.take().unwrap();
    let data = input.to_vec();
    let w = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let mut out = vec![];
    c.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let mut err = String::new();
    c.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    let st = c.wait().unwrap();
    let _ = w.join();
    (out, err, st.code())
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

#[test]
fn a_vorbis_stream_comes_out_as_a_header_and_the_samples_of_the_reference() {
    let (out, err, code) = run(&fixture("stereo-22k.ogg"));
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(&out[..4], b"IYPC");
    assert_eq!(u32::from_le_bytes([out[4], out[5], out[6], out[7]]), 22050);
    assert_eq!(u16::from_le_bytes([out[8], out[9]]), 2);
    let reference = fixture("stereo-22k.ref.s16");
    assert!((out.len() - 10).abs_diff(reference.len()) <= 1024);
    let n = (out.len() - 10).min(reference.len());
    // ffmpeg's decode of this file is 256 sample frames shorter than the stream's own count; compare the aligned start
    let max = out[10..10 + n].chunks_exact(2).zip(reference[..n].chunks_exact(2)).map(|(a, b)| (i16::from_le_bytes([a[0], a[1]]) as i32 - i16::from_le_bytes([b[0], b[1]]) as i32).abs()).max().unwrap();
    assert!(max <= 3, "{max}");
}

#[test]
fn a_stream_that_is_not_ogg_ends_with_status_two_and_words_on_the_error_output() {
    let (out, err, code) = run(b"this is not an ogg stream at all, it is plain text");
    assert!(out.is_empty());
    assert_eq!(code, Some(2));
    assert!(err.contains("no Ogg Vorbis audio"), "{err}");
}

#[test]
fn an_empty_input_is_the_same_plain_failure() {
    let (out, err, code) = run(b"");
    assert!(out.is_empty() && code == Some(2) && !err.is_empty());
}

#[test]
fn a_cut_stream_ends_cleanly_with_what_was_whole_or_with_the_plain_failure() {
    // the fixture keeps its sound in one or two pages, so a cut gives either the header and some sound or nothing at all
    let f = fixture("stereo-22k.ogg");
    for cut in [100, 1500, 3700, f.len() * 4 / 5, f.len() - 1] {
        let (out, err, code) = run(&f[..cut]);
        assert!(code == Some(0) || (code == Some(2) && out.is_empty() && !err.is_empty()), "cut {cut}: {code:?} {err}");
        assert!(out.len() < 10 + 4 * 13_230 + 16);
    }
}

#[test]
fn an_opus_stream_comes_out_as_a_header_at_48_khz_and_the_samples_of_the_reference() {
    for (name, channels) in [("celt-stereo-20ms", 2u16), ("hybrid-mono-fb-20ms", 1), ("silk-stereo-wb-20ms", 2)] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opus");
        let (out, err, code) = run(&std::fs::read(dir.join(format!("{name}.opus"))).unwrap());
        assert_eq!(code, Some(0), "{name}: {err}");
        assert_eq!(&out[..4], b"IYPC");
        assert_eq!(u32::from_le_bytes([out[4], out[5], out[6], out[7]]), 48000);
        assert_eq!(u16::from_le_bytes([out[8], out[9]]), channels);
        let reference = std::fs::read(dir.join(format!("{name}.ref.s16"))).unwrap();
        assert_eq!(out.len() - 10, reference.len(), "{name}");
        let max = out[10..].chunks_exact(2).zip(reference.chunks_exact(2)).map(|(a, b)| (i16::from_le_bytes([a[0], a[1]]) as i32 - i16::from_le_bytes([b[0], b[1]]) as i32).abs()).max().unwrap();
        assert!(max <= 2, "{name}: {max}");
    }
}

use super::*;
use iyakki_core_local::devctl::a2dp_aac::{adts_wrap, FRAME_SAMPLES};
use std::os::unix::fs::PermissionsExt;

const LIMIT: Duration = Duration::from_secs(60);

fn cfg() -> AacConfig {
    AacConfig { rate: 48000, channels: 2, bit_rate: 128_000 }
}

fn script(tag: &str, body: &str) -> String {
    let d = std::env::temp_dir().join(format!("iyakki-btaac-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("ffmpeg");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

#[test]
fn the_calls_are_fixed_lists_with_numbers_from_the_setting_and_no_shell() {
    assert_eq!(decode_args(&cfg()).join(" "), "-hide_banner -loglevel error -fflags nobuffer -probesize 2048 -analyzeduration 0 -f aac -i pipe:0 -f s16le -ac 2 -ar 48000 pipe:1");
    assert_eq!(encode_args(&AacConfig { rate: 44100, channels: 1, bit_rate: 96_000 }).join(" "), "-hide_banner -loglevel error -f s16le -ar 44100 -ac 1 -i pipe:0 -c:a aac -profile:a aac_low -b:a 96000 -flush_packets 1 -f adts pipe:1");
    // a silly bit rate is held to the range
    assert!(encode_args(&AacConfig { bit_rate: 1, ..cfg() }).contains(&"16000".to_string()));
    assert!(encode_args(&AacConfig { bit_rate: u32::MAX, ..cfg() }).contains(&"320000".to_string()));
    // the arguments really are what the program receives
    let rec = std::env::temp_dir().join(format!("iyakki-btaac-args-{}", std::process::id()));
    let p = script("args", &format!("echo \"$@\" > {}; cat >/dev/null", rec.display()));
    let mut d = FfmpegDecoder::spawn(&p, &cfg()).unwrap();
    assert!(d.finish_and_wait(LIMIT));
    assert_eq!(std::fs::read_to_string(&rec).unwrap().trim(), decode_args(&cfg()).join(" "));
    let _ = std::fs::remove_file(rec);
}

#[test]
fn a_decoder_program_gets_the_frames_and_its_sound_is_handed_out_as_samples() {
    // stand-in: copies its input to its output
    let p = script("cat", "cat");
    let mut d = FfmpegDecoder::spawn(&p, &cfg()).unwrap();
    d.feed(&[1, 0, 2, 0, 3, 0]).unwrap();
    assert!(d.finish_and_wait(LIMIT));
    assert_eq!(d.drain().unwrap(), vec![1, 2, 3]);
    assert_eq!(d.drain().unwrap(), Vec::<i16>::new());
}

#[test]
fn an_encoder_program_output_is_cut_into_raw_frames_whatever_the_chunks() {
    // stand-in: ignores its input and writes two ADTS frames
    let c = cfg();
    let mut stream = adts_wrap(&c, &[7u8; 30]).unwrap();
    stream.extend(adts_wrap(&c, &[8u8; 20]).unwrap());
    let d = std::env::temp_dir().join(format!("iyakki-btaac-frames-{}", std::process::id()));
    std::fs::write(&d, &stream).unwrap();
    let p = script("frames", &format!("cat {}; cat >/dev/null", d.display()));
    let mut e = FfmpegEncoder::spawn(&p, &c).unwrap();
    e.feed(&[0i16; 64]).unwrap();
    assert!(e.finish_and_wait(LIMIT));
    assert_eq!(e.drain().unwrap(), vec![vec![7u8; 30], vec![8u8; 20]]);
    let _ = std::fs::remove_file(d);
}

#[test]
fn a_program_that_fails_is_reported_with_its_last_words_and_a_missing_one_with_the_add_on_message() {
    let p = script("fail", "echo 'libavcodec.so.60: cannot open shared object file' >&2; exit 3");
    let mut d = FfmpegDecoder::spawn(&p, &cfg()).unwrap();
    d.pipe.wait_end(LIMIT);
    let e = d.drain().unwrap_err();
    assert!(e.contains("audio add-on stopped") && e.contains("incomplete"), "{e}");
    // input to a program that is gone is an error too, with the same words
    let e = (0..100).find_map(|_| d.feed(&[0u8; 4096]).err()).unwrap_or_else(|| d.drain().unwrap_err());
    assert!(e.contains("audio add-on stopped"), "{e}");
    assert!(FfmpegDecoder::spawn("/nonexistent/ffmpeg", &cfg()).is_err());
    let b = FfmpegBackend::with_program(|| None);
    assert!(!b.available());
    assert_eq!(b.decoder(&cfg()).err().unwrap(), iyakki_core_local::devctl::a2dp_aac::NEEDS_ADDON);
    assert_eq!(b.encoder(&cfg()).err().unwrap(), iyakki_core_local::devctl::a2dp_aac::NEEDS_ADDON);
    let p = script("ok", "cat");
    let b = FfmpegBackend::with_program(move || Some(p.clone()));
    assert!(b.available());
    assert!(b.decoder(&cfg()).is_ok() && b.encoder(&cfg()).is_ok());
}

#[test]
fn a_program_that_never_reads_makes_the_queue_drop_input_instead_of_blocking() {
    let p = script("stuck", "sleep 30");
    let mut d = FfmpegDecoder::spawn(&p, &cfg()).unwrap();
    for _ in 0..500 {
        d.feed(&[0u8; 65536]).unwrap();
    }
    drop(d);
}

fn real_ffmpeg() -> Option<String> {
    crate::player::find_program("ffmpeg").filter(|p| crate::player::runs(p))
}

/// The real thing, where the host has an ffmpeg (skipped otherwise): a tone goes through the encoder to ADTS frames, the frames
/// through LATM and back, and through the decoder to sound of the same pitch.
#[test]
fn a_tone_through_the_real_encoder_and_decoder_comes_back_at_the_same_pitch() {
    let Some(ff) = real_ffmpeg() else { return };
    let c = AacConfig { rate: 44100, channels: 2, bit_rate: 128_000 };
    let n = 44100;
    let pcm: Vec<i16> = (0..n).flat_map(|i| std::iter::repeat_n(((i as f64 / 44100.0 * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16, 2)).collect();
    let mut enc = FfmpegEncoder::spawn(&ff, &c).unwrap();
    for ch in pcm.chunks(4096) {
        enc.feed(ch).unwrap();
    }
    assert!(enc.finish_and_wait(LIMIT));
    let frames = enc.drain().unwrap();
    assert!(frames.len() >= 40, "{}", frames.len());
    let mut dec = FfmpegDecoder::spawn(&ff, &c).unwrap();
    for f in &frames {
        // the way the Bluetooth path carries them: LATM in the packet, ADTS for the decoder
        let latm = iyakki_core_local::devctl::a2dp_aac::latm_wrap(f);
        let raw = iyakki_core_local::devctl::a2dp_aac::latm_unwrap(&latm).unwrap();
        dec.feed(&adts_wrap(&c, raw).unwrap()).unwrap();
    }
    assert!(dec.finish_and_wait(LIMIT));
    let out = dec.drain().unwrap();
    assert!(out.len() >= 2 * FRAME_SAMPLES * 40, "{}", out.len());
    let m: Vec<i16> = out.iter().step_by(2).copied().skip(4000).collect();
    let z = m.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    let hz = z as f64 / 2.0 / (m.len() as f64 / 44100.0);
    assert!((hz - 440.0).abs() < 15.0, "{hz}");
}

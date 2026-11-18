//! The MP3 decoder the radio and the MEDIA app use, against Apple's own decoder (`afconvert`, CoreAudio) on the two sample files of the images: the
//! sound is compared as numbers (length, loudness, and the best alignment of the two waves), not by ear. Skipped where there is no `afconvert`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn sample(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/samples").join(rel)
}

/// Every frame of the file through the decoder: interleaved 16-bit samples, the rate and the channel count.
fn ours(bytes: &[u8]) -> (Vec<i16>, u32, usize) {
    let skip = if bytes.starts_with(b"ID3") { 10 + ((bytes[6] as usize & 0x7f) << 21 | (bytes[7] as usize & 0x7f) << 14 | (bytes[8] as usize & 0x7f) << 7 | (bytes[9] as usize & 0x7f)) } else { 0 };
    let mut dec = nanomp3::Decoder::new();
    let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
    let (mut pos, mut out, mut rate, mut ch) = (skip, Vec::new(), 0u32, 0usize);
    while pos < bytes.len() {
        let (used, frame) = dec.decode(&bytes[pos..], &mut pcm);
        pos += used.max(1);
        if let Ok(f) = frame {
            rate = f.sample_rate;
            ch = f.channels as usize;
            out.extend_from_slice(&pcm[..f.samples_produced * ch]);
        }
    }
    (out, rate, ch)
}

/// A 16-bit PCM WAV: the samples, the rate and the channel count.
fn read_wav(d: &[u8]) -> Option<(Vec<i16>, u32, usize)> {
    if d.get(..4)? != b"RIFF" || d.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut rate, mut ch, mut bits) = (12, 0u32, 0usize, 0u16);
    while at + 8 <= d.len() {
        let id = &d[at..at + 4];
        let len = u32::from_le_bytes(d[at + 4..at + 8].try_into().ok()?) as usize;
        if id == b"fmt " {
            ch = u16::from_le_bytes(d.get(at + 10..at + 12)?.try_into().ok()?) as usize;
            rate = u32::from_le_bytes(d.get(at + 12..at + 16)?.try_into().ok()?);
            bits = u16::from_le_bytes(d.get(at + 22..at + 24)?.try_into().ok()?);
        } else if id == b"data" {
            if bits != 16 {
                return None;
            }
            let end = (at + 8 + len).min(d.len());
            return Some((d[at + 8..end].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(), rate, ch));
        }
        at += 8 + len + (len & 1);
    }
    None
}

fn rms(v: &[i16]) -> f64 {
    (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
}

/// The first channel of an interleaved wave.
fn first_channel(v: &[i16], ch: usize) -> Vec<f64> {
    v.chunks_exact(ch.max(1)).map(|c| c[0] as f64).collect()
}

/// The best normalised correlation of `a` against `b` over lags of -max..=max samples, on a window from the middle of the sound.
fn best_correlation(a: &[f64], b: &[f64], max: i64) -> (i64, f64) {
    let (start, win) = (a.len() / 2, 16_384usize.min(a.len() / 4));
    let mut best = (0, -2.0);
    for lag in -max..=max {
        let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for i in 0..win {
            let (x, j) = (a[start + i], (start + i) as i64 + lag);
            if j < 0 || j as usize >= b.len() {
                continue;
            }
            let y = b[j as usize];
            ab += x * y;
            aa += x * x;
            bb += y * y;
        }
        let c = if aa > 0.0 && bb > 0.0 { ab / (aa.sqrt() * bb.sqrt()) } else { 0.0 };
        if c > best.1 {
            best = (lag, c);
        }
    }
    best
}

#[test]
fn the_sample_mp3s_decode_to_the_same_sound_in_ours_and_in_apples_decoder() {
    if Command::new("afconvert").arg("-h").output().is_err() {
        eprintln!("skipped: no afconvert on this machine");
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-apple-dec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut compared = 0;
    for name in ["Music/IYAKKI Sample Melody.mp3", "Music/IYAKKI Sample Beats.mp3"] {
        let src = sample(name);
        let Ok(bytes) = std::fs::read(&src) else { continue };
        let wav = dir.join("out.wav");
        let ok = Command::new("afconvert").args(["-f", "WAVE", "-d", "LEI16"]).arg(&src).arg(&wav).output().is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipped: afconvert could not read {name}");
            continue;
        }
        let (theirs, trate, tch) = read_wav(&std::fs::read(&wav).unwrap()).expect("a 16-bit WAV from Apple's decoder");
        let (mine, mrate, mch) = ours(&bytes);
        assert_eq!((mrate, mch), (trate, tch), "{name}: the same rate and channels");
        // the same length to within the decoder delay and the last frame
        let (nm, nt) = (mine.len() / mch, theirs.len() / tch);
        assert!((nm as i64 - nt as i64).abs() <= 2 * 1152 + 1105, "{name}: {nm} samples against {nt}");
        // the same loudness to within a few percent
        let (rm, rt) = (rms(&mine), rms(&theirs));
        assert!(rm > 100.0 && (rm - rt).abs() / rt < 0.05, "{name}: rms {rm} against {rt}");
        // the same wave: after the alignment (delay, trimming) the two follow each other closely
        let (lag, corr) = best_correlation(&first_channel(&mine, mch), &first_channel(&theirs, tch), 2400);
        assert!(corr > 0.99, "{name}: best correlation {corr} at lag {lag}");
        compared += 1;
    }
    let _ = std::fs::remove_dir_all(&dir);
    if compared == 0 {
        eprintln!("skipped: no sample file could be compared here");
    }
}

//! The browser's MP3 decoder (`dist/webview/wpe-helper/mp3dec.c`, built around minimp3, CC0) and the radio's (`nanomp3`, a Rust port of
//! minimp3) must give the same sound for the same file. The C side is compiled here with the machine's `cc` into a tiny program that
//! decodes a file frame by frame (the same call the plugin makes) and prints the samples; the Rust side decodes the file with `nanomp3`.
//! Without a C compiler the test says so and passes: the QEMU boot test (`IYAKKI-SELFTEST`) and the Mac still cover the plugin itself.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const HARNESS: &str = r#"
#define MINIMP3_IMPLEMENTATION
#define MINIMP3_ONLY_MP3
#include "minimp3.h"
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 2;
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    unsigned char *b = malloc(n);
    if (fread(b, 1, n, f) != (size_t)n) return 3;
    mp3dec_t d; mp3dec_init(&d);
    mp3d_sample_t pcm[MINIMP3_MAX_SAMPLES_PER_FRAME];
    mp3dec_frame_info_t info;
    long pos = 0;
    while (pos < n) {
        int s = mp3dec_decode_frame(&d, b + pos, (int)(n - pos), pcm, &info);
        if (info.frame_bytes <= 0) break;
        pos += info.frame_bytes;
        if (s > 0) {
            printf("F %d %d %d\n", info.hz, info.channels, s);
            for (int i = 0; i < s * info.channels; i++) printf("%d\n", pcm[i]);
        }
    }
    return 0;
}
"#;

/// (rate, channels, samples) per frame and every sample, from the C decoder.
fn c_decode(fixture: &Path, dir: &Path) -> Option<(Vec<(u32, u32, usize)>, Vec<i16>)> {
    let src = dir.join("h.c");
    std::fs::write(&src, HARNESS).unwrap();
    let exe = dir.join("h");
    let inc = root().join("dist/webview/wpe-helper/third_party");
    let built = Command::new("cc").arg("-O1").arg("-I").arg(&inc).arg("-o").arg(&exe).arg(&src).status().ok()?;
    assert!(built.success(), "the C harness compiles");
    let out = Command::new(&exe).arg(fixture).output().unwrap();
    assert!(out.status.success(), "the C harness ran");
    let (mut frames, mut pcm) = (vec![], vec![]);
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        if let Some(h) = line.strip_prefix("F ") {
            let v: Vec<usize> = h.split(' ').map(|x| x.parse().unwrap()).collect();
            frames.push((v[0] as u32, v[1] as u32, v[2]));
        } else {
            pcm.push(line.parse().unwrap());
        }
    }
    Some((frames, pcm))
}

fn rust_decode(bytes: &[u8]) -> (Vec<(u32, u32, usize)>, Vec<i16>) {
    let mut dec = nanomp3::Decoder::new();
    let mut buf = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
    let (mut frames, mut pcm, mut pos) = (vec![], vec![], 0usize);
    while pos < bytes.len() {
        let (used, frame) = dec.decode(&bytes[pos..], &mut buf);
        pos += used.max(1);
        if let Ok(f) = frame {
            if f.samples_produced > 0 {
                frames.push((f.sample_rate, f.channels as u32, f.samples_produced));
                pcm.extend_from_slice(&buf[..f.samples_produced * f.channels as usize]);
            }
        }
    }
    (frames, pcm)
}

#[test]
fn the_browser_and_the_radio_decode_an_mp3_to_the_same_sound() {
    let fixture = root().join("crates/iyakki-agent/tests/fixtures/sine440-stereo-44k1.mp3");
    let dir = std::env::temp_dir().join(format!("iyakki-mp3match-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let c = c_decode(&fixture, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    let Some((c_frames, c_pcm)) = c else {
        eprintln!("no C compiler (cc) on this machine: the C half of the MP3 comparison was not run");
        return;
    };
    let (r_frames, r_pcm) = rust_decode(&std::fs::read(&fixture).unwrap());
    assert!(!c_frames.is_empty() && !c_pcm.is_empty(), "the C decoder produced sound");
    assert_eq!(c_frames.first().map(|f| (f.0, f.1)), Some((44100, 2)), "44.1 kHz stereo");
    assert_eq!(c_frames, r_frames, "the same frames (rate, channels, samples per frame)");
    assert_eq!(c_pcm.len(), r_pcm.len(), "the same number of samples");
    // the port may round differently in the last bit; more than one step apart would be a different decoder
    let worst = c_pcm.iter().zip(&r_pcm).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
    assert!(worst <= 1, "the decoders agree within one step, worst difference {worst}");
    let peak = c_pcm.iter().map(|v| v.saturating_abs()).max().unwrap_or(0);
    assert!(peak > 1500 && peak < 32700, "the sine is audible and not clipped, peak {peak}");
}

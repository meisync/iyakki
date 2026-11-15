//! The AAC stream detector and the ADTS frame reader against AAC made by the operating system's own encoder (`afconvert`), with `afinfo` as the
//! second reader of the same bytes (its packet count and sample rate). A test whose tool is not on the machine says so and passes without checking.

use super::*;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool} >/dev/null 2>&1")).status().is_ok_and(|s| s.success())
}

/// A 16-bit mono sine wave as a WAV file.
fn wav(rate: u32, seconds: u32) -> Vec<u8> {
    let n = (rate * seconds) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / rate as f32;
        let v = ((t * 440.0 * std::f32::consts::TAU).sin() * 12000.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    w
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-audiofmt-real-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `afinfo` says "audio packets: N" for an ADTS file.
fn afinfo_packets(file: &std::path::Path) -> Option<u64> {
    let o = Command::new("afinfo").arg(file).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout).into_owned();
    t.lines().find_map(|l| l.trim().strip_prefix("audio packets:").and_then(|v| v.trim().parse().ok()))
}

#[test]
fn an_adts_stream_made_by_the_real_encoder_is_taken_for_aac_and_walked_frame_by_frame_to_the_same_count_a_second_reader_finds() {
    if !have("afconvert") || !have("afinfo") {
        eprintln!("skipped: no afconvert or afinfo on this machine");
        return;
    }
    let d = tmp("adts");
    let mut done = 0;
    for (rate, bitrate) in [(44100u32, "64000"), (48000, "96000"), (22050, "32000")] {
        let src = d.join(format!("in{rate}.wav"));
        std::fs::write(&src, wav(rate, 3)).unwrap();
        let out = d.join(format!("out{rate}.aac"));
        let made = Command::new("afconvert").args(["-f", "adts", "-d", "aac", "-b", bitrate, "-c", "1"]).arg(&src).arg(&out).output();
        if !made.is_ok_and(|o| o.status.success()) || !out.is_file() {
            eprintln!("skipped {rate} Hz: afconvert could not make ADTS here");
            continue;
        }
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(sniff(&bytes), Some(Codec::Aac), "{rate} Hz: the first bytes are taken for AAC");
        // the same stream with an ID3 tag in front (a radio station may send one) is still AAC
        let mut tagged = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 4, 1, 2, 3, 4];
        tagged.extend_from_slice(&bytes);
        assert_eq!(sniff(&tagged), Some(Codec::Aac), "{rate} Hz: behind an ID3 tag");
        // walk every frame
        let (mut at, mut frames, mut first) = (0usize, 0u64, None);
        loop {
            match scan_adts(&bytes[at..], true) {
                Scan::Frame { skip, header } => {
                    first.get_or_insert(header);
                    at += skip + header.frame_len;
                    frames += 1;
                }
                _ => break,
            }
        }
        let h = first.expect("a first frame");
        assert_eq!((h.sample_rate, h.channels, h.object_type), (rate, 1, 2), "{rate} Hz: the header says what the encoder was asked");
        assert_eq!(at, bytes.len(), "{rate} Hz: the frames cover the whole file");
        if let Some(theirs) = afinfo_packets(&out) {
            assert_eq!(frames, theirs, "{rate} Hz: afinfo counts the same packets in the same bytes");
        } else {
            eprintln!("note: afinfo gave no packet count for {rate} Hz");
        }
        assert!(frames >= (rate as u64 * 3) / 1024, "{rate} Hz: about three seconds of 1024-sample frames, got {frames}");
        done += 1;
    }
    if done == 0 {
        eprintln!("skipped: afconvert made no ADTS file at all here");
    }
    let _ = std::fs::remove_dir_all(&d);
}

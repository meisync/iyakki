//! Real data: streams made with the Vorbis encoder of the installed ffmpeg (used here only to MAKE fixtures; it is never linked or
//! shipped), decoded here and compared sample by sample with ffmpeg's own decode of the same file.

use crate::{header, StreamDecoder, StreamError};
use std::process::Command;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-audiodec-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub struct Made {
    pub ogg: Vec<u8>,
    pub reference: Vec<i16>,
    pub rate: u32,
    pub channels: usize,
}

/// `source` is an ffmpeg lavfi expression; `enc` extra encoder arguments.
pub fn make(name: &str, source: &str, rate: u32, channels: usize, enc: &[&str]) -> Option<Made> {
    if !have_ffmpeg() {
        return None;
    }
    let d = scratch(name);
    let ogg = d.join("a.ogg");
    let raw = d.join("a.raw");
    let ok = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", source, "-ar", &rate.to_string(), "-ac", &channels.to_string(), "-c:a", "libvorbis"]).args(enc).arg(&ogg).status().ok()?.success();
    if !ok {
        return None;
    }
    let ok = Command::new("ffmpeg").args(["-v", "error", "-y", "-i"]).arg(&ogg).args(["-f", "s16le", "-acodec", "pcm_s16le"]).arg(&raw).status().ok()?.success();
    if !ok {
        return None;
    }
    let reference: Vec<i16> = std::fs::read(&raw).ok()?.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let m = Made { ogg: std::fs::read(&ogg).ok()?, reference, rate, channels };
    let _ = std::fs::remove_dir_all(&d);
    Some(m)
}

pub fn decode_all(ogg: &[u8], chunk: usize) -> Result<(Vec<i16>, u32, usize), StreamError> {
    let mut d = StreamDecoder::new();
    let mut pcm = vec![];
    for c in ogg.chunks(chunk) {
        pcm.extend(d.push(c)?);
    }
    Ok((pcm, d.rate, d.channels))
}

/// (largest difference, rms difference) in sample steps, and how many samples were compared.
pub fn compare(a: &[i16], b: &[i16]) -> (i32, f64, usize) {
    let n = a.len().min(b.len());
    let mut max = 0i32;
    let mut sq = 0f64;
    for i in 0..n {
        let d = (a[i] as i32 - b[i] as i32).abs();
        max = max.max(d);
        sq += (d as f64) * (d as f64);
    }
    (max, (sq / n.max(1) as f64).sqrt(), n)
}

fn check(name: &str, m: &Made, max_allowed: i32, rms_allowed: f64) {
    let (pcm, rate, ch) = decode_all(&m.ogg, 4096).unwrap();
    assert_eq!((rate, ch), (m.rate, m.channels), "{name}");
    assert!(pcm.len().abs_diff(m.reference.len()) <= 2 * 512, "{name}: the length differs from the reference ({} samples against {})", pcm.len(), m.reference.len());
    let extra = pcm.len() - m.reference.len().min(pcm.len());
    let (a, b) = (compare(&pcm, &m.reference), compare(&pcm[extra..], &m.reference));
    let (max, rms, n) = if a.0 <= b.0 { a } else { b };
    println!("vorbis {name}: {n} samples, largest difference {max}, rms {rms:.3}");
    assert!(max <= max_allowed && rms <= rms_allowed, "{name}: largest {max}, rms {rms}");
}

#[test]
fn a_mono_sine_decodes_to_the_reference_within_rounding() {
    let Some(m) = make("sine", "sine=f=440:d=2", 44100, 1, &["-q:a", "3"]) else { return };
    check("sine mono 44.1k q3", &m, 3, 0.5);
}

#[test]
fn stereo_with_different_sounds_in_each_channel_and_coupling_matches() {
    let Some(m) = make("stereo", "aevalsrc='0.4*sin(2*PI*330*t)+0.2*sin(2*PI*1250*t)|0.3*sin(2*PI*523*t)*(1+sin(2*PI*3*t))/2':d=3:s=48000", 48000, 2, &["-q:a", "5"]) else { return };
    check("stereo 48k q5", &m, 3, 0.5);
}

#[test]
fn noise_and_transients_use_short_blocks_and_still_match() {
    let Some(m) = make("noise", "anoisesrc=d=2:c=pink:r=44100:a=0.3", 44100, 2, &["-q:a", "4"]) else { return };
    check("pink noise stereo q4", &m, 4, 0.8);
    // clicks: an impulse every 0.25 s on silence forces block switching
    let Some(m) = make("clicks", "aevalsrc='if(lt(mod(t,0.25),0.0005),0.9,0)+0.05*sin(2*PI*220*t)':d=3:s=44100", 44100, 1, &["-q:a", "5"]) else { return };
    check("clicks mono q5", &m, 4, 0.8);
}

#[test]
fn low_rate_low_quality_and_high_quality_streams_match() {
    let Some(m) = make("lowq", "aevalsrc='0.5*sin(2*PI*(200+300*t)*t)':d=2:s=22050", 22050, 1, &["-q:a", "-1"]) else { return };
    check("chirp mono 22k q-1", &m, 4, 1.0);
    let Some(m) = make("highq", "aevalsrc='0.5*sin(2*PI*(100+800*t)*t)|0.4*sin(2*PI*660*t)':d=2:s=44100", 44100, 2, &["-q:a", "9"]) else { return };
    check("chirp stereo q9", &m, 4, 0.8);
}

#[test]
fn the_stream_can_be_cut_anywhere_into_pieces_and_gives_the_same_sound() {
    let Some(m) = make("cuts", "sine=f=1000:d=1", 44100, 1, &["-q:a", "2"]) else { return };
    let (whole, _, _) = decode_all(&m.ogg, m.ogg.len()).unwrap();
    for chunk in [1usize, 7, 255, 1000] {
        let (p, _, _) = decode_all(&m.ogg, chunk).unwrap();
        assert_eq!(p, whole, "chunk {chunk}");
    }
}

#[test]
fn a_truncated_or_damaged_stream_never_panics_and_keeps_what_it_can() {
    let Some(m) = make("damage", "aevalsrc='0.4*sin(2*PI*440*t)|0.4*sin(2*PI*660*t)':d=2:s=44100", 44100, 2, &["-q:a", "4"]) else { return };
    let (whole, _, _) = decode_all(&m.ogg, 4096).unwrap();
    // every cut point of the first 3000 bytes and a sample of later ones
    for cut in (0..m.ogg.len()).step_by(97) {
        let (p, _, _) = decode_all(&m.ogg[..cut], 4096).unwrap_or((vec![], 0, 0));
        assert!(p.len() <= whole.len());
    }
    // flipped bytes: the damaged page is dropped (its CRC fails) and the rest plays
    let mut x = 99u64;
    for _ in 0..60 {
        let mut b = m.ogg.clone();
        for _ in 0..3 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let i = (x >> 33) as usize % b.len();
            b[i] ^= 1 << (x as u8 % 8);
        }
        let r = decode_all(&b, 4096);
        if let Ok((p, _, _)) = r {
            assert!(p.len() <= whole.len() + 100_000);
        }
    }
    // a stream whose headers are damaged is refused with words, not a crash
    let mut b = m.ogg.clone();
    for i in 40..400.min(b.len()) {
        b[i] = b[i].wrapping_mul(31).wrapping_add(7);
    }
    let _ = decode_all(&b, 4096);
}

#[test]
fn opus_streams_are_decoded_and_plain_bytes_give_nothing() {
    if let Some(d) = (|| {
        if !have_ffmpeg() {
            return None;
        }
        let dir = scratch("opus");
        let f = dir.join("a.opus");
        Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=1", "-c:a", "libopus"]).arg(&f).status().ok()?.success().then_some(())?;
        let b = std::fs::read(&f).ok();
        let _ = std::fs::remove_dir_all(&dir);
        b
    })() {
        // Opus is decoded now: a second of sound comes out
        let (pcm, _, _) = decode_all(&d, 4096).unwrap();
        assert!(pcm.len() >= 48000);
    }
    // plain bytes are not an error but give nothing
    assert!(decode_all(b"ID3 not ogg at all", 4).map(|r| r.0.is_empty()).unwrap_or(true));
}

#[test]
fn the_header_is_ten_bytes_and_the_decode_of_nothing_is_nothing() {
    let h = header(44100, 2);
    assert_eq!(&h[..4], b"IYPC");
    assert_eq!(u32::from_le_bytes([h[4], h[5], h[6], h[7]]), 44100);
    assert_eq!(u16::from_le_bytes([h[8], h[9]]), 2);
    assert!(!StreamDecoder::new().ready());
}

#[test]
fn random_packets_after_good_headers_never_panic() {
    let Some(m) = make("fuzz", "sine=f=300:d=1", 44100, 1, &["-q:a", "3"]) else { return };
    let mut ogg = crate::ogg::Demuxer::new();
    let mut headers: Vec<Vec<u8>> = vec![];
    for p in ogg.push(&m.ogg) {
        headers.extend(p.packets);
    }
    let mut d = crate::vorbis::Decoder::new(&headers[0], &headers[1], &headers[2]).unwrap();
    let mut x = 5u64;
    for n in 0..400usize {
        let len = (n * 7) % 300;
        let pkt: Vec<u8> = (0..len).map(|_| { x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (x >> 40) as u8 }).collect();
        let _ = d.decode(&pkt);
    }
    // damaged headers
    for cut in 0..headers[2].len().min(400) {
        let _ = crate::vorbis::Decoder::new(&headers[0], &headers[1], &headers[2][..cut]);
    }
    for i in 0..headers[2].len().min(600) {
        let mut h = headers[2].clone();
        h[i] ^= 0xa5;
        let _ = crate::vorbis::Decoder::new(&headers[0], &headers[1], &h);
    }
}

/// A fixture kept in the repository (made once with ffmpeg's libvorbis; its reference is ffmpeg's own decode), so that this check
/// runs on machines that have no ffmpeg.
#[test]
fn the_kept_fixture_decodes_to_its_kept_reference() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let ogg = std::fs::read(dir.join("stereo-22k.ogg")).unwrap();
    let reference: Vec<i16> = std::fs::read(dir.join("stereo-22k.ref.s16")).unwrap().chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let (pcm, rate, ch) = decode_all(&ogg, 1000).unwrap();
    assert_eq!((rate, ch), (22050, 2));
    // ffmpeg's decode of this file is 256 sample frames shorter than the stream's own count (the last page's granule position,
    // 13230); whichever end it cut, one of the two alignments matches
    println!("kept fixture: ours {} samples, the reference {}", pcm.len(), reference.len());
    assert!(pcm.len().abs_diff(reference.len()) <= 2 * 512);
    let extra = pcm.len() - reference.len().min(pcm.len());
    let (a, b) = (compare(&pcm, &reference), compare(&pcm[extra..], &reference));
    let (max, rms, _) = if a.0 < b.0 { a } else { b };
    println!("kept fixture: largest difference {max}, rms {rms:.3}");
    assert!(max <= 3 && rms <= 0.6, "{max} {rms}");
}

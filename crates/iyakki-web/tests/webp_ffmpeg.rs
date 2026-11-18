//! WebP pictures made on the spot by a real encoder (ffmpeg's libwebp) and read back by our decoder: lossless files must give the
//! source pixels exactly (with alpha, and every frame of an animation with its delay), lossy files must stay close to the source.
//! The test is skipped (and says so) when `ffmpeg` with libwebp is not installed; it is a test of ours against a real tool.

use iyakki_web::image::decode;
use iyakki_web::webp::decode_animated;
use std::path::{Path, PathBuf};
use std::process::Command;

const W: usize = 64;
const H: usize = 48;
const FRAMES: usize = 6;

fn ffmpeg_has_libwebp() -> bool {
    Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("libwebp_anim")).unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-webp-ffmpeg-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn ff(args: &[&str]) {
    let o = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().expect("ffmpeg runs");
    assert!(o.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

/// The source frames as raw RGBA (what the encoder was given).
fn source(vf: &str, frames: usize) -> Vec<u8> {
    let raw = tmp(&format!("src-{}.rgba", vf.len()));
    let size = format!("size={W}x{H}:rate=10");
    let src = format!("testsrc2={size}");
    ff(&["-f", "lavfi", "-i", &src, "-frames:v", &frames.to_string(), "-vf", vf, "-f", "rawvideo", "-pix_fmt", "rgba", raw.to_str().unwrap()]);
    std::fs::read(raw).unwrap()
}

fn encode(vf: &str, frames: usize, codec: &str, extra: &[&str], out: &Path) {
    let size = format!("testsrc2=size={W}x{H}:rate=10");
    let frames = frames.to_string();
    let mut a: Vec<&str> = vec!["-f", "lavfi", "-i", &size, "-frames:v", &frames, "-vf", vf, "-c:v", codec];
    a.extend_from_slice(extra);
    a.push(out.to_str().unwrap());
    ff(&a);
}

#[test]
fn a_lossless_picture_with_alpha_from_a_real_encoder_comes_back_exactly() {
    if !ffmpeg_has_libwebp() {
        eprintln!("SKIPPED: ffmpeg with libwebp is not installed");
        return;
    }
    let vf = "format=rgba,colorchannelmixer=aa=0.5";
    let out = tmp("alpha.webp");
    encode(vf, 1, "libwebp", &["-lossless", "1"], &out);
    let want = source(vf, 1);
    let got = decode(&std::fs::read(&out).unwrap()).expect("our decoder reads it");
    assert_eq!((got.width as usize, got.height as usize), (W, H));
    assert_eq!(got.rgba.len(), want.len());
    assert!(got.rgba.chunks(4).any(|p| p[3] != 255), "the picture has transparency");
    assert_eq!(got.rgba, want, "lossless pixels differ from the source");
}

#[test]
fn a_lossy_picture_from_a_real_encoder_stays_close_to_the_source() {
    if !ffmpeg_has_libwebp() {
        eprintln!("SKIPPED: ffmpeg with libwebp is not installed");
        return;
    }
    let out = tmp("lossy.webp");
    encode("format=rgba", 1, "libwebp", &["-lossless", "0", "-quality", "90"], &out);
    let want = source("format=rgba", 1);
    let got = decode(&std::fs::read(&out).unwrap()).expect("our decoder reads it");
    assert_eq!((got.width as usize, got.height as usize), (W, H));
    let sum: u64 = got.rgba.iter().zip(&want).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64).sum();
    let mean = sum as f64 / want.len() as f64;
    assert!(mean < 6.0, "mean difference {mean} is too large for quality 90");
    assert!(mean > 0.0, "a lossy file that equals the source is not lossy");
}

#[test]
fn every_frame_of_an_animation_from_a_real_encoder_comes_back_with_its_delay() {
    if !ffmpeg_has_libwebp() {
        eprintln!("SKIPPED: ffmpeg with libwebp is not installed");
        return;
    }
    let out = tmp("anim.webp");
    encode("format=rgba", FRAMES, "libwebp_anim", &["-lossless", "1", "-loop", "0"], &out);
    let bytes = std::fs::read(&out).unwrap();
    let want = source("format=rgba", FRAMES);
    let anim = decode_animated(&bytes).expect("our decoder reads the animation");
    assert_eq!((anim.width as usize, anim.height as usize), (W, H));
    assert_eq!(anim.frames.len(), FRAMES, "all frames");
    for (i, f) in anim.frames.iter().enumerate() {
        assert_eq!(f.delay_ms, 100, "frame {i}: ten pictures a second");
        let w = &want[i * W * H * 4..(i + 1) * W * H * 4];
        assert_eq!(f.rgba, w, "frame {i} differs from the source after blending and disposal");
    }
    assert!(anim.frames[0].rgba != anim.frames[FRAMES - 1].rgba, "the frames really change");
    // The first frame is what a still reader draws.
    assert_eq!(decode(&bytes).unwrap().rgba, anim.frames[0].rgba);
}

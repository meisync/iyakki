//! The in-house picture decoders against a second decoder: pictures made by macOS's own encoders (`sips`) are decoded by `iyakki_web` and by `sips` itself
//! (back to PNG), and the two results are compared pixel by pixel (F-BROW-13). JPEG at three qualities and GIF, from a test picture with smooth gradients,
//! hard edges and flat areas. Skipped, with a printed line, where there is no `sips`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::qaimage::Image;

const W: u32 = 96;
const H: u32 = 64;

fn picture() -> Image {
    let mut px = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let (mut r, mut g, mut b) = ((x * 255 / W) as u8, (y * 255 / H) as u8, 128u8);
            if (20..50).contains(&x) && (10..40).contains(&y) {
                (r, g, b) = (230, 30, 30);
            }
            if (x as i32 - 70).pow(2) + (y as i32 - 40).pow(2) < 200 {
                (r, g, b) = (20, 40, 220);
            }
            if y > 52 {
                (r, g, b) = ((x * 2) as u8, 255 - (x * 2) as u8, 90);
            }
            px.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Image::new(W, H, px).unwrap()
}

fn have_sips() -> bool {
    Command::new("sips").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn sips(args: &[&str], i: &Path, o: &Path) -> bool {
    Command::new("sips").args(args).arg(i).arg("--out").arg(o).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-imgq-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// (mean absolute difference over the colour channels, share of pixels more than 12 apart)
fn compare(a: &[u8], b: &[u8]) -> (f32, f32) {
    let n = a.len() / 4;
    let mean = (0..n).map(|i| (0..3).map(|c| (a[i * 4 + c] as i32 - b[i * 4 + c] as i32).unsigned_abs() as f32).sum::<f32>()).sum::<f32>() / (n * 3) as f32;
    let far = (0..n).filter(|&i| (0..3).any(|c| (a[i * 4 + c] as i32 - b[i * 4 + c] as i32).abs() > 12)).count();
    (mean, far as f32 * 100.0 / n as f32)
}

#[test]
fn the_picture_decoders_agree_with_macos_on_pictures_its_encoders_made() {
    if !have_sips() {
        eprintln!("no sips here: this test did not run");
        return;
    }
    let dir = tmp();
    let src = dir.join("src.png");
    std::fs::write(&src, picture().to_png()).unwrap();
    let mut bad = Vec::new();
    for (name, fmt, extra, ext) in [("jpeg quality 20", "jpeg", Some("20"), "jpg"), ("jpeg quality 60", "jpeg", Some("60"), "jpg"), ("jpeg quality 95", "jpeg", Some("95"), "jpg"), ("gif", "gif", None, "gif")] {
        let made = dir.join(format!("m.{ext}"));
        let mut args = vec!["-s", "format", fmt];
        if let Some(q) = extra {
            args.extend(["-s", "formatOptions", q]);
        }
        assert!(sips(&args, &src, &made), "{name}: sips could not make the picture");
        let back = dir.join("back.png");
        assert!(sips(&["-s", "format", "png"], &made, &back), "{name}: sips could not read its own picture");
        let theirs = crate::qaimage::decode_png(&std::fs::read(&back).unwrap()).unwrap();
        let bytes = std::fs::read(&made).unwrap();
        let ours = match fmt {
            "jpeg" => iyakki_web::jpeg::decode(&bytes),
            _ => iyakki_web::gif::decode(&bytes),
        }
        .unwrap_or_else(|e| panic!("{name}: our decoder says {e}"));
        assert_eq!((ours.width, ours.height), (theirs.w, theirs.h), "{name}: size");
        let (mean, far) = compare(&ours.rgba, &theirs.rgba);
        eprintln!("decoders vs sips: {name}: mean difference {mean:.2} of 255, {far:.2} % of the pixels more than 12 apart");
        if mean > 1.5 || far > 2.5 {
            bad.push(format!("{name}: mean {mean:.2}, {far:.2} % far"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    let _ = std::fs::remove_dir_all(dir);
}

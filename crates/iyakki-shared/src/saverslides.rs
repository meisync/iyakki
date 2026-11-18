//! The photo slideshow of the screen saver (F-SCRN-05): which pictures, in what order, and what is on screen at second `t`.
//!
//! Pure rules, no drawing and no decoding (the shell does both): a folder listing that only takes plain picture files of a sane size, a
//! shuffled order from a seed, and the timeline (each picture shows for [`SHOW_SECS`] and fades into the next during the last
//! [`FADE_SECS`]) with a slow zoom and pan so the picture is never still.

use std::path::{Path, PathBuf};

pub const SHOW_SECS: f64 = 12.0;
pub const FADE_SECS: f64 = 2.0;
/// The most pictures taken from a folder, and the biggest file (a 40 megapixel JPEG is about this).
pub const MAX_PHOTOS: usize = 500;
pub const MAX_BYTES: u64 = 32 << 20;
/// How far the picture zooms in over one slide (0.08 = 8 percent).
pub const ZOOM: f64 = 0.08;

/// Is this a file name the slideshow reads?
pub fn is_photo_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !n.starts_with('.') && [".png", ".jpg", ".jpeg", ".webp", ".gif"].iter().any(|e| n.ends_with(e))
}

/// The pictures of `dir` (not its sub-folders), by name. Links, devices, empty and over-big files are left out.
pub fn list_photos(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_photo_name))
        .filter(|e| e.path().symlink_metadata().is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() <= MAX_BYTES))
        .map(|e| e.path())
        .collect();
    v.sort();
    v.truncate(MAX_PHOTOS);
    v
}

/// A shuffled copy of `items` (Fisher-Yates over a small generator), the same for the same seed.
pub fn shuffled<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
    let mut v = items.to_vec();
    let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
    for i in (1..v.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.swap(i, (s % (i as u64 + 1)) as usize);
    }
    v
}

/// What is on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Index of the picture now showing, and (while fading) the one coming in with how far it has come (0 to 1).
    pub cur: usize,
    pub next: Option<(usize, f32)>,
    /// Seconds into the current slide (it is also what the zoom follows).
    pub local: f64,
}

/// The frame at `t` seconds since the slideshow started, for `n` pictures. `None` for no pictures.
pub fn frame_at(t: f64, n: usize) -> Option<Frame> {
    if n == 0 {
        return None;
    }
    let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
    let slide = (t / SHOW_SECS).floor();
    let local = t - slide * SHOW_SECS;
    let cur = (slide as u64 % n as u64) as usize;
    let fade_from = SHOW_SECS - FADE_SECS;
    let next = (n > 1 && local >= fade_from).then(|| ((cur + 1) % n, ((local - fade_from) / FADE_SECS).clamp(0.0, 1.0) as f32));
    Some(Frame { cur, next, local })
}

/// Where a picture of `iw` x `ih` is drawn on a `sw` x `sh` screen at `local` seconds into its slide: it covers the screen (nothing
/// but the picture shows, the overflow is cut) and zooms in slowly while panning towards the middle of the overflow. The coming-in picture
/// is drawn at `local - SHOW_SECS` (negative), so it starts where the zoom started. Returns left, top, width, height.
pub fn cover_rect(iw: u32, ih: u32, sw: u32, sh: u32, local: f64) -> (f32, f32, f32, f32) {
    if iw == 0 || ih == 0 {
        return (0.0, 0.0, sw as f32, sh as f32);
    }
    let (iw, ih, sw, sh) = (iw as f64, ih as f64, sw as f64, sh as f64);
    let p = (local / (SHOW_SECS + FADE_SECS)).clamp(0.0, 1.0);
    let k = (sw / iw).max(sh / ih) * (1.0 + ZOOM * p);
    let (w, h) = (iw * k, ih * k);
    // the picture's centre drifts from slightly left/up of the screen's centre to slightly right/down, within the overflow
    let (ox, oy) = ((w - sw).max(0.0), (h - sh).max(0.0));
    let (x, y) = (-ox * (0.35 + 0.3 * p), -oy * (0.35 + 0.3 * p));
    (x as f32, y as f32, w as f32, h as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-slides-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn only_plain_picture_files_of_a_sane_size_are_listed_by_name() {
        let d = tmp("list");
        for (n, len) in [("b.JPG", 10usize), ("a.png", 10), ("c.txt", 10), (".hidden.png", 10), ("empty.png", 0), ("d.webp", 10)] {
            std::fs::write(d.join(n), vec![1u8; len]).unwrap();
        }
        std::fs::create_dir(d.join("sub.png")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", d.join("link.png")).unwrap();
        let names: Vec<String> = list_photos(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a.png", "b.JPG", "d.webp"]);
        assert!(list_photos(&d.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_shuffle_keeps_every_picture_once_and_repeats_for_a_seed() {
        let v: Vec<u32> = (0..50).collect();
        let a = shuffled(&v, 7);
        assert_eq!(a, shuffled(&v, 7));
        assert_ne!(a, shuffled(&v, 8));
        assert_ne!(a, v);
        let mut s = a.clone();
        s.sort();
        assert_eq!(s, v);
        assert!(shuffled::<u32>(&[], 1).is_empty() && shuffled(&[5], 1) == [5]);
    }

    #[test]
    fn the_timeline_shows_each_picture_fades_into_the_next_and_loops() {
        assert_eq!(frame_at(0.0, 0), None);
        let f = frame_at(0.0, 3).unwrap();
        assert_eq!((f.cur, f.next), (0, None));
        assert_eq!(frame_at(9.9, 3).unwrap().next, None);
        let mid = frame_at(11.0, 3).unwrap();
        assert_eq!((mid.cur, mid.next), (0, Some((1, 0.5))));
        let f = frame_at(12.0, 3).unwrap();
        assert_eq!((f.cur, f.next, f.local), (1, None, 0.0));
        assert_eq!(frame_at(12.0 * 3.0, 3).unwrap().cur, 0, "after the last it starts over");
        assert_eq!(frame_at(12.0 * 3.0 - 1.0, 3).unwrap().next.map(|n| n.0), Some(0), "the last fades into the first");
        assert_eq!(frame_at(11.5, 1).unwrap().next, None, "one picture never fades into itself");
        for bad in [f64::NAN, f64::INFINITY, -5.0] {
            assert_eq!(frame_at(bad, 2).unwrap().cur, 0);
        }
    }

    #[test]
    fn a_picture_always_covers_the_screen_and_moves_slowly() {
        for (iw, ih, sw, sh) in [(4000, 3000, 1920, 1080), (1000, 3000, 1920, 1080), (64, 48, 200, 150), (1920, 1080, 1920, 1080), (10, 10, 1, 1)] {
            let mut last = None;
            for step in 0..=14 {
                let (x, y, w, h) = cover_rect(iw, ih, sw, sh, step as f64);
                assert!(x <= 0.001 && y <= 0.001, "{iw}x{ih} on {sw}x{sh}: starts outside the screen");
                assert!(x + w >= sw as f32 - 0.01 && y + h >= sh as f32 - 0.01, "{iw}x{ih} on {sw}x{sh} at {step}: leaves a gap");
                if let Some((lw, _)) = last {
                    assert!(w >= lw, "only zooms in");
                }
                last = Some((w, h));
            }
        }
        let (_, _, w0, _) = cover_rect(1920, 1080, 1920, 1080, 0.0);
        let (_, _, w1, _) = cover_rect(1920, 1080, 1920, 1080, 14.0);
        assert!((w1 / w0 - 1.08).abs() < 0.001);
        assert_eq!(cover_rect(0, 0, 100, 50, 3.0), (0.0, 0.0, 100.0, 50.0));
    }
}

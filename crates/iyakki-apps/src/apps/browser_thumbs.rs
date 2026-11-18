//! Small pictures of the sites a person visits most (F-BROX-02), shown on the browser's start page. A picture is a snapshot of the top of
//! the page as it was drawn, shrunk to 192 x 120 and kept as a PNG of the person's account, one per site (the newest replaces the
//! older one), at most 60. They are the one place the browser keeps page content, so the rules are strict: only for a site visited at
//! least twice, only https, never a page with a password or file field, only when the switch is on (`thumbnails off` in the address
//! bar, or the assistant), and a picture goes with Shift+D (clear history), X (forget the site) and when the switch goes off.

use std::path::{Path, PathBuf};

use tiny_skia::Pixmap;

pub const W: u32 = 192;
pub const H: u32 = 120;
pub const MAX_FILES: usize = 60;
/// A site must have been visited this often before it gets a picture.
pub const MIN_VISITS: u32 = 2;
pub const SCHEME: &str = "iyakki-thumb:";

pub struct Thumbs {
    dir: PathBuf,
}

/// A name for a host that is safe as a file name.
pub fn key(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    let h = h.strip_prefix("www.").unwrap_or(&h);
    let h = h.split(':').next().unwrap_or("");
    let mut x: u64 = 0xcbf29ce484222325;
    for b in h.bytes() {
        x = (x ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{x:016x}")
}

impl Thumbs {
    pub fn new(dir: &Path) -> Thumbs {
        Thumbs { dir: dir.to_path_buf() }
    }

    fn path(&self, host: &str) -> PathBuf {
        self.dir.join(format!("{}.png", key(host)))
    }

    pub fn save(&self, host: &str, pm: &Pixmap) -> Result<(), String> {
        let bytes = pm.encode_png().map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 {
            return Err("the picture is larger than 64 KB".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!(".{}.tmp", key(host)));
        std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, self.path(host))).map_err(|e| e.to_string())?;
        self.prune();
        Ok(())
    }

    pub fn load(&self, host: &str) -> Option<Pixmap> {
        let bytes = std::fs::read(self.path(host)).ok()?;
        if bytes.len() > 64 * 1024 {
            return None;
        }
        let pm = Pixmap::decode_png(&bytes).ok()?;
        (pm.width() == W && pm.height() == H).then_some(pm)
    }

    pub fn has(&self, host: &str) -> bool {
        self.path(host).is_file()
    }

    pub fn forget(&self, host: &str) {
        let _ = std::fs::remove_file(self.path(host));
    }

    pub fn clear(&self) {
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    pub fn count(&self) -> usize {
        std::fs::read_dir(&self.dir).map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "png")).count()).unwrap_or(0)
    }

    /// Keep the newest [`MAX_FILES`] pictures.
    fn prune(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "png")).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
        files.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        for (_, p) in files.into_iter().skip(MAX_FILES) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The top of a drawn frame, shrunk to the picture size: `rect` (x, y, w, h in the frame's pixels) is cut to the picture's shape from
/// its top, centred, and each picture pixel is the average of the block it covers.
pub fn capture(frame: &Pixmap, rect: (f32, f32, f32, f32)) -> Option<Pixmap> {
    let (fw, fh) = (frame.width() as f32, frame.height() as f32);
    let (x, y, w, h) = rect;
    if !(w.is_finite() && h.is_finite() && x.is_finite() && y.is_finite()) || w < 32.0 || h < 20.0 {
        return None;
    }
    let ratio = W as f32 / H as f32;
    let cw = w.min(h * ratio);
    let ch = cw / ratio;
    let sx = (x + (w - cw) / 2.0).max(0.0).min(fw - 1.0);
    let sy = y.max(0.0).min(fh - 1.0);
    let (cw, ch) = (cw.min(fw - sx), ch.min(fh - sy));
    if cw < 8.0 || ch < 5.0 {
        return None;
    }
    let mut out = Pixmap::new(W, H)?;
    let src = frame.data();
    let stride = frame.width() as usize;
    for ty in 0..H {
        let y0 = (sy + ch * ty as f32 / H as f32) as usize;
        let y1 = ((sy + ch * (ty + 1) as f32 / H as f32) as usize).max(y0 + 1);
        for tx in 0..W {
            let x0 = (sx + cw * tx as f32 / W as f32) as usize;
            let x1 = ((sx + cw * (tx + 1) as f32 / W as f32) as usize).max(x0 + 1);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for yy in y0..y1.min(frame.height() as usize) {
                for xx in x0..x1.min(stride) {
                    let o = (yy * stride + xx) * 4;
                    for c in 0..4 {
                        acc[c] += src[o + c] as u32;
                    }
                    n += 1;
                }
            }
            if n > 0 {
                let o = ((ty * W + tx) * 4) as usize;
                for c in 0..4 {
                    out.data_mut()[o + c] = (acc[c] / n) as u8;
                }
            }
        }
    }
    Some(out)
}

/// May this page get a picture? `visits` is how often its site was visited.
pub fn eligible(https: bool, visits: u32, has_password_or_file_field: bool, switch_on: bool) -> bool {
    switch_on && https && visits >= MIN_VISITS && !has_password_or_file_field
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-thumbs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                let o = ((y * w + x) * 4) as usize;
                p.data_mut()[o..o + 4].copy_from_slice(&c);
            }
        }
        p
    }

    #[test]
    fn a_capture_has_the_picture_size_the_colours_of_the_top_of_the_frame_and_nothing_from_outside() {
        // red above y=100, blue below: only the top part of the frame is in the picture
        let f = frame(800, 600, |_, y| if y < 100 { [200, 0, 0, 255] } else { [0, 0, 200, 255] });
        let t = capture(&f, (0.0, 0.0, 800.0, 90.0)).unwrap();
        assert_eq!((t.width(), t.height()), (W, H));
        assert!(t.data().chunks_exact(4).all(|p| p == [200, 0, 0, 255]), "the red part only");
        // a wide view is cut to the shape of the picture and centred
        let f = frame(1000, 400, |x, _| if (300..700).contains(&x) { [0, 200, 0, 255] } else { [9, 9, 9, 255] });
        let t = capture(&f, (0.0, 0.0, 1000.0, 400.0)).unwrap();
        assert_eq!(t.data().chunks_exact(4).filter(|p| p[1] == 200).count(), (W * H * 5 / 8) as usize, "the middle 640 of the 1000 columns are cut out; 400 of them are green");
        // averaging, not picking one pixel
        let f = frame(384, 240, |x, y| if (x + y) % 2 == 0 { [0, 0, 0, 255] } else { [200, 200, 200, 255] });
        let t = capture(&f, (0.0, 0.0, 384.0, 240.0)).unwrap();
        assert!(t.data().chunks_exact(4).all(|p| (95..=105).contains(&p[0])), "grey");
        // nonsense rectangles
        assert!(capture(&f, (0.0, 0.0, 10.0, 10.0)).is_none() && capture(&f, (f32::NAN, 0.0, 300.0, 200.0)).is_none());
        let _ = capture(&f, (5000.0, 5000.0, 300.0, 200.0)); // outside the frame: no panic
        assert!(capture(&f, (0.0, 0.0, f32::INFINITY, 200.0)).is_none());
    }

    #[test]
    fn pictures_are_kept_per_site_replaced_pruned_and_forgotten() {
        let d = dir("keep");
        let t = Thumbs::new(&d);
        let pm = frame(W, H, |x, _| [x as u8, 0, 0, 255]);
        assert!(t.load("a.org").is_none() && !t.has("a.org"));
        t.save("A.org", &pm).unwrap();
        assert_eq!(t.load("www.a.org:443").unwrap().data(), pm.data(), "www, case and port are the same site");
        assert_eq!(key("a.org"), key("WWW.A.ORG:8080"));
        assert_ne!(key("a.org"), key("b.org"));
        assert_eq!(key("a.org").len(), 16);
        for i in 0..MAX_FILES + 10 {
            t.save(&format!("site{i}.org"), &pm).unwrap();
        }
        assert!(t.count() <= MAX_FILES, "{}", t.count());
        t.forget("site70.org");
        assert!(!t.has("site70.org"));
        t.clear();
        assert_eq!(t.count(), 0);
        // a damaged or wrongly sized file is not shown
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{}.png", key("bad.org"))), b"not a png").unwrap();
        assert!(t.load("bad.org").is_none());
        let small = frame(10, 10, |_, _| [1, 1, 1, 255]);
        small.save_png(d.join(format!("{}.png", key("small.org"))).as_path()).unwrap();
        assert!(t.load("small.org").is_none());
    }

    #[test]
    fn only_a_site_visited_twice_on_https_without_a_password_field_and_with_the_switch_on_gets_one() {
        assert!(eligible(true, 2, false, true));
        assert!(!eligible(true, 1, false, true) && !eligible(false, 9, false, true) && !eligible(true, 9, true, true) && !eligible(true, 9, false, false));
    }
}

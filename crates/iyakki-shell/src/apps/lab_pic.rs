//! LAB: pictures in and out. Reading what a person gives (a file, a camera frame, a screenshot, the clipboard), making it small enough
//! to send to a vision model (a bounded size and weight, and the size sent is reported), and the thumbnails the tile draws.
//! Everything stays in memory.

use std::sync::Arc;

use iyakki_agent::lab::{MAX_INPUT_FILE_BYTES, VISION_MAX_BYTES, VISION_MAX_SIDE};
use iyakki_web::image::{self, Kind};
use tiny_skia::Pixmap;

use super::chatpic::resize_box;

/// The most pixels a picture may have before it is even decoded (a decompression bomb guard); 64 megapixels.
const MAX_PIXELS: u64 = 64_000_000;

/// A picture ready to send: its PNG, its size, and what became of the original.
#[derive(Clone)]
pub struct Prepared {
    pub png: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// What came in: its size in pixels and bytes.
    pub from: (u32, u32),
    pub from_bytes: u64,
    /// Where it came from (`file clip.jpg`, `the camera`, `the screen`, `the clipboard`).
    pub source: String,
    /// What the tile draws.
    pub thumb: Arc<Pixmap>,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Prepared {{ {}x{}, {} bytes, from {}x{} }}", self.w, self.h, self.png.len(), self.from.0, self.from.1)
    }
}

impl Prepared {
    pub fn scaled(&self) -> bool {
        (self.w, self.h) != self.from
    }

    /// `Sending 1024x768, 412 KB (was 4032x3024, 3.1 MB: made smaller)`.
    pub fn sent_line(&self) -> String {
        use iyakki_agent::lab::bytes_text;
        let base = format!("Sending {}x{}, {}", self.w, self.h, bytes_text(self.png.len() as u64));
        if self.scaled() || self.png.len() as u64 > self.from_bytes {
            format!("{base} (was {}x{}, {}{})", self.from.0, self.from.1, bytes_text(self.from_bytes), if self.scaled() { ": made smaller to be sent" } else { "" })
        } else {
            base
        }
    }
}

/// The pixel size from a PNG or JPEG header, without decoding it.
pub fn header_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() > 24 && b.starts_with(&[0x89, b'P', b'N', b'G']) && &b[12..16] == b"IHDR" {
        return Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)));
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if m == 0xFF {
                i += 1;
                continue;
            }
            if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                return Some((u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32, u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32));
            }
            if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
                i += 2;
                continue;
            }
            i += 2 + u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        }
    }
    None
}

fn fit_side(w: u32, h: u32, max: u32) -> (u32, u32) {
    let k = (max as f32 / w.max(h) as f32).min(1.0);
    (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1))
}

/// A picture file's bytes (PNG, JPEG, GIF, WebP or SVG) made ready to send: decoded, shrunk so that neither side is over
/// [`VISION_MAX_SIDE`] and the PNG is no heavier than [`VISION_MAX_BYTES`].
pub fn prepare(bytes: &[u8], source: &str) -> Result<Prepared, String> {
    if bytes.len() as u64 > MAX_INPUT_FILE_BYTES {
        return Err(format!("That picture is too large to open ({} MB; the limit is {} MB).", bytes.len() >> 20, MAX_INPUT_FILE_BYTES >> 20));
    }
    if image::sniff(bytes) == Kind::Unknown {
        return Err("That is not a picture IYAKKI can read (PNG, JPEG, GIF and WebP are).".into());
    }
    let header = header_size(bytes);
    if let Some((w, h)) = header {
        if w as u64 * h as u64 > MAX_PIXELS {
            return Err(format!("That picture is too large to open ({w}x{h} pixels; the limit is 64 megapixels)."));
        }
    }
    let pm = super::browser_images::to_pixmap(bytes)?;
    prepare_pixmap(pm, header, bytes.len() as u64, source)
}

/// The same for a picture already decoded (a camera frame, the screen, the clipboard): `from_bytes` is what it weighed.
pub fn prepare_pixmap(pm: Pixmap, header: Option<(u32, u32)>, from_bytes: u64, source: &str) -> Result<Prepared, String> {
    let from = header.unwrap_or((pm.width(), pm.height()));
    let (mut w, mut h) = fit_side(pm.width(), pm.height(), VISION_MAX_SIDE);
    let mut png = Vec::new();
    // the first size, then smaller while the PNG is too heavy (a photograph compresses badly as PNG)
    for _ in 0..6 {
        let small = if (w, h) == (pm.width(), pm.height()) { pm.clone() } else { resize_box(&pm, w, h).ok_or("The picture has no size.")? };
        png = small.encode_png().map_err(|e| format!("The picture could not be prepared ({e})."))?;
        if png.len() <= VISION_MAX_BYTES {
            let thumb = Arc::new(small);
            return Ok(Prepared { png, w, h, from, from_bytes, source: source.to_string(), thumb });
        }
        (w, h) = ((w as f32 * 0.75) as u32, (h as f32 * 0.75) as u32);
        if w < 32 || h < 32 {
            break;
        }
    }
    Err(format!("The picture could not be made small enough to send ({} MB after shrinking).", png.len() >> 20))
}

/// A picture from RGBA bytes (an opaque camera frame, the screen, the clipboard).
pub fn prepare_rgba(w: u32, h: u32, rgba: Vec<u8>, source: &str) -> Result<Prepared, String> {
    let n = w as usize * h as usize * 4;
    if w == 0 || h == 0 || rgba.len() != n {
        return Err("The picture was empty or damaged.".into());
    }
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err("The picture is too large (over 64 megapixels).".into());
    }
    let pm = crate::camera::pixels::pixmap(w, h, rgba).ok_or("The picture has no size.")?;
    prepare_pixmap(pm, Some((w, h)), n as u64, source)
}

/// A small copy of a pixmap for a list.
pub fn thumb(pm: &Pixmap, max: u32) -> Arc<Pixmap> {
    let (w, h) = fit_side(pm.width(), pm.height(), max);
    if (w, h) == (pm.width(), pm.height()) {
        return Arc::new(pm.clone());
    }
    Arc::new(resize_box(pm, w, h).unwrap_or_else(|| pm.clone()))
}

/// Draw `pm` inside `r`, centred, as large as fits with the proportions kept. Returns the rectangle it took.
pub fn draw_fit(g: &mut crate::gfx::Gfx, pm: &Pixmap, r: crate::ui::R) -> crate::ui::R {
    let k = (r.w / pm.width() as f32).min(r.h / pm.height() as f32).max(0.01);
    let (dw, dh) = (pm.width() as f32 * k, pm.height() as f32 * k);
    let (dx, dy) = (r.x + (r.w - dw) / 2.0, r.y + (r.h - dh) / 2.0);
    let mut paint = tiny_skia::PixmapPaint::default();
    paint.quality = tiny_skia::FilterQuality::Bilinear;
    g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, tiny_skia::Transform::from_translate(dx, dy).pre_scale(k, k), None);
    crate::ui::R::new(dx, dy, dw, dh)
}

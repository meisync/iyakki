//! LAB: pictures in and out. Reading what a person gives (a file, a camera frame, a screenshot, the clipboard), making it small enough
//! to send to a vision model (a bounded size and weight, and the size sent is reported), and the thumbnails the tile draws.
//! Everything stays in memory.

use std::sync::Arc;

use iyakki_lab_model::{MAX_INPUT_FILE_BYTES, VISION_MAX_BYTES, VISION_MAX_SIDE};
use tiny_skia::Pixmap;

use iyakki_uikit::tilekit::resize_box;

/// What the tile needs to read a picture file (the shell's decoders: PNG, JPEG, GIF, WebP, SVG; a fake in the tests).
pub trait PictureCodec: Send + Sync {
    /// Is this a picture file the decoders know (by its first bytes)?
    fn is_picture(&self, bytes: &[u8]) -> bool;
    /// The picture as a pixmap (premultiplied, as the renderer wants it).
    fn decode(&self, bytes: &[u8]) -> Result<Pixmap, String>;
}

/// An opaque RGBA picture as a pixmap (straight and premultiplied are the same when opaque).
fn opaque_pixmap(w: u32, h: u32, mut rgba: Vec<u8>) -> Option<Pixmap> {
    rgba.resize(w as usize * h as usize * 4, 255);
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)
}

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
        use iyakki_lab_model::bytes_text;
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
pub fn prepare(codec: &dyn PictureCodec, bytes: &[u8], source: &str) -> Result<Prepared, String> {
    if bytes.len() as u64 > MAX_INPUT_FILE_BYTES {
        return Err(format!("That picture is too large to open ({} MB; the limit is {} MB).", bytes.len() >> 20, MAX_INPUT_FILE_BYTES >> 20));
    }
    if !codec.is_picture(bytes) {
        return Err("That is not a picture IYAKKI can read (PNG, JPEG, GIF and WebP are).".into());
    }
    let header = header_size(bytes);
    if let Some((w, h)) = header {
        if w as u64 * h as u64 > MAX_PIXELS {
            return Err(format!("That picture is too large to open ({w}x{h} pixels; the limit is 64 megapixels)."));
        }
    }
    let pm = codec.decode(bytes)?;
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
    let pm = opaque_pixmap(w, h, rgba).ok_or("The picture has no size.")?;
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

pub use iyakki_uikit::tilekit::draw_fit;

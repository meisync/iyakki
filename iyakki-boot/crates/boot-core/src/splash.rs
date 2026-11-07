// SPDX-License-Identifier: MIT OR Apache-2.0
//! Splash picture: a small, bounds-checked decoder for uncompressed BMP files, nearest-neighbour scaling to fit a screen, and
//! centring. No I/O and no drawing; the front end feeds it the file bytes and draws the result.
//!
//! Accepted file: BMP with a `BITMAPINFOHEADER` (40 bytes) or a V4/V5 header (108/124 bytes), 24 or 32 bits per pixel, no compression
//! (`BI_RGB`), or 32 bits with `BI_BITFIELDS` when the masks are the usual B8G8R8 (alpha is ignored either way). Bottom-up and
//! top-down (negative height) rows are both read. Palettes (8 bits or fewer), RLE, JPEG/PNG inside BMP and OS/2 headers are refused.

use alloc::vec::Vec;

/// Largest file accepted, in bytes.
pub const MAX_FILE: usize = 16 * 1024 * 1024 + 1024;
/// Largest width or height, in pixels.
pub const MAX_DIM: u32 = 4096;
/// Most pixels in one picture.
pub const MAX_PIXELS: u64 = 2048 * 2048;

/// Why a file was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Longer than `MAX_FILE`.
    TooLarge,
    /// The headers or the pixel rows run past the end of the file.
    Truncated,
    /// Does not start with `BM`.
    NotBmp,
    /// A valid BMP of a kind this decoder does not read; the text says which.
    Unsupported(&'static str),
    /// Zero, negative or too large width or height.
    BadSize,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::TooLarge => write!(f, "file larger than {MAX_FILE} bytes"),
            Error::Truncated => write!(f, "file is cut short"),
            Error::NotBmp => write!(f, "not a BMP file"),
            Error::Unsupported(w) => write!(f, "unsupported BMP: {w}"),
            Error::BadSize => write!(f, "picture size out of range"),
        }
    }
}

/// A picture: `px` holds `w * h` pixels row by row from the top, each `0x00RRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    /// Pixels.
    pub px: Vec<u32>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

/// Decode a BMP file. Never panics, whatever the bytes.
pub fn decode_bmp(d: &[u8]) -> Result<Image, Error> {
    if d.len() > MAX_FILE {
        return Err(Error::TooLarge);
    }
    if d.len() < 2 || &d[..2] != b"BM" {
        return Err(Error::NotBmp);
    }
    let t = Error::Truncated;
    let offset = u32_at(d, 10).ok_or(t)? as usize;
    let hsize = u32_at(d, 14).ok_or(t)? as usize;
    if hsize < 40 {
        return Err(Error::Unsupported("old OS/2 header"));
    }
    if !matches!(hsize, 40 | 52 | 56 | 64 | 108 | 124) {
        return Err(Error::Unsupported("unknown header size"));
    }
    if d.len() < 14 + hsize {
        return Err(t);
    }
    let width = u32_at(d, 18).ok_or(t)? as i32;
    let height = u32_at(d, 22).ok_or(t)? as i32;
    if u16_at(d, 26).ok_or(t)? != 1 {
        return Err(Error::BadSize);
    }
    let bpp = u16_at(d, 28).ok_or(t)?;
    let compression = u32_at(d, 30).ok_or(t)?;
    if bpp != 24 && bpp != 32 {
        return Err(Error::Unsupported("only 24 and 32 bits per pixel"));
    }
    match compression {
        0 => {}
        3 if bpp == 32 => {
            // BI_BITFIELDS: the three masks follow the 40-byte part of the header; accept only the plain B8G8R8 layout
            let masks = (u32_at(d, 54), u32_at(d, 58), u32_at(d, 62));
            if masks != (Some(0x00ff_0000), Some(0x0000_ff00), Some(0x0000_00ff)) {
                return Err(Error::Unsupported("bit masks other than B8G8R8"));
            }
        }
        _ => return Err(Error::Unsupported("compressed")),
    }
    if width <= 0 || height == 0 || height == i32::MIN {
        return Err(Error::BadSize);
    }
    let (w, h, top_down) = (width as u32, height.unsigned_abs(), height < 0);
    if w > MAX_DIM || h > MAX_DIM || (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(Error::BadSize);
    }
    if offset < 14 + hsize {
        return Err(Error::Unsupported("pixel data inside the header"));
    }
    let bytes_pp = (bpp / 8) as u64;
    let stride = ((w as u64 * bpp as u64 + 31) / 32) * 4;
    let end = (offset as u64).checked_add(stride * h as u64).ok_or(t)?;
    if end > d.len() as u64 {
        return Err(t);
    }
    let mut px = Vec::new();
    px.try_reserve_exact((w as usize) * (h as usize)).map_err(|_| Error::TooLarge)?;
    for y in 0..h as u64 {
        let src_row = if top_down { y } else { h as u64 - 1 - y };
        let base = (offset as u64 + src_row * stride) as usize;
        for x in 0..w as u64 {
            let o = base + (x * bytes_pp) as usize;
            let (b, g, r) = (d[o] as u32, d[o + 1] as u32, d[o + 2] as u32);
            px.push(r << 16 | g << 8 | b);
        }
    }
    Ok(Image { w, h, px })
}

/// Size of a `w` x `h` picture after `Image::fit(max_w, max_h)` (never enlarged, aspect ratio kept).
pub fn fit_dims(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if max_w == 0 || max_h == 0 {
        return (0, 0);
    }
    if w <= max_w && h <= max_h {
        return (w, h);
    }
    let (w, h) = (w as u64, h as u64);
    if w * max_h as u64 >= h * max_w as u64 {
        (max_w, (h * max_w as u64 / w).max(1) as u32)
    } else {
        ((w * max_h as u64 / h).max(1) as u32, max_h)
    }
}

impl Image {
    /// The part of the picture that differs from the background colour `bg`, plus `pad` pixels on each side (a logo drawn in
    /// the middle of a big empty picture is cut to the logo, so the menu can sit close below it). A picture with nothing but
    /// background is returned whole.
    pub fn crop_to_content(&self, bg: [u8; 3], pad: u32) -> Image {
        let far = |p: u32| {
            let d = |sh: u32, c: u8| ((p >> sh) as u8).abs_diff(c);
            d(16, bg[0]).max(d(8, bg[1])).max(d(0, bg[2])) > 8
        };
        let (mut x0, mut y0, mut x1, mut y1) = (self.w, self.h, 0u32, 0u32);
        for y in 0..self.h {
            for x in 0..self.w {
                if far(self.px[(y * self.w + x) as usize]) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return self.clone();
        }
        let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
        let (x1, y1) = ((x1 + pad).min(self.w), (y1 + pad).min(self.h));
        let mut px = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
        for y in y0..y1 {
            px.extend_from_slice(&self.px[(y * self.w + x0) as usize..(y * self.w + x1) as usize]);
        }
        Image { w: x1 - x0, h: y1 - y0, px }
    }

    /// The picture scaled down (nearest neighbour, aspect ratio kept) so it fits `max_w` x `max_h`. A picture that already fits is
    /// returned unchanged: it is never enlarged.
    pub fn fit(self, max_w: u32, max_h: u32) -> Image {
        if max_w == 0 || max_h == 0 {
            return Image { w: 0, h: 0, px: Vec::new() };
        }
        if self.w <= max_w && self.h <= max_h {
            return self;
        }
        let (w, h) = (self.w as u64, self.h as u64);
        let (nw, nh) = fit_dims(self.w, self.h, max_w, max_h);
        let (nw, nh) = (nw as u64, nh as u64);
        let mut px = Vec::with_capacity((nw * nh) as usize);
        for y in 0..nh {
            let sy = y * h / nh;
            for x in 0..nw {
                px.push(self.px[(sy * w + x * w / nw) as usize]);
            }
        }
        Image { w: nw as u32, h: nh as u32, px }
    }

    /// The largest area a picture may use on a `screen_w` x `screen_h` screen: the screen less a margin of 1/16 of the shorter
    /// side on every edge, so an oversized picture never touches the edges.
    pub fn margin_box(screen_w: u32, screen_h: u32) -> (u32, u32) {
        let m = screen_w.min(screen_h) / 16;
        (screen_w.saturating_sub(2 * m), screen_h.saturating_sub(2 * m))
    }

    /// Top-left corner that centres this picture on a `screen_w` x `screen_h` screen.
    pub fn centred(&self, screen_w: u32, screen_h: u32) -> (u32, u32) {
        (screen_w.saturating_sub(self.w) / 2, screen_h.saturating_sub(self.h) / 2)
    }
}

/// Parse `#RRGGBB` (exactly that form).
pub fn parse_colour(s: &str) -> Option<[u8; 3]> {
    let b = s.as_bytes();
    if b.len() != 7 || b[0] != b'#' {
        return None;
    }
    let hex = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
    let mut out = [0u8; 3];
    for (i, o) in out.iter_mut().enumerate() {
        *o = hex(b[1 + 2 * i])? << 4 | hex(b[2 + 2 * i])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 24-bit bottom-up BMP: pixel (x, y) from the top is `f(x, y)` as (r, g, b).
    fn bmp24(w: u32, h: u32, f: impl Fn(u32, u32) -> (u8, u8, u8)) -> Vec<u8> {
        let stride = ((w * 24 + 31) / 32 * 4) as usize;
        let mut d = Vec::new();
        d.extend_from_slice(b"BM");
        d.extend_from_slice(&((54 + stride * h as usize) as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&54u32.to_le_bytes());
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&24u16.to_le_bytes());
        d.extend_from_slice(&[0; 24]);
        for y in (0..h).rev() {
            let mut row = Vec::new();
            for x in 0..w {
                let (r, g, b) = f(x, y);
                row.extend_from_slice(&[b, g, r]);
            }
            row.resize(stride, 0);
            d.extend_from_slice(&row);
        }
        d
    }

    fn set32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn decodes_24_bit_bottom_up() {
        let d = bmp24(3, 2, |x, y| (x as u8 * 10, y as u8 * 20, 7));
        let i = decode_bmp(&d).unwrap();
        assert_eq!((i.w, i.h), (3, 2));
        assert_eq!(i.px[0], 0x00_00_00_07);
        assert_eq!(i.px[2], 0x00_14_00_07);
        assert_eq!(i.px[3], 0x00_00_14_07);
        assert_eq!(i.px[5], 0x00_14_14_07);
    }

    #[test]
    fn decodes_top_down_and_32_bit() {
        let mut d = bmp24(2, 2, |x, y| (x as u8, y as u8, 0));
        set32(&mut d, 22, (-2i32) as u32); // top-down: rows are stored in the other order
        let i = decode_bmp(&d).unwrap();
        assert_eq!(i.px[0], 0x00_00_01_00); // the stored first row is now the bottom row of the old picture: y = 1
        // 32 bit, alpha ignored
        let mut e = Vec::new();
        e.extend_from_slice(&d[..54]);
        set32(&mut e, 10, 54);
        e[28] = 32;
        set32(&mut e, 22, 1);
        set32(&mut e, 18, 1);
        e.extend_from_slice(&[1, 2, 3, 0]);
        let i = decode_bmp(&e).unwrap();
        assert_eq!(i.px, [0x00_03_02_01]);
        // BI_BITFIELDS with standard masks
        let mut f = e.clone();
        set32(&mut f, 30, 3);
        f.splice(54..54, [0u8; 0]);
        let mut g = f[..54].to_vec();
        g.extend_from_slice(&0x00ff0000u32.to_le_bytes());
        g.extend_from_slice(&0x0000ff00u32.to_le_bytes());
        g.extend_from_slice(&0x000000ffu32.to_le_bytes());
        set32(&mut g, 10, 66);
        g.extend_from_slice(&[1, 2, 3, 0xff]);
        assert_eq!(decode_bmp(&g).unwrap().px, [0x00_03_02_01]);
        set32(&mut g, 54, 0xff00_0000); // other masks refused
        assert!(matches!(decode_bmp(&g), Err(Error::Unsupported(_))));
    }

    #[test]
    fn refuses_unsupported_kinds() {
        let good = bmp24(2, 2, |_, _| (1, 2, 3));
        let mut d = good.clone();
        d[28] = 8;
        assert!(matches!(decode_bmp(&d), Err(Error::Unsupported(_))));
        let mut d = good.clone();
        set32(&mut d, 30, 1); // RLE8
        assert!(matches!(decode_bmp(&d), Err(Error::Unsupported(_))));
        let mut d = good.clone();
        set32(&mut d, 14, 12); // OS/2 header
        assert!(matches!(decode_bmp(&d), Err(Error::Unsupported(_))));
        let mut d = good.clone();
        set32(&mut d, 14, 41);
        assert!(matches!(decode_bmp(&d), Err(Error::Unsupported(_))));
        assert_eq!(decode_bmp(b"PNG....."), Err(Error::NotBmp));
        assert_eq!(decode_bmp(b""), Err(Error::NotBmp));
        assert_eq!(decode_bmp(b"B"), Err(Error::NotBmp));
        assert_eq!(decode_bmp(b"BM"), Err(Error::Truncated));
    }

    #[test]
    fn refuses_bad_sizes() {
        let good = bmp24(2, 2, |_, _| (1, 2, 3));
        for (w, h) in [(0u32, 2u32), (2, 0), (0x8000_0000, 2), (2, 0x8000_0000), (4097, 1), (1, 4097), (4096, 4096), (u32::MAX, u32::MAX)] {
            let mut d = good.clone();
            set32(&mut d, 18, w);
            set32(&mut d, 22, h);
            assert!(decode_bmp(&d).is_err(), "{w}x{h}");
        }
        let mut d = good.clone();
        d[26] = 2; // planes
        assert_eq!(decode_bmp(&d), Err(Error::BadSize));
        let mut d = good.clone();
        set32(&mut d, 10, 20); // pixel data would start inside the header
        assert!(matches!(decode_bmp(&d), Err(Error::Unsupported(_))));
        let mut d = good.clone();
        set32(&mut d, 10, u32::MAX); // offset far away
        assert_eq!(decode_bmp(&d), Err(Error::Truncated));
        let big = alloc::vec![0u8; MAX_FILE + 1];
        assert_eq!(decode_bmp(&big), Err(Error::TooLarge));
    }

    #[test]
    fn every_truncation_is_an_error() {
        let d = bmp24(5, 4, |x, y| (x as u8, y as u8, 9));
        for n in 0..d.len() {
            assert!(decode_bmp(&d[..n]).is_err(), "length {n}");
        }
        assert!(decode_bmp(&d).is_ok());
    }

    #[test]
    fn header_bytes_changed_never_panic() {
        let d = bmp24(5, 4, |x, y| (x as u8, y as u8, 9));
        for i in 0..70 {
            for v in [0x00u8, 0x01, 0x7f, 0x80, 0xff] {
                let mut e = d.clone();
                e[i] = v;
                if let Ok(img) = decode_bmp(&e) {
                    assert_eq!(img.px.len() as u64, img.w as u64 * img.h as u64);
                    assert!(img.w <= MAX_DIM && img.h <= MAX_DIM);
                }
            }
        }
    }

    #[test]
    fn random_mutations_never_panic() {
        let base = bmp24(7, 5, |x, y| (x as u8 * 3, y as u8 * 5, 1));
        let mut s = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..20_000 {
            let mut e = base.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % e.len() as u64) as usize;
                e[i] = next() as u8;
            }
            if next() % 4 == 0 {
                let n = (next() % e.len() as u64) as usize;
                e.truncate(n);
            }
            if let Ok(img) = decode_bmp(&e) {
                assert_eq!(img.px.len() as u64, img.w as u64 * img.h as u64);
            }
        }
        // pure noise after a valid signature
        for _ in 0..2000 {
            let n = (next() % 200) as usize;
            let mut e: Vec<u8> = (0..n).map(|_| next() as u8).collect();
            if e.len() >= 2 {
                e[0] = b'B';
                e[1] = b'M';
            }
            let _ = decode_bmp(&e);
        }
    }

    fn solid(w: u32, h: u32) -> Image {
        Image { w, h, px: (0..w * h).collect() }
    }

    #[test]
    fn fit_scales_down_only() {
        let i = solid(100, 50).fit(200, 200);
        assert_eq!((i.w, i.h), (100, 50)); // not enlarged
        let i = solid(100, 50).fit(50, 50);
        assert_eq!((i.w, i.h), (50, 25));
        assert_eq!(i.px.len(), 50 * 25);
        let i = solid(40, 100).fit(1000, 10);
        assert_eq!((i.w, i.h), (4, 10));
        let i = solid(10000 / 5, 1).fit(7, 7);
        assert_eq!((i.w, i.h), (7, 1)); // never zero high
        let i = solid(4, 4).fit(0, 5);
        assert_eq!((i.w, i.h, i.px.len()), (0, 0, 0));
        // nearest neighbour keeps the corners
        let i = solid(8, 8).fit(2, 2);
        assert_eq!(i.px, [0, 4, 32, 36]);
    }

    #[test]
    fn oversized_pictures_keep_a_margin() {
        assert_eq!(Image::margin_box(1920, 1080), (1920 - 134, 1080 - 134));
        let (mw, mh) = Image::margin_box(800, 600);
        let i = solid(4000, 3000).fit(mw, mh);
        assert!(i.w < 800 && i.h < 600);
        assert_eq!(i.centred(800, 600).0, (800 - i.w) / 2);
        assert_eq!(Image::margin_box(10, 10), (10, 10));
    }

    #[test]
    fn centring() {
        let i = solid(100, 50);
        assert_eq!(i.centred(800, 600), (350, 275));
        assert_eq!(i.centred(100, 50), (0, 0));
        assert_eq!(i.centred(10, 10), (0, 0));
        assert_eq!(solid(101, 1).centred(800, 600), (349, 299));
    }

    #[test]
    fn colours() {
        assert_eq!(parse_colour("#204080"), Some([0x20, 0x40, 0x80]));
        assert_eq!(parse_colour("#FFaa00"), Some([0xff, 0xaa, 0]));
        for bad in ["", "204080", "#20408", "#2040800", "#20408g", "#２04080", "# 04080", "#+04080"] {
            assert_eq!(parse_colour(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn crop_and_fit_dims() {
        let mut px = alloc::vec![0x101010u32; 20 * 10];
        for y in 4..6 {
            for x in 8..12 {
                px[y * 20 + x] = 0xffffff;
            }
        }
        let img = Image { w: 20, h: 10, px };
        let c = img.crop_to_content([0x10, 0x10, 0x10], 0);
        assert_eq!((c.w, c.h), (4, 2));
        assert!(c.px.iter().all(|&p| p == 0xffffff));
        let c = img.crop_to_content([0x10, 0x10, 0x10], 1);
        assert_eq!((c.w, c.h), (6, 4));
        // padding never leaves the picture; nothing but background keeps the whole picture
        assert_eq!(img.crop_to_content([0x10, 0x10, 0x10], 100).w, 20);
        assert_eq!(Image { w: 3, h: 3, px: alloc::vec![0; 9] }.crop_to_content([0, 0, 0], 0).w, 3);
        // fit_dims agrees with fit
        for (w, h, mw, mh) in [(800, 450, 640, 100), (800, 450, 2000, 2000), (10, 1000, 50, 50), (1, 1, 1, 1), (800, 450, 0, 5)] {
            let f = Image { w, h, px: alloc::vec![0; (w * h) as usize] }.fit(mw, mh);
            assert_eq!(fit_dims(w, h, mw, mh), (f.w, f.h));
        }
    }
}

//! Pictures for the browser QA nodes: crop, join, PNG in and out, and a simple pixel comparison with a diff picture. In-house; the
//! only library is `miniz_oxide` (zlib), which the PNG writer already uses.

use crate::browserjobs::png;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    /// `w * h * 4` bytes, opaque RGBA.
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(w: u32, h: u32, rgba: Vec<u8>) -> Result<Image, String> {
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
            return Err("the picture is empty or damaged".into());
        }
        Ok(Image { w, h, rgba })
    }

    pub fn blank(w: u32, h: u32, c: [u8; 4]) -> Image {
        Image { w, h, rgba: c.iter().copied().cycle().take(w as usize * h as usize * 4).collect() }
    }

    pub fn to_png(&self) -> Vec<u8> {
        png::encode_rgba(self.w, self.h, &self.rgba)
    }

    /// The part inside the rectangle (clipped to the picture); `None` when nothing is left.
    pub fn crop(&self, x: i64, y: i64, w: i64, h: i64) -> Option<Image> {
        let x0 = x.clamp(0, self.w as i64);
        let y0 = y.clamp(0, self.h as i64);
        let x1 = (x + w).clamp(0, self.w as i64);
        let y1 = (y + h).clamp(0, self.h as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut out = Vec::with_capacity(cw * ch * 4);
        for row in y0 as usize..y1 as usize {
            let a = (row * self.w as usize + x0 as usize) * 4;
            out.extend_from_slice(&self.rgba[a..a + cw * 4]);
        }
        Some(Image { w: cw as u32, h: ch as u32, rgba: out })
    }

    /// Pictures of the same width, one below the other.
    pub fn stack(parts: &[Image]) -> Option<Image> {
        let w = parts.first()?.w;
        if parts.iter().any(|p| p.w != w) {
            return None;
        }
        let h: u32 = parts.iter().map(|p| p.h).sum();
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for p in parts {
            rgba.extend_from_slice(&p.rgba);
        }
        Some(Image { w, h, rgba })
    }
}

// ------------------------------------------------------------------------------------------------ PNG reading

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Reads a non-interlaced 8-bit PNG (grey, grey+alpha, RGB, RGBA, palette): what this program and common tools write.
pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() < 33 || bytes[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Err("this is not a PNG file".into());
    }
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut idat: Vec<u8> = Vec::new();
    let mut plte: Vec<u8> = Vec::new();
    let mut pos = 8;
    while pos + 8 <= bytes.len() {
        let len = be32(&bytes[pos..]) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let data = bytes.get(pos + 8..pos + 8 + len).ok_or("the PNG file is cut short")?;
        match kind {
            b"IHDR" if len >= 13 => {
                w = be32(data);
                h = be32(&data[4..]);
                depth = data[8];
                ctype = data[9];
                interlace = data[12];
            }
            b"PLTE" => plte = data.to_vec(),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if w == 0 || h == 0 || w > 16384 || h > 32768 {
        return Err("the PNG file has a size this program will not read".into());
    }
    if depth != 8 || interlace != 0 {
        return Err("only 8-bit PNG files without interlacing can be compared".into());
    }
    let channels = match ctype {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err("this PNG colour type is not supported".into()),
    };
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, 1 << 30).map_err(|_| "the PNG data is damaged".to_string())?;
    let stride = w as usize * channels;
    if raw.len() < (stride + 1) * h as usize {
        return Err("the PNG data is too short".into());
    }
    let mut cur = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let f = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for i in 0..stride {
            let a = if i >= channels { cur[i - channels] } else { 0 } as i32;
            let b = prev[i] as i32;
            let c = if i >= channels { prev[i - channels] } else { 0 } as i32;
            let x = line[i];
            cur[i] = match f {
                0 => x,
                1 => x.wrapping_add(a as u8),
                2 => x.wrapping_add(b as u8),
                3 => x.wrapping_add(((a + b) / 2) as u8),
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    let pr = if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c };
                    x.wrapping_add(pr as u8)
                }
                _ => return Err("the PNG data is damaged (unknown filter)".into()),
            };
        }
        for px in cur.chunks_exact(channels) {
            match ctype {
                0 => rgba.extend_from_slice(&[px[0], px[0], px[0], 255]),
                2 => rgba.extend_from_slice(&[px[0], px[1], px[2], 255]),
                3 => {
                    let i = px[0] as usize * 3;
                    let p = plte.get(i..i + 3).unwrap_or(&[0, 0, 0]);
                    rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
                4 => rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]),
                _ => rgba.extend_from_slice(&[px[0], px[1], px[2], px[3]]),
            }
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Ok(Image { w, h, rgba })
}

// ------------------------------------------------------------------------------------------------ comparing

#[derive(Debug)]
pub struct Diff {
    pub total: u64,
    pub changed: u64,
    /// Percent of pixels that differ (more than the tolerance in any colour channel).
    pub percent: f64,
    /// The new picture dimmed, with every changed pixel in red.
    pub image: Image,
}

/// Compare two pictures of the same size.
pub fn diff(base: &Image, now: &Image, tolerance: u8) -> Result<Diff, String> {
    if base.w != now.w || base.h != now.h {
        return Err(format!("the picture is {}x{} but the baseline is {}x{}", now.w, now.h, base.w, base.h));
    }
    let mut out = Vec::with_capacity(now.rgba.len());
    let mut changed = 0u64;
    for (a, b) in base.rgba.chunks_exact(4).zip(now.rgba.chunks_exact(4)) {
        let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
        if d > tolerance {
            changed += 1;
            out.extend_from_slice(&[255, 0, 60, 255]);
        } else {
            out.extend_from_slice(&[b[0] / 3 + 150, b[1] / 3 + 150, b[2] / 3 + 150, 255]);
        }
    }
    let total = base.w as u64 * base.h as u64;
    Ok(Diff { total, changed, percent: changed as f64 * 100.0 / total as f64, image: Image { w: now.w, h: now.h, rgba: out } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Image {
        Image::new(w, h, (0..w * h).flat_map(|i| [(i % w) as u8 * 5, (i / w) as u8 * 7, 160, 255]).collect()).unwrap()
    }

    #[test]
    fn png_round_trip_and_crop_and_stack() {
        let im = gradient(12, 9);
        let back = decode_png(&im.to_png()).unwrap();
        assert_eq!(back, im);
        let c = im.crop(2, 3, 5, 4).unwrap();
        assert_eq!((c.w, c.h), (5, 4));
        assert_eq!(c.rgba[..4], im.rgba[((3 * 12 + 2) * 4)..((3 * 12 + 2) * 4 + 4)]);
        assert!(im.crop(20, 0, 5, 5).is_none());
        let s = Image::stack(&[im.crop(0, 0, 12, 4).unwrap(), im.crop(0, 4, 12, 5).unwrap()]).unwrap();
        assert_eq!(s, im);
        assert!(decode_png(b"nope").is_err());
    }

    #[test]
    fn the_diff_counts_pixels_beyond_the_tolerance() {
        let a = gradient(10, 10);
        let mut b = a.clone();
        assert_eq!(diff(&a, &b, 0).unwrap().changed, 0);
        for i in 0..5 {
            b.rgba[i * 4] = b.rgba[i * 4].wrapping_add(100);
        }
        let d = diff(&a, &b, 8).unwrap();
        assert_eq!(d.changed, 5);
        assert!((d.percent - 5.0).abs() < 1e-9);
        assert_eq!(&d.image.rgba[..4], &[255, 0, 60, 255]);
        b.rgba[5 * 4] = b.rgba[5 * 4].wrapping_add(4);
        assert_eq!(diff(&a, &b, 8).unwrap().changed, 5, "a small shift is inside the tolerance");
        assert!(diff(&a, &gradient(9, 10), 0).is_err());
    }
}

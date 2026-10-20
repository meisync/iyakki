//! Pixel work for the camera: YUYV (the uncompressed format every UVC camera offers) to RGBA, cutting out and scaling a
//! part of a picture, mirroring, and PNG files that carry their print size (the `pHYs` chunk, so a 300 dpi passport photo
//! prints at 35×45 mm).

/// One video line of YUYV 4:2:2 (Y0 U Y1 V for every two pixels, BT.601 studio range) to straight RGBA.
pub fn yuyv_to_rgba(w: u32, h: u32, data: &[u8]) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let clip = |v: i32| v.clamp(0, 255) as u8;
    let stride = w * 2;
    for y in 0..h {
        let row = &data[(y * stride).min(data.len())..((y + 1) * stride).min(data.len())];
        for (pair, q) in row.chunks_exact(4).enumerate() {
            let (u, v) = (q[1] as i32 - 128, q[3] as i32 - 128);
            for (k, yy) in [q[0], q[2]].into_iter().enumerate() {
                let x = pair * 2 + k;
                if x >= w {
                    break;
                }
                let c = 298 * (yy as i32 - 16);
                let i = (y * w + x) * 4;
                out[i] = clip((c + 409 * v + 128) >> 8);
                out[i + 1] = clip((c - 100 * u - 208 * v + 128) >> 8);
                out[i + 2] = clip((c + 516 * u + 128) >> 8);
                out[i + 3] = 255;
            }
        }
    }
    out
}

/// RGBA to YUYV (for the fake camera of the tests and the screenshots).
pub fn rgba_to_yuyv(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = Vec::with_capacity(w * h * 2);
    let yuv = |i: usize| {
        let (r, g, b) = (rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32);
        (16.0 + 0.257 * r + 0.504 * g + 0.098 * b, 128.0 - 0.148 * r - 0.291 * g + 0.439 * b, 128.0 + 0.439 * r - 0.368 * g - 0.071 * b)
    };
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let a = yuv((y * w + x) * 4);
            let b = yuv((y * w + (x + 1).min(w - 1)) * 4);
            out.extend_from_slice(&[a.0.round() as u8, ((a.1 + b.1) / 2.0).round() as u8, b.0.round() as u8, ((a.2 + b.2) / 2.0).round() as u8]);
        }
    }
    out
}

/// A rectangle in source pixels (fractions allowed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Cut `r` out of a `sw`×`sh` RGBA picture and scale it to `ow`×`oh`, flipped left to right when `mirror`. Each output
/// pixel averages a small grid of bilinear samples, so shrinking does not alias.
pub fn crop_scale(sw: u32, sh: u32, rgba: &[u8], r: RectF, ow: u32, oh: u32, mirror: bool) -> Vec<u8> {
    let (swu, shu) = (sw.max(1) as usize, sh.max(1) as usize);
    let (ow, oh) = (ow.max(1) as usize, oh.max(1) as usize);
    let mut out = vec![0u8; ow * oh * 4];
    let (fx, fy) = (r.w / ow as f32, r.h / oh as f32);
    let n = (fx.max(fy).ceil() as usize).clamp(1, 4);
    let sample = |x: f32, y: f32| -> [f32; 4] {
        let x = x.clamp(0.0, swu as f32 - 1.0);
        let y = y.clamp(0.0, shu as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(swu - 1), (y0 + 1).min(shu - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize, c: usize| rgba.get((yy * swu + xx) * 4 + c).copied().unwrap_or(0) as f32;
        let mut v = [0f32; 4];
        for (c, o) in v.iter_mut().enumerate() {
            let top = p(x0, y0, c) * (1.0 - tx) + p(x1, y0, c) * tx;
            let bot = p(x0, y1, c) * (1.0 - tx) + p(x1, y1, c) * tx;
            *o = top * (1.0 - ty) + bot * ty;
        }
        v
    };
    for oy in 0..oh {
        for ox in 0..ow {
            let src_x = if mirror { ow - 1 - ox } else { ox };
            let mut acc = [0f32; 4];
            for j in 0..n {
                for i in 0..n {
                    let sx = r.x + (src_x as f32 + (i as f32 + 0.5) / n as f32) * fx - 0.5;
                    let sy = r.y + (oy as f32 + (j as f32 + 0.5) / n as f32) * fy - 0.5;
                    let s = sample(sx, sy);
                    for c in 0..4 {
                        acc[c] += s[c];
                    }
                }
            }
            let k = (n * n) as f32;
            let o = (oy * ow + ox) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / k).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Flip an RGBA picture left to right.
pub fn mirror(w: u32, h: u32, rgba: &mut [u8]) {
    let w = w as usize;
    for row in rgba.chunks_exact_mut(w * 4).take(h as usize) {
        for x in 0..w / 2 {
            for c in 0..4 {
                row.swap(x * 4 + c, (w - 1 - x) * 4 + c);
            }
        }
    }
}

/// The CRC-32 of PNG chunks (and zip): polynomial 0xEDB88320.
pub fn crc32(parts: &[&[u8]]) -> u32 {
    let mut table = [0u32; 256];
    for (n, t) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for p in parts {
        for &b in *p {
            c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

/// An opaque RGBA picture as a tiny-skia pixmap (straight and premultiplied are the same when opaque).
pub fn pixmap(w: u32, h: u32, mut rgba: Vec<u8>) -> Option<tiny_skia::Pixmap> {
    rgba.resize(w as usize * h as usize * 4, 255);
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)
}

/// A PNG of an opaque RGBA picture that says it is `dpi` dots per inch (`pHYs`, in dots per metre).
pub fn png_with_dpi(w: u32, h: u32, rgba: &[u8], dpi: f32) -> Result<Vec<u8>, String> {
    let pm = pixmap(w, h, rgba.to_vec()).ok_or("the picture has no size")?;
    let png = pm.encode_png().map_err(|e| e.to_string())?;
    // the signature (8) and IHDR (4 + 4 + 13 + 4) come first; pHYs goes right after
    if png.len() < 33 || &png[12..16] != b"IHDR" {
        return Err("unexpected PNG layout".into());
    }
    let ppm = (dpi / 0.0254).round() as u32;
    let mut data = Vec::with_capacity(9);
    data.extend_from_slice(&ppm.to_be_bytes());
    data.extend_from_slice(&ppm.to_be_bytes());
    data.push(1);
    let mut out = png[..33].to_vec();
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(b"pHYs");
    out.extend_from_slice(&data);
    out.extend_from_slice(&crc32(&[b"pHYs", &data]).to_be_bytes());
    out.extend_from_slice(&png[33..]);
    Ok(out)
}

/// The dots per inch a PNG says it has (`pHYs` in metres), if any.
pub fn png_dpi(png: &[u8]) -> Option<f32> {
    let mut i = 8;
    while i + 12 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().ok()?) as usize;
        let kind = &png[i + 4..i + 8];
        if kind == b"pHYs" && len == 9 && png.get(i + 16) == Some(&1) {
            let ppm = u32::from_be_bytes(png[i + 8..i + 12].try_into().ok()?);
            return Some(ppm as f32 * 0.0254);
        }
        i += 12 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuyv_becomes_the_right_colours() {
        // black, white, and the studio-range primaries
        let px = |y: u8, u: u8, v: u8| yuyv_to_rgba(2, 1, &[y, u, y, v])[..4].to_vec();
        assert_eq!(px(16, 128, 128), vec![0, 0, 0, 255]);
        assert_eq!(px(235, 128, 128), vec![255, 255, 255, 255]);
        let red = px(81, 90, 240);
        assert!(red[0] > 250 && red[1] < 5 && red[2] < 5, "{red:?}");
        let green = px(145, 54, 34);
        assert!(green[1] > 250 && green[0] < 5 && green[2] < 5, "{green:?}");
        let blue = px(41, 240, 110);
        assert!(blue[2] > 250 && blue[0] < 5 && blue[1] < 5, "{blue:?}");
        // two pixels share U and V but keep their own Y
        let two = yuyv_to_rgba(2, 1, &[16, 128, 235, 128]);
        assert_eq!(two, vec![0, 0, 0, 255, 255, 255, 255, 255]);
        // a short buffer gives black, not a panic; odd widths work
        assert_eq!(yuyv_to_rgba(3, 2, &[128, 128]).len(), 24);
        // round trip through our own RGBA -> YUYV
        let src: Vec<u8> = (0..16).flat_map(|i| [i * 15, 255 - i * 15, 100, 255]).collect();
        let back = yuyv_to_rgba(4, 4, &rgba_to_yuyv(4, 4, &src));
        let err: i32 = src.iter().zip(&back).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(err <= 40, "chroma is shared per pair, luma is kept: {err}");
    }

    #[test]
    fn cutting_scaling_and_mirroring() {
        // a 4x2 picture: left half red, right half blue
        let mut src = Vec::new();
        for _y in 0..2 {
            for x in 0..4 {
                src.extend_from_slice(if x < 2 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let right = crop_scale(4, 2, &src, RectF { x: 2.0, y: 0.0, w: 2.0, h: 2.0 }, 1, 1, false);
        assert_eq!(right, vec![0, 0, 255, 255]);
        let whole = crop_scale(4, 2, &src, RectF { x: 0.0, y: 0.0, w: 4.0, h: 2.0 }, 2, 1, true);
        assert_eq!(&whole[..4], &[0, 0, 255, 255], "mirrored: blue first");
        assert_eq!(&whole[4..], &[255, 0, 0, 255]);
        let mut m = src.clone();
        mirror(4, 2, &mut m);
        assert_eq!(&m[..4], &[0, 0, 255, 255]);
        // enlarging keeps the size asked for
        assert_eq!(crop_scale(4, 2, &src, RectF { x: 0.0, y: 0.0, w: 4.0, h: 2.0 }, 40, 20, false).len(), 40 * 20 * 4);
    }

    #[test]
    fn png_files_carry_their_print_size() {
        let rgba = vec![200u8; 10 * 6 * 4];
        let png = png_with_dpi(10, 6, &rgba, 300.0).unwrap();
        assert!((png_dpi(&png).unwrap() - 300.0).abs() < 0.1);
        let pm = tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert_eq!((pm.width(), pm.height()), (10, 6));
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
    }
}

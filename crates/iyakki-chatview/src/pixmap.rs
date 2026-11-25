//! Picture bytes to the premultiplied pixmap the renderer draws: the one decoder of the chat transcript, the browser and the tiles.
//! (Moved from the shell's browser pictures so the UI-layer chat view can name it; `browser_images` re-exports it.)

use iyakki_web::image::{self, Kind};
use tiny_skia::Pixmap;

/// Pictures larger than this are shrunk when they arrive.
pub const MAX_SIDE: u32 = 2048;

/// Straight RGBA to the premultiplied pixmap the renderer wants.
pub fn bitmap_pixmap(w: u32, h: u32, rgba: &[u8]) -> Result<Pixmap, String> {
    let mut pm = Pixmap::new(w, h).ok_or("the picture has no size")?;
    for (dst, src) in pm.data_mut().chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = src[3] as u32;
        dst[0] = (src[0] as u32 * a / 255) as u8;
        dst[1] = (src[1] as u32 * a / 255) as u8;
        dst[2] = (src[2] as u32 * a / 255) as u8;
        dst[3] = src[3];
    }
    Ok(pm)
}

/// Decode a picture file into a pixmap (premultiplied, as the renderer wants it).
pub fn to_pixmap(bytes: &[u8]) -> Result<Pixmap, String> {
    let pm = match image::sniff(bytes) {
        Kind::Png => Pixmap::decode_png(bytes).map_err(|e| format!("the PNG could not be read ({e})"))?,
        Kind::Jpeg | Kind::Gif | Kind::Webp | Kind::Svg => {
            let b = image::decode(bytes)?;
            bitmap_pixmap(b.width, b.height, &b.rgba)?
        }
        _ => return image::decode(bytes).map(|_| unreachable!()),
    };
    Ok(shrink(pm))
}

/// Average blocks of pixels so that neither side is over [`MAX_SIDE`].
pub fn shrink(pm: Pixmap) -> Pixmap {
    let (w, h) = (pm.width(), pm.height());
    let f = w.max(h).div_ceil(MAX_SIDE);
    if f <= 1 {
        return pm;
    }
    let (nw, nh) = ((w / f).max(1), (h / f).max(1));
    let Some(mut out) = Pixmap::new(nw, nh) else { return pm };
    let src = pm.data();
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for dy in 0..f {
                for dx in 0..f {
                    let (sx, sy) = ((x * f + dx).min(w - 1), (y * f + dy).min(h - 1));
                    let o = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += src[o + c] as u32;
                    }
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out.data_mut()[o + c] = (acc[c] / (f * f)) as u8;
            }
        }
    }
    out
}

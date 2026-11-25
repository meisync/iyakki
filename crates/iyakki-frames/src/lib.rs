//! Picture plumbing with no I/O: the PNG writer ([`png`]) and [`fit`].

pub mod png;

/// The size a picture of `src` gets inside `area`: as large as fits, the proportions kept, both sides even (the decoder wants it).
pub fn fit(src: (u32, u32), area: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (src.0.max(1) as f64, src.1.max(1) as f64);
    let k = (area.0.max(2) as f64 / sw).min(area.1.max(2) as f64 / sh);
    let even = |v: f64| ((v.round() as u32).max(2)) & !1;
    (even(sw * k), even(sh * k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_proportions_and_both_sides_even() {
        assert_eq!(fit((1920, 1080), (960, 960)), (960, 540));
        assert_eq!(fit((100, 200), (50, 50)), (24, 50));
        let (w, h) = fit((0, 0), (0, 0));
        assert!(w >= 2 && h >= 2 && w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn a_png_has_its_signature_and_chunks_in_order() {
        let p = png::encode_rgba(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&p[12..16], b"IHDR");
        assert_eq!(&p[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
        let pos = |tag: &[u8]| p.windows(4).position(|w| w == tag).unwrap();
        assert!(pos(b"IHDR") < pos(b"IDAT") && pos(b"IDAT") < pos(b"IEND"));
    }
}

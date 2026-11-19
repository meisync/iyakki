//! Per-frame pixel work between the engine's picture and the screen scene: the channel swap, the 1:1 and the 2x "smooth" copy of a
//! changed rectangle, and the strips of white around a picture that does not fill its area. Pure functions on byte slices (no
//! unsafe code, nothing allocated per row), tested against slow reference versions, so the shell's tile path and the probe's
//! benchmark (`examples/probe.rs`, step `bench`) run exactly the same code.

/// One row of the engine's picture (BGRA, premultiplied) as opaque RGBA over white. Both slices have the same length, a multiple of 4.
pub fn bgra_to_rgba_opaque(src: &[u8], dst: &mut [u8]) {
    // one branch per pixel: the compiler turns it into a select and wide shuffles (a separate "all opaque" pass measured slower)
    let n = src.len().min(dst.len()) / 4 * 4;
    let (src, dst) = (&src[..n], &mut dst[..n]);
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        if s[3] == 255 {
            d.copy_from_slice(&[s[2], s[1], s[0], 255]);
        } else {
            let rest = 255 - s[3];
            d.copy_from_slice(&[s[2].saturating_add(rest), s[1].saturating_add(rest), s[0].saturating_add(rest), 255]);
        }
    }
}

/// The parts of the view `(vx, vy, vw, vh)` that a picture of `pw` x `ph` pixels drawn at its top-left corner does not cover, as
/// `(x, y, w, h)` rectangles: the strip to its right, then the strip below it (the whole view when there is no picture size).
pub fn uncovered(vx: i32, vy: i32, vw: i32, vh: i32, pw: i32, ph: i32) -> Vec<(i32, i32, i32, i32)> {
    let (vw, vh) = (vw.max(0), vh.max(0));
    if vw == 0 || vh == 0 {
        return Vec::new();
    }
    let (cw, ch) = (pw.clamp(0, vw), ph.clamp(0, vh));
    if cw == 0 || ch == 0 {
        return vec![(vx, vy, vw, vh)];
    }
    let mut v = Vec::new();
    if cw < vw {
        v.push((vx + cw, vy, vw - cw, vh));
    }
    if ch < vh {
        v.push((vx, vy + ch, cw, vh - ch));
    }
    v
}

/// Copy `region` `(x, y, w, h)` of a `pw` x `ph` RGBA picture, drawn with its top-left corner at `view`'s `(x, y)` and clipped to the
/// view's width and height, into a `gw` x `gh` RGBA screen buffer; rows are copied whole. Returns the screen rectangle written
/// `(x, y, w, h)`, or `None` when nothing of the region is visible. Never indexes outside either buffer.
pub fn blit_region(src: &[u8], pw: i32, ph: i32, dst: &mut [u8], gw: i32, gh: i32, view: (i32, i32, i32, i32), region: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (vx, vy, vw, vh) = view;
    if pw <= 0 || ph <= 0 || gw <= 0 || gh <= 0 || src.len() < (pw as usize) * (ph as usize) * 4 || dst.len() < (gw as usize) * (gh as usize) * 4 {
        return None;
    }
    // picture pixels that are inside the picture, the view and (once moved by the view's position) the screen
    let x0 = region.0.max(0).max(-vx);
    let x1 = (region.0.saturating_add(region.2)).min(pw).min(vw).min(gw - vx);
    let y0 = region.1.max(0).max(-vy);
    let y1 = (region.1.saturating_add(region.3)).min(ph).min(vh).min(gh - vy);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let n = ((x1 - x0) * 4) as usize;
    for y in y0..y1 {
        let a = ((y * pw + x0) * 4) as usize;
        let b = (((vy + y) * gw + vx + x0) * 4) as usize;
        dst[b..b + n].copy_from_slice(&src[a..a + n]);
    }
    Some((vx + x0, vy + y0, x1 - x0, y1 - y0))
}

/// How many screen pixels one picture pixel takes: 2 when smooth is on and the picture is about half the view's width (the engine
/// has caught up with the new size), else 1 (a picture of the old size is still shown as it is).
pub fn stretch_for(smooth: bool, pw: i32, vw: i32) -> i32 {
    if smooth && pw > 0 && pw * 2 <= vw && pw * 2 + 3 >= vw {
        2
    } else {
        1
    }
}

/// Per byte `(3a + b + 2) / 4` for the four bytes packed in each of two `u32`s (no byte crosses into the next one).
#[inline]
fn mix31(a: u32, b: u32) -> u32 {
    const M: u32 = 0x00FF_00FF;
    let rb = (((a & M) * 3 + (b & M) + 0x0002_0002) >> 2) & M;
    let ag = ((((a >> 8) & M) * 3 + ((b >> 8) & M) + 0x0002_0002) >> 2) & M;
    rb | (ag << 8)
}

/// [`blit_region`] for a picture shown at twice its size (the "Smooth" setting: the engine draws a quarter of the pixels and the
/// shell stretches them, with the two nearest source pixels weighing 3 to 1 in each direction). `region` is in picture pixels;
/// the screen rectangle written is that region doubled plus the one pixel border its neighbours influence, clipped to the
/// view (which starts at `view`'s `(x, y)` and shows `2 * pw` x `2 * ph` pixels at most) and the screen. Returns it.
pub fn blit_region_x2(src: &[u8], pw: i32, ph: i32, dst: &mut [u8], gw: i32, gh: i32, view: (i32, i32, i32, i32), region: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (vx, vy, vw, vh) = view;
    if pw <= 0 || ph <= 0 || gw <= 0 || gh <= 0 || src.len() < (pw as usize) * (ph as usize) * 4 || dst.len() < (gw as usize) * (gh as usize) * 4 {
        return None;
    }
    // local (view) coordinates of the destination: the doubled region and a one pixel border, inside the stretched picture, the view and the screen
    let rx1 = region.0.saturating_add(region.2);
    let ry1 = region.1.saturating_add(region.3);
    if region.2 <= 0 || region.3 <= 0 || region.0 >= pw || region.1 >= ph || rx1 <= 0 || ry1 <= 0 {
        return None;
    }
    let lx0 = (region.0.max(0) * 2 - 1).max(0).max(-vx);
    let lx1 = (rx1.min(pw) * 2 + 1).min(pw * 2).min(vw).min(gw - vx);
    let ly0 = (region.1.max(0) * 2 - 1).max(0).max(-vy);
    let ly1 = (ry1.min(ph) * 2 + 1).min(ph * 2).min(vh).min(gh - vy);
    if lx1 <= lx0 || ly1 <= ly0 {
        return None;
    }
    let (pwu, c0, c1) = (pw as usize, ((lx0 / 2 - 1).max(0)) as usize, ((((lx1 - 1) / 2) + 1).min(pw - 1)) as usize);
    let (n0, n1) = (lx0 as usize, lx1 as usize);
    // `tmp`: a source row blended with its vertical neighbour; `exp`: that row stretched to twice the width
    let mut tmp = vec![0u32; pwu];
    let mut exp = vec![0u32; pwu * 2];
    for ly in ly0..ly1 {
        let sy = ly / 2;
        let oy = if ly % 2 == 0 { (sy - 1).max(0) } else { (sy + 1).min(ph - 1) };
        let ra = &src[(sy as usize * pwu) * 4..][..pwu * 4];
        let rb = &src[(oy as usize * pwu) * 4..][..pwu * 4];
        for ((t, a), b) in tmp[c0..=c1].iter_mut().zip(ra[c0 * 4..(c1 + 1) * 4].chunks_exact(4)).zip(rb[c0 * 4..(c1 + 1) * 4].chunks_exact(4)) {
            *t = mix31(u32::from_ne_bytes([a[0], a[1], a[2], a[3]]), u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
        }
        // columns c0 and c1 may read a neighbour outside what was blended: their outputs are not copied below
        for sx in c0..=c1 {
            let (c, l, r) = (tmp[sx], tmp[sx.saturating_sub(1)], tmp[(sx + 1).min(pwu - 1)]);
            exp[2 * sx] = mix31(c, l);
            exp[2 * sx + 1] = mix31(c, r);
        }
        let o = ((vy + ly) * gw + vx + lx0) as usize * 4;
        for (d, v) in dst[o..o + (n1 - n0) * 4].chunks_exact_mut(4).zip(&exp[n0..n1]) {
            d.copy_from_slice(&v.to_ne_bytes());
        }
    }
    Some((vx + lx0, vy + ly0, lx1 - lx0, ly1 - ly0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fast_row_conversion_gives_the_same_pixels_as_the_exact_one() {
        let mut r = Lcg(7);
        for translucent in [false, true] {
            let n = 997;
            let src: Vec<u8> = (0..n * 4).map(|i| if i % 4 == 3 && !translucent { 255 } else { r.next() as u8 }).collect();
            let mut fast = vec![0u8; n * 4];
            bgra_to_rgba_opaque(&src, &mut fast);
            for (i, (d, s)) in fast.chunks_exact(4).zip(src.chunks_exact(4)).enumerate() {
                let rest = 255 - s[3];
                let want = [s[2].saturating_add(rest), s[1].saturating_add(rest), s[0].saturating_add(rest), 255];
                assert_eq!(d, want, "pixel {i} (translucent pictures: {translucent})");
            }
        }
        bgra_to_rgba_opaque(&[], &mut []); // an empty row is fine
    }

    #[test]
    fn the_white_strips_and_the_picture_cover_the_view_exactly() {
        let mut r = Lcg(11);
        for _ in 0..300 {
            let (vx, vy, vw, vh) = (r.below(40) as i32 - 20, r.below(40) as i32 - 20, r.below(60) as i32, r.below(60) as i32);
            let (pw, ph) = (r.below(80) as i32, r.below(80) as i32);
            let mut cover = vec![0u8; (vw.max(0) * vh.max(0)) as usize];
            let mark = |cover: &mut Vec<u8>, x0: i32, y0: i32, w: i32, h: i32| {
                for y in y0..y0 + h {
                    for x in x0..x0 + w {
                        let (cx, cy) = (x - vx, y - vy);
                        assert!(cx >= 0 && cy >= 0 && cx < vw && cy < vh, "a strip leaves the view");
                        cover[(cy * vw + cx) as usize] += 1;
                    }
                }
            };
            for (x, y, w, h) in uncovered(vx, vy, vw, vh, pw, ph) {
                mark(&mut cover, x, y, w, h);
            }
            let (cw, ch) = (pw.clamp(0, vw.max(0)), ph.clamp(0, vh.max(0)));
            if cw > 0 && ch > 0 {
                mark(&mut cover, vx, vy, cw, ch);
            }
            assert!(cover.iter().all(|&c| c == 1), "view {vw}x{vh} picture {pw}x{ph}: every view pixel is covered once");
        }
    }

    /// Painting the changed rectangles one by one gives exactly the picture a whole draw gives, at any position of the view
    /// (also partly off the screen) and for any rectangles, and nothing outside the view is touched.
    #[test]
    fn painting_only_the_changed_rectangles_equals_painting_everything() {
        let mut r = Lcg(3);
        for case in 0..200 {
            let (gw, gh) = (20 + r.below(60) as i32, 20 + r.below(60) as i32);
            let (pw, ph) = (1 + r.below(70) as i32, 1 + r.below(70) as i32);
            let view = (r.below(50) as i32 - 25, r.below(50) as i32 - 25, 1 + r.below(70) as i32, 1 + r.below(70) as i32);
            let mut pic: Vec<u8> = (0..pw * ph * 4).map(|_| r.next() as u8).collect();
            let background: Vec<u8> = (0..gw * gh * 4).map(|_| r.next() as u8).collect();
            // the screen after a whole draw of the first picture, then the picture changes in a few rectangles
            let mut part = background.clone();
            blit_region(&pic, pw, ph, &mut part, gw, gh, view, (0, 0, pw, ph));
            for _ in 0..1 + r.below(4) {
                let (x, y) = (r.below(pw as u32) as i32, r.below(ph as u32) as i32);
                let (w, h) = (1 + r.below((pw - x) as u32) as i32, 1 + r.below((ph - y) as u32) as i32);
                for yy in y..y + h {
                    for xx in x..x + w {
                        for c in 0..4 {
                            pic[((yy * pw + xx) * 4 + c) as usize] = r.next() as u8;
                        }
                    }
                }
                blit_region(&pic, pw, ph, &mut part, gw, gh, view, (x, y, w, h));
            }
            let mut whole = background.clone();
            blit_region(&pic, pw, ph, &mut whole, gw, gh, view, (0, 0, pw, ph));
            assert!(part == whole, "case {case}: view {view:?} picture {pw}x{ph} screen {gw}x{gh}");
            // nothing outside the view (or the picture) changed
            for y in 0..gh {
                for x in 0..gw {
                    let (px, py) = (x - view.0, y - view.1);
                    let inside = px >= 0 && py >= 0 && px < view.2.min(pw) && py < view.3.min(ph);
                    if !inside {
                        let i = ((y * gw + x) * 4) as usize;
                        assert_eq!(whole[i..i + 4], background[i..i + 4], "case {case}: pixel {x},{y} is outside the view");
                    }
                }
            }
        }
    }

    #[test]
    fn a_blit_never_indexes_outside_its_buffers_and_says_what_it_wrote() {
        let pic = vec![9u8; 4 * 4 * 4];
        let mut screen = vec![0u8; 8 * 8 * 4];
        assert_eq!(blit_region(&pic, 4, 4, &mut screen, 8, 8, (6, 6, 4, 4), (0, 0, 4, 4)), Some((6, 6, 2, 2)), "clipped by the screen");
        assert_eq!(blit_region(&pic, 4, 4, &mut screen, 8, 8, (-2, -2, 4, 4), (0, 0, 4, 4)), Some((0, 0, 2, 2)), "clipped at the top left");
        assert_eq!(blit_region(&pic, 4, 4, &mut screen, 8, 8, (20, 0, 4, 4), (0, 0, 4, 4)), None, "off the screen");
        assert_eq!(blit_region(&pic, 4, 4, &mut screen, 8, 8, (0, 0, 4, 4), (10, 10, 4, 4)), None, "a rectangle outside the picture");
        assert_eq!(blit_region(&pic[..10], 4, 4, &mut screen, 8, 8, (0, 0, 4, 4), (0, 0, 4, 4)), None, "a short picture buffer");
        assert_eq!(blit_region(&pic, 4, 4, &mut screen[..30], 8, 8, (0, 0, 4, 4), (0, 0, 4, 4)), None, "a short screen buffer");
        assert_eq!(blit_region(&pic, 4, 4, &mut screen, 8, 8, (0, 0, 4, 4), (i32::MAX - 1, 0, 9, 9)), None, "a huge rectangle does not overflow");
        assert_eq!(blit_region(&pic, 0, 0, &mut screen, 8, 8, (0, 0, 4, 4), (0, 0, 4, 4)), None, "no picture yet");
    }

    #[test]
    fn the_smooth_stretch_is_the_two_pass_weighted_average_and_partial_paints_equal_the_whole() {
        let mut r = Lcg(5);
        for case in 0..120 {
            let (pw, ph) = (1 + r.below(40) as i32, 1 + r.below(40) as i32);
            let (gw, gh) = (10 + r.below(100) as i32, 10 + r.below(100) as i32);
            let view = (r.below(40) as i32 - 20, r.below(40) as i32 - 20, 1 + r.below(100) as i32, 1 + r.below(100) as i32);
            let mut pic: Vec<u8> = (0..pw * ph * 4).map(|_| r.next() as u8).collect();
            let big = stretched(&pic, pw, ph);
            let background: Vec<u8> = (0..gw * gh * 4).map(|_| r.next() as u8).collect();
            let mut whole = background.clone();
            blit_region_x2(&pic, pw, ph, &mut whole, gw, gh, view, (0, 0, pw, ph));
            // every screen pixel inside the view and the stretched picture equals the reference; the others are untouched
            for y in 0..gh {
                for x in 0..gw {
                    let (lx, ly) = (x - view.0, y - view.1);
                    let i = ((y * gw + x) * 4) as usize;
                    if lx >= 0 && ly >= 0 && lx < (pw * 2).min(view.2) && ly < (ph * 2).min(view.3) {
                        let j = ((ly * pw * 2 + lx) * 4) as usize;
                        assert_eq!(whole[i..i + 4], big[j..j + 4], "case {case}: pixel {x},{y}");
                    } else {
                        assert_eq!(whole[i..i + 4], background[i..i + 4], "case {case}: pixel {x},{y} is outside");
                    }
                }
            }
            // change a few rectangles of the picture and paint only them: the same screen as painting everything
            let mut part = whole.clone();
            for _ in 0..1 + r.below(3) {
                let (x, y) = (r.below(pw as u32) as i32, r.below(ph as u32) as i32);
                let (w, h) = (1 + r.below((pw - x) as u32) as i32, 1 + r.below((ph - y) as u32) as i32);
                for yy in y..y + h {
                    for xx in x..x + w {
                        for c in 0..4 {
                            pic[((yy * pw + xx) * 4 + c) as usize] = r.next() as u8;
                        }
                    }
                }
                blit_region_x2(&pic, pw, ph, &mut part, gw, gh, view, (x, y, w, h));
            }
            let mut again = background.clone();
            blit_region_x2(&pic, pw, ph, &mut again, gw, gh, view, (0, 0, pw, ph));
            assert!(part == again, "case {case}: partial paints equal a whole paint (view {view:?}, picture {pw}x{ph}, screen {gw}x{gh})");
        }
    }

    #[test]
    fn a_stretch_never_indexes_outside_its_buffers() {
        let pic = vec![7u8; 4 * 4 * 4];
        let mut screen = vec![0u8; 5 * 5 * 4];
        assert!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (0, 0, 100, 100), (0, 0, 4, 4)).is_some());
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (30, 30, 8, 8), (0, 0, 4, 4)), None, "off the screen");
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (0, 0, 8, 8), (9, 9, 4, 4)), None, "a region outside the picture");
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (0, 0, 8, 8), (0, 0, i32::MAX, i32::MAX)).is_some(), true, "a huge region is clipped");
        assert_eq!(blit_region_x2(&pic[..8], 4, 4, &mut screen, 5, 5, (0, 0, 8, 8), (0, 0, 4, 4)), None);
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen[..9], 5, 5, (0, 0, 8, 8), (0, 0, 4, 4)), None);
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (-100, -100, 8, 8), (0, 0, 4, 4)), None, "far to the top left");
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (0, 0, 8, 8), (0, 0, 0, 4)), None, "an empty region");
        assert_eq!(blit_region_x2(&pic, 4, 4, &mut screen, 5, 5, (0, 0, 8, 8), (-5, -5, 3, 3)), None, "a region left of the picture");
    }

    #[test]
    fn the_stretch_applies_only_to_a_picture_of_half_the_view() {
        assert_eq!(stretch_for(true, 640, 1280), 2);
        assert_eq!(stretch_for(true, 639, 1279), 2, "an odd view width loses one pixel");
        assert_eq!(stretch_for(true, 1280, 1280), 1, "a full size picture (the engine has not resized yet) is shown as it is");
        assert_eq!(stretch_for(false, 640, 1280), 1, "smooth off");
        assert_eq!(stretch_for(true, 0, 1280), 1);
        assert_eq!(stretch_for(true, 600, 1280), 1, "a picture of another size is not stretched");
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
        fn below(&mut self, n: u32) -> u32 {
            self.next() % n.max(1)
        }
    }

    /// The stretched picture computed the slow, obvious way: vertical blend, then horizontal, per channel `(3a + b + 2) / 4`.
    fn stretched(pic: &[u8], pw: i32, ph: i32) -> Vec<u8> {
        let at = |x: i32, y: i32, c: usize| pic[(((y.clamp(0, ph - 1)) * pw + x.clamp(0, pw - 1)) * 4) as usize + c] as u32;
        let (w2, h2) = (pw * 2, ph * 2);
        let mut out = vec![0u8; (w2 * h2 * 4) as usize];
        for ly in 0..h2 {
            let (sy, oy) = (ly / 2, if ly % 2 == 0 { ly / 2 - 1 } else { ly / 2 + 1 });
            for lx in 0..w2 {
                let (sx, ox) = (lx / 2, if lx % 2 == 0 { lx / 2 - 1 } else { lx / 2 + 1 });
                for c in 0..4 {
                    let v = |x: i32| (3 * at(x, sy, c) + at(x, oy, c) + 2) / 4;
                    out[((ly * w2 + lx) * 4) as usize + c] = ((3 * v(sx) + v(ox) + 2) / 4) as u8;
                }
            }
        }
        out
    }
}

//! The frost behind the home screen's island (LAUNCHER-PLAN slice 3, F-LAUNCH-07).
//!
//! `tiny-skia` has no blur, so the wallpaper is blurred once, here, in our own code: the finished background of the screen is shrunk to a
//! quarter of its size, blurred with three box passes (which together come close to a Gaussian) and kept as a small picture. The island then
//! paints that picture, stretched back to the screen's size, through its own rounded shape: a frame costs one fill, never a blur. The
//! picture is rebuilt only when the screen's size, the theme or the wallpaper changes (the same key the background cache uses).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tiny_skia::Pixmap;

/// How much the picture is shrunk before it is blurred (the blur is soft, so nothing is lost; it is 16 times fewer pixels to blur).
pub const SHRINK: u32 = 4;

/// What the island needs: the blurred picture and how many screen pixels one of its pixels covers.
pub struct Frost {
    pub pm: Pixmap,
    pub factor: u32,
}

type Key = (u32, u32, u32, u32);

thread_local! {
    static CACHE: RefCell<Option<(Key, Rc<Frost>)>> = const { RefCell::new(None) };
    static BUILDS: Cell<u32> = const { Cell::new(0) };
}

/// How many times the picture was built on this thread (tests: it must stay put while frames are drawn).
pub fn builds() -> u32 {
    BUILDS.with(Cell::get)
}

/// The blurred picture for the background the screen shows now, built on first use and kept until the background changes.
/// None when no background has been drawn yet or the screen is too small to bother.
pub fn current() -> Option<Rc<Frost>> {
    let (key, pic) = crate::fx::background_snapshot(|key, data| (*key, data.to_vec()))?;
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((k, f)) = &*c {
            if *k == key {
                return Some(f.clone());
            }
        }
        let f = Rc::new(build(&pic, key.0, key.1)?);
        BUILDS.with(|b| b.set(b.get() + 1));
        *c = Some((key, f.clone()));
        Some(f)
    })
}

/// Shrink `rgba` (a `w` x `h` picture) and blur it. The blur's radius follows the screen's width, so the frost looks alike at every size.
pub fn build(rgba: &[u8], w: u32, h: u32) -> Option<Frost> {
    let _p = crate::prof::scope("blur.build");
    if w < SHRINK * 8 || h < SHRINK * 8 || rgba.len() != (w * h * 4) as usize {
        return None;
    }
    let (sw, sh) = (w / SHRINK, h / SHRINK);
    let mut small = shrink(rgba, w, h, sw, sh);
    let radius = (sw / 64).max(2) as usize;
    for _ in 0..3 {
        box_blur(&mut small, sw as usize, sh as usize, radius);
    }
    let mut pm = Pixmap::new(sw, sh)?;
    pm.data_mut().copy_from_slice(&small);
    Some(Frost { pm, factor: SHRINK })
}

/// Average blocks of `SHRINK` x `SHRINK` pixels.
fn shrink(src: &[u8], w: u32, h: u32, sw: u32, sh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (sw * sh * 4) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for dy in 0..SHRINK {
                for dx in 0..SHRINK {
                    let (px, py) = ((x * SHRINK + dx).min(w - 1), (y * SHRINK + dy).min(h - 1));
                    let i = ((py * w + px) * 4) as usize;
                    for k in 0..4 {
                        acc[k] += src[i + k] as u32;
                    }
                    n += 1;
                }
            }
            let o = ((y * sw + x) * 4) as usize;
            for k in 0..4 {
                out[o + k] = (acc[k] / n) as u8;
            }
        }
    }
    out
}

/// One box blur pass of `radius` pixels in both directions (running sums: the cost does not depend on the radius). The edge pixels repeat.
pub fn box_blur(px: &mut [u8], w: usize, h: usize, radius: usize) {
    let _p = crate::prof::scope("blur.box_blur");
    if w == 0 || h == 0 || radius == 0 || px.len() != w * h * 4 {
        return;
    }
    let mut line = vec![0u8; w.max(h) * 4];
    // rows
    for y in 0..h {
        let row = &mut px[y * w * 4..(y + 1) * w * 4];
        blur_line(row, &mut line[..w * 4], w, 4, radius);
        row.copy_from_slice(&line[..w * 4]);
    }
    // columns, gathered into a line and put back
    let mut col = vec![0u8; h * 4];
    for x in 0..w {
        for y in 0..h {
            let i = (y * w + x) * 4;
            col[y * 4..y * 4 + 4].copy_from_slice(&px[i..i + 4]);
        }
        blur_line(&col, &mut line[..h * 4], h, 4, radius);
        for y in 0..h {
            let i = (y * w + x) * 4;
            px[i..i + 4].copy_from_slice(&line[y * 4..y * 4 + 4]);
        }
    }
}

fn blur_line(src: &[u8], dst: &mut [u8], n: usize, ch: usize, r: usize) {
    let win = (2 * r + 1) as i64;
    for c in 0..ch {
        let at = |i: i64| src[(i.max(0).min(n as i64 - 1) as usize) * ch + c] as i64;
        let mut sum: i64 = (-(r as i64)..=r as i64).map(at).sum();
        for i in 0..n as i64 {
            dst[i as usize * ch + c] = (sum / win) as u8;
            sum += at(i + r as i64 + 1) - at(i - r as i64);
        }
    }
}

#[cfg(test)]
#[path = "blur_tests.rs"]
mod tests;

//! Keep a finished piece of a frame (F-SHELL-64). Drawing something with soft shadows and gradients costs milliseconds, and most frames
//! draw the same thing again over the same pixels. [`draw`] remembers, for a key that names everything the drawing depends on, the pixels
//! of a box before and after the drawing; the next time the same key finds the same pixels under it, it puts the "after" back with one
//! copy. The pixels under the box are part of the test, so the result is always exactly what drawing would have made (nothing here
//! guesses what lies beneath). A different key or different pixels simply draw again and keep the new picture.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::gfx::Gfx;

/// Most pieces kept at once (the lot is dropped when it is exceeded; one home screen needs about twenty).
const MAX_PIECES: usize = 96;

struct Piece {
    bbox: (i32, i32, i32, i32),
    before: Vec<u8>,
    after: Vec<u8>,
}

thread_local! {
    static PIECES: RefCell<HashMap<u64, Piece>> = RefCell::new(HashMap::new());
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static COUNTS: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
}

/// (times a piece was put back, times it was drawn) so far on this thread.
pub fn counts() -> (u32, u32) {
    COUNTS.with(Cell::get)
}

/// Forget every piece (tests).
pub fn clear() {
    PIECES.with(|p| p.borrow_mut().clear());
}

/// Draw with `f` inside `bbox` (the box the drawing can touch), or put back what it drew last time under the same `key` and the same pixels.
pub fn draw(g: &mut Gfx, key: u64, bbox: (i32, i32, i32, i32), f: impl FnOnce(&mut Gfx)) {
    let hit = SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        g.grab_into(bbox, &mut s);
        PIECES.with(|p| {
            let p = p.borrow();
            match p.get(&key) {
                Some(piece) if piece.bbox == bbox && piece.before == *s => {
                    g.put(bbox, &piece.after);
                    true
                }
                _ => false,
            }
        })
    });
    if hit {
        COUNTS.with(|c| c.set((c.get().0 + 1, c.get().1)));
        return;
    }
    let before = SCRATCH.with(|s| s.borrow().clone());
    f(g);
    let after = g.grab(bbox);
    PIECES.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() >= MAX_PIECES {
            p.clear();
        }
        p.insert(key, Piece { bbox, before, after });
    });
    COUNTS.with(|c| c.set((c.get().0, c.get().1 + 1)));
}

/// A key from the things a drawing depends on.
pub fn key(parts: &[u64]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piece_comes_back_by_copy_only_for_the_same_key_the_same_box_and_the_same_pixels_under_it() {
        clear();
        let mut g = Gfx::new(200, 100);
        let paint = |g: &mut Gfx| {
            g.rrect(20.0, 20.0, 100.0, 40.0, 10.0, [200, 40, 40, 128]);
        };
        let bbox = (10, 10, 130, 60);
        g.clear([30, 30, 30, 255]);
        let c0 = counts();
        draw(&mut g, 7, bbox, paint);
        let first = g.rgba().to_vec();
        g.clear([30, 30, 30, 255]);
        draw(&mut g, 7, bbox, |_| panic!("the same key over the same pixels must not draw"));
        assert_eq!(g.rgba(), &first[..]);
        assert_eq!((counts().0 - c0.0, counts().1 - c0.1), (1, 1));
        // other pixels under it: drawn again, and the result is what drawing gives
        g.clear([90, 30, 30, 255]);
        let mut ran = false;
        draw(&mut g, 7, bbox, |g| {
            ran = true;
            paint(g);
        });
        assert!(ran);
        let mut fresh = Gfx::new(200, 100);
        fresh.clear([90, 30, 30, 255]);
        paint(&mut fresh);
        assert_eq!(g.rgba(), fresh.rgba());
        // another key or another box: drawn
        let mut again = 0;
        g.clear([90, 30, 30, 255]);
        draw(&mut g, 8, bbox, |g| {
            again += 1;
            paint(g);
        });
        g.clear([90, 30, 30, 255]);
        draw(&mut g, 8, (10, 10, 131, 60), |g| {
            again += 1;
            paint(g);
        });
        assert_eq!(again, 2);
        assert_ne!(key(&[1, 2]), key(&[2, 1]));
    }
}

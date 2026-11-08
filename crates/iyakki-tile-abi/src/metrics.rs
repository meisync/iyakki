//! Text metrics of the built-in reference font. Pure integer arithmetic, so every host (device,
//! browser, test) gets the same numbers. A real font reaches a tile only through the host's
//! `measure_text` import, which may use richer tables; the guest never assumes either.

/// Advance of one character, in pixels, for a text size in pixels (6/7 of the size, at least 1).
pub fn advance(_face: u8, size: u32) -> u32 {
    (size.saturating_mul(6) / 7).max(1)
}

/// Width in pixels of `text` (one advance per Unicode scalar).
pub fn measure_text(face: u8, size: u32, text: &str) -> i32 {
    let n = text.chars().count() as u64;
    (n * advance(face, size) as u64).min(i32::MAX as u64) as i32
}

/// Height of a line of text of this size (size + size/4).
pub fn line_height(size: u32) -> i32 {
    (size + size / 4).min(i32::MAX as u32) as i32
}

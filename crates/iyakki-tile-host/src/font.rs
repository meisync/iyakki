//! The tile font table (F-WASM-13): real glyph shapes from the DejaVu fonts the shell bundles (Bitstream Vera licence with
//! public-domain changes, `crates/iyakki-shell/assets/DEJAVU-LICENSE.txt`; the table is a derived work and carries that notice),
//! drawn at the ABI's fixed advance (`6 * size / 7`). The table is made by `dist/tilefont/gen.py`, which also writes the
//! browser renderer's copy, so the two renderers read the same bytes. Format: see that script.
use iyakki_tile_abi::measure_text;

static TABLE: &[u8] = include_bytes!("tilefont.bin");

/// Printable ASCII U+0020..U+007E then Latin-1 U+00A0..U+00FF.
const GLYPHS: usize = 95 + 96;

fn index_of(c: char) -> usize {
    let u = c as u32;
    match u {
        0x20..=0x7E => (u - 0x20) as usize,
        0xA0..=0xFF => 95 + (u - 0xA0) as usize,
        _ => (b'?' - 0x20) as usize,
    }
}

/// The glyph table of one face at one listed size.
pub struct Cell {
    /// The listed size this cell was made for.
    pub size: i32,
    /// Its advance in pixels.
    pub adv: i32,
    bits: &'static [u8],
}

impl Cell {
    /// Is the pixel (x, y) of the glyph for `c` set? Outside the cell: false.
    pub fn set(&self, c: char, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.adv || y >= self.size {
            return false;
        }
        let per = ((self.adv * self.size + 7) / 8) as usize;
        let at = index_of(c) * per;
        let n = (y * self.adv + x) as usize;
        self.bits.get(at + n / 8).is_some_and(|b| (b >> (7 - n % 8)) & 1 == 1)
    }
}

/// The cell for `face` (0 sans, 1 mono, 2 bold; anything else is 0) nearest to `size` (a tie goes to the larger).
pub fn cell(face: u8, size: u32) -> Option<Cell> {
    let t = TABLE;
    if t.len() < 6 || &t[..4] != b"IYTF" {
        return None;
    }
    let (faces, nsizes) = (t[4] as usize, t[5] as usize);
    let sizes = t.get(6..6 + nsizes)?;
    let face = if (face as usize) < faces { face as usize } else { 0 };
    let size_ix = (0..nsizes).min_by_key(|&i| ((sizes[i] as i64 - size as i64).abs(), -(sizes[i] as i64)))?;
    let mut off = 6 + nsizes;
    let bytes_of = |s: i32| -> usize { ((advance(s) * s + 7) / 8) as usize * GLYPHS };
    for f in 0..faces {
        for (i, &s) in sizes.iter().enumerate() {
            let len = bytes_of(s as i32);
            if f == face && i == size_ix {
                return Some(Cell { size: s as i32, adv: advance(s as i32), bits: t.get(off..off + len)? });
            }
            off += len;
        }
    }
    None
}

fn advance(size: i32) -> i32 {
    measure_text(0, size as u32, "a")
}

/// The sizes the table holds.
pub fn listed_sizes() -> Vec<u32> {
    let n = TABLE.get(5).copied().unwrap_or(0) as usize;
    TABLE.get(6..6 + n).map(|s| s.iter().map(|&x| x as u32).collect()).unwrap_or_default()
}

/// Is pixel (dx, dy) of the cell `adv` x `size` set for `c`? The listed glyph is scaled to the cell by whole-number
/// arithmetic, the same sums as `tilerender.js`.
pub fn pixel(cell: &Cell, c: char, dx: i32, dy: i32, adv: i32, size: i32) -> bool {
    let sx = (dx as i64 * cell.adv as i64 / adv as i64) as i32;
    let sy = (dy as i64 * cell.size as i64 / size as i64) as i32;
    cell.set(c, sx, sy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render;
    use iyakki_tile_abi::{Color, DisplayList, Op};

    fn ink(face: u8, size: u32, text: &str) -> (Vec<u8>, i32, i32) {
        let mut l = DisplayList::new();
        l.push(Op::Text { x: 1, y: 1, face, size, color: Color(0, 0, 0, 255), text: text.into() });
        let w = iyakki_tile_abi::measure_text(face, size, text) + 2;
        let h = size as i32 + 2;
        let img = render(&l, w, h, Color(255, 255, 255, 255));
        (img.rgba.chunks(4).map(|p| p[0]).collect(), w, h)
    }

    #[test]
    fn the_table_holds_three_faces_at_six_sizes_with_the_abis_advance() {
        assert_eq!(listed_sizes(), vec![8, 10, 12, 16, 22, 32]);
        for face in 0..3u8 {
            for s in listed_sizes() {
                let c = cell(face, s).expect("cell");
                assert_eq!((c.size, c.adv), (s as i32, measure_text(face, s, "a")));
            }
        }
        // the nearest listed size, the larger on a tie, the first face for an unknown one
        assert_eq!(cell(0, 9).unwrap().size, 10);
        assert_eq!(cell(0, 14).unwrap().size, 16);
        assert_eq!(cell(0, 400).unwrap().size, 32);
        assert_eq!(cell(0, 1).unwrap().size, 8);
        assert!(cell(77, 12).unwrap().set('A', 3, 6) == cell(0, 12).unwrap().set('A', 3, 6));
    }

    #[test]
    fn real_shapes_lower_case_is_not_upper_case_and_the_three_faces_differ() {
        let (a, ..) = ink(0, 16, "g");
        let (b, ..) = ink(0, 16, "G");
        assert_ne!(a, b, "a lower case letter has its own shape");
        let (sans, ..) = ink(0, 16, "Hamburg");
        let (mono, ..) = ink(1, 16, "Hamburg");
        let (bold, ..) = ink(2, 16, "Hamburg");
        assert!(sans != mono && sans != bold && mono != bold);
        let dark = |v: &[u8]| v.iter().filter(|p| **p < 128).count();
        assert!(dark(&bold) > dark(&sans), "bold has more ink: {} vs {}", dark(&bold), dark(&sans));
        // Latin-1 letters are real glyphs, not the question mark; a character outside the table is the question mark
        let (e, ..) = ink(1, 16, "\u{e9}");
        let (q, ..) = ink(1, 16, "?");
        assert_ne!(e, q);
        let (u, ..) = ink(1, 16, "\u{65e5}");
        assert_eq!(u, q);
    }

    #[test]
    fn every_glyph_has_ink_inside_its_cell_except_the_spaces_and_the_soft_hyphen_and_nothing_leaks_out_of_the_cell() {
        for face in 0..3u8 {
            for s in listed_sizes() {
                let c = cell(face, s).unwrap();
                for cp in (0x21u32..0x7f).chain(0xa1..0x100).filter(|c| *c != 0xad) {
                    let ch = char::from_u32(cp).unwrap();
                    let n = (0..c.size).flat_map(|y| (0..c.adv).map(move |x| (x, y))).filter(|&(x, y)| c.set(ch, x, y)).count();
                    assert!(n > 0, "face {face} size {s}: {ch:?} has no ink");
                    assert!(n < (c.size * c.adv) as usize, "face {face} size {s}: {ch:?} is a solid block");
                }
                assert!((0..c.size).all(|y| (0..c.adv).all(|x| !c.set(' ', x, y))));
                assert!(!c.set('A', c.adv, 0) && !c.set('A', 0, c.size) && !c.set('A', -1, 0));
            }
        }
    }

    #[test]
    fn a_size_that_is_not_listed_scales_the_nearest_cell_by_whole_number_sums() {
        // size 14 is drawn from the 16 cell: the picture has the cell's size and the glyph is not empty or flooded
        let (px, w, h) = ink(1, 14, "Wg");
        assert_eq!((w, h), (2 * 12 + 2, 16));
        let dark = px.iter().filter(|p| **p < 128).count();
        assert!(dark > 20 && dark < (w * h / 2) as usize, "{dark}");
        // a listed size is a one-to-one copy of the table
        let c = cell(1, 16).unwrap();
        let (px, w, _) = ink(1, 16, "A");
        for y in 0..16 {
            for x in 0..c.adv {
                assert_eq!(px[((y + 1) * w + x + 1) as usize] < 128, c.set('A', x, y), "({x},{y})");
            }
        }
    }

    /// The table is what `dist/tilefont/gen.py` makes from the bundled fonts (both the binary and the browser copy). Skipped when python
    /// with Pillow is not on this machine.
    #[test]
    fn the_checked_in_table_is_what_the_generator_makes_from_the_fonts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let ok = std::process::Command::new("python3").arg("-c").arg("import PIL.ImageFont").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            eprintln!("SKIP: python3 with Pillow is not available (the table was not regenerated)");
            return;
        }
        let out = std::process::Command::new("python3").arg(root.join("dist/tilefont/gen.py")).arg("--check").output().expect("run gen.py");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    }
}

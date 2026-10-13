//! Text in scripts that need more than one glyph per letter: Hindi and the other Indic scripts (letters join and reorder),
//! Arabic (letters take their joined forms and run right to left) and Hebrew. English and the other Latin text never comes
//! here; it is drawn letter by letter as before. The shaping itself is rustybuzz (a Rust port of HarfBuzz, MIT), the
//! left/right ordering is the Unicode bidirectional algorithm (`unicode-bidi`), the glyphs come from bundled Noto Sans fonts.

use fontdue::{Font, FontSettings};
use rustybuzz::{Direction, UnicodeBuffer};
use unicode_bidi::BidiInfo;

/// One glyph to draw: which font, which glyph, and where relative to the start of the line (in pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub font: usize,
    pub glyph: u16,
    pub x: f32,
    pub y: f32,
}

struct Entry {
    font: Font,
    face: rustybuzz::Face<'static>,
}

pub struct Shaper {
    fonts: Vec<Entry>,
}

/// (first code point, last code point, index into the font list); index 0 is the Latin font.
const RANGES: &[(u32, u32, usize)] = &[
    (0x0590, 0x05FF, 10), // Hebrew
    (0x0600, 0x06FF, 9),  // Arabic
    (0x0750, 0x077F, 9),
    (0x08A0, 0x08FF, 9),
    (0xFB1D, 0xFB4F, 10),
    (0xFB50, 0xFDFF, 9),
    (0xFE70, 0xFEFF, 9),
    (0x0900, 0x097F, 1), // Devanagari
    (0xA8E0, 0xA8FF, 1),
    (0x0980, 0x09FF, 2), // Bengali
    (0x0A00, 0x0A7F, 3), // Gurmukhi
    (0x0A80, 0x0AFF, 4), // Gujarati
    (0x0B80, 0x0BFF, 5), // Tamil
    (0x0C00, 0x0C7F, 6), // Telugu
    (0x0C80, 0x0CFF, 7), // Kannada
    (0x0D00, 0x0D7F, 8), // Malayalam
];

fn bytes(i: usize) -> &'static [u8] {
    match i {
        0 => include_bytes!("../assets/DejaVuSans.ttf"),
        1 => include_bytes!("../assets/NotoSansDevanagari-Regular.ttf"),
        2 => include_bytes!("../assets/NotoSansBengali-Regular.ttf"),
        3 => include_bytes!("../assets/NotoSansGurmukhi-Regular.ttf"),
        4 => include_bytes!("../assets/NotoSansGujarati-Regular.ttf"),
        5 => include_bytes!("../assets/NotoSansTamil-Regular.ttf"),
        6 => include_bytes!("../assets/NotoSansTelugu-Regular.ttf"),
        7 => include_bytes!("../assets/NotoSansKannada-Regular.ttf"),
        8 => include_bytes!("../assets/NotoSansMalayalam-Regular.ttf"),
        9 => include_bytes!("../assets/NotoSansArabic-Regular.ttf"),
        _ => include_bytes!("../assets/NotoSansHebrew-Regular.ttf"),
    }
}

const FONT_COUNT: usize = 11;

/// The font a letter belongs to, or `None` for letters that take the font of their neighbours (spaces, digits,
/// punctuation, the joiners).
fn script_of(c: char) -> Option<usize> {
    let u = c as u32;
    if let Some(&(_, _, i)) = RANGES.iter().find(|&&(a, b, _)| u >= a && u <= b) {
        return Some(i);
    }
    if c.is_alphabetic() {
        Some(0)
    } else {
        None
    }
}

/// True when `s` has a letter that must be shaped or ordered right to left.
pub fn needs_shaping(s: &str) -> bool {
    s.chars().any(|c| (c as u32) >= 0x0590 && matches!(script_of(c), Some(i) if i != 0))
}

impl Default for Shaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Shaper {
    pub fn new() -> Shaper {
        let fonts = (0..FONT_COUNT)
            .map(|i| Entry {
                font: Font::from_bytes(bytes(i), FontSettings::default()).expect("bundled font is valid"),
                face: rustybuzz::Face::from_slice(bytes(i), 0).expect("bundled font is valid"),
            })
            .collect();
        Shaper { fonts }
    }

    pub fn font(&self, i: usize) -> &Font {
        &self.fonts[i].font
    }

    /// The glyphs of `s` at `px` pixels in the order they appear on screen, and the total advance.
    pub fn layout(&self, s: &str, px: f32) -> (Vec<Placed>, f32) {
        let s: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        if s.is_empty() {
            return (Vec::new(), 0.0);
        }
        let bidi = BidiInfo::new(&s, None);
        let mut out = Vec::new();
        let mut pen = 0.0f32;
        for para in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(para, para.range.clone());
            for run in runs {
                let rtl = levels[run.start].is_rtl();
                let mut pieces = self.split_by_font(&s[run.clone()]);
                if rtl {
                    pieces.reverse();
                }
                for (font, text) in pieces {
                    pen = self.shape_piece(font, text, px, rtl, pen, &mut out);
                }
            }
        }
        (out, pen)
    }

    /// Cut a same-direction run into pieces that each use one font.
    fn split_by_font<'a>(&self, text: &'a str) -> Vec<(usize, &'a str)> {
        let mut pieces: Vec<(usize, usize, usize)> = Vec::new(); // font, start, end
        let mut current = text.chars().find_map(script_of).unwrap_or(0);
        for (i, c) in text.char_indices() {
            let mut f = script_of(c).unwrap_or(current);
            // punctuation takes its neighbours' font only when that font has it
            if script_of(c).is_none() && self.fonts[f].face.glyph_index(c).is_none() {
                f = 0;
            }
            let end = i + c.len_utf8();
            match pieces.last_mut() {
                Some(last) if last.0 == f => last.2 = end,
                _ => pieces.push((f, i, end)),
            }
            if script_of(c).is_some() {
                current = f;
            }
        }
        pieces.into_iter().map(|(f, a, b)| (f, &text[a..b])).collect()
    }

    fn shape_piece(&self, font: usize, text: &str, px: f32, rtl: bool, mut pen: f32, out: &mut Vec<Placed>) -> f32 {
        let face = &self.fonts[font].face;
        let mut buf = UnicodeBuffer::new();
        buf.push_str(text);
        buf.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
        buf.guess_segment_properties();
        let shaped = rustybuzz::shape(face, &[], buf);
        let scale = px / face.units_per_em() as f32;
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            out.push(Placed { font, glyph: info.glyph_id as u16, x: pen + pos.x_offset as f32 * scale, y: pos.y_offset as f32 * scale });
            pen += pos.x_advance as f32 * scale;
        }
        pen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_scripts_that_join_or_run_right_to_left_are_shaped() {
        assert!(!needs_shaping("Hello, world 123 — ñandú"));
        assert!(needs_shaping("नमस्ते"));
        assert!(needs_shaping("hello مرحبا"));
        assert!(needs_shaping("שלום"));
        assert!(needs_shaping("தமிழ்"));
    }

    #[test]
    fn hindi_letters_join_into_fewer_glyphs_than_code_points_and_have_width() {
        let sh = Shaper::new();
        let word = "हिन्दी";
        let (g, w) = sh.layout(word, 20.0);
        assert!(w > 10.0, "width {w}");
        assert!(!g.is_empty() && g.len() < word.chars().count(), "{} glyphs for {} letters", g.len(), word.chars().count());
        assert!(g.iter().all(|p| p.font == 1 && p.glyph != 0), "every glyph is real and from the Devanagari font");
    }

    #[test]
    fn tamil_bengali_and_the_other_indic_scripts_all_find_their_glyphs() {
        let sh = Shaper::new();
        for (text, font) in [("தமிழ்", 5), ("বাংলা", 2), ("తెలుగు", 6), ("ಕನ್ನಡ", 7), ("മലയാളം", 8), ("ગુજરાતી", 4), ("ਪੰਜਾਬੀ", 3)] {
            let (g, w) = sh.layout(text, 18.0);
            assert!(w > 5.0, "{text}");
            assert!(g.iter().all(|p| p.font == font && p.glyph != 0), "{text}: missing glyphs");
        }
    }

    #[test]
    fn arabic_is_drawn_right_to_left_with_joined_forms() {
        let sh = Shaper::new();
        let (g, w) = sh.layout("مرحبا", 20.0);
        assert!(w > 10.0 && !g.is_empty());
        assert!(g.iter().all(|p| p.font == 9 && p.glyph != 0));
        // pen positions grow left to right on screen, and the first letter of the word is at the right end
        assert!(g.windows(2).all(|p| p[0].x <= p[1].x));
        let (first_letter_alone, _) = sh.layout("م", 20.0);
        let (last_glyph, _) = (g.last().unwrap().glyph, ());
        assert_ne!(first_letter_alone[0].glyph, last_glyph, "joined forms differ from the stand-alone letter's form at the line start");
    }

    #[test]
    fn english_inside_an_arabic_line_keeps_its_left_to_right_order() {
        let sh = Shaper::new();
        let (g, _) = sh.layout("مرحبا abc", 20.0);
        let latin: Vec<_> = g.iter().filter(|p| p.font == 0).collect();
        assert_eq!(latin.len(), 3);
        assert!(latin.windows(2).all(|p| p[0].x < p[1].x));
        // the whole line is right to left, so the Latin word sits on the left of the Arabic one
        let arabic_x = g.iter().filter(|p| p.font == 9).map(|p| p.x).fold(f32::MAX, f32::min);
        assert!(latin.iter().all(|p| p.x < arabic_x));
    }

    #[test]
    fn punctuation_a_script_font_lacks_comes_from_the_latin_font() {
        let sh = Shaper::new();
        let (g, _) = sh.layout("שלום: עולם", 16.0);
        assert!(g.iter().all(|p| p.glyph != 0), "no empty boxes");
    }

    #[test]
    fn mixed_english_and_hindi_use_the_right_font_for_each_part() {
        let sh = Shaper::new();
        let (g, _) = sh.layout("Hello नमस्ते 42", 16.0);
        assert!(g.iter().any(|p| p.font == 0) && g.iter().any(|p| p.font == 1));
        assert!(g.iter().all(|p| p.glyph != 0));
        assert!(g.windows(2).all(|p| p[0].x <= p[1].x), "left to right");
    }
}

//! Text that needs shaping or right-to-left layout in the writer (F-PDF-15): Devanagari, Bengali, Gurmukhi, Gujarati, Tamil,
//! Telugu, Kannada, Malayalam, Thai, Arabic and Hebrew, drawn with the Noto Sans fonts the shell already carries (SIL OFL-1.1,
//! see `crates/iyakki-shell/assets/NOTO-OFL-LICENSE.txt`).
//!
//! Glyph choice is rustybuzz (the same MIT shaper the UI uses, `iyakki-uikit` shaping); the order of letters on a line is the
//! Unicode bidirectional algorithm (`unicode-bidi`). A line is shaped as one piece per direction run and font, every glyph is
//! then written at its own position (`Td`), so what the shaper decided is exactly what the page shows. Each cluster (the glyphs
//! that came from one group of letters) is wrapped in marked content with `/ActualText` holding the letters in logical order, so
//! copying and searching give the text the person typed, not the glyph order.
//!
//! Limits: Regular weight only (headings are not bold in these scripts); no vertical metrics tuning; a word wider than the line is
//! cut before a letter that is not a mark, which can separate a rare long cluster. Chinese, Japanese and Korean need a very large
//! font and are not drawn (they are refused with the script named).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use rustybuzz::{Direction, UnicodeBuffer};
use unicode_bidi::{BidiInfo, Level};

use crate::ttf::Ttf;

/// Font list: 0 Latin (DejaVu Sans), 1 Devanagari, 2 Bengali, 3 Gurmukhi, 4 Gujarati, 5 Tamil, 6 Telugu, 7 Kannada, 8 Malayalam,
/// 9 Arabic, 10 Hebrew, 11 Thai.
pub const FONT_COUNT: usize = 12;
pub const NAMES: [&str; FONT_COUNT] = ["DejaVuSans", "NotoSansDevanagari", "NotoSansBengali", "NotoSansGurmukhi", "NotoSansGujarati", "NotoSansTamil", "NotoSansTelugu", "NotoSansKannada", "NotoSansMalayalam", "NotoSansArabic", "NotoSansHebrew", "NotoSansThai"];

pub fn bytes(i: usize) -> &'static [u8] {
    match i {
        0 => include_bytes!("../../iyakki-shell/assets/DejaVuSans.ttf"),
        1 => include_bytes!("../../iyakki-shell/assets/NotoSansDevanagari-Regular.ttf"),
        2 => include_bytes!("../../iyakki-shell/assets/NotoSansBengali-Regular.ttf"),
        3 => include_bytes!("../../iyakki-shell/assets/NotoSansGurmukhi-Regular.ttf"),
        4 => include_bytes!("../../iyakki-shell/assets/NotoSansGujarati-Regular.ttf"),
        5 => include_bytes!("../../iyakki-shell/assets/NotoSansTamil-Regular.ttf"),
        6 => include_bytes!("../../iyakki-shell/assets/NotoSansTelugu-Regular.ttf"),
        7 => include_bytes!("../../iyakki-shell/assets/NotoSansKannada-Regular.ttf"),
        8 => include_bytes!("../../iyakki-shell/assets/NotoSansMalayalam-Regular.ttf"),
        9 => include_bytes!("../../iyakki-shell/assets/NotoSansArabic-Regular.ttf"),
        10 => include_bytes!("../../iyakki-shell/assets/NotoSansHebrew-Regular.ttf"),
        _ => include_bytes!("../../iyakki-shell/assets/NotoSansThai-Regular.ttf"),
    }
}

const RANGES: &[(u32, u32, usize)] = &[
    (0x0590, 0x05FF, 10),
    (0x0600, 0x06FF, 9),
    (0x0750, 0x077F, 9),
    (0x08A0, 0x08FF, 9),
    (0xFB1D, 0xFB4F, 10),
    (0xFB50, 0xFDFF, 9),
    (0xFE70, 0xFEFF, 9),
    (0x0900, 0x097F, 1),
    (0xA8E0, 0xA8FF, 1),
    (0x0980, 0x09FF, 2),
    (0x0A00, 0x0A7F, 3),
    (0x0A80, 0x0AFF, 4),
    (0x0B80, 0x0BFF, 5),
    (0x0C00, 0x0C7F, 6),
    (0x0C80, 0x0CFF, 7),
    (0x0D00, 0x0D7F, 8),
    (0x0E00, 0x0E7F, 11),
];

fn script_font(c: char) -> Option<usize> {
    let u = c as u32;
    RANGES.iter().find(|&&(a, b, _)| u >= a && u <= b).map(|r| r.2)
}

/// Does the text hold a letter that needs this module (shaping or right to left)?
pub fn needs(text: &str) -> bool {
    text.chars().any(|c| script_font(c).is_some())
}

struct Loaded {
    face: rustybuzz::Face<'static>,
}

fn loaded() -> &'static Vec<Loaded> {
    static L: OnceLock<Vec<Loaded>> = OnceLock::new();
    L.get_or_init(|| (0..FONT_COUNT).map(|i| Loaded { face: rustybuzz::Face::from_slice(bytes(i), 0).expect("bundled font is valid") }).collect())
}

fn ttfs() -> &'static Vec<Option<Ttf<'static>>> {
    static T: OnceLock<Vec<Option<Ttf<'static>>>> = OnceLock::new();
    T.get_or_init(|| (0..FONT_COUNT).map(|i| Ttf::parse(bytes(i))).collect())
}

/// The font program (for subsetting) of font `i`.
pub fn ttf(i: usize) -> Option<&'static Ttf<'static>> {
    ttfs().get(i)?.as_ref()
}

fn has_glyph(font: usize, c: char) -> bool {
    loaded()[font].face.glyph_index(c).is_some()
}

/// Can some bundled font draw `c`? (Formatting characters such as the joiners are always kept.)
pub fn can_draw(c: char) -> bool {
    matches!(c as u32, 0x200C | 0x200D | 0x200E | 0x200F | 0x061C) || (!c.is_control() && (0..FONT_COUNT).any(|f| has_glyph(f, c)))
}

/// One cluster: the glyphs that came from one group of letters, and those letters in logical order.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub text: String,
    /// (font, glyph, x, y) in points from the start of the line (x) and from the baseline (y).
    pub glyphs: Vec<(usize, u16, f64, f64)>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Line {
    /// Visual order, left to right.
    pub clusters: Vec<Cluster>,
    pub width: f64,
    /// The paragraph runs right to left: the line is set against the right margin.
    pub rtl: bool,
    /// The letters of the line in logical order (what the person typed), for copying and searching.
    pub text: String,
}

/// The font a letter takes: its script's, else the font of its neighbours when that font has it, else Latin.
fn split_by_font(text: &str) -> Vec<(usize, &str)> {
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
    let mut current = text.chars().find_map(|c| script_font(c).or(if c.is_alphabetic() { Some(0) } else { None })).unwrap_or(0);
    for (i, c) in text.char_indices() {
        let own = script_font(c).or(if c.is_alphabetic() { Some(0) } else { None });
        let mut f = own.unwrap_or(current);
        if own.is_none() && !has_glyph(f, c) {
            f = 0;
        }
        let end = i + c.len_utf8();
        match pieces.last_mut() {
            Some(l) if l.0 == f => l.2 = end,
            _ => pieces.push((f, i, end)),
        }
        if own.is_some() {
            current = f;
        }
    }
    pieces.into_iter().map(|(f, a, b)| (f, &text[a..b])).collect()
}

fn shape_piece(font: usize, text: &str, size: f64, rtl: bool, pen: &mut f64, out: &mut Vec<Cluster>) {
    let face = &loaded()[font].face;
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    buf.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
    buf.guess_segment_properties();
    let shaped = rustybuzz::shape(face, &[], buf);
    let scale = size / face.units_per_em() as f64;
    let starts: BTreeSet<u32> = shaped.glyph_infos().iter().map(|g| g.cluster).collect();
    let text_of = |c: u32| -> String {
        let end = starts.range(c + 1..).next().copied().unwrap_or(text.len() as u32);
        text.get(c as usize..end as usize).unwrap_or("").to_string()
    };
    let mut last: Option<u32> = None;
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        if last != Some(info.cluster) {
            out.push(Cluster { text: text_of(info.cluster), glyphs: Vec::new() });
            last = Some(info.cluster);
        }
        let cl = out.last_mut().expect("a cluster was just pushed");
        cl.glyphs.push((font, info.glyph_id as u16, *pen + pos.x_offset as f64 * scale, pos.y_offset as f64 * scale));
        *pen += pos.x_advance as f64 * scale;
    }
}

/// Shape one line (no line breaks inside) at `size` points, in the paragraph direction `para`.
pub fn shape_line(text: &str, size: f64, para: Level) -> Line {
    let mut line = Line { rtl: para.is_rtl(), text: text.to_string(), ..Default::default() };
    if text.is_empty() {
        return line;
    }
    let bidi = BidiInfo::new(text, Some(para));
    let mut pen = 0.0;
    for p in &bidi.paragraphs {
        let (levels, runs) = bidi.visual_runs(p, p.range.clone());
        for run in runs {
            let rtl = levels[run.start].is_rtl();
            let mut pieces = split_by_font(&text[run.clone()]);
            if rtl {
                pieces.reverse();
            }
            for (font, t) in pieces {
                shape_piece(font, t, size, rtl, &mut pen, &mut line.clusters);
            }
        }
    }
    line.width = pen;
    line
}

fn clean(text: &str, replaced: &mut usize) -> String {
    text.chars()
        .filter(|c| *c != '\r' && *c != '\u{200b}' && *c != '\u{feff}')
        .map(|c| if c == '\t' { ' ' } else { c })
        .map(|c| {
            if c == '\n' || c == ' ' || can_draw(c) {
                c
            } else {
                *replaced += 1;
                '?'
            }
        })
        .collect()
}

/// Break `text` into lines no wider than `max` points. Paragraphs are separated by `\n`; each takes the direction of its first
/// strong letter (so a Hebrew paragraph is set against the right margin).
pub fn layout(text: &str, size: f64, max: f64, replaced: &mut usize) -> Vec<Line> {
    let text = clean(text, replaced);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let level = BidiInfo::new(para, None).paragraphs.first().map(|p| p.level).unwrap_or_else(Level::ltr);
        let measure = |s: &str| shape_line(s, size, level).width;
        let mut cur = String::new();
        // units a line may end between: the words between spaces, and inside Thai words the pieces the dictionary finds
        let mut units: Vec<(String, bool)> = Vec::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            if word.chars().any(crate::thai::is_thai) {
                for (k, piece) in crate::thai::pieces(word).into_iter().enumerate() {
                    units.push((piece, k == 0));
                }
            } else {
                units.push((word.to_string(), true));
            }
        }
        for (word, spaced) in units {
            let mut w = word;
            let mut spaced = spaced;
            loop {
                let cand = if cur.is_empty() {
                    w.clone()
                } else if spaced {
                    format!("{cur} {w}")
                } else {
                    format!("{cur}{w}")
                };
                if measure(&cand) <= max || (cur.is_empty() && w.chars().count() <= 1) {
                    cur = cand;
                    break;
                }
                if cur.is_empty() {
                    // a word wider than the line: cut it before a letter that is not a mark
                    let chars: Vec<(usize, char)> = w.char_indices().collect();
                    let mut cut = chars.len();
                    for k in (1..chars.len()).rev() {
                        let (at, c) = chars[k];
                        let prev = chars[k - 1].1;
                        if c.is_alphanumeric() && prev.is_alphanumeric() && measure(&w[..at]) <= max {
                            cut = k;
                            break;
                        }
                    }
                    let at = if cut == chars.len() { chars[1.min(chars.len() - 1)].0 } else { chars[cut].0 };
                    lines.push(shape_line(&w[..at], size, level));
                    w = w[at..].to_string();
                    if w.is_empty() {
                        break;
                    }
                } else {
                    lines.push(shape_line(&std::mem::take(&mut cur), size, level));
                    spaced = true;
                }
            }
        }
        lines.push(shape_line(&cur, size, level));
    }
    while lines.len() > 1 && lines.last().is_some_and(|l| l.clusters.is_empty()) {
        lines.pop();
    }
    lines
}

/// `/ActualText` payload: UTF-16BE with a byte order mark, as a hex string.
fn actual_text(s: &str) -> String {
    let mut h = String::from("<FEFF");
    for u in s.encode_utf16() {
        h.push_str(&format!("{u:04X}"));
    }
    h.push('>');
    h
}

/// The content-stream text of a line whose left end is at `x0` and baseline at `y`. `used` collects the glyphs per font for the
/// subsets: glyph number to the single letter it stands for (`'\0'` when a cluster has several glyphs).
pub fn draw(line: &Line, size: f64, x0: f64, y: f64, used: &mut BTreeMap<usize, BTreeMap<u16, char>>) -> String {
    draw_weight(line, size, x0, y, used, false)
}

/// As [`draw`]; with `bold` the letters are filled and stroked with a thin line (the bundled fonts have one weight, so the heavier
/// weight is made, as word processors do when a font has no bold face).
pub fn draw_weight(line: &Line, size: f64, x0: f64, y: f64, used: &mut BTreeMap<usize, BTreeMap<u16, char>>, bold: bool) -> String {
    let mut out = String::new();
    if bold {
        out.push_str(&format!("q 2 Tr {:.3} w\n", size * 0.028));
    }
    for cl in &line.clusters {
        let one = {
            let mut it = cl.text.chars();
            match (it.next(), it.next()) {
                (Some(c), None) if cl.glyphs.len() == 1 => c,
                _ => '\0',
            }
        };
        out.push_str(&format!("/Span << /ActualText {} >> BDC\n", actual_text(&cl.text)));
        for &(font, gid, x, dy) in &cl.glyphs {
            used.entry(font).or_default().entry(gid).or_insert(one);
            out.push_str(&format!("BT /S{font} {size:.1} Tf {:.2} {:.2} Td <{gid:04X}> Tj ET\n", x0 + x, y + dy));
        }
        out.push_str("EMC\n");
    }
    if bold {
        out.push_str("Q\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(l: &Line) -> usize {
        l.clusters.iter().map(|c| c.glyphs.len()).sum()
    }

    #[test]
    fn only_these_scripts_are_taken() {
        assert!(!needs("Hello, world 123 ñandú Ελληνικά Привет"));
        for s in ["नमस्ते", "தமிழ்", "ไทย", "שלום", "مرحبا", "বাংলা", "తెలుగు", "ಕನ್ನಡ", "മലയാളം", "ગુજરાતી", "ਪੰਜਾਬੀ"] {
            assert!(needs(s), "{s}");
        }
        assert!(!needs("日本語 中文 한국어"));
    }

    #[test]
    fn every_font_parses_for_the_subsetter() {
        for i in 0..FONT_COUNT {
            assert!(ttf(i).is_some(), "{} cannot be subset", NAMES[i]);
        }
    }

    #[test]
    fn hindi_joins_letters_and_tamil_reorders() {
        let l = shape_line("हिन्दी", 20.0, Level::ltr());
        assert!(l.width > 10.0 && glyphs(&l) < "हिन्दी".chars().count());
        assert!(l.clusters.iter().flat_map(|c| &c.glyphs).all(|g| g.0 == 1 && g.1 != 0));
        // the clusters give the letters back in logical order
        let back: String = l.clusters.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(back.chars().count(), 6);
        // Tamil: the vowel sign கெ (U+0B95 U+0BC6) is drawn BEFORE the consonant, so the glyph order differs from the letter order
        let t = shape_line("கெ", 20.0, Level::ltr());
        assert!(t.clusters.iter().flat_map(|c| &c.glyphs).all(|g| g.1 != 0));
        assert!(glyphs(&t) >= 2);
    }

    #[test]
    fn thai_marks_stack_over_their_letters_without_advance() {
        let l = shape_line("ที่", 20.0, Level::ltr());
        assert!(glyphs(&l) >= 2 && l.clusters.iter().flat_map(|c| &c.glyphs).all(|g| g.1 != 0));
        let pen_after_first = l.clusters.iter().flat_map(|c| &c.glyphs).map(|g| g.2).collect::<Vec<_>>();
        assert!(pen_after_first.windows(2).any(|w| (w[1] - w[0]).abs() < 8.0), "marks sit near their letter");
    }

    #[test]
    fn hebrew_and_arabic_run_right_to_left_and_digits_stay_left_to_right() {
        let h = shape_line("שלום", 20.0, Level::rtl());
        assert!(h.rtl && h.width > 10.0);
        // visual order is the reverse of the typed order: the first typed letter (shin) is the last cluster
        assert_eq!(h.clusters.first().unwrap().text, "ם");
        assert_eq!(h.clusters.last().unwrap().text, "ש");
        let mixed = shape_line("שלום 123", 20.0, Level::rtl());
        let digits: Vec<&str> = mixed.clusters.iter().map(|c| c.text.as_str()).filter(|t| t.chars().all(|c| c.is_ascii_digit())).collect();
        assert_eq!(digits, vec!["1", "2", "3"], "numbers read left to right");
        let a = shape_line("مرحبا", 20.0, Level::rtl());
        assert!(a.width > 10.0 && glyphs(&a) <= 5 && a.clusters.iter().flat_map(|c| &c.glyphs).all(|g| g.1 != 0));
    }

    #[test]
    fn wrapping_keeps_each_line_within_the_width_and_loses_no_letter() {
        let text = "यह एक लंबा वाक्य है जो कई पंक्तियों में बँटना चाहिए ताकि हर पंक्ति पन्ने की चौड़ाई में बैठे";
        let mut rep = 0;
        let lines = layout(text, 12.0, 120.0, &mut rep);
        assert!(lines.len() >= 3 && rep == 0);
        for l in &lines {
            assert!(l.width <= 120.5, "{}", l.width);
        }
        let joined: String = lines.iter().map(|l| l.clusters.iter().map(|c| c.text.as_str()).collect::<String>()).collect::<Vec<_>>().join(" ");
        assert_eq!(joined.split_whitespace().collect::<String>(), text.split_whitespace().collect::<String>());
        // a paragraph that starts with Hebrew is set right to left; blank lines survive
        let l2 = layout("שלום עולם\n\nHello", 12.0, 200.0, &mut rep);
        assert!(l2[0].rtl && !l2[2].rtl && l2.len() == 3);
    }

    #[test]
    fn letters_no_bundled_font_has_become_question_marks_and_are_counted() {
        let mut rep = 0;
        let l = layout("नमस्ते 日本", 12.0, 300.0, &mut rep);
        assert_eq!(rep, 2);
        assert!(l[0].clusters.iter().any(|c| c.text == "?"));
    }

    #[test]
    fn a_word_wider_than_the_line_is_cut_and_nothing_is_lost() {
        let mut rep = 0;
        let word = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".repeat(3);
        let text = format!("{word} नमस्ते");
        let lines = layout(&text, 12.0, 60.0, &mut rep);
        let all: String = lines.iter().flat_map(|l| l.clusters.iter().map(|c| c.text.clone())).collect();
        assert_eq!(all.chars().filter(|c| c.is_ascii_uppercase()).count(), 78);
        assert!(lines.iter().all(|l| l.width <= 60.5), "{:?}", lines.iter().map(|l| l.width).collect::<Vec<_>>());
    }
}

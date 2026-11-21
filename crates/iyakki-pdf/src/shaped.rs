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
//! Weights: regular, and a designed bold face of every script (Noto Sans Bold, DejaVu Sans Bold for Latin; `layout_weight`); no vertical metrics tuning; a word wider than the line is
//! cut before a letter that is not a mark, which can separate a rare long cluster. Chinese, Japanese and Korean need a very large
//! font and are not drawn (they are refused with the script named).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use rustybuzz::{Direction, UnicodeBuffer};
use unicode_bidi::{BidiInfo, Level};

use crate::ttf::Ttf;

/// Font list: 0 Latin (DejaVu Sans), 1 Devanagari, 2 Bengali, 3 Gurmukhi, 4 Gujarati, 5 Tamil, 6 Telugu, 7 Kannada, 8 Malayalam,
/// 9 Arabic, 10 Hebrew, 11 Thai, 12 Armenian, 13 Georgian, 14 Ethiopic (the three scripts that need no shaping rules of their own),
/// 15 Sinhala, 16 Khmer, 17 Lao, 18 Myanmar, 19 Tibetan (Noto Serif Tibetan: the Sans face is not in the Ubuntu font package; all SIL OFL; rustybuzz carries the
/// Indic, Khmer, Myanmar and Tibetan shaping engines, so reordering, stacked and subjoined letters come from the font tables and no rule is written here).
pub const FONT_COUNT: usize = 20;
/// Every font with its weight: the fonts above are the regular weight; font `BOLD_OFFSET + i` is the designed bold face of font `i`
/// (Noto Sans Bold of the same script, SIL OFL, kept deflated in `data/bold/` and opened on first use; DejaVu Sans Bold for Latin).
pub const BOLD_OFFSET: usize = FONT_COUNT;
pub const TOTAL_FONTS: usize = 2 * FONT_COUNT;
pub const NAMES: [&str; TOTAL_FONTS] = ["DejaVuSans", "NotoSansDevanagari", "NotoSansBengali", "NotoSansGurmukhi", "NotoSansGujarati", "NotoSansTamil", "NotoSansTelugu", "NotoSansKannada", "NotoSansMalayalam", "NotoSansArabic", "NotoSansHebrew", "NotoSansThai", "NotoSansArmenian", "NotoSansGeorgian", "NotoSansEthiopic", "NotoSansSinhala", "NotoSansKhmer", "NotoSansLao", "NotoSansMyanmar", "NotoSerifTibetan", "DejaVuSans-Bold", "NotoSansDevanagari-Bold", "NotoSansBengali-Bold", "NotoSansGurmukhi-Bold", "NotoSansGujarati-Bold", "NotoSansTamil-Bold", "NotoSansTelugu-Bold", "NotoSansKannada-Bold", "NotoSansMalayalam-Bold", "NotoSansArabic-Bold", "NotoSansHebrew-Bold", "NotoSansThai-Bold", "NotoSansArmenian-Bold", "NotoSansGeorgian-Bold", "NotoSansEthiopic-Bold", "NotoSansSinhala-Bold", "NotoSansKhmer-Bold", "NotoSansLao-Bold", "NotoSansMyanmar-Bold", "NotoSerifTibetan-Bold"];

/// The program of font `i` (a regular face, or `BOLD_OFFSET + i`: the bold one, inflated once).
pub fn bytes(i: usize) -> &'static [u8] {
    if i >= BOLD_OFFSET {
        return bold_bytes(i - BOLD_OFFSET);
    }
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
        11 => include_bytes!("../../iyakki-shell/assets/NotoSansThai-Regular.ttf"),
        12 => include_bytes!("../../iyakki-shell/assets/NotoSansArmenian-Regular.ttf"),
        13 => include_bytes!("../../iyakki-shell/assets/NotoSansGeorgian-Regular.ttf"),
        14 => include_bytes!("../../iyakki-shell/assets/NotoSansEthiopic-Regular.ttf"),
        15 => include_bytes!("../../iyakki-shell/assets/NotoSansSinhala-Regular.ttf"),
        16 => include_bytes!("../../iyakki-shell/assets/NotoSansKhmer-Regular.ttf"),
        17 => include_bytes!("../../iyakki-shell/assets/NotoSansLao-Regular.ttf"),
        18 => include_bytes!("../../iyakki-shell/assets/NotoSansMyanmar-Regular.ttf"),
        _ => include_bytes!("../../iyakki-shell/assets/NotoSerifTibetan-Regular.ttf"),
    }
}

const BOLD_Z: [&[u8]; FONT_COUNT] = [
    &[],
    include_bytes!("../data/bold/NotoSansDevanagari-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansBengali-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansGurmukhi-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansGujarati-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansTamil-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansTelugu-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansKannada-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansMalayalam-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansArabic-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansHebrew-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansThai-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansArmenian-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansGeorgian-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansEthiopic-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansSinhala-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansKhmer-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansLao-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSansMyanmar-Bold.ttf.z"),
    include_bytes!("../data/bold/NotoSerifTibetan-Bold.ttf.z"),
];

/// The bold face of base font `i`: Latin is the DejaVu Sans Bold the shell carries, the rest are inflated from `data/bold/` the first time.
fn bold_bytes(i: usize) -> &'static [u8] {
    if i == 0 {
        return include_bytes!("../../iyakki-shell/assets/DejaVuSans-Bold.ttf");
    }
    static CACHE: [OnceLock<Vec<u8>>; FONT_COUNT] = [const { OnceLock::new() }; FONT_COUNT];
    CACHE[i].get_or_init(|| crate::filter::inflate(BOLD_Z[i], 8 << 20)).as_slice()
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
    (0x0530, 0x058F, 12),
    (0xFB13, 0xFB17, 12),
    (0x10A0, 0x10FF, 13),
    (0x1C90, 0x1CBF, 13),
    (0x2D00, 0x2D2F, 13),
    (0x1200, 0x139F, 14),
    (0x2D80, 0x2DDF, 14),
    (0xAB00, 0xAB2F, 14),
    (0x0D80, 0x0DFF, 15),
    (0x111E0, 0x111FF, 15),
    (0x1780, 0x17FF, 16),
    (0x19E0, 0x19FF, 16),
    (0x0E80, 0x0EFF, 17),
    (0x1000, 0x109F, 18),
    (0xA9E0, 0xA9FF, 18),
    (0xAA60, 0xAA7F, 18),
    (0x0F00, 0x0FFF, 19),
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

/// The shaping face of font `i` (a bold one is opened when it is first asked for).
fn face(i: usize) -> &'static rustybuzz::Face<'static> {
    static L: [OnceLock<Loaded>; TOTAL_FONTS] = [const { OnceLock::new() }; TOTAL_FONTS];
    &L[i].get_or_init(|| Loaded { face: rustybuzz::Face::from_slice(bytes(i), 0).expect("bundled font is valid") }).face
}

/// The font program (for subsetting) of font `i`.
pub fn ttf(i: usize) -> Option<&'static Ttf<'static>> {
    static T: [OnceLock<Option<Ttf<'static>>>; TOTAL_FONTS] = [const { OnceLock::new() }; TOTAL_FONTS];
    T.get(i)?.get_or_init(|| Ttf::parse(bytes(i))).as_ref()
}

fn has_glyph(font: usize, c: char) -> bool {
    face(font).glyph_index(c).is_some()
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
    let face = face(font);
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
    shape_line_weight(text, size, para, false)
}

/// As [`shape_line`]; with `bold` each piece is shaped with the designed bold face of its script (so the widths are the bold ones), unless
/// that face lacks a letter the regular face has: then that piece stays regular and [`draw_weight`] thickens it.
pub fn shape_line_weight(text: &str, size: f64, para: Level, bold: bool) -> Line {
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
                let font = if bold && t.chars().all(|c| !has_glyph(font, c) || has_glyph(font + BOLD_OFFSET, c)) { font + BOLD_OFFSET } else { font };
                shape_piece(font, t, size, rtl, &mut pen, &mut line.clusters);
            }
        }
    }
    line.width = pen;
    line
}

/// Tibetan is written with no spaces: a syllable ends at a tsek (U+0F0B, U+0F0C), and a line may end after it.
fn tibetan_syllables(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in word.chars() {
        cur.push(c);
        if c == '\u{0F0B}' || c == '\u{0F0C}' {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
    layout_weight(text, size, max, replaced, false)
}

/// As [`layout`], in the bold faces when `bold` is set (see [`shape_line_weight`]).
pub fn layout_weight(text: &str, size: f64, max: f64, replaced: &mut usize, bold: bool) -> Vec<Line> {
    let shape_line = |s: &str, size: f64, level: Level| shape_line_weight(s, size, level, bold);
    let text = clean(text, replaced);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let level = BidiInfo::new(para, None).paragraphs.first().map(|p| p.level).unwrap_or_else(Level::ltr);
        let measure = |s: &str| shape_line(s, size, level).width;
        let mut cur = String::new();
        // units a line may end between: the words between spaces, and inside Thai words the pieces the dictionary finds
        let mut units: Vec<(String, bool)> = Vec::new();
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            if crate::thai::has_dictionary_script(word) {
                for (k, piece) in crate::thai::pieces(word).into_iter().enumerate() {
                    units.push((piece, k == 0));
                }
            } else if word.chars().any(|c| (0x0F00..=0x0FFF).contains(&(c as u32))) {
                for (k, piece) in tibetan_syllables(word).into_iter().enumerate() {
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

/// As [`draw`]; with `bold` the letters of a regular face (a piece whose bold face lacks a letter, or a line not shaped with
/// [`layout_weight`]) are filled and stroked with a thin line, as word processors do when a font has no bold face; letters already shaped
/// with a bold face (font number from `BOLD_OFFSET`) are drawn as they are.
pub fn draw_weight(line: &Line, size: f64, x0: f64, y: f64, used: &mut BTreeMap<usize, BTreeMap<u16, char>>, bold: bool) -> String {
    let mut out = String::new();
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
            let thicken = bold && font < BOLD_OFFSET;
            if thicken {
                out.push_str(&format!("q 2 Tr {:.3} w\n", size * 0.028));
            }
            out.push_str(&format!("BT /S{font} {size:.1} Tf {:.2} {:.2} Td <{gid:04X}> Tj ET\n", x0 + x, y + dy));
            if thicken {
                out.push_str("Q\n");
            }
        }
        out.push_str("EMC\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(l: &Line) -> usize {
        l.clusters.iter().map(|c| c.glyphs.len()).sum()
    }

    /// One word of each script, for the bold tests.
    const SAMPLES: [(usize, &str); 19] = [
        (1, "हिन्दी भाषा"),
        (2, "বাংলা ভাষা"),
        (3, "ਪੰਜਾਬੀ ਭਾਸ਼ਾ"),
        (4, "ગુજરાતી ભાષા"),
        (5, "தமிழ் மொழி"),
        (6, "తెలుగు భాష"),
        (7, "ಕನ್ನಡ ಭಾಷೆ"),
        (8, "മലയാളം ഭാഷ"),
        (9, "مرحبا بالعالم"),
        (10, "שלום עולם"),
        (11, "ภาษาไทย"),
        (12, "Հայերեն"),
        (13, "ქართული"),
        (14, "አማርኛ"),
        (15, "සිංහල"),
        (16, "ភាសាខ្មែរ"),
        (17, "ພາສາລາວ"),
        (18, "မြန်မာဘာသာ"),
        (19, "བོད་སྐད་"),
    ];

    #[test]
    fn every_script_has_a_designed_bold_face_that_draws_its_sample_and_differs_from_the_regular_one() {
        for (font, word) in SAMPLES {
            assert!(word.chars().all(|c| c == ' ' || script_font(c) == Some(font)), "{word} is not font {font}");
            let regular = shape_line_weight(word, 20.0, Level::ltr(), false);
            let bold = shape_line_weight(word, 20.0, Level::ltr(), true);
            let ids = |l: &Line| l.clusters.iter().flat_map(|c| c.glyphs.iter().map(|g| g.0)).collect::<BTreeSet<usize>>();
            assert!(ids(&regular).iter().all(|&f| f < BOLD_OFFSET), "{word}: regular line");
            assert!(ids(&bold).contains(&(font + BOLD_OFFSET)), "{}: the bold face of font {font} is used: {:?}", NAMES[font], ids(&bold));
            assert!(bold.clusters.iter().flat_map(|c| &c.glyphs).all(|g| g.1 != 0), "{}: a .notdef glyph in the bold line", NAMES[font]);
            // the same letters, in the same logical order
            assert_eq!(bold.text, regular.text);
            let back: String = bold.clusters.iter().map(|c| c.text.as_str()).collect();
            assert!(!back.is_empty());
            // a designed bold face has its own advance widths
            assert!((bold.width - regular.width).abs() > 1e-6, "{}: bold and regular have the same width", NAMES[font]);
        }
        // Latin: DejaVu Sans Bold
        let b = shape_line_weight("Heading", 20.0, Level::ltr(), true);
        assert!(b.clusters.iter().flat_map(|c| &c.glyphs).all(|g| g.0 == BOLD_OFFSET));
    }

    #[test]
    fn a_bold_line_is_drawn_without_the_thickening_and_a_regular_line_asked_for_bold_still_gets_it() {
        let mut used = BTreeMap::new();
        let bold = shape_line_weight("ภาษาไทย", 12.0, Level::ltr(), true);
        let c = draw_weight(&bold, 12.0, 10.0, 20.0, &mut used, true);
        assert!(!c.contains("2 Tr") && c.contains("/S31 "), "{c}");
        assert!(used.contains_key(&(BOLD_OFFSET + 11)));
        let regular = shape_line("ภาษาไทย", 12.0, Level::ltr());
        let c = draw_weight(&regular, 12.0, 10.0, 20.0, &mut BTreeMap::new(), true);
        assert!(c.contains("2 Tr"), "a face with no bold copy is thickened as before");
        let c = draw_weight(&regular, 12.0, 10.0, 20.0, &mut BTreeMap::new(), false);
        assert!(!c.contains("2 Tr"));
    }

    #[test]
    fn a_bold_layout_wraps_inside_the_width_with_the_bold_widths_and_loses_no_letter() {
        let text = "ภาษาไทยเป็นภาษาที่ใช้ในประเทศไทย ".repeat(6);
        let mut rep = 0;
        let lines = layout_weight(&text, 14.0, 120.0, &mut rep, true);
        assert_eq!(rep, 0);
        assert!(lines.len() > 2);
        assert!(lines.iter().all(|l| l.width <= 120.0 + 1e-6 || l.clusters.len() <= 1), "{:?}", lines.iter().map(|l| l.width).collect::<Vec<_>>());
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let back: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(squash(&back), squash(&text));
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
        for i in 0..TOTAL_FONTS {
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
    fn armenian_georgian_and_ethiopic_have_fonts_and_are_drawn_not_replaced() {
        for (word, font) in [("\u{540}\u{561}\u{575}\u{565}\u{580}\u{565}\u{576}", 12), ("\u{10e5}\u{10d0}\u{10e0}\u{10d7}\u{10e3}\u{10da}\u{10d8}", 13), ("\u{12a0}\u{121b}\u{122d}\u{129b}", 14)] {
            assert!(needs(word), "{word}");
            assert!(word.chars().all(|c| script_font(c) == Some(font)));
            let mut rep = 0;
            let l = layout(word, 12.0, 300.0, &mut rep);
            assert_eq!(rep, 0, "no letter of {word} became a question mark");
            assert!(l[0].width > 10.0 && l[0].clusters.iter().all(|c| c.text != "?"));
        }
        assert_eq!(TOTAL_FONTS, NAMES.len());
        assert!((0..TOTAL_FONTS).all(|i| crate::ttf::Ttf::parse(bytes(i)).is_some()), "every bundled font reads");
    }

    #[test]
    fn sinhala_khmer_lao_myanmar_and_tibetan_have_fonts_and_are_shaped_not_replaced() {
        // words written out as escapes so no table is typed from memory beyond the code points of each script's block
        let samples: [(&str, usize); 5] = [
            ("\u{0DC1}\u{0DCA}\u{200D}\u{0DBB}\u{0DD3}\u{0DBD}\u{0D82}\u{0D9A}\u{0DCF}", 15),
            ("\u{1781}\u{17D2}\u{1798}\u{17C2}\u{179A}\u{1797}\u{17B6}\u{179F}\u{17B6}", 16),
            ("\u{0EA5}\u{0EB2}\u{0EA7}\u{0E9E}\u{0EB2}\u{0EAA}\u{0EB2}", 17),
            ("\u{1019}\u{103C}\u{1014}\u{103A}\u{1019}\u{102C}\u{1018}\u{102C}\u{101E}\u{102C}", 18),
            ("\u{0F56}\u{0F7C}\u{0F51}\u{0F0B}\u{0F61}\u{0F72}\u{0F42}", 19),
        ];
        for (word, font) in samples {
            assert!(needs(word), "{word}");
            assert!(word.chars().all(|c| script_font(c) == Some(font) || c == '\u{200D}'), "{word}");
            let mut rep = 0;
            let l = layout(word, 12.0, 300.0, &mut rep);
            assert_eq!(rep, 0, "no letter of {word} became a question mark");
            assert!(l[0].width > 10.0 && l[0].clusters.iter().all(|c| c.text != "?"));
            assert!(l[0].clusters.iter().flat_map(|c| c.glyphs.iter()).any(|g| g.0 == font), "{word} drawn with its own font");
            let back: String = l[0].clusters.iter().map(|c| c.text.as_str()).collect();
            let mut sorted: Vec<char> = back.chars().collect();
            sorted.sort();
            let mut want: Vec<char> = word.chars().collect();
            want.sort();
            assert_eq!(sorted, want, "every typed letter is kept in the clusters");
        }
    }

    #[test]
    fn a_stacked_khmer_consonant_and_a_reordered_vowel_become_fewer_clusters_than_letters() {
        // the coeng (U+17D2) joins the next consonant under the first: shaping makes one cluster of three code points
        let l = shape_line("\u{1781}\u{17D2}\u{1798}\u{17C2}\u{179A}", 12.0, Level::ltr());
        assert!(l.clusters.len() < 5, "{} clusters", l.clusters.len());
        assert!(l.clusters.iter().any(|c| c.text.contains('\u{17D2}')));
    }

    #[test]
    fn tibetan_wraps_after_a_tsek_and_nothing_is_lost() {
        let word: String = "\u{0F56}\u{0F7C}\u{0F51}\u{0F0B}".repeat(30);
        let mut rep = 0;
        let lines = layout(&word, 12.0, 120.0, &mut rep);
        assert!(lines.len() > 1);
        let text: String = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(text, word);
        assert!(lines.iter().all(|l| l.text.ends_with('\u{0F0B}')), "every line ends at a tsek");
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

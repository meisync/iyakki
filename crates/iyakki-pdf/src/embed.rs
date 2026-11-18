//! Embedded fonts for the writer (F-PDF-11): text the standard Helvetica and Courier fonts cannot write (Greek, Cyrillic, Latin Extended,
//! Armenian, Georgian, symbols, arrows, math signs, ...) is drawn with DejaVu Sans, Sans Bold or Sans Mono (the fonts the shell already
//! carries, a permissive licence, see crates/iyakki-shell/assets/DEJAVU-LICENSE.txt), as a Type 0 font with a CIDFontType2 descendant,
//! `Identity-H` encoding (the character code is the glyph number), widths from the font and a ToUnicode map, so the text can be copied
//! and found. Only the outlines of the used glyphs are stored.
//!
//! Not supported, and refused by [`supports`]: scripts that need shaping or reordering (Devanagari, Tamil and the other Indic scripts)
//! or right-to-left layout (Hebrew, Arabic, Syriac, Thaana): drawing them letter by letter in logical order would be wrong text on the
//! page. Characters the font has no glyph for (Chinese, Japanese, Korean, emoji) are refused too.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::ttf::Ttf;

const SANS: &[u8] = include_bytes!("../../iyakki-shell/assets/DejaVuSans.ttf");
const BOLD: &[u8] = include_bytes!("../../iyakki-shell/assets/DejaVuSans-Bold.ttf");
const MONO: &[u8] = include_bytes!("../../iyakki-shell/assets/DejaVuSansMono.ttf");
const NAMES: [&str; 3] = ["DejaVuSans", "DejaVuSans-Bold", "DejaVuSansMono"];

pub struct Face {
    pub ttf: Ttf<'static>,
    pub map: HashMap<u32, u16>,
    pub name: &'static str,
}

impl Face {
    pub fn gid(&self, c: char) -> u16 {
        self.map.get(&(c as u32)).copied().unwrap_or(0)
    }
    /// Advance width of a character in 1/1000 of the font size.
    pub fn width(&self, c: char) -> f64 {
        self.ttf.advance(self.gid(c)) as f64 * 1000.0 / self.ttf.units as f64
    }
}

/// Face 0 is Sans, 1 Sans Bold, 2 Sans Mono.
pub fn face(ix: usize) -> Option<&'static Face> {
    static FACES: OnceLock<Vec<Option<Face>>> = OnceLock::new();
    FACES
        .get_or_init(|| {
            [SANS, BOLD, MONO]
                .iter()
                .zip(NAMES)
                .map(|(d, name)| Ttf::parse(d).map(|ttf| Face { map: ttf.cmap_map(), ttf, name }))
                .collect()
        })
        .get(ix)?
        .as_ref()
}

/// Scripts the writer does not draw: they need shaping or right-to-left layout.
fn needs_layout(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Can the embedded font draw this character correctly? (No: other scripts, no glyph.)
pub fn supports(c: char) -> bool {
    !needs_layout(c) && !c.is_control() && face(0).is_some_and(|f| f.gid(c) != 0)
}

/// A short name in a few words for the first character of a script the writer cannot draw, for the refusal message.
pub fn script_of(c: char) -> &'static str {
    match c as u32 {
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => "Hebrew (right to left)",
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => "Arabic (right to left)",
        0x0700..=0x074F => "Syriac (right to left)",
        0x0780..=0x07BF => "Thaana (right to left)",
        0x0900..=0x097F => "Devanagari",
        0x0980..=0x09FF => "Bengali",
        0x0A00..=0x0A7F => "Gurmukhi",
        0x0A80..=0x0AFF => "Gujarati",
        0x0B00..=0x0B7F => "Odia",
        0x0B80..=0x0BFF => "Tamil",
        0x0C00..=0x0C7F => "Telugu",
        0x0C80..=0x0CFF => "Kannada",
        0x0D00..=0x0D7F => "Malayalam",
        0x0D80..=0x0DFF => "Sinhala",
        0x0E00..=0x0E7F => "Thai",
        0x3040..=0x30FF => "Japanese",
        0x4E00..=0x9FFF => "Chinese",
        0xAC00..=0xD7AF => "Korean",
        0x1F000..=0x1FAFF => "emoji",
        _ => "characters this font does not have",
    }
}

/// The objects of one used face, in order: the Type 0 font (`first`), its CID font, the descriptor, the font file and the ToUnicode map.
/// `used` maps the glyph numbers drawn to the character each stands for.
pub fn objects(face_ix: usize, used: &BTreeMap<u16, char>, first: usize) -> Vec<Vec<u8>> {
    let f = face(face_ix).expect("font present");
    objects_for(&f.ttf, f.name, used, first, if face_ix == 2 { 4 | 1 } else { 4 }, if face_ix == 1 { 140 } else { 80 })
}

/// The same five objects for any font: `used` maps glyph numbers to the character each stands for, `'\0'` for a glyph with no
/// single character of its own (a mark, a part of a ligature: it is stored and has a width but gets no ToUnicode entry).
pub fn objects_for(ttf: &Ttf<'_>, name: &str, used: &BTreeMap<u16, char>, first: usize, flags: u32, stem_v: u32) -> Vec<Vec<u8>> {
    let keep = ttf.closure(&used.keys().copied().collect::<BTreeSet<u16>>());
    let chars: Vec<(u32, u16)> = used.iter().filter(|(_, c)| **c != '\0').map(|(g, c)| (*c as u32, *g)).collect();
    let file = ttf.subset(&keep, &chars);
    let tag: String = {
        let mut h = 0u32;
        for g in &keep {
            h = h.wrapping_mul(31).wrapping_add(*g as u32 + 1);
        }
        (0..6).map(|i| (b'A' + ((h >> (i * 4)) % 26) as u8) as char).collect()
    };
    let base = format!("{tag}+{name}");
    let mut w = String::from("[");
    for g in used.keys() {
        let _ = write!(w, " {} [{}]", g, (ttf.advance(*g) as f64 * 1000.0 / ttf.units as f64).round() as i64);
    }
    w.push_str(" ]");
    let z = crate::filter::zlib_deflate(&file);
    let mut ff = format!("<< /Length1 {} /Filter /FlateDecode /Length {} >>\nstream\n", file.len(), z.len()).into_bytes();
    ff.extend_from_slice(&z);
    ff.extend_from_slice(b"\nendstream");
    let mut cm = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    let entries: Vec<(&u16, &char)> = used.iter().filter(|(_, c)| **c != '\0').collect();
    for chunk in entries.chunks(100) {
        let _ = writeln!(cm, "{} beginbfchar", chunk.len());
        for (g, c) in chunk {
            let mut u = [0u16; 2];
            let hex: String = c.encode_utf16(&mut u).iter().map(|x| format!("{x:04X}")).collect();
            let _ = writeln!(cm, "<{g:04X}> <{hex}>");
        }
        cm.push_str("endbfchar\n");
    }
    cm.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let zc = crate::filter::zlib_deflate(cm.as_bytes());
    let mut tu = format!("<< /Filter /FlateDecode /Length {} >>\nstream\n", zc.len()).into_bytes();
    tu.extend_from_slice(&zc);
    tu.extend_from_slice(b"\nendstream");
    let bb = ttf.bbox();
    let s = 1000.0 / ttf.units as f64;
    let sc = |v: i16| (v as f64 * s).round() as i64;
    let desc = format!(
        "<< /Type /FontDescriptor /FontName /{base} /Flags {flags} /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV {} /FontFile2 {} 0 R >>",
        sc(bb[0]),
        sc(bb[1]),
        sc(bb[2]),
        sc(bb[3]),
        sc(ttf.ascent()),
        sc(ttf.descent()),
        sc(ttf.ascent()) * 3 / 4,
        stem_v,
        first + 3
    );
    vec![
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{base} /Encoding /Identity-H /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>", first + 1, first + 4).into_bytes(),
        format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R /CIDToGIDMap /Identity /DW 600 /W {w} >>", first + 2).into_bytes(),
        desc.into_bytes(),
        ff,
        tu,
    ]
}

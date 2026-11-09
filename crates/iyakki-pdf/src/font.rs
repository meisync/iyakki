//! Fonts as far as reading text needs them: how a string's bytes split into character codes, what Unicode each code means (ToUnicode,
//! /Encoding with /Differences, the base encodings) and how wide it is. Glyph outlines of embedded font programs are not read; a
//! substitute drawn by the viewer's own font stands in (docs/PDF.md).

use crate::file::File;
use crate::obj::*;
use std::collections::HashMap;

pub const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{fffd}', '\u{17d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{fffd}', '\u{17e}', '\u{178}',
];

const MAC_HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{f8ff}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    Win,
    Mac,
    Standard,
}

pub fn base_table(b: Base) -> Vec<Option<char>> {
    let mut t: Vec<Option<char>> = (0..256).map(|_| None).collect();
    for c in 32..127u32 {
        t[c as usize] = char::from_u32(c);
    }
    match b {
        Base::Win => {
            for c in 0xa0..=0xffu32 {
                t[c as usize] = char::from_u32(c);
            }
            for (i, ch) in CP1252_HIGH.iter().enumerate() {
                if *ch != '\u{fffd}' {
                    t[0x80 + i] = Some(*ch);
                }
            }
            t[0x7f] = Some('\u{2022}');
        }
        Base::Mac => {
            for (i, ch) in MAC_HIGH.chars().enumerate().take(128) {
                t[0x80 + i] = Some(ch);
            }
        }
        Base::Standard => {
            t[0x27] = Some('\u{2019}');
            t[0x60] = Some('\u{2018}');
            let hi: [(usize, char); 36] = [
                (0xa1, '¡'), (0xa2, '¢'), (0xa3, '£'), (0xa4, '\u{2044}'), (0xa5, '¥'), (0xa6, 'ƒ'), (0xa7, '§'), (0xa8, '¤'), (0xa9, '\''), (0xaa, '\u{201c}'),
                (0xab, '«'), (0xac, '\u{2039}'), (0xad, '\u{203a}'), (0xae, '\u{fb01}'), (0xaf, '\u{fb02}'), (0xb1, '\u{2013}'), (0xb2, '\u{2020}'), (0xb3, '\u{2021}'),
                (0xb4, '·'), (0xb6, '¶'), (0xb7, '\u{2022}'), (0xb8, '\u{201a}'), (0xb9, '\u{201e}'), (0xba, '\u{201d}'), (0xbb, '»'), (0xbc, '\u{2026}'),
                (0xbd, '\u{2030}'), (0xbf, '¿'), (0xd0, '\u{2014}'), (0xe1, 'Æ'), (0xe3, 'ª'), (0xe9, 'Ø'), (0xeb, 'º'), (0xf1, 'æ'), (0xf9, 'ø'), (0xfb, 'ß'),
            ];
            for (i, c) in hi {
                t[i] = Some(c);
            }
        }
    }
    t
}

/// A glyph name to its Unicode character (the common Adobe Glyph List names, `uniXXXX`, `uXXXX`).
pub fn glyph_unicode(name: &str) -> Option<char> {
    if name.chars().count() == 1 {
        return name.chars().next();
    }
    if let Some(h) = name.strip_prefix("uni") {
        if h.len() >= 4 && h.is_char_boundary(4) {
            return u32::from_str_radix(&h[..4], 16).ok().and_then(char::from_u32);
        }
    }
    if let Some(h) = name.strip_prefix('u') {
        if (4..=6).contains(&h.len()) {
            if let Ok(v) = u32::from_str_radix(h, 16) {
                return char::from_u32(v);
            }
        }
    }
    let base = name.split('.').next().unwrap_or(name);
    if base != name && !base.is_empty() {
        return glyph_unicode(base);
    }
    const T: &[(&str, char)] = &[
        ("space", ' '), ("exclam", '!'), ("quotedbl", '"'), ("numbersign", '#'), ("dollar", '$'), ("percent", '%'), ("ampersand", '&'),
        ("quotesingle", '\''), ("parenleft", '('), ("parenright", ')'), ("asterisk", '*'), ("plus", '+'), ("comma", ','), ("hyphen", '-'),
        ("minus", '\u{2212}'), ("period", '.'), ("slash", '/'), ("zero", '0'), ("one", '1'), ("two", '2'), ("three", '3'), ("four", '4'), ("five", '5'),
        ("six", '6'), ("seven", '7'), ("eight", '8'), ("nine", '9'), ("colon", ':'), ("semicolon", ';'), ("less", '<'), ("equal", '='),
        ("greater", '>'), ("question", '?'), ("at", '@'), ("bracketleft", '['), ("backslash", '\\'), ("bracketright", ']'), ("asciicircum", '^'),
        ("underscore", '_'), ("grave", '`'), ("braceleft", '{'), ("bar", '|'), ("braceright", '}'), ("asciitilde", '~'), ("quoteleft", '\u{2018}'),
        ("quoteright", '\u{2019}'), ("quotedblleft", '\u{201c}'), ("quotedblright", '\u{201d}'), ("quotesinglbase", '\u{201a}'),
        ("quotedblbase", '\u{201e}'), ("bullet", '\u{2022}'), ("endash", '\u{2013}'), ("emdash", '\u{2014}'), ("ellipsis", '\u{2026}'),
        ("dagger", '\u{2020}'), ("daggerdbl", '\u{2021}'), ("perthousand", '\u{2030}'), ("guilsinglleft", '\u{2039}'), ("guilsinglright", '\u{203a}'),
        ("guillemotleft", '«'), ("guillemotright", '»'), ("trademark", '\u{2122}'), ("Euro", '\u{20ac}'), ("fi", '\u{fb01}'), ("fl", '\u{fb02}'),
        ("ff", '\u{fb00}'), ("ffi", '\u{fb03}'), ("ffl", '\u{fb04}'), ("copyright", '©'), ("registered", '®'), ("degree", '°'), ("plusminus", '±'),
        ("multiply", '×'), ("divide", '÷'), ("section", '§'), ("paragraph", '¶'), ("periodcentered", '·'), ("exclamdown", '¡'), ("questiondown", '¿'),
        ("cent", '¢'), ("sterling", '£'), ("yen", '¥'), ("currency", '¤'), ("brokenbar", '¦'), ("dieresis", '¨'), ("ordfeminine", 'ª'),
        ("ordmasculine", 'º'), ("logicalnot", '¬'), ("macron", '¯'), ("acute", '´'), ("mu", 'µ'), ("cedilla", '¸'), ("onesuperior", '¹'),
        ("twosuperior", '²'), ("threesuperior", '³'), ("onequarter", '¼'), ("onehalf", '½'), ("threequarters", '¾'), ("nbspace", '\u{a0}'),
        ("nonbreakingspace", '\u{a0}'), ("sfthyphen", '-'), ("Agrave", 'À'), ("Aacute", 'Á'), ("Acircumflex", 'Â'), ("Atilde", 'Ã'),
        ("Adieresis", 'Ä'), ("Aring", 'Å'), ("AE", 'Æ'), ("Ccedilla", 'Ç'), ("Egrave", 'È'), ("Eacute", 'É'), ("Ecircumflex", 'Ê'),
        ("Edieresis", 'Ë'), ("Igrave", 'Ì'), ("Iacute", 'Í'), ("Icircumflex", 'Î'), ("Idieresis", 'Ï'), ("Eth", 'Ð'), ("Ntilde", 'Ñ'),
        ("Ograve", 'Ò'), ("Oacute", 'Ó'), ("Ocircumflex", 'Ô'), ("Otilde", 'Õ'), ("Odieresis", 'Ö'), ("Oslash", 'Ø'), ("Ugrave", 'Ù'),
        ("Uacute", 'Ú'), ("Ucircumflex", 'Û'), ("Udieresis", 'Ü'), ("Yacute", 'Ý'), ("Thorn", 'Þ'), ("germandbls", 'ß'), ("agrave", 'à'),
        ("aacute", 'á'), ("acircumflex", 'â'), ("atilde", 'ã'), ("adieresis", 'ä'), ("aring", 'å'), ("ae", 'æ'), ("ccedilla", 'ç'),
        ("egrave", 'è'), ("eacute", 'é'), ("ecircumflex", 'ê'), ("edieresis", 'ë'), ("igrave", 'ì'), ("iacute", 'í'), ("icircumflex", 'î'),
        ("idieresis", 'ï'), ("eth", 'ð'), ("ntilde", 'ñ'), ("ograve", 'ò'), ("oacute", 'ó'), ("ocircumflex", 'ô'), ("otilde", 'õ'),
        ("odieresis", 'ö'), ("oslash", 'ø'), ("ugrave", 'ù'), ("uacute", 'ú'), ("ucircumflex", 'û'), ("udieresis", 'ü'), ("yacute", 'ý'),
        ("thorn", 'þ'), ("ydieresis", 'ÿ'), ("Scaron", 'Š'), ("scaron", 'š'), ("Zcaron", 'Ž'), ("zcaron", 'ž'), ("OE", 'Œ'), ("oe", 'œ'),
        ("Ydieresis", 'Ÿ'), ("florin", 'ƒ'), ("circumflex", 'ˆ'), ("tilde", '˜'), ("dotlessi", 'ı'), ("Lslash", 'Ł'), ("lslash", 'ł'),
        ("fraction", '\u{2044}'), ("summation", '∑'), ("product", '∏'), ("radical", '√'), ("infinity", '∞'), ("notequal", '≠'),
        ("lessequal", '≤'), ("greaterequal", '≥'), ("approxequal", '≈'), ("partialdiff", '∂'), ("integral", '∫'), ("Omega", 'Ω'), ("pi", 'π'),
        ("arrowright", '→'), ("arrowleft", '←'), ("arrowup", '↑'), ("arrowdown", '↓'), ("checkmark", '✓'),
    ];
    T.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Helvetica advance widths for ASCII 32..=126, in 1/1000 em.
pub const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667,
    667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500,
    556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

pub fn helv_width(c: char) -> u16 {
    let u = c as u32;
    if (32..127).contains(&u) {
        HELV[(u - 32) as usize]
    } else if c == '\u{a0}' {
        278
    } else {
        556
    }
}

#[derive(Clone, Debug, Default)]
pub struct Font {
    pub base_name: String,
    pub two_byte: bool,
    pub type3: bool,
    pub vertical: bool,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub serif: bool,
    pub embedded: bool,
    enc: Vec<Option<char>>,
    to_unicode: HashMap<u32, String>,
    has_to_unicode: bool,
    widths: HashMap<u32, f64>,
    default_width: Option<f64>,
    /// Glyph space to text space multiplier x1000 for Type 3 fonts (0.001 -> 1.0).
    width_scale: f64,
    std_widths: bool,
    pub symbolic: bool,
}

fn style_from_name(name: &str, f: &mut Font) {
    let n = name.to_ascii_lowercase();
    f.bold = n.contains("bold") || n.contains("black") || n.contains("heavy") || n.contains("semibold");
    f.italic = n.contains("italic") || n.contains("oblique");
    f.mono = n.contains("courier") || n.contains("mono") || n.contains("consolas") || n.contains("typewriter");
    f.serif = !f.mono && (n.contains("times") || n.contains("serif") && !n.contains("sans") || n.contains("georgia") || n.contains("garamond") || n.contains("palatino") || n.contains("minion") || n.contains("roman") || n.contains("cambria"));
}

impl Font {
    pub fn load(file: &File, fd: &Dict, depth: usize) -> Font {
        let mut f = Font { width_scale: 1.0, ..Font::default() };
        let subtype = file.get(fd, "Subtype");
        let sub = subtype.name().unwrap_or("Type1").to_string();
        f.base_name = file.get(fd, "BaseFont").name().unwrap_or("").to_string();
        if let Some((_, rest)) = f.base_name.split_once('+') {
            if f.base_name.len() > 7 && f.base_name.as_bytes()[6] == b'+' {
                f.base_name = rest.to_string();
            }
        }
        let name = f.base_name.clone();
        style_from_name(&name, &mut f);
        f.type3 = sub == "Type3";
        if sub == "Type0" {
            f.two_byte = true;
            if let Some(enc) = file.get(fd, "Encoding").name() {
                f.vertical = enc.ends_with("-V");
            }
            let desc = file.get(fd, "DescendantFonts");
            let dd = desc.arr().and_then(|a| a.first().cloned()).map(|x| file.resolve(&x)).and_then(|x| x.dict().cloned());
            if let Some(dd) = dd {
                f.default_width = Some(file.get(&dd, "DW").num().unwrap_or(1000.0));
                if let Some(w) = file.get(&dd, "W").arr() {
                    let w: Vec<Obj> = w.iter().map(|x| file.resolve(x)).collect();
                    let mut i = 0;
                    let mut count = 0;
                    while i < w.len() && count < 200_000 {
                        let Some(c1) = w[i].int() else { break };
                        match w.get(i + 1) {
                            Some(Obj::Arr(a)) => {
                                for (k, x) in a.iter().enumerate() {
                                    if let Some(v) = file.resolve(x).num() {
                                        f.widths.insert((c1 + k as i64) as u32, v);
                                        count += 1;
                                    }
                                }
                                i += 2;
                            }
                            Some(c2) => {
                                let (Some(c2), Some(v)) = (c2.int(), w.get(i + 2).and_then(|x| x.num())) else { break };
                                let mut c = c1;
                                while c <= c2 && c - c1 < 70_000 && count < 200_000 {
                                    f.widths.insert(c as u32, v);
                                    c += 1;
                                    count += 1;
                                }
                                i += 3;
                            }
                            None => break,
                        }
                    }
                }
                let fdesc = file.get(&dd, "FontDescriptor");
                if let Some(d) = fdesc.dict() {
                    f.embedded = ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| d.contains_key(*k));
                    let fl = file.get(d, "Flags").int().unwrap_or(0);
                    f.serif |= fl & 2 != 0;
                    f.mono |= fl & 1 != 0;
                    f.italic |= fl & 64 != 0;
                    f.bold |= fl & (1 << 18) != 0;
                }
            }
        } else {
            let fdesc = file.get(fd, "FontDescriptor");
            let mut missing = None;
            if let Some(d) = fdesc.dict() {
                f.embedded = ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| d.contains_key(*k));
                let fl = file.get(d, "Flags").int().unwrap_or(0);
                f.serif |= fl & 2 != 0;
                f.mono |= fl & 1 != 0;
                f.italic |= fl & 64 != 0;
                f.bold |= fl & (1 << 18) != 0;
                f.symbolic = fl & 4 != 0 && fl & 32 == 0;
                missing = file.get(d, "MissingWidth").num();
            }
            let first = file.get(fd, "FirstChar").int().unwrap_or(0).clamp(0, 255);
            if let Some(w) = file.get(fd, "Widths").arr() {
                for (i, x) in w.iter().enumerate().take(256) {
                    if let Some(v) = file.resolve(x).num() {
                        f.widths.insert((first + i as i64) as u32, v);
                    }
                }
            } else {
                f.std_widths = true;
            }
            f.default_width = missing;
            if f.type3 {
                if let Some(m) = file.get(fd, "FontMatrix").arr() {
                    f.width_scale = m.first().and_then(|x| file.resolve(x).num()).unwrap_or(0.001) * 1000.0;
                }
            }
            // encoding
            let encobj = file.get(fd, "Encoding");
            let mut base = if sub == "TrueType" { Base::Win } else { Base::Standard };
            let mut diffs: Option<Obj> = None;
            match &encobj {
                Obj::Name(n) => base = base_from_name(n, base),
                Obj::Dict(d) => {
                    if let Some(n) = file.get(d, "BaseEncoding").name() {
                        base = base_from_name(n, base);
                    }
                    diffs = Some(file.get(d, "Differences"));
                }
                _ => {}
            }
            let mut table = base_table(base);
            if let Some(Obj::Arr(a)) = diffs {
                let mut code = 0usize;
                for x in a.iter().take(2000) {
                    match file.resolve(x) {
                        Obj::Int(n) => code = n.clamp(0, 255) as usize,
                        Obj::Name(n) => {
                            if code < 256 {
                                table[code] = glyph_unicode(&n);
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
            f.enc = table;
        }
        if let Obj::Stream(..) = file.get(fd, "ToUnicode") {
            if depth < 4 {
                let s = file.get(fd, "ToUnicode");
                if let Ok(d) = file.stream_data(&s) {
                    f.to_unicode = parse_cmap(&d.data);
                    f.has_to_unicode = !f.to_unicode.is_empty();
                }
            }
        }
        f
    }

    /// Split a shown string into character codes.
    pub fn codes(&self, s: &[u8]) -> Vec<u32> {
        if self.two_byte {
            s.chunks(2).map(|c| if c.len() == 2 { (c[0] as u32) << 8 | c[1] as u32 } else { c[0] as u32 }).collect()
        } else {
            s.iter().map(|&b| b as u32).collect()
        }
    }

    pub fn unicode(&self, code: u32) -> Option<String> {
        if let Some(s) = self.to_unicode.get(&code) {
            return Some(s.clone());
        }
        if self.two_byte {
            return None;
        }
        self.enc.get(code as usize).copied().flatten().map(|c| c.to_string()).or_else(|| if code >= 32 && code < 127 && self.enc.is_empty() { char::from_u32(code).map(|c| c.to_string()) } else { None })
    }

    /// Advance width of a code in 1/1000 text-space units.
    pub fn width(&self, code: u32) -> f64 {
        if let Some(w) = self.widths.get(&code) {
            return *w * self.width_scale;
        }
        if let Some(d) = self.default_width {
            if self.two_byte || !self.std_widths {
                return d;
            }
        }
        if self.mono {
            return 600.0;
        }
        if self.two_byte {
            return 1000.0;
        }
        match self.unicode(code).and_then(|s| s.chars().next()) {
            Some(c) => helv_width(c) as f64 * if self.serif { 0.92 } else { 1.0 },
            None => 556.0,
        }
    }

    pub fn can_extract(&self) -> bool {
        self.has_to_unicode || !self.two_byte
    }
}

fn base_from_name(n: &str, dflt: Base) -> Base {
    match n {
        "WinAnsiEncoding" => Base::Win,
        "MacRomanEncoding" => Base::Mac,
        "StandardEncoding" => Base::Standard,
        _ => dflt,
    }
}

fn hex_to_u32(b: &[u8]) -> u32 {
    b.iter().take(4).fold(0u32, |a, &x| (a << 8) | x as u32)
}

fn utf16_be(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks(2).map(|c| if c.len() == 2 { (c[0] as u16) << 8 | c[1] as u16 } else { c[0] as u16 }).collect();
    String::from_utf16_lossy(&units)
}

/// A ToUnicode CMap: `bfchar` and `bfrange` only (what is needed to read text).
pub fn parse_cmap(data: &[u8]) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let mut lx = Lexer::new(data, 0);
    let mut stack: Vec<Obj> = Vec::new();
    let mut mode = 0; // 1 bfchar, 2 bfrange
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > 2_000_000 || map.len() > 200_000 {
            break;
        }
        match parse_obj(&mut lx, false, 0) {
            Ok(Tok::Eof) | Err(_) => break,
            Ok(Tok::Keyword(k)) => {
                match k.as_str() {
                    "beginbfchar" => mode = 1,
                    "beginbfrange" => mode = 2,
                    "endbfchar" | "endbfrange" => mode = 0,
                    _ => {}
                }
                stack.clear();
            }
            Ok(Tok::Obj(o)) => {
                stack.push(o);
                if mode == 1 && stack.len() == 2 {
                    if let (Some(s), Some(d)) = (stack[0].str_bytes(), stack[1].str_bytes()) {
                        map.insert(hex_to_u32(s), utf16_be(d));
                    }
                    stack.clear();
                } else if mode == 2 && stack.len() == 3 {
                    if let (Some(lo), Some(hi)) = (stack[0].str_bytes(), stack[1].str_bytes()) {
                        let (lo, hi) = (hex_to_u32(lo), hex_to_u32(hi));
                        match &stack[2] {
                            Obj::Str(d) => {
                                let base = utf16_be(d);
                                let mut chars: Vec<char> = base.chars().collect();
                                for (i, c) in (lo..=hi.min(lo.saturating_add(65535))).enumerate() {
                                    if let Some(l) = chars.last_mut() {
                                        if i > 0 {
                                            *l = char::from_u32(*l as u32 + 1).unwrap_or(*l);
                                        }
                                    }
                                    map.insert(c, chars.iter().collect());
                                }
                            }
                            Obj::Arr(a) => {
                                for (i, x) in a.iter().enumerate().take(65536) {
                                    if let Some(d) = x.str_bytes() {
                                        map.insert(lo + i as u32, utf16_be(d));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    stack.clear();
                }
            }
            Ok(_) => {}
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_table_has_128_entries() {
        assert_eq!(MAC_HIGH.chars().count(), 128);
    }

    #[test]
    fn win_ansi_and_glyph_names() {
        let t = base_table(Base::Win);
        assert_eq!(t[0x41], Some('A'));
        assert_eq!(t[0x93], Some('\u{201c}'));
        assert_eq!(t[0xe9], Some('é'));
        assert_eq!(glyph_unicode("Eacute"), Some('É'));
        assert_eq!(glyph_unicode("uni20AC"), Some('€'));
        assert_eq!(glyph_unicode("a.sc"), Some('a'));
        assert_eq!(glyph_unicode("nonsense-name"), None);
    }

    #[test]
    fn tounicode_bfchar_and_bfrange() {
        let cm = b"1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <01> <0041> <02> <00660069> endbfchar 2 beginbfrange <10> <12> <0061> <20> <21> [<0058> <0059>] endbfrange";
        let m = parse_cmap(cm);
        assert_eq!(m[&1], "A");
        assert_eq!(m[&2], "fi");
        assert_eq!(m[&0x10], "a");
        assert_eq!(m[&0x12], "c");
        assert_eq!(m[&0x21], "Y");
    }

    #[test]
    fn helvetica_widths_are_known() {
        assert_eq!(helv_width('a'), 556);
        assert_eq!(helv_width('i'), 222);
        assert_eq!(helv_width(' '), 278);
    }
}

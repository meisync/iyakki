//! The Compact Font Format (Adobe Technical Note 5176) as far as reading text needs: the header and the INDEX structures, the Top DICT,
//! the charset (which glyph is which name) and the built-in encoding (which character code draws which glyph). A PDF font with no
//! `/Encoding` of its own uses the encoding of its program, so for `FontFile3` (Type1C) fonts this is what turns codes into letters.
//! Glyph outlines are not read here.

/// The names of the standard strings 1 to 228 (Appendix A of the note), the ones every font can use without carrying them.
const STD_NAMES: [&str; 229] = [
    ".notdef", "space", "exclam", "quotedbl", "numbersign", "dollar", "percent", "ampersand", "quoteright", "parenleft", "parenright", "asterisk", "plus", "comma",
    "hyphen", "period", "slash", "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "colon", "semicolon", "less", "equal", "greater",
    "question", "at", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z", "bracketleft",
    "backslash", "bracketright", "asciicircum", "underscore", "quoteleft", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
    "s", "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar", "braceright", "asciitilde", "exclamdown", "cent", "sterling", "fraction", "yen", "florin", "section",
    "currency", "quotesingle", "quotedblleft", "guillemotleft", "guilsinglleft", "guilsinglright", "fi", "fl", "endash", "dagger", "daggerdbl", "periodcentered",
    "paragraph", "bullet", "quotesinglbase", "quotedblbase", "quotedblright", "guillemotright", "ellipsis", "perthousand", "questiondown", "grave", "acute",
    "circumflex", "tilde", "macron", "breve", "dotaccent", "dieresis", "ring", "cedilla", "hungarumlaut", "ogonek", "caron", "emdash", "AE", "ordfeminine", "Lslash",
    "Oslash", "OE", "ordmasculine", "ae", "dotlessi", "lslash", "oslash", "oe", "germandbls", "onesuperior", "logicalnot", "mu", "trademark", "Eth", "onehalf",
    "plusminus", "Thorn", "onequarter", "divide", "brokenbar", "degree", "thorn", "threequarters", "twosuperior", "registered", "minus", "eth", "multiply",
    "threesuperior", "copyright", "Aacute", "Acircumflex", "Adieresis", "Agrave", "Aring", "Atilde", "Ccedilla", "Eacute", "Ecircumflex", "Edieresis", "Egrave",
    "Iacute", "Icircumflex", "Idieresis", "Igrave", "Ntilde", "Oacute", "Ocircumflex", "Odieresis", "Ograve", "Otilde", "Scaron", "Uacute", "Ucircumflex",
    "Udieresis", "Ugrave", "Yacute", "Ydieresis", "Zcaron", "aacute", "acircumflex", "adieresis", "agrave", "aring", "atilde", "ccedilla", "eacute", "ecircumflex",
    "edieresis", "egrave", "iacute", "icircumflex", "idieresis", "igrave", "ntilde", "oacute", "ocircumflex", "odieresis", "ograve", "otilde", "scaron", "uacute",
    "ucircumflex", "udieresis", "ugrave", "yacute", "ydieresis", "zcaron",
];

/// An INDEX at `at`: the byte ranges of its objects, and the offset just past it.
fn index(d: &[u8], at: usize) -> Option<(Vec<(usize, usize)>, usize)> {
    let count = u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]) as usize;
    if count == 0 {
        return Some((vec![], at + 2));
    }
    let osz = *d.get(at + 2)? as usize;
    if !(1..=4).contains(&osz) {
        return None;
    }
    let off = |i: usize| -> Option<usize> {
        let p = at + 3 + i * osz;
        let b = d.get(p..p + osz)?;
        Some(b.iter().fold(0usize, |a, &x| (a << 8) | x as usize))
    };
    let base = at + 3 + (count + 1) * osz - 1;
    let mut out = Vec::with_capacity(count.min(1 << 16));
    for i in 0..count {
        let (s, e) = (off(i)?, off(i + 1)?);
        if e < s || base + e > d.len() {
            return None;
        }
        out.push((base + s, base + e));
    }
    let end = base + off(count)?;
    Some((out, end))
}

/// A DICT as (operator, operands) pairs; the two-byte operators 12 x are numbered 1200 + x.
fn dict(d: &[u8]) -> Vec<(u16, Vec<f64>)> {
    let mut out = Vec::new();
    let mut ops: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let b = d[i];
        i += 1;
        match b {
            0..=21 => {
                let op = if b == 12 {
                    let x = d.get(i).copied().unwrap_or(0);
                    i += 1;
                    1200 + x as u16
                } else {
                    b as u16
                };
                out.push((op, std::mem::take(&mut ops)));
            }
            28 => {
                if i + 2 > d.len() {
                    break;
                }
                ops.push(i16::from_be_bytes([d[i], d[i + 1]]) as f64);
                i += 2;
            }
            29 => {
                if i + 4 > d.len() {
                    break;
                }
                ops.push(i32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as f64);
                i += 4;
            }
            30 => {
                // a real: nibbles until 0xf; the value is not needed here, only that it is skipped
                let mut s = String::new();
                'r: while i < d.len() {
                    let byte = d[i];
                    i += 1;
                    for n in [byte >> 4, byte & 15] {
                        match n {
                            0..=9 => s.push((b'0' + n) as char),
                            10 => s.push('.'),
                            11 => s.push('E'),
                            12 => s.push_str("E-"),
                            14 => s.push('-'),
                            15 => break 'r,
                            _ => {}
                        }
                    }
                }
                ops.push(s.parse().unwrap_or(0.0));
            }
            32..=246 => ops.push(b as f64 - 139.0),
            247..=250 => {
                let b1 = d.get(i).copied().unwrap_or(0) as f64;
                i += 1;
                ops.push((b as f64 - 247.0) * 256.0 + b1 + 108.0);
            }
            251..=254 => {
                let b1 = d.get(i).copied().unwrap_or(0) as f64;
                i += 1;
                ops.push(-(b as f64 - 251.0) * 256.0 - b1 - 108.0);
            }
            _ => {}
        }
    }
    out
}

pub struct Cff {
    /// The glyph names in glyph order (None for a glyph whose name is not known, and always for a CID-keyed font).
    pub glyph_names: Vec<Option<String>>,
    /// The built-in encoding: for each character code the glyph number, or None. `None` altogether means the Standard encoding.
    pub encoding: Option<Vec<Option<u16>>>,
    pub cid_keyed: bool,
}

impl Cff {
    pub fn parse(d: &[u8]) -> Option<Cff> {
        if d.len() < 4 || d[0] != 1 {
            return None;
        }
        let hdr = d[2] as usize;
        let (_names, p) = index(d, hdr)?;
        let (tops, p) = index(d, p)?;
        let (strings, _p) = index(d, p)?;
        let (s, e) = *tops.first()?;
        let top = dict(&d[s..e]);
        let get = |op: u16| top.iter().find(|(o, _)| *o == op).map(|(_, v)| v.clone());
        let cid_keyed = get(1230).is_some();
        let cs_off = get(17)?.first().copied()? as usize;
        let (charstrings, _) = index(d, cs_off)?;
        let n = charstrings.len();
        let name_of = |sid: usize| -> Option<String> {
            if sid < STD_NAMES.len() {
                Some(STD_NAMES[sid].to_string())
            } else if sid >= 391 {
                let (s, e) = *strings.get(sid - 391)?;
                std::str::from_utf8(&d[s..e]).ok().map(|x| x.to_string())
            } else {
                None
            }
        };
        // charset: the SID of each glyph (glyph 0 is .notdef)
        let cs = get(15).and_then(|v| v.first().copied()).unwrap_or(0.0) as usize;
        let mut sids: Vec<usize> = vec![0];
        if cs <= 2 {
            // the predefined charsets: ISOAdobe is SID = glyph number; the Expert sets are not listed here
            if cs == 0 {
                sids.extend(1..n.min(229));
            }
        } else {
            let fmt = *d.get(cs)?;
            let mut at = cs + 1;
            match fmt {
                0 => {
                    while sids.len() < n {
                        let v = u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]) as usize;
                        sids.push(v);
                        at += 2;
                    }
                }
                1 | 2 => {
                    while sids.len() < n {
                        let first = u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]) as usize;
                        let (left, w) = if fmt == 1 { (*d.get(at + 2)? as usize, 3) } else { (u16::from_be_bytes([*d.get(at + 2)?, *d.get(at + 3)?]) as usize, 4) };
                        at += w;
                        for k in 0..=left {
                            if sids.len() >= n {
                                break;
                            }
                            sids.push(first + k);
                        }
                    }
                }
                _ => return None,
            }
        }
        let glyph_names: Vec<Option<String>> = (0..n).map(|g| if cid_keyed { None } else { sids.get(g).and_then(|&s| name_of(s)) }).collect();
        // built-in encoding
        let enc_off = get(16).and_then(|v| v.first().copied()).unwrap_or(0.0) as usize;
        let encoding = if enc_off <= 1 || cid_keyed {
            None
        } else {
            let fmt = *d.get(enc_off)?;
            let mut table: Vec<Option<u16>> = vec![None; 256];
            let mut at = enc_off + 1;
            match fmt & 0x7f {
                0 => {
                    let nc = *d.get(at)? as usize;
                    at += 1;
                    for g in 0..nc {
                        let code = *d.get(at + g)? as usize;
                        table[code] = Some(g as u16 + 1);
                    }
                    at += nc;
                }
                1 => {
                    let nr = *d.get(at)? as usize;
                    at += 1;
                    let mut g = 1u16;
                    for r in 0..nr {
                        let first = *d.get(at + 2 * r)? as usize;
                        let left = *d.get(at + 2 * r + 1)? as usize;
                        for k in 0..=left {
                            if first + k < 256 {
                                table[first + k] = Some(g);
                            }
                            g += 1;
                        }
                    }
                    at += 2 * nr;
                }
                _ => return None,
            }
            if fmt & 0x80 != 0 {
                // supplements: a code and the SID of the glyph it draws
                let ns = *d.get(at)? as usize;
                at += 1;
                for k in 0..ns {
                    let code = *d.get(at + 3 * k)? as usize;
                    let sid = u16::from_be_bytes([*d.get(at + 3 * k + 1)?, *d.get(at + 3 * k + 2)?]) as usize;
                    if let Some(g) = sids.iter().position(|&s| s == sid) {
                        table[code] = Some(g as u16);
                    }
                }
            }
            Some(table)
        };
        Some(Cff { glyph_names, encoding, cid_keyed })
    }

    /// For each character code the glyph name, where the program's own encoding gives one. None when the program uses the Standard
    /// encoding (the caller's table is then right) or is CID-keyed.
    pub fn code_names(&self) -> Option<Vec<Option<String>>> {
        let enc = self.encoding.as_ref()?;
        Some(enc.iter().map(|g| g.and_then(|g| self.glyph_names.get(g as usize).cloned().flatten())).collect())
    }
}

#[cfg(test)]
pub(crate) mod build {
    //! A minimal CFF writer for the tests: names, a charset and an encoding, with one empty charstring per glyph.

    fn index(items: &[Vec<u8>]) -> Vec<u8> {
        if items.is_empty() {
            return vec![0, 0];
        }
        let mut out = (items.len() as u16).to_be_bytes().to_vec();
        out.push(2);
        let mut off = 1u16;
        out.extend_from_slice(&off.to_be_bytes());
        for it in items {
            off += it.len() as u16;
            out.extend_from_slice(&off.to_be_bytes());
        }
        for it in items {
            out.extend_from_slice(it);
        }
        out
    }

    fn num(v: i32) -> Vec<u8> {
        let mut o = vec![29];
        o.extend_from_slice(&v.to_be_bytes());
        o
    }

    /// `glyphs`: the names of glyphs 1.. (all custom strings); `codes`: for each of those glyphs the character code. `format1`
    /// writes the charset as ranges and the encoding as ranges when the codes are consecutive; `supplement`: (code, glyph name) pairs.
    pub fn font(glyphs: &[&str], codes: &[u8], supplement: &[(u8, &str)]) -> Vec<u8> {
        let header = vec![1u8, 0, 4, 2];
        let name_index = index(&[b"Test".to_vec()]);
        let mut strings: Vec<Vec<u8>> = glyphs.iter().map(|g| g.as_bytes().to_vec()).collect();
        for (_, s) in supplement {
            strings.push(s.as_bytes().to_vec());
        }
        let string_index = index(&strings);
        let gsubr = index(&[]);
        let charstrings = index(&vec![vec![14u8]; glyphs.len() + 1]);
        // the charset: format 0, a SID (391 + i) per glyph
        let mut charset = vec![0u8];
        for i in 0..glyphs.len() {
            charset.extend_from_slice(&((391 + i) as u16).to_be_bytes());
        }
        // the encoding: format 0 (and the supplement flag when there are supplements)
        let mut encoding = vec![if supplement.is_empty() { 0u8 } else { 0x80 }, codes.len() as u8];
        encoding.extend_from_slice(codes);
        if !supplement.is_empty() {
            encoding.push(supplement.len() as u8);
            for (k, (code, _)) in supplement.iter().enumerate() {
                encoding.push(*code);
                encoding.extend_from_slice(&((391 + glyphs.len() + k) as u16).to_be_bytes());
            }
        }
        // the top DICT has fixed-size operands so its length does not depend on the offsets
        let top_len = 3 * 5 + 3 + 2; // three 5-byte operands and operators 15, 16, 17 (1 byte each) plus the index header added below
        let _ = top_len;
        let base = header.len() + name_index.len();
        let top_probe = index(&[vec![0u8; 5 * 3 + 3]]);
        let after_top = base + top_probe.len() + string_index.len() + gsubr.len();
        let cs_at = after_top;
        let charset_at = cs_at + charstrings.len();
        let enc_at = charset_at + charset.len();
        let mut top = Vec::new();
        top.extend(num(charset_at as i32));
        top.push(15);
        top.extend(num(enc_at as i32));
        top.push(16);
        top.extend(num(cs_at as i32));
        top.push(17);
        let top_index = index(&[top]);
        assert_eq!(top_index.len(), top_probe.len());
        let mut out = header;
        out.extend(name_index);
        out.extend(top_index);
        out.extend(string_index);
        out.extend(gsubr);
        out.extend(charstrings);
        out.extend(charset);
        out.extend(encoding);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_a_private_encoding_are_read_back() {
        let f = build::font(&["alpha", "g17", "beta"], &[1, 2, 7], &[]);
        let c = Cff::parse(&f).unwrap();
        assert_eq!(c.glyph_names, vec![Some(".notdef".into()), Some("alpha".into()), Some("g17".into()), Some("beta".into())]);
        let names = c.code_names().unwrap();
        assert_eq!(names[1].as_deref(), Some("alpha"));
        assert_eq!(names[2].as_deref(), Some("g17"));
        assert_eq!(names[7].as_deref(), Some("beta"));
        assert_eq!(names[3], None);
    }

    #[test]
    fn supplemental_codes_point_at_their_glyph() {
        let f = build::font(&["one"], &[1], &[(9, "one")]);
        let c = Cff::parse(&f).unwrap();
        assert_eq!(c.code_names().unwrap()[1].as_deref(), Some("one"));
    }

    #[test]
    fn standard_names_are_in_the_order_of_the_note() {
        assert_eq!(STD_NAMES[1], "space");
        assert_eq!(STD_NAMES[34], "A");
        assert_eq!(STD_NAMES[66], "a");
        assert_eq!(STD_NAMES[96], "exclamdown");
        assert_eq!(STD_NAMES[149], "germandbls");
        assert_eq!(STD_NAMES[228], "zcaron");
        assert_eq!(STD_NAMES.len(), 229);
    }

    #[test]
    fn damaged_programs_are_refused_and_never_panic() {
        let f = build::font(&["alpha", "beta"], &[1, 2], &[]);
        for cut in 0..f.len() {
            let _ = Cff::parse(&f[..cut]);
        }
        let mut seed = 99u32;
        for _ in 0..300 {
            let mut g = f.clone();
            for _ in 0..4 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let i = (seed >> 8) as usize % g.len();
                g[i] = (seed >> 24) as u8;
            }
            let _ = Cff::parse(&g).map(|c| c.code_names());
        }
        assert!(Cff::parse(b"").is_none());
    }
}

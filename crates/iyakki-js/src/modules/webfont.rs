//! Glyph widths and kerning read from the page's own web fonts (F-BROX-19), so the box estimate of a page that sets its text in a font it
//! loads is made with that font's advances and not with the common sans or serif table (`glyphs`).
//!
//! Read from the font file itself, in-house, from the published TrueType tables: `head` (units per em), `hhea` and `hmtx` (advances),
//! `cmap` (formats 4 and 12), `kern` (format 0 pairs) and the `kern` feature of `GPOS` (pair adjustment, formats 1 and 2, also behind an
//! extension lookup). Plain TrueType and OpenType files ("\0\1\0\0", "true", "OTTO") are read, and WOFF files (zlib per table, unpacked
//! with `miniz_oxide`, which the reader already uses for PNG) and WOFF2 files (`woff2`: Brotli per file, outlines not rebuilt, the metrics tables only). NOT read: font collections. A file that
//! cannot be read is simply not used: the text is measured with the common tables as before. Every length is checked; a hostile file gives
//! `None`, never a panic.

use std::collections::HashMap;

const MAX_GLYPHS: usize = 70_000;
const MAX_KERN_PAIRS: usize = 200_000;
const MAX_CMAP_ENTRIES: usize = 300_000;

pub struct FontMetrics {
    upem: f32,
    advance: Vec<u16>,
    cmap: HashMap<u32, u16>,
    kern: HashMap<(u16, u16), i16>,
    /// `GPOS` class-based pair adjustment subtables, in lookup order.
    class_kern: Vec<ClassKern>,
}

/// One `GPOS` PairPos format 2 subtable: the first glyphs it covers, the classes of both glyphs and the matrix of x advances.
struct ClassKern {
    covered: HashMap<u16, u16>,
    class1: HashMap<u16, u16>,
    class2: HashMap<u16, u16>,
    class2_count: usize,
    values: Vec<i16>,
}

const MAX_WOFF_OUT: usize = 16 * 1024 * 1024;

/// Unpack a WOFF 1.0 file into a plain sfnt file (tables stored uncompressed); `None` for anything damaged or too large.
fn unwoff(d: &[u8]) -> Option<Vec<u8>> {
    if d.get(..4)? != b"wOFF" {
        return None;
    }
    let flavor = d.get(4..8)?;
    let n = u16_at(d, 12)? as usize;
    if n == 0 || n > 64 {
        return None;
    }
    let mut dirs: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut total = 0usize;
    for i in 0..n {
        let r = 44 + i * 20;
        let tag: [u8; 4] = d.get(r..r + 4)?.try_into().ok()?;
        let (off, comp, orig) = (u32_at(d, r + 4)? as usize, u32_at(d, r + 8)? as usize, u32_at(d, r + 12)? as usize);
        total = total.checked_add(orig)?;
        if total > MAX_WOFF_OUT || comp > orig {
            return None;
        }
        let raw = d.get(off..off.checked_add(comp)?)?;
        let data = if comp < orig {
            let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(raw, orig).ok()?;
            if out.len() != orig {
                return None;
            }
            out
        } else {
            raw.to_vec()
        };
        dirs.push((tag, data));
    }
    let mut out = Vec::with_capacity(total + 12 + n * 16);
    out.extend_from_slice(flavor);
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&[0u8; 6]);
    let mut off = 12 + n * 16;
    for (tag, data) in &dirs {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        off += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &dirs {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Some(out)
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn table<'a>(d: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = u16_at(d, 4)? as usize;
    for i in 0..n.min(64) {
        let r = 12 + i * 16;
        if d.get(r..r + 4)? == tag {
            let (off, len) = (u32_at(d, r + 8)? as usize, u32_at(d, r + 12)? as usize);
            return d.get(off..off.checked_add(len)?);
        }
    }
    None
}

impl FontMetrics {
    /// Read the metrics of a TrueType or OpenType file; `None` for anything else (a collection, a damaged file).
    pub fn parse(d: &[u8]) -> Option<FontMetrics> {
        if d.get(..4) == Some(b"wOFF") {
            return Self::parse_sfnt(&unwoff(d)?);
        }
        if d.get(..4) == Some(b"wOF2") {
            return Self::parse_sfnt(&super::woff2::unwoff2(d)?);
        }
        Self::parse_sfnt(d)
    }

    fn parse_sfnt(d: &[u8]) -> Option<FontMetrics> {
        match d.get(..4)? {
            [0, 1, 0, 0] | b"true" | b"OTTO" => {}
            _ => return None,
        }
        let head = table(d, b"head")?;
        let upem = u16_at(head, 18)?;
        if !(16..=16384).contains(&upem) {
            return None;
        }
        let hhea = table(d, b"hhea")?;
        let n_metrics = u16_at(hhea, 34)? as usize;
        let n_glyphs = u16_at(table(d, b"maxp")?, 4)? as usize;
        if n_metrics == 0 || n_glyphs == 0 || n_glyphs > MAX_GLYPHS {
            return None;
        }
        let hmtx = table(d, b"hmtx")?;
        let mut advance = Vec::with_capacity(n_glyphs);
        for g in 0..n_glyphs {
            let at = g.min(n_metrics - 1);
            advance.push(u16_at(hmtx, at * 4)?);
        }
        let cmap = read_cmap(table(d, b"cmap")?)?;
        let mut kern = HashMap::new();
        let mut class_kern = Vec::new();
        if let Some(g) = table(d, b"GPOS") {
            read_gpos(g, &mut kern, &mut class_kern);
        }
        if kern.is_empty() && class_kern.is_empty() {
            kern = table(d, b"kern").map(read_kern).unwrap_or_default();
        }
        Some(FontMetrics { upem: f32::from(upem), advance, cmap, kern, class_kern })
    }

    fn pair(&self, l: u16, r: u16) -> i16 {
        if let Some(v) = self.kern.get(&(l, r)) {
            return *v;
        }
        for c in &self.class_kern {
            if c.covered.contains_key(&l) {
                let (a, b) = (usize::from(c.class1.get(&l).copied().unwrap_or(0)), usize::from(c.class2.get(&r).copied().unwrap_or(0)));
                return c.values.get(a * c.class2_count + b).copied().unwrap_or(0);
            }
        }
        0
    }

    fn glyph(&self, c: char) -> Option<u16> {
        self.cmap.get(&(c as u32)).copied().filter(|&g| usize::from(g) < self.advance.len())
    }

    /// The width of `text` in ems: the advance of every character (a character the font has no glyph for takes the width of glyph 0, the
    /// missing-glyph box, as a browser draws it) plus the kerning of each pair. White space is collapsed to one space as in `glyphs`.
    pub fn text_em(&self, text: &str) -> f32 {
        let (mut w, mut prev_space, mut prev): (f32, bool, Option<u16>) = (0.0, false, None);
        for c in text.chars() {
            if c.is_control() {
                continue;
            }
            let c = if c.is_whitespace() {
                if prev_space {
                    continue;
                }
                prev_space = true;
                ' '
            } else {
                prev_space = false;
                c
            };
            let g = self.glyph(c).unwrap_or(0);
            w += f32::from(self.advance[usize::from(g)]);
            if let Some(p) = prev {
                w += f32::from(self.pair(p, g));
            }
            prev = Some(g);
        }
        w / self.upem
    }
}

fn read_cmap(t: &[u8]) -> Option<HashMap<u32, u16>> {
    let n = u16_at(t, 2)? as usize;
    // prefer a Unicode full repertoire (format 12), then a BMP table (format 4)
    let mut best: Option<(u8, usize)> = None;
    for i in 0..n.min(32) {
        let r = 4 + i * 8;
        let (pid, eid, off) = (u16_at(t, r)?, u16_at(t, r + 2)?, u32_at(t, r + 4)? as usize);
        let fmt = u16_at(t, off)?;
        let rank = match (pid, eid, fmt) {
            (3, 10, 12) | (0, 4 | 6, 12) => 3,
            (3, 1, 4) | (0, _, 4) => 2,
            (_, _, 12) => 1,
            _ => 0,
        };
        if rank > 0 && best.is_none_or(|(b, _)| rank > b) {
            best = Some((rank, off));
        }
    }
    let (_, off) = best?;
    let sub = t.get(off..)?;
    let mut map = HashMap::new();
    match u16_at(sub, 0)? {
        4 => {
            let segs = u16_at(sub, 6)? as usize / 2;
            let (ends, starts, deltas, offs) = (14, 16 + segs * 2, 16 + segs * 4, 16 + segs * 6);
            for s in 0..segs.min(4096) {
                let end = u16_at(sub, ends + s * 2)?;
                let start = u16_at(sub, starts + s * 2)?;
                let delta = u16_at(sub, deltas + s * 2)?;
                let ro = u16_at(sub, offs + s * 2)?;
                if start > end {
                    continue;
                }
                for c in start..=end {
                    if c == 0xffff {
                        break;
                    }
                    let g = if ro == 0 {
                        c.wrapping_add(delta)
                    } else {
                        let at = offs + s * 2 + usize::from(ro) + usize::from(c - start) * 2;
                        match u16_at(sub, at)? {
                            0 => 0,
                            g => g.wrapping_add(delta),
                        }
                    };
                    if g != 0 {
                        map.insert(u32::from(c), g);
                        if map.len() > MAX_CMAP_ENTRIES {
                            return None;
                        }
                    }
                }
            }
        }
        12 => {
            let groups = u32_at(sub, 12)? as usize;
            for gi in 0..groups.min(65_536) {
                let r = 16 + gi * 12;
                let (start, end, g0) = (u32_at(sub, r)?, u32_at(sub, r + 4)?, u32_at(sub, r + 8)?);
                if start > end || end > 0x10ffff {
                    continue;
                }
                for c in start..=end {
                    let g = g0.checked_add(c - start)?;
                    if let Ok(g) = u16::try_from(g) {
                        map.insert(c, g);
                    }
                    if map.len() > MAX_CMAP_ENTRIES {
                        return None;
                    }
                }
            }
        }
        _ => return None,
    }
    (!map.is_empty()).then_some(map)
}

/// Format 0 kerning subtables of the old `kern` table (horizontal, not minimum, not cross-stream).
fn read_kern(t: &[u8]) -> HashMap<(u16, u16), i16> {
    let mut out = HashMap::new();
    let n = u16_at(t, 2).unwrap_or(0) as usize;
    let mut at = 4;
    for _ in 0..n.min(8) {
        let (Some(len), Some(cov)) = (u16_at(t, at + 2), u16_at(t, at + 4)) else { break };
        let (format, horizontal, minimum) = (cov >> 8, cov & 1 != 0, cov & 2 != 0);
        if format == 0 && horizontal && !minimum {
            let pairs = u16_at(t, at + 6).unwrap_or(0) as usize;
            for p in 0..pairs {
                let r = at + 14 + p * 6;
                let (Some(l), Some(rt), Some(v)) = (u16_at(t, r), u16_at(t, r + 2), u16_at(t, r + 4)) else { break };
                if out.len() >= MAX_KERN_PAIRS {
                    break;
                }
                out.insert((l, rt), v as i16);
            }
        }
        if len == 0 {
            break;
        }
        at += usize::from(len);
    }
    out
}

fn coverage(t: &[u8], at: usize) -> Option<HashMap<u16, u16>> {
    let mut m = HashMap::new();
    match u16_at(t, at)? {
        1 => {
            for i in 0..u16_at(t, at + 2)? as usize {
                m.insert(u16_at(t, at + 4 + i * 2)?, i as u16);
            }
        }
        2 => {
            for i in 0..u16_at(t, at + 2)? as usize {
                let r = at + 4 + i * 6;
                let (a, b, s) = (u16_at(t, r)?, u16_at(t, r + 2)?, u16_at(t, r + 4)?);
                if b < a || m.len() + usize::from(b - a) > MAX_GLYPHS {
                    return None;
                }
                for g in a..=b {
                    m.insert(g, s.wrapping_add(g - a));
                }
            }
        }
        _ => return None,
    }
    Some(m)
}

fn class_def(t: &[u8], at: usize) -> Option<HashMap<u16, u16>> {
    let mut m = HashMap::new();
    match u16_at(t, at)? {
        1 => {
            let start = u16_at(t, at + 2)?;
            for i in 0..u16_at(t, at + 4)? as usize {
                m.insert(start.checked_add(i as u16)?, u16_at(t, at + 6 + i * 2)?);
            }
        }
        2 => {
            for i in 0..u16_at(t, at + 2)? as usize {
                let r = at + 4 + i * 6;
                let (a, b, c) = (u16_at(t, r)?, u16_at(t, r + 2)?, u16_at(t, r + 4)?);
                if b < a || m.len() + usize::from(b - a) > MAX_GLYPHS {
                    return None;
                }
                for g in a..=b {
                    m.insert(g, c);
                }
            }
        }
        _ => return None,
    }
    Some(m)
}

/// The x advance inside a value record of `fmt` (bit 0x4), and the size of the record in bytes.
fn xadv_slot(fmt: u16) -> (Option<usize>, usize) {
    let size = fmt.count_ones() as usize * 2;
    (if fmt & 4 != 0 { Some((fmt & 3).count_ones() as usize * 2) } else { None }, size)
}

/// The pair adjustments of the lookups the `kern` feature names (lookup type 2, also as the target of an extension lookup, type 9).
fn read_gpos(t: &[u8], pairs: &mut HashMap<(u16, u16), i16>, classes: &mut Vec<ClassKern>) {
    let mut go = || -> Option<()> {
        let (flist, llist) = (u16_at(t, 6)? as usize, u16_at(t, 8)? as usize);
        let mut wanted: Vec<usize> = Vec::new();
        for i in 0..u16_at(t, flist)?.min(256) as usize {
            let r = flist + 2 + i * 6;
            if t.get(r..r + 4)? == b"kern" {
                let ft = flist + u16_at(t, r + 4)? as usize;
                for j in 0..u16_at(t, ft + 2)?.min(256) as usize {
                    wanted.push(u16_at(t, ft + 4 + j * 2)? as usize);
                }
            }
        }
        for li in wanted {
            let lt = llist + u16_at(t, llist + 2 + li * 2)? as usize;
            let (lty, n) = (u16_at(t, lt)?, u16_at(t, lt + 4)?.min(256) as usize);
            for s in 0..n {
                let mut ty = lty;
                let mut st = lt + u16_at(t, lt + 6 + s * 2)? as usize;
                if ty == 9 {
                    ty = u16_at(t, st + 2)?;
                    st += u32_at(t, st + 4)? as usize;
                }
                if ty == 2 {
                    let _ = pair_pos(t, st, pairs, classes);
                }
            }
        }
        Some(())
    };
    let _ = go();
}

fn pair_pos(t: &[u8], st: usize, pairs: &mut HashMap<(u16, u16), i16>, classes: &mut Vec<ClassKern>) -> Option<()> {
    let (fmt, cov) = (u16_at(t, st)?, st + u16_at(t, st + 2)? as usize);
    let (f1, f2) = (u16_at(t, st + 4)?, u16_at(t, st + 6)?);
    let ((slot, s1), (_, s2)) = (xadv_slot(f1), xadv_slot(f2));
    let slot = slot?;
    let covered = coverage(t, cov)?;
    if fmt == 1 {
        let mut order: Vec<(u16, u16)> = covered.iter().map(|(g, i)| (*i, *g)).collect();
        order.sort_unstable();
        for i in 0..u16_at(t, st + 8)? as usize {
            let ps = st + u16_at(t, st + 10 + i * 2)? as usize;
            let Some((_, first)) = order.get(i) else { continue };
            for k in 0..u16_at(t, ps)? as usize {
                let r = ps + 2 + k * (2 + s1 + s2);
                let v = u16_at(t, r + 2 + slot)? as i16;
                if pairs.len() < MAX_KERN_PAIRS {
                    pairs.entry((*first, u16_at(t, r)?)).or_insert(v);
                }
            }
        }
    } else if fmt == 2 {
        let (c1, c2) = (st + u16_at(t, st + 8)? as usize, st + u16_at(t, st + 10)? as usize);
        let (n1, n2) = (u16_at(t, st + 12)? as usize, u16_at(t, st + 14)? as usize);
        if n1 * n2 > MAX_KERN_PAIRS || classes.len() >= 64 {
            return None;
        }
        let mut values = Vec::with_capacity(n1 * n2);
        for k in 0..n1 * n2 {
            values.push(u16_at(t, st + 16 + k * (s1 + s2) + slot)? as i16);
        }
        classes.push(ClassKern { covered, class1: class_def(t, c1)?, class2: class_def(t, c2)?, class2_count: n2, values });
    }
    Some(())
}

/// The fonts a page loaded, by lower-case family name.
#[derive(Default, Clone)]
pub struct FontSet {
    fonts: std::sync::Arc<HashMap<String, std::sync::Arc<FontMetrics>>>,
}

impl FontSet {
    pub fn new(fonts: HashMap<String, std::sync::Arc<FontMetrics>>) -> FontSet {
        FontSet { fonts: std::sync::Arc::new(fonts) }
    }

    pub fn get(&self, family: &str) -> Option<&FontMetrics> {
        if family.is_empty() {
            return None;
        }
        self.fonts.get(family).map(|f| &**f)
    }

    /// The face of `family` for a weight and a slant: the one that matches both, else the one of the same weight, else the one of the same
    /// slant, else the regular one (a browser draws a bold or slanted copy of that). The second value says whether the face found IS bold,
    /// so the caller adds the width of a synthetic bold only when it is not.
    pub fn get_styled(&self, family: &str, bold: bool, italic: bool) -> Option<(&FontMetrics, bool)> {
        if family.is_empty() {
            return None;
        }
        let tries = [(bold, italic), (bold, false), (false, italic), (false, false)];
        tries.iter().find_map(|&(b, i)| self.fonts.get(&variant_key(family, b, i)).map(|f| (&**f, b)))
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }
}

/// The key of one face of a family in a [`FontSet`]: the family alone for the regular face, with `\u{1}b` and/or `\u{1}i` for bold and
/// italic ones.
pub fn variant_key(family: &str, bold: bool, italic: bool) -> String {
    match (bold, italic) {
        (false, false) => family.to_string(),
        (true, false) => format!("{family}\u{1}b"),
        (false, true) => format!("{family}\u{1}i"),
        (true, true) => format!("{family}\u{1}bi"),
    }
}

/// One face a style sheet declares: its family, the address of the file and which weight and slant it is for.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FaceRule {
    pub family: String,
    pub url: String,
    pub bold: bool,
    pub italic: bool,
}

/// The weights a `font-weight` descriptor covers as (regular, bold): a word, one number (600 and above is bold) or a range of a variable
/// font (it serves both when it spans 599 and 600). No descriptor means the regular face.
fn weights_of(v: Option<&str>) -> (bool, bool) {
    let Some(v) = v else { return (true, false) };
    let nums: Vec<u32> = v.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    match nums.as_slice() {
        [] => {
            let w = v.trim().to_ascii_lowercase();
            if w.starts_with("bold") { (false, true) } else { (true, false) }
        }
        [one] => (*one < 600, *one >= 600),
        [lo, hi, ..] => (*lo < 600, *hi >= 600),
    }
}

/// The `@font-face` rules of a style sheet's text: (family, address of the first `url(...)` source). Only what a rule says in plain text;
/// `local()` sources and rules without a url are skipped, at most 32 rules.
pub fn font_faces(css: &str) -> Vec<(String, String)> {
    font_face_rules(css).into_iter().map(|r| (r.family, r.url)).collect()
}

/// The same rules with the weight and slant each is for (a rule for a range of weights gives a face for the regular and one for the bold
/// weight, with the same file).
pub fn font_face_rules(css: &str) -> Vec<FaceRule> {
    let mut out = Vec::new();
    let lower = css.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("@font-face") {
        let start = from + i;
        let Some(open) = lower[start..].find('{') else { break };
        let Some(close) = lower[start + open..].find('}') else { break };
        let body = &css[start + open + 1..start + open + close];
        from = start + open + close + 1;
        let decl = |name: &str| -> Option<&str> {
            let lb = body.to_ascii_lowercase();
            let mut p = 0;
            while let Some(i) = lb[p..].find(name) {
                let at = p + i;
                let before_ok = at == 0 || !lb.as_bytes()[at - 1].is_ascii_alphanumeric() && lb.as_bytes()[at - 1] != b'-';
                let rest = lb[at + name.len()..].trim_start();
                if before_ok && rest.starts_with(':') {
                    let vstart = at + name.len() + (lb[at + name.len()..].len() - rest.len()) + 1;
                    let vend = lb[vstart..].find(';').map_or(lb.len(), |e| vstart + e);
                    return Some(body[vstart..vend].trim());
                }
                p = at + name.len();
            }
            None
        };
        let (Some(fam), Some(src)) = (decl("font-family"), decl("src")) else { continue };
        let family = fam.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_ascii_lowercase();
        // every url(...) source with its format hint; one this reader can open (not WOFF2) is preferred, the first one otherwise
        let low = src.to_ascii_lowercase();
        let mut cands: Vec<(String, bool)> = Vec::new();
        let mut at = 0;
        while let Some(u) = low[at..].find("url(") {
            let begin = at + u + 4;
            let Some(end) = src[begin..].find(')') else { break };
            let url = src[begin..begin + end].trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string();
            let tail_end = low[begin + end..].find("url(").map_or(low.len(), |e| begin + end + e);
            let hint = &low[begin + end..tail_end];
            let woff2 = hint.contains("woff2") || low[begin..begin + end].split('?').next().is_some_and(|p| p.trim_end_matches(['"', '\'']).ends_with(".woff2"));
            if !url.is_empty() {
                cands.push((url, woff2));
            }
            at = begin + end;
        }
        let Some((url, _)) = cands.iter().find(|c| !c.1).or(cands.first()).cloned() else { continue };
        if !family.is_empty() && !url.is_empty() {
            let italic = decl("font-style").is_some_and(|v| {
                let v = v.trim().to_ascii_lowercase();
                v.starts_with("italic") || v.starts_with("oblique")
            });
            let (regular, bold) = weights_of(decl("font-weight"));
            if regular {
                out.push(FaceRule { family: family.clone(), url: url.clone(), bold: false, italic });
            }
            if bold {
                out.push(FaceRule { family, url, bold: true, italic });
            }
        }
        if out.len() >= 32 {
            break;
        }
    }
    out
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::modules) fn be16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn be32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    /// A small TrueType file made here: 1000 units per em, glyphs 0 (missing, 500), 1 = 'A' (700), 2 = 'V' (700), 3 = space (250),
    /// kerning A V by -140.
    pub(in crate::modules) fn font() -> Vec<u8> {
        font_with(true, None)
    }

    fn font_with(with_kern: bool, gpos: Option<Vec<u8>>) -> Vec<u8> {
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[34..36].copy_from_slice(&4u16.to_be_bytes());
        let mut maxp = vec![0u8; 6];
        maxp[4..6].copy_from_slice(&4u16.to_be_bytes());
        let mut hmtx = Vec::new();
        for a in [500u16, 700, 700, 250] {
            be16(&mut hmtx, a);
            be16(&mut hmtx, 0);
        }
        // cmap format 4: segments [32,32] -> 3, [65,65] -> 1, [86,86] -> 2, and the end segment
        let segs: [(u16, u16, u16); 4] = [(32, 32, 3u16.wrapping_sub(32)), (65, 65, 1u16.wrapping_sub(65)), (86, 86, 2u16.wrapping_sub(86)), (0xffff, 0xffff, 1)];
        let mut sub = Vec::new();
        be16(&mut sub, 4);
        be16(&mut sub, (16 + segs.len() * 8) as u16);
        be16(&mut sub, 0);
        be16(&mut sub, (segs.len() * 2) as u16);
        be16(&mut sub, 0);
        be16(&mut sub, 0);
        be16(&mut sub, 0);
        for s in &segs {
            be16(&mut sub, s.1);
        }
        be16(&mut sub, 0);
        for s in &segs {
            be16(&mut sub, s.0);
        }
        for s in &segs {
            be16(&mut sub, s.2);
        }
        for _ in &segs {
            be16(&mut sub, 0);
        }
        let mut cmap = Vec::new();
        be16(&mut cmap, 0);
        be16(&mut cmap, 1);
        be16(&mut cmap, 3);
        be16(&mut cmap, 1);
        be32(&mut cmap, 12);
        cmap.extend_from_slice(&sub);
        // kern: one format 0 subtable, one pair
        let mut kern = Vec::new();
        be16(&mut kern, 0);
        be16(&mut kern, 1);
        be16(&mut kern, 0);
        be16(&mut kern, 20);
        be16(&mut kern, 1);
        be16(&mut kern, 1);
        be16(&mut kern, 0);
        be16(&mut kern, 0);
        be16(&mut kern, 0);
        be16(&mut kern, 1);
        be16(&mut kern, 2);
        be16(&mut kern, (-140i16) as u16);
        let mut tables: Vec<(&[u8; 4], &Vec<u8>)> = vec![(b"cmap", &cmap), (b"head", &head), (b"hhea", &hhea), (b"hmtx", &hmtx), (b"maxp", &maxp)];
        if with_kern {
            tables.push((b"kern", &kern));
        }
        if let Some(g) = &gpos {
            tables.push((b"GPOS", g));
        }
        let mut out = vec![0, 1, 0, 0];
        be16(&mut out, tables.len() as u16);
        out.extend_from_slice(&[0u8; 6]);
        let mut off = 12 + tables.len() * 16;
        for (tag, data) in &tables {
            out.extend_from_slice(*tag);
            be32(&mut out, 0);
            be32(&mut out, off as u32);
            be32(&mut out, data.len() as u32);
            off += data.len().div_ceil(4) * 4;
        }
        for (_, data) in &tables {
            out.extend_from_slice(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
        out
    }

    #[test]
    fn a_font_gives_advances_kerning_and_the_missing_glyph_width() {
        let f = FontMetrics::parse(&font()).expect("the file is read");
        assert!((f.text_em("A") - 0.7).abs() < 1e-6);
        assert!((f.text_em("AV") - (0.7 + 0.7 - 0.14)).abs() < 1e-6, "the pair is kerned");
        assert!((f.text_em("VA") - 1.4).abs() < 1e-6, "the other order is not");
        assert!((f.text_em("A   A") - (0.7 + 0.25 + 0.7)).abs() < 1e-6, "white space collapses");
        assert!((f.text_em("Q") - 0.5).abs() < 1e-6, "a character the font lacks takes the missing glyph");
    }

    #[test]
    fn hostile_or_unsupported_files_are_refused_and_never_panic() {
        assert!(FontMetrics::parse(b"wOFF\0\0\0\0").is_none() && FontMetrics::parse(b"wOF2").is_none() && FontMetrics::parse(&[]).is_none());
        let good = font();
        for cut in (0..good.len()).step_by(7) {
            let _ = FontMetrics::parse(&good[..cut]);
        }
        for i in (0..good.len()).step_by(3) {
            let mut v = good.clone();
            v[i] ^= 0xff;
            let _ = FontMetrics::parse(&v).map(|f| f.text_em("AV A"));
        }
    }

    #[test]
    fn font_face_rules_give_the_family_and_the_first_url() {
        let css = "p{color:red} @font-face { font-family: 'My Face'; src: local(Foo), url(\"/f/my.ttf\") format('truetype'), url(b.woff2); font-weight: 400 } @font-face{font-family:x;src:local(y)} @FONT-FACE{FONT-FAMILY:Other;SRC:url(o.otf)}";
        assert_eq!(font_faces(css), vec![("my face".to_string(), "/f/my.ttf".to_string()), ("other".to_string(), "o.otf".to_string())]);
        // a WOFF2 source is passed over for one that can be read
        let two = "@font-face{font-family:a;src:url(a.woff2) format('woff2'),url(a.woff) format('woff')} @font-face{font-family:b;src:url(b.woff2?v=3)} @font-face{font-family:c;src:url(\"c.woff2\"),url(c.ttf)}";
        assert_eq!(font_faces(two), vec![("a".to_string(), "a.woff".to_string()), ("b".to_string(), "b.woff2?v=3".to_string()), ("c".to_string(), "c.ttf".to_string())]);
        assert!(font_faces("@font-face{").is_empty());
    }

    #[test]
    fn font_face_rules_say_which_weight_and_slant_each_face_is_for() {
        let css = "@font-face{font-family:A;src:url(r.ttf)} @font-face{font-family:A;src:url(b.ttf);font-weight:700} @font-face{font-family:A;src:url(i.ttf);font-style:italic} @font-face{font-family:'A';src:url(bi.ttf);font-weight:bold;font-style:oblique 10deg} @font-face{font-family:V;src:url(v.ttf);font-weight:100 900} @font-face{font-family:L;src:url(l.ttf);font-weight:300}";
        let got: Vec<(String, String, bool, bool)> = font_face_rules(css).into_iter().map(|r| (r.family, r.url, r.bold, r.italic)).collect();
        let want = |f: &str, u: &str, b: bool, i: bool| (f.to_string(), u.to_string(), b, i);
        assert_eq!(got, vec![want("a", "r.ttf", false, false), want("a", "b.ttf", true, false), want("a", "i.ttf", false, true), want("a", "bi.ttf", true, true), want("v", "v.ttf", false, false), want("v", "v.ttf", true, false), want("l", "l.ttf", false, false)]);
        assert_eq!(font_faces(css).len(), 7, "the older call lists the same faces");
    }

    #[test]
    fn a_face_is_taken_by_weight_and_slant_and_the_regular_one_stands_in_for_the_rest() {
        let f = |w: u16| std::sync::Arc::new(FontMetrics { upem: 1000.0, advance: vec![w], cmap: HashMap::new(), kern: HashMap::new(), class_kern: Vec::new() });
        let mut m = HashMap::new();
        m.insert(variant_key("a", false, false), f(1));
        m.insert(variant_key("a", true, false), f(2));
        m.insert(variant_key("b", false, false), f(3));
        m.insert(variant_key("b", false, true), f(4));
        let set = FontSet::new(m);
        let adv = |fam: &str, b: bool, i: bool| set.get_styled(fam, b, i).map(|(f, is_bold)| (f.advance[0], is_bold));
        assert_eq!(adv("a", false, false), Some((1, false)));
        assert_eq!(adv("a", true, false), Some((2, true)), "the bold face is used and is bold");
        assert_eq!(adv("a", true, true), Some((2, true)), "no italic bold: the bold one");
        assert_eq!(adv("a", false, true), Some((1, false)), "no italic: the regular one");
        assert_eq!(adv("b", true, false), Some((3, false)), "no bold: the regular one, which is not bold, so the caller makes it wider");
        assert_eq!(adv("b", false, true), Some((4, false)));
        assert_eq!(adv("c", false, false), None);
        assert_eq!(adv("", false, false), None);
    }

    struct Faces(Vec<(&'static str, Vec<u8>)>);
    impl iyakki_web::Fetch for Faces {
        fn get(&self, url: &iyakki_web::Url) -> Result<iyakki_web::fetch::Response, String> {
            match self.0.iter().find(|(p, _)| *p == url.path) {
                Some((_, b)) => Ok(iyakki_web::fetch::Response { status: 200, content_type: "font/ttf".into(), body: b.clone(), ..Default::default() }),
                None => Err("offline".into()),
            }
        }
    }

    fn width_with_faces(html: &str) -> f32 {
        use crate::{run_page_with, Limits, SiteData};
        // the regular file kerns A V (width 126 at 100 px), the bold one does not (140)
        let web: std::sync::Arc<dyn iyakki_web::Fetch> = std::sync::Arc::new(Faces(vec![("/f/r.ttf", font()), ("/f/b.ttf", font_with(false, None))]));
        let data = SiteData { viewport: Some(iyakki_web::css::Viewport { width: 1280.0, height: 720.0, ..Default::default() }), ..SiteData::default() };
        let mut out = String::new();
        let res = run_page_with(html, &iyakki_web::Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
            out = e.eval("t", "String(document.getElementById('s').offsetWidth)").unwrap_or_else(|x| format!("THROWN {x}"));
        });
        assert_eq!(res.report.failed, 0, "{:?}", res.report.notes);
        out.parse().unwrap_or_else(|_| panic!("{out}"))
    }

    #[test]
    fn bold_text_in_a_page_font_uses_the_pages_bold_face_when_it_has_one_and_a_wider_regular_one_when_it_has_not() {
        let both = "<style>@font-face{font-family:F;src:url(/f/r.ttf)} @font-face{font-family:F;src:url(/f/b.ttf);font-weight:700} #s{display:inline-block;font-family:F;font-size:100px}</style>";
        assert_eq!(width_with_faces(&format!("{both}<span id=s>AV</span>")), 126.0, "regular text: the regular face");
        assert_eq!(width_with_faces(&format!("{both}<b id=s>AV</b>")), 140.0, "bold text: the page's bold face, measured as it is");
        let regular_only = "<style>@font-face{font-family:F;src:url(/f/r.ttf)} #s{display:inline-block;font-family:F;font-size:100px}</style>";
        let w = width_with_faces(&format!("{regular_only}<b id=s>AV</b>"));
        assert!((133.0..=134.0).contains(&w), "no bold face: the regular one 6 percent wider, {w}");
    }

    struct Files(Vec<u8>);
    impl iyakki_web::Fetch for Files {
        fn get(&self, url: &iyakki_web::Url) -> Result<iyakki_web::fetch::Response, String> {
            if url.path == "/f/my.ttf" {
                Ok(iyakki_web::fetch::Response { status: 200, content_type: "font/ttf".into(), body: self.0.clone(), ..Default::default() })
            } else {
                Err("offline".into())
            }
        }
    }

    fn width_of(html: &str, allow: fn(&str) -> bool) -> String {
        use crate::{run_page_with, Limits, SiteData};
        let web: std::sync::Arc<dyn iyakki_web::Fetch> = std::sync::Arc::new(Files(font()));
        let data = SiteData { viewport: Some(iyakki_web::css::Viewport { width: 1280.0, height: 720.0, ..Default::default() }), ..SiteData::default() };
        let mut out = String::new();
        let res = run_page_with(html, &iyakki_web::Url::parse("https://a.org/").unwrap(), web, allow, &data, Limits::default(), |e| {
            out = e.eval("t", "String(document.getElementById('s').offsetWidth)").unwrap_or_else(|x| format!("THROWN {x}"));
        });
        assert_eq!(res.report.failed, 0, "{:?}", res.report.notes);
        out
    }

    #[test]
    fn text_in_a_page_font_is_measured_with_that_fonts_advances_and_kerning_and_a_refused_font_falls_back() {
        let page = "<style>@font-face{font-family:'My Face';src:url(/f/my.ttf)} #s{display:inline-block;font-family:'My Face',serif;font-size:100px}</style><span id=s>AV</span>";
        assert_eq!(width_of(page, |_| true), "126", "(700 + 700 - 140) units of 1000, at 100 px");
        // the same text with the font refused by the account's host rules is measured with the common serif table
        let fallback = width_of(page, |h| h != "a.org" || false);
        assert_ne!(fallback, "126", "{fallback}");
        // a family with no @font-face is measured with the common tables
        let none = width_of("<style>#s{display:inline-block;font-family:'My Face';font-size:100px}</style><span id=s>AV</span>", |_| true);
        assert_eq!(none, fallback);
    }

    /// A GPOS table with the `kern` feature and one pair lookup (format 1: A V -140, or format 2 by classes: the same pair -100), the
    /// lookup behind an extension lookup when `ext` is set.
    fn gpos(format2: bool, ext: bool) -> Vec<u8> {
        let mut sub = Vec::new();
        if format2 {
            for x in [2u16, 24, 4, 0, 30, 38, 2, 2] {
                be16(&mut sub, x);
            }
            for v in [0i16, 0, 0, -100] {
                be16(&mut sub, v as u16);
            }
            for x in [1u16, 1, 1] {
                be16(&mut sub, x);
            }
            for x in [1u16, 1, 1, 1] {
                be16(&mut sub, x);
            }
            for x in [1u16, 2, 1, 1] {
                be16(&mut sub, x);
            }
            // classdef 1: format 1, start glyph 1, one glyph, class 1 (written above as 1,1,1,1); classdef 2: start 2, one glyph, class 1
        } else {
            for x in [1u16, 18, 4, 0, 1, 12, 1, 2, (-140i16) as u16, 1, 1, 1] {
                be16(&mut sub, x);
            }
        }
        let mut g = Vec::new();
        be32(&mut g, 0x0001_0000);
        for x in [0u16, 10, 24] {
            be16(&mut g, x);
        }
        for x in [1u16] {
            be16(&mut g, x);
        }
        g.extend_from_slice(b"kern");
        for x in [8u16, 0, 1, 0] {
            be16(&mut g, x);
        }
        // lookup list at 24
        be16(&mut g, 1);
        be16(&mut g, 4);
        if ext {
            for x in [9u16, 0, 1, 8, 1, 2] {
                be16(&mut g, x);
            }
            be32(&mut g, 8);
        } else {
            for x in [2u16, 0, 1, 8] {
                be16(&mut g, x);
            }
        }
        g.extend_from_slice(&sub);
        g
    }

    fn to_woff(sfnt: &[u8], compress: bool) -> Vec<u8> {
        let n = u16_at(sfnt, 4).unwrap() as usize;
        let mut dir = Vec::new();
        let mut body = Vec::new();
        let base = 44 + n * 20;
        for i in 0..n {
            let r = 12 + i * 16;
            let (off, len) = (u32_at(sfnt, r + 8).unwrap() as usize, u32_at(sfnt, r + 12).unwrap() as usize);
            let raw = &sfnt[off..off + len];
            let z = miniz_oxide::deflate::compress_to_vec_zlib(raw, 6);
            let data: &[u8] = if compress && z.len() < raw.len() { &z } else { raw };
            dir.extend_from_slice(&sfnt[r..r + 4]);
            be32(&mut dir, (base + body.len()) as u32);
            be32(&mut dir, data.len() as u32);
            be32(&mut dir, len as u32);
            be32(&mut dir, 0);
            body.extend_from_slice(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
        }
        let mut out = b"wOFF".to_vec();
        out.extend_from_slice(&sfnt[..4]);
        be32(&mut out, (base + body.len()) as u32);
        be16(&mut out, n as u16);
        be16(&mut out, 0);
        be32(&mut out, sfnt.len() as u32);
        out.extend_from_slice(&[0u8; 24]);
        out.extend_from_slice(&dir);
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn a_woff_file_is_unpacked_and_measured_like_the_plain_file_and_a_damaged_one_is_refused() {
        let plain = font();
        for compress in [true, false] {
            let w = to_woff(&plain, compress);
            let f = FontMetrics::parse(&w).expect("the WOFF file is read");
            assert!((f.text_em("AV") - 1.26).abs() < 1e-6, "kerned like the plain file");
        }
        let w = to_woff(&plain, true);
        for cut in (0..w.len()).step_by(5) {
            let _ = FontMetrics::parse(&w[..cut]);
        }
        for i in (44..w.len()).step_by(3) {
            let mut v = w.clone();
            v[i] ^= 0x5a;
            let _ = FontMetrics::parse(&v).map(|f| f.text_em("AV"));
        }
        // a table that claims more than it holds, or a size over the cap, is refused
        let mut big = w.clone();
        big[44 + 12..44 + 16].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(FontMetrics::parse(&big).is_none());
    }

    #[test]
    fn the_gpos_kern_feature_gives_pair_kerning_by_glyph_pairs_and_by_classes_and_behind_an_extension() {
        for (format2, ext, want) in [(false, false, 1.26), (false, true, 1.26), (true, false, 1.30), (true, true, 1.30)] {
            let f = FontMetrics::parse(&font_with(false, Some(gpos(format2, ext)))).expect("read");
            assert!((f.text_em("AV") - want).abs() < 1e-6, "format2={format2} ext={ext}: {}", f.text_em("AV"));
            assert!((f.text_em("VA") - 1.4).abs() < 1e-6, "the other order is not kerned");
        }
        // GPOS wins over the old kern table when both exist; a damaged GPOS never panics
        let both = font_with(true, Some(gpos(true, false)));
        assert!((FontMetrics::parse(&both).unwrap().text_em("AV") - 1.30).abs() < 1e-6);
        for cut in (0..both.len()).step_by(11) {
            let _ = FontMetrics::parse(&both[..cut]);
        }
    }
}

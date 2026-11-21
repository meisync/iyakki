//! A small TrueType reader and subsetter for the PDF writer (F-PDF-11): character to glyph (cmap formats 4 and 12), advance widths, and a
//! subset that keeps the glyph numbers (so the PDF can use the glyph number as the character code, `CIDToGIDMap /Identity`) but holds
//! the outlines of the used glyphs only. Hinting programs are dropped (a PDF viewer does not need them). Only glyf-based fonts
//! (DejaVu); a font that is not is refused. Every offset and length read from the font is bounds-checked.

use std::collections::{BTreeSet, HashMap};

pub struct Ttf<'a> {
    data: &'a [u8],
    tables: HashMap<[u8; 4], (usize, usize)>,
    pub units: u16,
    pub num_glyphs: u16,
    long_loca: bool,
    hmetrics: u16,
    cmap: Option<(usize, u16)>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    u16_at(d, o).map(|v| v as i16)
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

fn checksum(b: &[u8]) -> u32 {
    let mut sum = 0u32;
    for c in b.chunks(4) {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    sum
}

/// The cmap subtable to use: Unicode full repertoire (3,10), else Unicode BMP (3,1 or 0,x); a subset has none. `None` for a damaged table.
fn find_cmap(data: &[u8], tables: &HashMap<[u8; 4], (usize, usize)>) -> Option<Option<(usize, u16)>> {
    let mut best: Option<(u8, usize, u16)> = None;
    if let Some(&(cm, _)) = tables.get(b"cmap") {
        let count = u16_at(data, cm + 2)? as usize;
        for i in 0..count {
            let r = cm + 4 + i * 8;
            let (plat, enc) = (u16_at(data, r)?, u16_at(data, r + 2)?);
            let off = cm + u32_at(data, r + 4)? as usize;
            let fmt = u16_at(data, off)?;
            let rank = match (plat, enc, fmt) {
                (3, 10, 12) => 3,
                (3, 1, 4) => 2,
                (0, _, 4) | (0, _, 12) => 1,
                _ => 0,
            };
            if rank > 0 && best.is_none_or(|b| rank > b.0) {
                best = Some((rank, off, fmt));
            }
        }
    }
    Some(best.map(|(_, o, f)| (o, f)))
}

impl<'a> Ttf<'a> {
    /// Only the character map of an OpenType font that holds CFF outlines (`OTTO`) or TrueType outlines: enough to turn glyph numbers back
    /// into characters (F-PDF-18). Nothing else of the font is read, so the subsetting functions must not be used on it.
    pub fn cmap_only(data: &'a [u8]) -> Option<Ttf<'a>> {
        let tag = u32_at(data, 0)?;
        if tag != 0x0001_0000 && tag != 0x4F54_544F {
            return None;
        }
        let n = u16_at(data, 4)? as usize;
        let mut tables = HashMap::new();
        for i in 0..n.min(64) {
            let e = 12 + i * 16;
            let t: [u8; 4] = data.get(e..e + 4)?.try_into().ok()?;
            let (off, len) = (u32_at(data, e + 8)? as usize, u32_at(data, e + 12)? as usize);
            if off.checked_add(len)? > data.len() {
                return None;
            }
            tables.insert(t, (off, len));
        }
        let cmap = find_cmap(data, &tables)?;
        let num_glyphs = tables.get(b"maxp").and_then(|&(o, _)| u16_at(data, o + 4)).filter(|&n| n > 0).unwrap_or(u16::MAX);
        Some(Ttf { data, tables, units: 0, num_glyphs, long_loca: false, hmetrics: 0, cmap })
    }

    pub fn parse(data: &'a [u8]) -> Option<Ttf<'a>> {
        if u32_at(data, 0)? != 0x0001_0000 {
            return None;
        }
        let n = u16_at(data, 4)? as usize;
        let mut tables = HashMap::new();
        for i in 0..n {
            let e = 12 + i * 16;
            let tag: [u8; 4] = data.get(e..e + 4)?.try_into().ok()?;
            let (off, len) = (u32_at(data, e + 8)? as usize, u32_at(data, e + 12)? as usize);
            if off.checked_add(len)? > data.len() {
                return None;
            }
            tables.insert(tag, (off, len));
        }
        let head = tables.get(b"head")?.0;
        let units = u16_at(data, head + 18)?;
        let long_loca = i16_at(data, head + 50)? != 0;
        let num_glyphs = u16_at(data, tables.get(b"maxp")?.0 + 4)?;
        let hmetrics = u16_at(data, tables.get(b"hhea")?.0 + 34)?;
        tables.get(b"glyf")?;
        tables.get(b"loca")?;
        tables.get(b"hmtx")?;
        // the subsetter patches fixed offsets of these tables: a font whose tables are shorter is refused here (found by the fuzz test)
        if tables[b"head"].1 < 54 || tables[b"hhea"].1 < 36 || tables[b"maxp"].1 < 6 {
            return None;
        }
        if units == 0 || num_glyphs == 0 || hmetrics == 0 || hmetrics > num_glyphs {
            return None;
        }
        let cmap = find_cmap(data, &tables)?;
        Some(Ttf { data, tables, units, num_glyphs, long_loca, hmetrics, cmap })
    }

    fn table(&self, tag: &[u8; 4]) -> &'a [u8] {
        let (o, l) = self.tables[tag];
        &self.data[o..o + l]
    }

    /// Every (code point, glyph) of the cmap, glyph 0 left out.
    pub fn cmap_map(&self) -> HashMap<u32, u16> {
        let mut m = HashMap::new();
        let Some((off, fmt)) = self.cmap else { return m };
        let d = self.data;
        // a hostile cmap can name overlapping ranges again and again: the walk over code points stops after this many (a real font
        // has well under a million; found by the fuzz test)
        let mut budget: u64 = 1 << 21;
        if fmt == 12 {
            let (Some(n), true) = (u32_at(d, off + 12), true) else { return m };
            for g in 0..(n as usize).min(100_000) {
                let o = off + 16 + g * 12;
                let (Some(s), Some(e), Some(gl)) = (u32_at(d, o), u32_at(d, o + 4), u32_at(d, o + 8)) else { break };
                if e < s || e - s > 0x10_0000 || e > 0x10_FFFF {
                    continue;
                }
                if budget < (e - s) as u64 + 1 {
                    break;
                }
                budget -= (e - s) as u64 + 1;
                for c in s..=e {
                    let gid = gl.wrapping_add(c - s);
                    if gid != 0 && gid < self.num_glyphs as u32 {
                        m.insert(c, gid as u16);
                    }
                }
            }
        } else if fmt == 4 {
            let Some(segx2) = u16_at(d, off + 6) else { return m };
            let seg = segx2 as usize / 2;
            let (end, start, delta, range) = (off + 14, off + 16 + seg * 2, off + 16 + seg * 4, off + 16 + seg * 6);
            for i in 0..seg {
                let (Some(e), Some(s), Some(dl), Some(ro)) = (u16_at(d, end + i * 2), u16_at(d, start + i * 2), u16_at(d, delta + i * 2), u16_at(d, range + i * 2)) else { break };
                if s > e || s == 0xFFFF {
                    continue;
                }
                if budget < (e - s) as u64 + 1 {
                    break;
                }
                budget -= (e - s) as u64 + 1;
                for c in s..=e {
                    let gid = if ro == 0 {
                        c.wrapping_add(dl)
                    } else {
                        let p = range + i * 2 + ro as usize + (c - s) as usize * 2;
                        match u16_at(d, p) {
                            Some(0) | None => 0,
                            Some(g) => g.wrapping_add(dl),
                        }
                    };
                    if gid != 0 && gid < self.num_glyphs {
                        m.insert(c as u32, gid);
                    }
                }
            }
        }
        m
    }

    /// Advance width of a glyph in font units.
    pub fn advance(&self, gid: u16) -> u16 {
        let hm = self.table(b"hmtx");
        let i = (gid.min(self.hmetrics - 1)) as usize;
        u16_at(hm, i * 4).unwrap_or(0)
    }

    pub fn ascent(&self) -> i16 {
        i16_at(self.table(b"hhea"), 4).unwrap_or(0)
    }
    pub fn descent(&self) -> i16 {
        i16_at(self.table(b"hhea"), 6).unwrap_or(0)
    }
    /// xMin, yMin, xMax, yMax from `head`.
    pub fn bbox(&self) -> [i16; 4] {
        let h = self.table(b"head");
        [i16_at(h, 36).unwrap_or(0), i16_at(h, 38).unwrap_or(0), i16_at(h, 40).unwrap_or(0), i16_at(h, 42).unwrap_or(0)]
    }

    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        let loca = self.table(b"loca");
        let (a, b) = if self.long_loca { (u32_at(loca, gid as usize * 4)? as usize, u32_at(loca, gid as usize * 4 + 4)? as usize) } else { (u16_at(loca, gid as usize * 2)? as usize * 2, u16_at(loca, gid as usize * 2 + 2)? as usize * 2) };
        let glyf = self.table(b"glyf");
        (a <= b && b <= glyf.len()).then_some((a, b))
    }

    /// Bytes of a glyph's outline data (0 for an empty glyph).
    pub fn outline_len(&self, gid: u16) -> usize {
        self.glyph(gid).len()
    }

    fn glyph(&self, gid: u16) -> &'a [u8] {
        match self.glyph_range(gid) {
            Some((a, b)) => &self.table(b"glyf")[a..b],
            None => &[],
        }
    }

    /// The component glyphs of a composite glyph.
    fn components(&self, g: &[u8]) -> Vec<u16> {
        let mut out = Vec::new();
        if g.len() < 10 || i16_at(g, 0).unwrap_or(0) >= 0 {
            return out;
        }
        let mut o = 10;
        for _ in 0..64 {
            let (Some(flags), Some(gi)) = (u16_at(g, o), u16_at(g, o + 2)) else { break };
            out.push(gi);
            o += 4 + if flags & 1 != 0 { 4 } else { 2 };
            o += if flags & 0x8 != 0 { 2 } else if flags & 0x40 != 0 { 4 } else if flags & 0x80 != 0 { 8 } else { 0 };
            if flags & 0x20 == 0 {
                break;
            }
        }
        out
    }

    /// A glyph without its hinting instructions, padded to four bytes.
    fn stripped(&self, g: &[u8]) -> Vec<u8> {
        if g.len() < 10 {
            return Vec::new();
        }
        let nc = i16_at(g, 0).unwrap_or(0);
        let mut out;
        if nc >= 0 {
            let at = 10 + nc as usize * 2;
            let ilen = u16_at(g, at).unwrap_or(0) as usize;
            if at + 2 + ilen > g.len() {
                return Vec::new();
            }
            out = g[..at].to_vec();
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&g[at + 2 + ilen..]);
        } else {
            let mut o = 10;
            let mut last_flags_at = None;
            for _ in 0..64 {
                let Some(flags) = u16_at(g, o) else { return Vec::new() };
                last_flags_at = Some(o);
                o += 4 + if flags & 1 != 0 { 4 } else { 2 };
                o += if flags & 0x8 != 0 { 2 } else if flags & 0x40 != 0 { 4 } else if flags & 0x80 != 0 { 8 } else { 0 };
                if flags & 0x20 == 0 {
                    break;
                }
            }
            if o > g.len() {
                return Vec::new();
            }
            out = g[..o].to_vec();
            if let Some(f) = last_flags_at {
                let fl = u16_at(&out, f).unwrap_or(0) & !0x0100;
                out[f..f + 2].copy_from_slice(&fl.to_be_bytes());
            }
        }
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out
    }

    /// The glyph numbers `gids` need, with the component glyphs of composites, glyph 0 included.
    pub fn closure(&self, gids: &BTreeSet<u16>) -> BTreeSet<u16> {
        let mut all: BTreeSet<u16> = gids.iter().copied().filter(|g| *g < self.num_glyphs).collect();
        all.insert(0);
        let mut todo: Vec<u16> = all.iter().copied().collect();
        while let Some(g) = todo.pop() {
            for c in self.components(self.glyph(g)) {
                if c < self.num_glyphs && all.insert(c) {
                    todo.push(c);
                }
            }
        }
        all
    }

    /// A TrueType file with the same glyph numbers where only `keep` (see [`Ttf::closure`]) have outlines and widths.
    /// `chars` are (code point, glyph) pairs for a small Unicode cmap, so that other TrueType readers can look the characters up too.
    pub fn subset(&self, keep: &BTreeSet<u16>, chars: &[(u32, u16)]) -> Vec<u8> {
        let n = self.num_glyphs as usize;
        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity((n + 1) * 4);
        let mut hmtx = Vec::with_capacity(n * 4);
        for g in 0..n as u16 {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            if keep.contains(&g) {
                glyf.extend_from_slice(&self.stripped(self.glyph(g)));
                let hm = self.table(b"hmtx");
                let adv = self.advance(g);
                let lsb = if (g as usize) < self.hmetrics as usize { i16_at(hm, g as usize * 4 + 2) } else { i16_at(hm, self.hmetrics as usize * 4 + (g as usize - self.hmetrics as usize) * 2) }.unwrap_or(0);
                hmtx.extend_from_slice(&adv.to_be_bytes());
                hmtx.extend_from_slice(&lsb.to_be_bytes());
            } else {
                hmtx.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        let mut head = self.table(b"head").to_vec();
        head[8..12].copy_from_slice(&[0; 4]); // checkSumAdjustment, filled in below
        head[50..52].copy_from_slice(&1i16.to_be_bytes());
        let mut hhea = self.table(b"hhea").to_vec();
        hhea[34..36].copy_from_slice(&self.num_glyphs.to_be_bytes());
        let mut maxp = self.table(b"maxp").to_vec();
        if maxp.len() >= 32 {
            // no instructions and no function definitions are left
            for (o, v) in [(20usize, 0u16), (22, 0), (26, 0)] {
                maxp[o..o + 2].copy_from_slice(&v.to_be_bytes());
            }
        }
        let mut sorted: Vec<(u32, u16)> = chars.iter().copied().filter(|(_, g)| keep.contains(g)).collect();
        sorted.sort();
        sorted.dedup_by_key(|c| c.0);
        let mut groups: Vec<(u32, u32, u32)> = Vec::new();
        for (c, g) in sorted {
            match groups.last_mut() {
                Some(l) if l.1 + 1 == c && l.2 + (l.1 - l.0) + 1 == g as u32 => l.1 = c,
                _ => groups.push((c, c, g as u32)),
            }
        }
        let mut cmap = Vec::new();
        cmap.extend_from_slice(&[0, 0, 0, 1, 0, 3, 0, 10, 0, 0, 0, 12]);
        cmap.extend_from_slice(&[0, 12, 0, 0]);
        cmap.extend_from_slice(&((16 + groups.len() * 12) as u32).to_be_bytes());
        cmap.extend_from_slice(&[0; 4]);
        cmap.extend_from_slice(&(groups.len() as u32).to_be_bytes());
        for (s, e, g) in &groups {
            for v in [s, e, g] {
                cmap.extend_from_slice(&v.to_be_bytes());
            }
        }
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![(*b"cmap", cmap), (*b"glyf", glyf), (*b"head", head), (*b"hhea", hhea), (*b"hmtx", hmtx), (*b"loca", loca), (*b"maxp", maxp)];
        tables.sort_by_key(|t| t.0);
        let nt = tables.len() as u16;
        let mut pow = 1u16;
        let mut log = 0u16;
        while pow * 2 <= nt {
            pow *= 2;
            log += 1;
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        out.extend_from_slice(&nt.to_be_bytes());
        out.extend_from_slice(&(pow * 16).to_be_bytes());
        out.extend_from_slice(&log.to_be_bytes());
        out.extend_from_slice(&(nt * 16 - pow * 16).to_be_bytes());
        let mut off = 12 + tables.len() * 16;
        let mut body = Vec::new();
        let mut head_at = 0;
        for (tag, t) in &tables {
            out.extend_from_slice(tag);
            out.extend_from_slice(&checksum(t).to_be_bytes());
            out.extend_from_slice(&(off as u32).to_be_bytes());
            out.extend_from_slice(&(t.len() as u32).to_be_bytes());
            if tag == b"head" {
                head_at = off;
            }
            body.extend_from_slice(t);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            off = 12 + tables.len() * 16 + body.len();
        }
        out.extend_from_slice(&body);
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[head_at + 8..head_at + 12].copy_from_slice(&adj.to_be_bytes());
        out
    }
}

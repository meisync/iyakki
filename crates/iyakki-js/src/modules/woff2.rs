// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! WOFF2 web fonts (W3C Recommendation, F-BROX-19): the header and the table directory are read, the tables (one Brotli stream, `brotli`)
//! are unpacked and put in a plain sfnt file for `webfont::FontMetrics`. That reader needs advances, the character map and kerning only,
//! so the glyph outlines are not rebuilt: a transformed `glyf` and its `loca` are left out of the answer, and a transformed `hmtx` (the
//! left side bearings are left out of the file when they equal the outline's own minimum) is written with the advances and zero bearings.
//! The result is therefore a METRICS file, not a font to draw with. A font collection, a damaged file or one over 16 MiB gives `None`.

use super::brotli;

const MAX_OUT: usize = 16 * 1024 * 1024;

/// The 63 table names a directory entry can point to by number.
const KNOWN: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm", b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern", b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC", b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar", b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty", b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat", b"Gloc", b"Feat", b"Sill",
];

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// A `UIntBase128`: seven bits per byte, the high bit says more follow, at most five bytes, no leading zero group.
fn base128(d: &[u8], at: &mut usize) -> Option<u32> {
    let mut v: u32 = 0;
    for i in 0..5 {
        let b = *d.get(*at)?;
        *at += 1;
        if i == 0 && b == 0x80 {
            return None;
        }
        if v > 0x01ff_ffff {
            return None;
        }
        v = (v << 7) | u32::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

struct Entry {
    tag: [u8; 4],
    /// The bytes this table has in the unpacked stream.
    len: usize,
    transformed: bool,
}

/// Unpack a WOFF2 file into a plain sfnt file with the tables a metrics reader needs.
pub fn unwoff2(d: &[u8]) -> Option<Vec<u8>> {
    if d.get(..4)? != b"wOF2" {
        return None;
    }
    let flavor = d.get(4..8)?;
    if flavor == b"ttcf" {
        return None;
    }
    let n = u16_at(d, 12)? as usize;
    let compressed = u32_at(d, 20)? as usize;
    if n == 0 || n > 64 {
        return None;
    }
    let mut at = 48;
    let mut entries = Vec::with_capacity(n);
    let mut total = 0usize;
    for _ in 0..n {
        let flags = *d.get(at)?;
        at += 1;
        let idx = usize::from(flags & 0x3f);
        let tag: [u8; 4] = if idx == 63 {
            let t = d.get(at..at + 4)?.try_into().ok()?;
            at += 4;
            t
        } else {
            **KNOWN.get(idx)?
        };
        let version = flags >> 6;
        let orig = base128(d, &mut at)? as usize;
        // glyf and loca are transformed unless the version says 3; every other table is transformed when the version is not 0
        let transformed = if &tag == b"glyf" || &tag == b"loca" { version != 3 } else { version != 0 };
        let len = if transformed { base128(d, &mut at)? as usize } else { orig };
        total = total.checked_add(len)?;
        if total > MAX_OUT {
            return None;
        }
        entries.push(Entry { tag, len, transformed });
    }
    let stream = d.get(at..at.checked_add(compressed)?)?;
    let data = brotli::decompress(stream, total).ok()?;
    if data.len() != total {
        return None;
    }
    // slice the tables out of the stream
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut pos = 0;
    let mut hmtx_at = None;
    for e in &entries {
        let raw = &data[pos..pos + e.len];
        pos += e.len;
        match (&e.tag, e.transformed) {
            (b"glyf" | b"loca", true) => {}
            (b"hmtx", true) => {
                hmtx_at = Some(tables.len());
                tables.push((e.tag, raw.to_vec()));
            }
            _ => tables.push((e.tag, raw.to_vec())),
        }
    }
    if let Some(i) = hmtx_at {
        let hhea = tables.iter().find(|t| &t.0 == b"hhea")?;
        let maxp = tables.iter().find(|t| &t.0 == b"maxp")?;
        let n_metrics = u16_at(&hhea.1, 34)? as usize;
        let n_glyphs = u16_at(&maxp.1, 4)? as usize;
        let rebuilt = rebuild_hmtx(&tables[i].1, n_metrics, n_glyphs)?;
        tables[i].1 = rebuilt;
    }
    let count = tables.len();
    let mut out = Vec::with_capacity(total + 12 + count * 16);
    out.extend_from_slice(flavor);
    out.extend_from_slice(&(count as u16).to_be_bytes());
    out.extend_from_slice(&[0u8; 6]);
    let mut off = 12 + count * 16;
    for (tag, t) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(t.len() as u32).to_be_bytes());
        off += t.len().div_ceil(4) * 4;
    }
    for (_, t) in &tables {
        out.extend_from_slice(t);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Some(out)
}

/// A transformed `hmtx`: a flags byte, the advance widths, then the left side bearings that were not left out (bit 0: those of the first
/// `numberOfHMetrics` glyphs, bit 1: those of the rest). The advances are what the metrics reader uses; bearings left out are written as 0.
fn rebuild_hmtx(t: &[u8], n_metrics: usize, n_glyphs: usize) -> Option<Vec<u8>> {
    let flags = *t.first()?;
    if flags & 0xfc != 0 || n_metrics == 0 || n_metrics > n_glyphs {
        return None;
    }
    let mut at = 1;
    let advances = t.get(at..at + n_metrics * 2)?;
    at += n_metrics * 2;
    let lsb = |at: &mut usize, count: usize, present: bool| -> Option<Vec<u8>> {
        if present {
            let s = t.get(*at..*at + count * 2)?.to_vec();
            *at += count * 2;
            Some(s)
        } else {
            Some(vec![0; count * 2])
        }
    };
    let first = lsb(&mut at, n_metrics, flags & 1 == 0)?;
    let rest = lsb(&mut at, n_glyphs - n_metrics, flags & 2 == 0)?;
    let mut out = Vec::with_capacity(n_metrics * 4 + rest.len());
    for i in 0..n_metrics {
        out.extend_from_slice(&advances[i * 2..i * 2 + 2]);
        out.extend_from_slice(&first[i * 2..i * 2 + 2]);
    }
    out.extend_from_slice(&rest);
    Some(out)
}

#[cfg(test)]
#[path = "woff2_tests.rs"]
mod tests;

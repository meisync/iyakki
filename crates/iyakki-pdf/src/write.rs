//! A small PDF writer: headings, paragraphs, lists, quotes, code, rules and pictures laid out on A4 or Letter pages with the standard
//! Helvetica and Courier fonts. Text is limited to the Latin characters of Windows-1252; others are written as `?` and counted
//! (embedding a font program is not done). No compression of text (readable and diff-able); pictures are stored raw.

use crate::font::helv_width;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSize {
    A4,
    Letter,
}

impl PageSize {
    pub fn points(self) -> (f64, f64) {
        match self {
            PageSize::A4 => (595.28, 841.89),
            PageSize::Letter => (612.0, 792.0),
        }
    }
    pub fn from_name(s: &str) -> Option<PageSize> {
        match s.trim().to_ascii_lowercase().as_str() {
            "a4" => Some(PageSize::A4),
            "letter" | "us letter" | "usletter" => Some(PageSize::Letter),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdfImage {
    pub w: u32,
    pub h: u32,
    /// `w * h * 3` bytes
    pub rgb: Vec<u8>,
    /// `w * h` bytes, when the picture has transparency
    pub alpha: Option<Vec<u8>>,
}

impl PdfImage {
    /// From straight RGBA pixels.
    pub fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Option<PdfImage> {
        if w == 0 || h == 0 || (w as u64) * (h as u64) > 16_000_000 || rgba.len() < (w as usize) * (h as usize) * 4 {
            return None;
        }
        let n = (w * h) as usize;
        let mut rgb = Vec::with_capacity(n * 3);
        let mut a = Vec::with_capacity(n);
        for px in rgba[..n * 4].chunks(4) {
            rgb.extend_from_slice(&px[..3]);
            a.push(px[3]);
        }
        let alpha = if a.iter().all(|v| *v == 255) { None } else { Some(a) };
        Some(PdfImage { w, h, rgb, alpha })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Para(String),
    Bullet(u8, String, String),
    Quote(String),
    Code(String),
    Rule,
    Image(PdfImage),
}

pub struct Builder {
    pub title: String,
    pub author: String,
    pub size: PageSize,
    pub blocks: Vec<Block>,
    /// `D:YYYYMMDDHHmmSS` when the caller has a clock
    pub date: Option<String>,
    pub footer: bool,
    pub margin: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub pages: usize,
    /// Characters outside Windows-1252 that were written as `?`.
    pub replaced_chars: usize,
}

/// A Windows-1252 byte for a character, when there is one.
pub fn winansi(c: char) -> Option<u8> {
    let u = c as u32;
    match u {
        0x20..=0x7e | 0xa0..=0xff => Some(u as u8),
        0x09 => Some(b' '),
        _ => crate::font::CP1252_HIGH.iter().position(|x| *x == c && c != '\u{fffd}').map(|i| 0x80 + i as u8),
    }
}

fn width_of(bytes: &[u8], size: f64, bold: bool, mono: bool) -> f64 {
    if mono {
        return bytes.len() as f64 * 0.6 * size;
    }
    let k = if bold { 1.07 } else { 1.0 };
    bytes.iter().map(|&b| {
        let ch = if b >= 0x80 && b < 0xa0 { crate::font::CP1252_HIGH[(b - 0x80) as usize] } else { b as char };
        helv_width(ch) as f64
    }).sum::<f64>() * size / 1000.0 * k
}

fn encode(s: &str, replaced: &mut usize) -> Vec<u8> {
    s.chars().filter(|c| *c != '\r' && *c != '\u{200b}' && *c != '\u{feff}').map(|c| match winansi(c) {
        Some(b) => b,
        None if c == '\n' => b'\n',
        None => {
            if !c.is_control() {
                *replaced += 1;
            }
            b'?'
        }
    }).collect()
}

fn escape(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() + 2);
    for &c in b {
        match c {
            b'(' | b')' | b'\\' => {
                s.push('\\');
                s.push(c as char);
            }
            0x20..=0x7e => s.push(c as char),
            _ => {
                let _ = write!(s, "\\{:03o}", c);
            }
        }
    }
    s
}

fn text_string(s: &str) -> String {
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        format!("({})", escape(s.as_bytes()))
    } else {
        let mut h = String::from("<FEFF");
        for u in s.encode_utf16() {
            let _ = write!(h, "{:04X}", u);
        }
        h.push('>');
        h
    }
}

fn wrap(bytes: &[u8], size: f64, max: f64, bold: bool, mono: bool) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    for para in bytes.split(|b| *b == b'\n') {
        let mut cur: Vec<u8> = Vec::new();
        for word in para.split(|b| *b == b' ').filter(|w| !w.is_empty() || mono) {
            let mut w = word.to_vec();
            loop {
                let try_len = if cur.is_empty() { width_of(&w, size, bold, mono) } else { width_of(&cur, size, bold, mono) + width_of(b" ", size, bold, mono) + width_of(&w, size, bold, mono) };
                if try_len <= max || (cur.is_empty() && w.len() <= 1) {
                    if !cur.is_empty() {
                        cur.push(b' ');
                    }
                    cur.extend_from_slice(&w);
                    break;
                }
                if cur.is_empty() {
                    // a word wider than the line: cut it
                    let mut cut = 1;
                    while cut < w.len() && width_of(&w[..cut + 1], size, bold, mono) <= max {
                        cut += 1;
                    }
                    lines.push(w[..cut].to_vec());
                    w = w[cut..].to_vec();
                    if w.is_empty() {
                        break;
                    }
                } else {
                    lines.push(std::mem::take(&mut cur));
                }
            }
        }
        lines.push(cur);
    }
    while lines.len() > 1 && lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

struct PageOut {
    content: String,
    images: Vec<usize>,
}

impl Builder {
    pub fn new(size: PageSize) -> Builder {
        Builder { title: String::new(), author: String::new(), size, blocks: Vec::new(), date: None, footer: true, margin: 54.0 }
    }

    pub fn build(&self) -> (Vec<u8>, Built) {
        let (pw, ph) = self.size.points();
        let m = self.margin.clamp(18.0, 120.0);
        let (cw, top, bottom) = (pw - 2.0 * m, ph - m, m + if self.footer { 14.0 } else { 0.0 });
        let mut replaced = 0usize;
        let mut pages: Vec<PageOut> = vec![PageOut { content: String::new(), images: vec![] }];
        let mut images: Vec<&PdfImage> = Vec::new();
        let mut y = top;
        macro_rules! new_page {
            () => {
                pages.push(PageOut { content: String::new(), images: vec![] });
                y = top;
            };
        }
        for (bi, b) in self.blocks.iter().enumerate() {
            let first_on_page = pages.last().map(|p| p.content.is_empty() && p.images.is_empty()).unwrap_or(true);
            if pages.len() > 2000 {
                break;
            }
            match b {
                Block::Rule => {
                    if y - 12.0 < bottom {
                        new_page!();
                    }
                    y -= 6.0;
                    let _ = writeln!(pages.last_mut().unwrap().content, "0.6 G 0.6 w {:.2} {:.2} m {:.2} {:.2} l S 0 G", m, y, m + cw, y);
                    y -= 8.0;
                }
                Block::Image(img) => {
                    let maxh = (top - bottom) * 0.85;
                    let mut w = (img.w as f64 * 0.75).min(cw);
                    let mut h = w * img.h as f64 / img.w as f64;
                    if h > maxh {
                        h = maxh;
                        w = h * img.w as f64 / img.h as f64;
                    }
                    if y - h < bottom && !first_on_page {
                        new_page!();
                    }
                    y -= h;
                    let idx = images.len();
                    images.push(img);
                    let p = pages.last_mut().unwrap();
                    p.images.push(idx);
                    let _ = writeln!(p.content, "q {:.2} 0 0 {:.2} {:.2} {:.2} cm /Im{} Do Q", w, h, m, y, idx);
                    y -= 8.0;
                }
                Block::Heading(_, _) | Block::Para(_) | Block::Bullet(..) | Block::Quote(_) | Block::Code(_) => {
                    let (text, size, bold, mono, indent, before, after, grey, marker): (&str, f64, bool, bool, f64, f64, f64, bool, Option<String>) = match b {
                        Block::Heading(l, t) => {
                            let sz = [22.0, 18.0, 15.0, 13.0, 12.0, 11.0][(*l).clamp(1, 6) as usize - 1];
                            (t, sz, true, false, 0.0, if bi == 0 { 0.0 } else { sz * 0.7 }, sz * 0.35, false, None)
                        }
                        Block::Para(t) => (t, 11.0, false, false, 0.0, 0.0, 6.0, false, None),
                        Block::Bullet(d, mk, t) => (t, 11.0, false, false, 14.0 + 14.0 * *d as f64, 0.0, 3.0, false, Some(mk.clone())),
                        Block::Quote(t) => (t, 11.0, false, false, 18.0, 0.0, 6.0, true, None),
                        Block::Code(t) => (t, 9.0, false, true, 8.0, 2.0, 8.0, false, None),
                        _ => unreachable!(),
                    };
                    let bytes = encode(text, &mut replaced);
                    if bytes.iter().all(|b| *b == b' ') {
                        continue;
                    }
                    let lines = wrap(&bytes, size, cw - indent, bold, mono);
                    let lh = size * if mono { 1.2 } else { 1.3 };
                    // keep a heading with the lines after it
                    let need = if matches!(b, Block::Heading(..)) { before + lh * 3.0 } else { lh };
                    if y - need < bottom && !first_on_page {
                        new_page!();
                    }
                    y -= if first_on_page { 0.0 } else { before };
                    for (li, line) in lines.iter().enumerate() {
                        if y - lh < bottom {
                            new_page!();
                        }
                        y -= lh;
                        let p = pages.last_mut().unwrap();
                        let font = if mono { "F3" } else if bold { "F2" } else { "F1" };
                        if grey {
                            let _ = writeln!(p.content, "0.35 g");
                        }
                        if li == 0 {
                            if let Some(mk) = &marker {
                                let mb = encode(mk, &mut replaced);
                                let _ = writeln!(p.content, "BT /F1 {size:.1} Tf {:.2} {:.2} Td ({}) Tj ET", m + indent - 12.0 - width_of(&mb, size, false, false).min(20.0) + 8.0, y, escape(&mb));
                            }
                        }
                        let _ = writeln!(p.content, "BT /{font} {size:.1} Tf {:.2} {:.2} Td ({}) Tj ET", m + indent, y, escape(line));
                        if grey {
                            let _ = writeln!(p.content, "0 g");
                            if li == 0 {
                                let _ = writeln!(p.content, "0.7 G 1.5 w {:.2} {:.2} m {:.2} {:.2} l S 0 G", m + 6.0, y - 3.0, m + 6.0, y + lh - 3.0 + (lines.len() as f64 - 1.0) * lh);
                            }
                        }
                    }
                    y -= after;
                }
            }
        }
        // footers
        let total = pages.len();
        if self.footer {
            let mut t = self.title.clone();
            if t.chars().count() > 70 {
                t = t.chars().take(69).collect::<String>() + "...";
            }
            for (i, p) in pages.iter_mut().enumerate() {
                let label = if t.is_empty() { format!("{} / {}", i + 1, total) } else { format!("{}   -   {} / {}", t, i + 1, total) };
                let lb = encode(&label, &mut replaced);
                let _ = writeln!(p.content, "0.5 g BT /F1 8 Tf {:.2} {:.2} Td ({}) Tj ET 0 g", m, m - 4.0, escape(&lb));
            }
        }
        // objects
        let mut objs: Vec<Vec<u8>> = Vec::new(); // object n is objs[n-1]
        let fixed = 6;
        let n_images = images.len();
        let page_base = fixed + 1 + n_images * 2; // first page object number
        let mut kids = String::new();
        for i in 0..total {
            let _ = write!(kids, "{} 0 R ", page_base + i * 2);
        }
        objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.trim_end(), total).into_bytes());
        for (name, base) in [("Helvetica", "F1"), ("Helvetica-Bold", "F2"), ("Courier", "F3")] {
            let _ = base;
            objs.push(format!("<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>").into_bytes());
        }
        let mut info = format!("<< /Title {} /Producer (IYAKKI) /Creator (IYAKKI)", text_string(&self.title));
        if !self.author.is_empty() {
            let _ = write!(info, " /Author {}", text_string(&self.author));
        }
        if let Some(d) = &self.date {
            if d.starts_with("D:") && d.bytes().all(|b| b.is_ascii_graphic()) {
                let _ = write!(info, " /CreationDate ({d})");
            }
        }
        info.push_str(" >>");
        objs.push(info.into_bytes());
        // images: object (fixed + 1 + 2k) = image, (fixed + 2 + 2k) = its soft mask or a placeholder
        for (k, img) in images.iter().enumerate() {
            let smask_no = fixed + 2 + 2 * k;
            let mut d = format!("<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8", img.w, img.h);
            if img.alpha.is_some() {
                let _ = write!(d, " /SMask {} 0 R", smask_no);
            }
            let _ = write!(d, " /Length {} >>\nstream\n", img.rgb.len());
            let mut o = d.into_bytes();
            o.extend_from_slice(&img.rgb);
            o.extend_from_slice(b"\nendstream");
            objs.push(o);
            match &img.alpha {
                Some(a) => {
                    let mut o = format!("<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n", img.w, img.h, a.len()).into_bytes();
                    o.extend_from_slice(a);
                    o.extend_from_slice(b"\nendstream");
                    objs.push(o);
                }
                None => objs.push(b"null".to_vec()),
            }
        }
        for (i, p) in pages.iter().enumerate() {
            let content_no = page_base + i * 2 + 1;
            let mut xo = String::new();
            for &ix in &p.images {
                let _ = write!(xo, "/Im{} {} 0 R ", ix, fixed + 1 + ix * 2);
            }
            objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >> /XObject << {} >> >> /Contents {} 0 R >>", pw, ph, xo, content_no).into_bytes());
            let mut c = format!("<< /Length {} >>\nstream\n", p.content.len()).into_bytes();
            c.extend_from_slice(p.content.as_bytes());
            c.extend_from_slice(b"\nendstream");
            objs.push(c);
        }
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", o).as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{}\n%%EOF\n", objs.len() + 1, xref).as_bytes());
        (out, Built { pages: total, replaced_chars: replaced })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winansi_maps_latin_and_curly_quotes() {
        assert_eq!(winansi('A'), Some(b'A'));
        assert_eq!(winansi('é'), Some(0xe9));
        assert_eq!(winansi('\u{201c}'), Some(0x93));
        assert_eq!(winansi('\u{20ac}'), Some(0x80));
        assert_eq!(winansi('\u{0b95}'), None);
    }

    #[test]
    fn wrapping_respects_the_width_and_cuts_long_words() {
        let lines = wrap(b"the quick brown fox jumps over the lazy dog", 11.0, 100.0, false, false);
        assert!(lines.len() >= 3);
        for l in &lines {
            assert!(width_of(l, 11.0, false, false) <= 100.5, "{}", String::from_utf8_lossy(l));
        }
        let cut = wrap(&[b'W'; 100], 11.0, 60.0, false, false);
        assert!(cut.len() > 5);
        assert_eq!(cut.iter().map(|l| l.len()).sum::<usize>(), 100);
    }
}

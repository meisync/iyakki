//! Pictures inside the assistant's answers: SVG written out as code, and images sent as data addresses or base64 text.
//!
//! `split` finds them in a message (fenced blocks, bare `<svg>` elements, `data:image/...` lines, Markdown images with a
//! data address, raw base64 that is clearly a PNG, JPEG, GIF, WebP or SVG). `PicSrc::decode` turns one into bytes within
//! caps, `raster` draws it to fit a column (SVG text goes through the shell's own fonts), and `save_file` writes it into the
//! person's folders under their write scope. The transcript (chat_view.rs) shows the picture and keeps the long text
//! folded away.

use crate::safeclamp::SafeClamp;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_agent::scope;
use iyakki_sec::policy::Policy;
use iyakki_web::image::{self, Kind};
use iyakki_web::svg::{self, TextMask, TextRenderer, TextStyle};
use tiny_skia::Pixmap;

use crate::gfx::{Face, Gfx};

/// The longest text that is looked at as a picture (base64 is a third bigger than the picture it carries).
pub const MAX_SOURCE: usize = 12 << 20;
/// The biggest picture that is decoded.
pub const MAX_DECODED: usize = 8 << 20;
const MAX_SVG: usize = 4 << 20;
/// The widest an inline picture is drawn, at scale 1.
pub const MAX_PIC_W: f32 = 560.0;
/// The picture a drawing without a size gets.
pub const DEFAULT_SVG: (f64, f64) = (400.0, 300.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PicKind {
    /// SVG markup.
    Svg,
    /// A data address or raw base64.
    Data,
}

/// One picture found in a message: what was written (`text`) and a cheap identity for caches.
#[derive(Clone, Debug)]
pub struct PicSrc {
    pub kind: PicKind,
    pub text: String,
    pub id: u64,
}

impl PartialEq for PicSrc {
    fn eq(&self, o: &PicSrc) -> bool {
        self.id == o.id && self.kind == o.kind && self.text == o.text
    }
}

impl Hash for PicSrc {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.kind.hash(h);
        self.text.len().hash(h);
    }
}

impl PicSrc {
    pub fn new(kind: PicKind, text: &str) -> PicSrc {
        let text = text.trim().to_string();
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        PicSrc { kind, id: h.finish(), text }
    }

    /// What the copy button puts on the clipboard: exactly what was written.
    pub fn copy_text(&self) -> &str {
        &self.text
    }

    /// The small line that stands in for the long text: `image/png data URL · 84 KB`.
    pub fn summary(&self) -> String {
        let kb = |n: usize| if n < 1024 { format!("{n} bytes") } else { format!("{} KB", n / 1024) };
        match self.kind {
            PicKind::Svg => format!("SVG drawing · {}", kb(self.text.len())),
            PicKind::Data => {
                let mime = self.text.strip_prefix("data:").and_then(|r| r.split([';', ',']).next()).filter(|m| m.starts_with("image/")).map(str::to_string);
                match mime {
                    Some(m) => format!("{m} data URL · {}", kb(self.text.len())),
                    None => format!("base64 image · {}", kb(self.text.len())),
                }
            }
        }
    }

    pub fn decode(&self) -> Result<Decoded, String> {
        decode(self)
    }
}

/// A picture's bytes and what they are.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub mime: &'static str,
    pub kind: Kind,
}

fn describe(k: Kind) -> Option<(&'static str, &'static str)> {
    Some(match k {
        Kind::Png => ("png", "image/png"),
        Kind::Jpeg => ("jpg", "image/jpeg"),
        Kind::Gif => ("gif", "image/gif"),
        Kind::Webp => ("webp", "image/webp"),
        Kind::Svg => ("svg", "image/svg+xml"),
        Kind::Unknown => return None,
    })
}

/// Bytes of a picture from a data address, raw base64 or SVG text, within the caps.
pub fn decode_text(kind: PicKind, text: &str) -> Result<Decoded, String> {
    let t = text.trim();
    if t.len() > MAX_SOURCE {
        return Err("the picture is too large to show (over 8 MB)".into());
    }
    let bytes = match kind {
        PicKind::Svg => {
            if t.len() > MAX_SVG {
                return Err("the SVG is too large (over 4 MB)".into());
            }
            t.as_bytes().to_vec()
        }
        PicKind::Data if t.starts_with("data:") => image::parse_data_url(t).map(|(_, b)| b).ok_or("the data address is not valid (the base64 text may be cut off)")?,
        PicKind::Data => image::base64_decode(t).ok_or("the base64 text is not valid (it may be cut off)")?,
    };
    if bytes.len() > MAX_DECODED {
        return Err("the picture is too large to show (over 8 MB)".into());
    }
    let kind = image::sniff(&bytes);
    let (ext, mime) = describe(kind).ok_or("this is not a PNG, JPEG, GIF, WebP or SVG picture")?;
    Ok(Decoded { bytes, ext, mime, kind })
}

fn decode(src: &PicSrc) -> Result<Decoded, String> {
    let d = decode_text(src.kind, &src.text)?;
    if src.kind == PicKind::Svg && d.kind != Kind::Svg {
        return Err("this is not an SVG picture".into());
    }
    Ok(d)
}

// ------------------------------------------------------------------ finding pictures in a message

#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Text(String),
    Pic(PicSrc),
}

struct Out {
    segs: Vec<Seg>,
    text: Vec<String>,
    found: bool,
}

impl Out {
    fn line(&mut self, l: &str) {
        self.text.push(l.to_string());
    }
    fn flush(&mut self) {
        if self.text.iter().any(|l| !l.trim().is_empty()) {
            self.segs.push(Seg::Text(self.text.join("\n")));
        }
        self.text.clear();
    }
    fn pic(&mut self, p: PicSrc) {
        self.flush();
        self.segs.push(Seg::Pic(p));
        self.found = true;
    }
}

fn is_b64(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'-' | b'_'))
}

/// Is this run of base64 characters a picture? (Only the start is decoded.)
fn blob_is_picture(s: &str) -> bool {
    if s.len() < 100 || !is_b64(s) {
        return false;
    }
    let head = &s[..(s.len().min(2048) / 4) * 4];
    image::base64_decode(head).is_some_and(|b| image::sniff(&b) != Kind::Unknown)
}

/// `<?xml ...?>`, comments and a doctype in front of an `<svg`.
fn svg_like(c: &str) -> bool {
    let mut t = c.trim_start();
    for _ in 0..8 {
        if t.starts_with("<?") {
            let Some(e) = t.find("?>") else { return false };
            t = t[e + 2..].trim_start();
        } else if t.starts_with("<!--") {
            let Some(e) = t.find("-->") else { return false };
            t = t[e + 3..].trim_start();
        } else if t.len() > 9 && t[..9].eq_ignore_ascii_case("<!doctype") {
            let Some(e) = t.find('>') else { return false };
            t = t[e + 1..].trim_start();
        } else {
            break;
        }
    }
    t.starts_with("<svg") && (t.len() == 4 || t[4..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/'))
}

fn classify_fence(lang: &str, content: &str) -> Option<PicSrc> {
    let c = content.trim();
    if c.is_empty() {
        return None;
    }
    let xmlish = matches!(lang, "" | "svg" | "xml" | "html" | "svg+xml" | "image/svg+xml");
    if xmlish && svg_like(c) {
        return Some(PicSrc::new(PicKind::Svg, c));
    }
    if lang == "svg" && c.contains("<svg") && c.contains("</svg>") {
        return Some(PicSrc::new(PicKind::Svg, &c[c.find("<svg").unwrap_or(0)..]));
    }
    let compact: String = c.split_whitespace().collect();
    if compact.starts_with("data:image/") {
        // percent-encoded text may hold spaces that matter
        return Some(PicSrc::new(PicKind::Data, if compact.contains(";base64,") { &compact } else { c }));
    }
    if matches!(lang, "" | "base64" | "b64" | "text" | "txt" | "plaintext") && blob_is_picture(&compact) {
        return Some(PicSrc::new(PicKind::Data, &compact));
    }
    None
}

/// The pictures in an assistant message, with the text around them; `None` when there are none.
pub fn split(md: &str) -> Option<Vec<Seg>> {
    const HINTS: [&str; 7] = ["<svg", "data:image/", "iVBOR", "/9j/", "R0lGOD", "UklGR", "PHN2Zy"];
    if !HINTS.iter().any(|h| md.contains(h)) {
        return None;
    }
    let lines: Vec<&str> = md.lines().collect();
    let mut o = Out { segs: Vec::new(), text: Vec::new(), found: false };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.starts_with("```") {
            let close = (i + 1..lines.len()).find(|&j| lines[j].trim() == "```");
            let Some(j) = close else {
                // still being written: all of it stays text
                for l in &lines[i..] {
                    o.line(l);
                }
                break;
            };
            let lang = t[3..].trim().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            match classify_fence(&lang, &lines[i + 1..j].join("\n")) {
                Some(p) => o.pic(p),
                None => {
                    for l in &lines[i..=j] {
                        o.line(l);
                    }
                }
            }
            i = j + 1;
            continue;
        }
        if t.starts_with("<svg") || t.starts_with("<?xml") {
            if let Some(end) = bare_svg_end(&lines, i) {
                let text = lines[i..=end].join("\n");
                if svg_like(&text) {
                    o.pic(PicSrc::new(PicKind::Svg, &text));
                    i = end + 1;
                    continue;
                }
            }
        }
        if let Some(a) = line.find("![") {
            if let Some(b) = line[a..].find("](data:image/") {
                let rest = &line[a + b + 2..];
                let end = if rest.starts_with("data:image/svg+xml") && !rest.contains(";base64,") { rest.rfind(')') } else { rest.find(')') };
                if let Some(e) = end {
                    if !line[..a].trim().is_empty() {
                        o.line(&line[..a]);
                    }
                    o.pic(PicSrc::new(PicKind::Data, &rest[..e]));
                    if !rest[e + 1..].trim().is_empty() {
                        o.line(&rest[e + 1..]);
                    }
                    i += 1;
                    continue;
                }
            }
        }
        let u = t.trim_matches('`');
        if u.starts_with("data:image/") && (!u.contains(char::is_whitespace) || u.starts_with("data:image/svg+xml")) {
            o.pic(PicSrc::new(PicKind::Data, u));
        } else if blob_is_picture(u) && u.len() >= 200 {
            o.pic(PicSrc::new(PicKind::Data, u));
        } else {
            o.line(line);
        }
        i += 1;
    }
    o.flush();
    o.found.then_some(o.segs)
}

/// The last line of a bare `<svg>` element that starts on line `i`, when it is complete.
fn bare_svg_end(lines: &[&str], i: usize) -> Option<usize> {
    let (mut opens, mut closes, mut bytes) = (0usize, 0usize, 0usize);
    for (j, l) in lines.iter().enumerate().skip(i) {
        bytes += l.len() + 1;
        if bytes > MAX_SVG {
            return None;
        }
        opens += l.matches("<svg").count();
        closes += l.matches("</svg>").count();
        // an element that closes itself: `<svg ... />`
        if j == i && l.trim_start().starts_with("<svg") {
            if let Some(g) = l.find('>') {
                if l[..g].ends_with('/') {
                    return Some(j);
                }
            }
        }
        if opens > 0 && closes >= opens {
            return Some(j);
        }
    }
    None
}

// ------------------------------------------------------------------ drawing

/// The shell's fonts for the text in an SVG.
pub struct GfxText<'a>(pub &'a mut Gfx);

impl TextRenderer for GfxText<'_> {
    fn measure(&mut self, text: &str, style: &TextStyle) -> f64 {
        self.0.text_width(style.size as f32, text, face(style)) as f64
    }
    fn render(&mut self, text: &str, style: &TextStyle) -> Option<TextMask> {
        let m = self.0.text_mask(style.size as f32, text, face(style));
        (m.w > 0).then_some(TextMask { advance: m.advance, x: m.x, y: m.y, w: m.w, h: m.h, alpha: m.alpha })
    }
}

fn face(s: &TextStyle) -> Face {
    if s.mono {
        Face::Mono
    } else if s.bold {
        Face::Bold
    } else {
        Face::Sans
    }
}

/// A drawn picture and how much to stretch it when painting (1.0: pixel for pixel).
#[derive(Clone)]
pub struct Rendered {
    pub pm: Arc<Pixmap>,
    pub k: f32,
    /// The file type to save it as: `svg`, `png`, `jpg`, `gif` or `webp`.
    pub ext: &'static str,
}

/// Averages blocks of pixels down to `nw` x `nh` (the pixmap is premultiplied, so averaging is right).
pub(crate) fn resize_box(pm: &Pixmap, nw: u32, nh: u32) -> Option<Pixmap> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut out = Pixmap::new(nw, nh)?;
    let src = pm.data();
    let (sx, sy) = (w as f32 / nw as f32, h as f32 / nh as f32);
    let dst = out.data_mut();
    for y in 0..nh as usize {
        let (y0, y1) = ((y as f32 * sy) as usize, (((y + 1) as f32 * sy).ceil() as usize).clamp_safe(1, h));
        for x in 0..nw as usize {
            let (x0, x1) = ((x as f32 * sx) as usize, (((x + 1) as f32 * sx).ceil() as usize).clamp_safe(1, w));
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for yy in y0..y1.max(y0 + 1).min(h) {
                for xx in x0..x1.max(x0 + 1).min(w) {
                    let o = (yy * w + xx) * 4;
                    for c in 0..4 {
                        acc[c] += src[o + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = (y * nw as usize + x) * 4;
            for c in 0..4 {
                dst[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    Some(out)
}

/// Draws a decoded picture to fit `max_w` device pixels (and a sensible height) at UI scale `s`.
pub fn raster(g: &mut Gfx, d: &Decoded, max_w: f32, s: f32) -> Result<Rendered, String> {
    let max_h = 520.0 * s;
    if d.kind == Kind::Svg {
        let (nw, nh) = svg::natural_size(&d.bytes)?;
        let (nw, nh) = if nw.is_finite() && nh.is_finite() && nw > 0.0 && nh > 0.0 { (nw, nh) } else { DEFAULT_SVG };
        // a drawing keeps its own size at this UI size, tiny ones are enlarged (it is vector), big ones shrink to the column
        let mut w = (nw * s as f64).max(160.0 * s as f64);
        let mut h = w * nh / nw;
        let k = ((max_w.min(MAX_PIC_W * s) as f64) / w).min(max_h as f64 / h).min(1.0);
        w *= k;
        h *= k;
        let (w, h) = ((w.round() as u32).clamp(1, 4096), (h.round() as u32).clamp(1, 4096));
        let b = svg::render_with(&d.bytes, Some((w, h)), Some(&mut GfxText(g)))?;
        let pm = crate::apps::browser_images::bitmap_pixmap(b.width, b.height, &b.rgba)?;
        return Ok(Rendered { pm: Arc::new(pm), k: 1.0, ext: d.ext });
    }
    let pm = crate::apps::browser_images::to_pixmap(&d.bytes)?;
    let (iw, ih) = (pm.width() as f32, pm.height() as f32);
    let k = (max_w.min(MAX_PIC_W * s) / iw).min(max_h / ih).min(s.max(1.0));
    if k < 1.0 {
        let (nw, nh) = ((iw * k).round().max(1.0) as u32, (ih * k).round().max(1.0) as u32);
        let small = resize_box(&pm, nw, nh).ok_or("the picture has no size")?;
        return Ok(Rendered { pm: Arc::new(small), k: 1.0, ext: d.ext });
    }
    Ok(Rendered { pm: Arc::new(pm), k, ext: d.ext })
}

/// Pictures that were drawn already, by (picture, width): each is decoded and drawn once.
#[derive(Default)]
pub struct PicCache {
    drawn: HashMap<(u64, u32), Result<Rendered, String>>,
    decoded: HashMap<u64, Result<Arc<Decoded>, String>>,
    /// How many pictures were drawn (tests watch this).
    pub rasterised: usize,
}

impl PicCache {
    pub fn decoded(&mut self, src: &PicSrc) -> Result<Arc<Decoded>, String> {
        if self.decoded.len() > 32 {
            self.decoded.clear();
        }
        self.decoded.entry(src.id).or_insert_with(|| src.decode().map(Arc::new)).clone()
    }

    pub fn draw(&mut self, g: &mut Gfx, src: &PicSrc, max_w: f32, s: f32) -> Result<Rendered, String> {
        let key = (src.id, (max_w * 4.0 + s * 1000.0) as u32);
        if let Some(r) = self.drawn.get(&key) {
            return r.clone();
        }
        let r = self.decoded(src).and_then(|d| {
            self.rasterised += 1;
            raster(g, &d, max_w, s)
        });
        if self.drawn.len() > 48 {
            self.drawn.clear();
        }
        self.drawn.insert(key, r.clone());
        r
    }

    /// A picture file the assistant made or the person owns (SVG gets its text drawn).
    pub fn draw_bytes(&mut self, g: &mut Gfx, id: u64, bytes: &[u8], max_w: f32, s: f32) -> Result<Rendered, String> {
        let key = (id, (max_w * 4.0 + s * 1000.0) as u32);
        if let Some(r) = self.drawn.get(&key) {
            return r.clone();
        }
        let kind = image::sniff(bytes);
        let (ext, mime) = describe(kind).ok_or("this is not a picture")?;
        self.rasterised += 1;
        let r = raster(g, &Decoded { bytes: bytes.to_vec(), ext, mime, kind }, max_w, s);
        if self.drawn.len() > 48 {
            self.drawn.clear();
        }
        self.drawn.insert(key, r.clone());
        r
    }
}

// ------------------------------------------------------------------ saving

/// A file name that is safe to create: one path component, no leading dot, with the right extension.
pub fn clean_name(name: &str, ext: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut n: String = base.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ' | '(' | ')')).collect();
    n = n.trim().trim_start_matches('.').to_string();
    n.truncate(100);
    let stem_ok = !n.is_empty();
    let has_ext = n.to_ascii_lowercase().ends_with(&format!(".{ext}")) || (ext == "jpg" && n.to_ascii_lowercase().ends_with(".jpeg"));
    if !stem_ok {
        return format!("picture.{ext}");
    }
    if has_ext {
        n
    } else {
        format!("{n}.{ext}")
    }
}

/// Writes `bytes` as `dir/name` (a number is added when the name is taken) after checking the person may write there.
pub fn save_file(policy: &Policy, user: &str, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    scope::check_write(policy, user, dir)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make the folder {}: {e}", dir.display()))?;
    let real = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    scope::check_write(policy, user, &real)?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..1000 {
        let file = if n == 0 { name.to_string() } else { format!("{stem}-{}{ext}", n + 1) };
        let path = real.join(&file);
        scope::check_write(policy, user, &path)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(bytes).map_err(|e| e.to_string())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("could not save {}: {e}", path.display())),
        }
    }
    Err("too many files with that name".into())
}

#[cfg(test)]
pub(crate) mod fixtures {
    pub const HELLO_SVG: &str = include_str!("testdata/hello.svg");

    pub fn b64(b: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut o = String::with_capacity(b.len() * 4 / 3 + 4);
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    o.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    o.push('=');
                }
            }
        }
        o
    }

    /// A PNG of one colour.
    pub fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
        pm.encode_png().unwrap()
    }

    /// A PNG that does not compress (a long base64 text).
    pub fn noise_png(w: u32, h: u32) -> Vec<u8> {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        let mut x = 88172645463325252u64;
        for b in pm.data_mut().chunks_exact_mut(4) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b.copy_from_slice(&[(x >> 8) as u8, (x >> 24) as u8, (x >> 40) as u8, 255]);
        }
        pm.encode_png().unwrap()
    }

    /// A file of iyakki-web's picture fixtures.
    pub fn web_data(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/../iyakki-web/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn pics(md: &str) -> Vec<PicSrc> {
        split(md).map(|s| s.into_iter().filter_map(|x| if let Seg::Pic(p) = x { Some(p) } else { None }).collect()).unwrap_or_default()
    }

    #[test]
    fn svg_is_found_in_fences_with_or_without_a_language_and_bare() {
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><rect width='10' height='10'/></svg>";
        for lang in ["svg", "xml", "", "html", "SVG"] {
            let p = pics(&format!("Here you go:\n```{lang}\n{svg}\n```\nEnjoy."));
            assert_eq!(p.len(), 1, "{lang:?}");
            assert_eq!(p[0].kind, PicKind::Svg);
            assert_eq!(p[0].text, svg, "the source is kept exactly");
        }
        let segs = split(&format!("before\n```svg\n{svg}\n```\nafter")).unwrap();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Seg::Text("before".into()));
        assert_eq!(segs[2], Seg::Text("after".into()));
        // an XML prolog and comments in front are fine
        assert_eq!(pics(&format!("```xml\n<?xml version=\"1.0\"?>\n<!-- logo -->\n{svg}\n```")).len(), 1);
        // a bare element over several lines, and one on a single line
        let multi = "Text\n<svg width='5' height='5'>\n  <circle r='2'/>\n</svg>\nmore";
        let p = pics(multi);
        assert_eq!(p.len(), 1);
        assert!(p[0].text.starts_with("<svg") && p[0].text.ends_with("</svg>"));
        assert_eq!(pics("<svg width='5' height='5'/>").len(), 1);
        // not pictures: other code, prose about the tag, an html page, an unfinished block
        assert!(split("```rust\nlet s = \"<svg>\";\n```").is_none());
        assert!(split("Use the <svg> element for this.").is_none());
        assert!(pics("```html\n<html><body><svg width='1'></svg></body></html>\n```").is_empty());
        assert!(pics(&format!("```svg\n{svg}")).is_empty(), "still being written: stays text");
        assert!(split("plain text only").is_none());
    }

    #[test]
    fn data_addresses_are_found_bare_in_code_in_markdown_images_and_as_raw_base64() {
        let p = png(8, 8, [0, 200, 0]);
        let url = format!("data:image/png;base64,{}", b64(&p));
        for md in [
            format!("Here:\n{url}\nDone"),
            format!("```\n{url}\n```"),
            format!("```text\n{url}\n```"),
            format!("![a green square]({url}) and more"),
            format!("`{url}`"),
            format!("This is a base64 image:\n\n```base64\n{}\n```", b64(&p).repeat(1)),
            format!("base64 image below\n{}\n", b64(&noise_png(20, 20))),
        ] {
            let found = pics(&md);
            assert_eq!(found.len(), 1, "{}", &md[..md.len().min(80)]);
            let d = found[0].decode().unwrap();
            assert_eq!(d.ext, "png");
        }
        let segs = split(&format!("pre ![x]({url}) post")).unwrap();
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], Seg::Text(t) if t == "pre "));
        // other types by their first bytes
        for (file, ext) in [("base420.jpg", "jpg"), ("pal.gif", "gif"), ("webp_lossy.webp", "webp")] {
            let bytes = web_data(file);
            let mime = if ext == "jpg" { "jpeg" } else { ext };
            let found = pics(&format!("data:image/{mime};base64,{}", b64(&bytes)));
            assert_eq!(found.len(), 1, "{file}");
            assert_eq!(found[0].decode().unwrap().ext, ext, "{file}");
            assert_eq!(pics(&format!("```base64\n{}\n```", b64(&bytes))).len(), 1, "{file} raw");
        }
        // svg: base64 and percent-encoded text
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='8' height='6'><rect width='8' height='6' fill='red'/></svg>";
        let a = pics(&format!("data:image/svg+xml;base64,{}", b64(svg.as_bytes())));
        assert_eq!(a[0].decode().unwrap().ext, "svg");
        let enc = svg.replace('<', "%3C").replace('>', "%3E").replace('\'', "%27");
        let b = pics(&format!("![logo](data:image/svg+xml;utf8,{enc})"));
        assert_eq!(b[0].decode().unwrap().ext, "svg");
        assert_eq!(pics(&format!("```\ndata:image/svg+xml;utf8,{svg}\n```"))[0].decode().unwrap().ext, "svg");
        // a long base64 text that is not a picture, and a short one, stay text
        assert!(split(&format!("```\n{}\n```", b64(&vec![7u8; 600]))).is_none());
        assert!(pics(&format!("{}", b64(b"hello world, this is some text"))).is_empty());
    }

    #[test]
    fn bad_and_oversized_pictures_say_why() {
        let p = png(30, 30, [1, 2, 3]);
        let full = format!("data:image/png;base64,{}", b64(&p));
        // cut off in the middle: the bytes are there but the picture is not whole
        let cut = PicSrc::new(PicKind::Data, &full[..full.len() / 2]);
        let d = cut.decode().unwrap();
        assert!(crate::apps::browser_images::to_pixmap(&d.bytes).is_err());
        for bad in ["data:image/png;base64,!!!!", "data:image/png;base64,", "data:image/png;base64,aGVsbG8gd29ybGQ=", "data:image/png,not%20a%20png"] {
            let r = PicSrc::new(PicKind::Data, bad).decode();
            assert!(r.is_err(), "{bad}: {r:?}");
        }
        let huge = PicSrc::new(PicKind::Data, &format!("data:image/png;base64,{}", "A".repeat(MAX_SOURCE + 10)));
        assert!(huge.decode().unwrap_err().contains("8 MB"));
        // 9 MB of bytes inside the text cap
        let mut big = png(4, 4, [0, 0, 0]);
        big.resize(9 << 20, 0);
        let r = PicSrc::new(PicKind::Data, &format!("data:image/png;base64,{}", b64(&big))).decode();
        assert!(r.unwrap_err().contains("8 MB"));
        assert!(PicSrc::new(PicKind::Svg, "<html></html>").decode().is_err());
        assert!(PicSrc::new(PicKind::Svg, &format!("<svg>{}</svg>", "x".repeat(MAX_SVG + 1))).decode().is_err());
    }

    #[test]
    fn the_summary_stands_in_for_the_long_text() {
        let p = PicSrc::new(PicKind::Data, &format!("data:image/png;base64,{}", "A".repeat(84 * 1024)));
        assert!(p.summary().starts_with("image/png data URL · 84 KB"), "{}", p.summary());
        assert_eq!(PicSrc::new(PicKind::Data, &"A".repeat(2048)).summary(), "base64 image · 2 KB");
        assert_eq!(PicSrc::new(PicKind::Svg, "<svg/>").summary(), "SVG drawing · 6 bytes");
    }

    #[test]
    fn file_names_are_cleaned_and_saving_stays_in_scope_with_unique_names() {
        assert_eq!(clean_name("logo", "svg"), "logo.svg");
        assert_eq!(clean_name("logo.SVG", "svg"), "logo.SVG");
        assert_eq!(clean_name("../../etc/passwd", "png"), "passwd.png");
        assert_eq!(clean_name("..", "png"), "picture.png");
        assert_eq!(clean_name(".hidden", "png"), "hidden.png");
        assert_eq!(clean_name("a\\b:c*?.jpeg", "jpg"), "bc.jpeg");
        assert_eq!(clean_name("", "gif"), "picture.gif");
        let dir = std::env::temp_dir().join(format!("iyakki-chatpic-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let mut pol = iyakki_sec::policy::Role::User.template();
        pol.fs_write = vec![dir.join("home").display().to_string()];
        let a = save_file(&pol, "u", &dir.join("home/Pictures"), "x.svg", b"one").unwrap();
        let b = save_file(&pol, "u", &dir.join("home/Pictures"), "x.svg", b"two").unwrap();
        let c = save_file(&pol, "u", &dir.join("home/Pictures"), "x.svg", b"three").unwrap();
        assert_eq!([a.file_name().unwrap().to_str().unwrap(), b.file_name().unwrap().to_str().unwrap(), c.file_name().unwrap().to_str().unwrap()], ["x.svg", "x-2.svg", "x-3.svg"]);
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        // outside the write scope: refused, nothing created
        let e = save_file(&pol, "u", &dir.join("elsewhere"), "x.svg", b"no").unwrap_err();
        assert!(e.contains("permission denied"), "{e}");
        assert!(!dir.join("elsewhere").exists());
        // a link inside the folder that leads out is caught
        std::os::unix::fs::symlink(dir.join("elsewhere2"), dir.join("home/out")).ok();
        std::fs::create_dir_all(dir.join("elsewhere2")).unwrap();
        assert!(save_file(&pol, "u", &dir.join("home/out"), "y.svg", b"no").is_err());
        assert!(!dir.join("elsewhere2/y.svg").exists());
    }
}

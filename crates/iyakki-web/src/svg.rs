//! SVG pictures: a small, safe rasteriser for the static subset that icons, logos and diagrams use.
//!
//! Supported: `svg`/`g`/`a`/`switch`/`use`/`symbol`, `viewBox`, `width`/`height`, `preserveAspectRatio`; `path` (all commands,
//! relative forms, arcs), `rect` (with `rx`/`ry`), `circle`, `ellipse`, `line`, `polyline`, `polygon`; `transform` lists;
//! fill and stroke with colours (named, `#rgb[a]`, `#rrggbb[aa]`, `rgb()`, `rgba()`, `hsl()`, `currentColor`), opacities,
//! `fill-rule`, line caps, joins, miter limit, dashes; group `opacity`; `linearGradient` and `radialGradient` (with `href`
//! inheritance, `gradientTransform`, `spreadMethod`); inline `style=` and `<style>` rules made of simple selectors
//! (`tag`, `.class`, `#id`, `*`, and combinations of those).
//!
//! `text` and `tspan` (`x`, `y`, `dx`, `dy`, `font-size`, `font-weight`, `font-family`, `text-anchor`, fill with colours or gradients; translate and
//! scale are honoured, rotation is not) are drawn through a [`TextRenderer`] the caller supplies, because this crate has no fonts;
//! without one (plain [`decode`]) text draws nothing.
//!
//! `clip-path` (a `clipPath` of shapes) and `mask` (luminance) are honoured. A `filter` runs its primitives in order with
//! named results: `feGaussianBlur`, `feOffset`, `feFlood`, `feColorMatrix` (matrix, saturate, hueRotate, luminanceToAlpha),
//! `feComposite` (over, in, out, atop, xor, arithmetic), `feBlend` (normal, multiply, screen, darken, lighten), `feMerge`
//! and `feDropShadow`; `SourceGraphic` and `SourceAlpha` are known inputs. The filter region is the whole picture.
//! A `pattern` paint (user-space or bounding-box units, `viewBox`, `patternTransform`) is drawn tile by tile, `marker`s
//! (`marker-start`, `marker-mid`, `marker-end` or `marker` set on the shape itself, `orient`, `markerUnits`, `viewBox`,
//! `refX`/`refY`) are put on the vertices of `path`, `line`, `polyline` and `polygon`, and a nested `svg` honours its
//! `x`/`y`/`width`/`height`/`viewBox`/`preserveAspectRatio`.
//!
//! Not supported (silently left out): `image`, `feTurbulence`/`feMorphology`/`feConvolveMatrix` and the lighting primitives,
//! pattern `href` inheritance, marker and nested-`svg` overflow clipping, markers set through CSS rules, CSS combinators and
//! pseudo-classes. Hostile input cannot panic or run away: sizes,
//! element counts, nesting, `use` expansion and total rasterising work are all capped.

use crate::image::Bitmap;
use std::collections::HashMap;

const MAX_SIDE: f64 = 4096.0;
const MAX_PIXELS: f64 = 16_000_000.0;
const MAX_INPUT: usize = 4 << 20;
const MAX_ELEMENTS: usize = 20_000;
const MAX_XML_DEPTH: usize = 64;
const MAX_RENDER_DEPTH: usize = 40;
const MAX_VISITS: usize = 60_000;
const MAX_WORK: u64 = 600_000_000;
const MAX_LAYER_PIXELS: u64 = 300_000_000;
const MAX_CMDS: usize = 200_000;
const SUB: usize = 16;
/// A picture that takes longer than this to draw is left unfinished.
const TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

/// How a piece of text should look; `size` is in device pixels (already scaled by the picture's transform).
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f64,
    pub bold: bool,
    pub mono: bool,
}

/// A line of text drawn as coverage: `alpha` is `w * h` bytes. `x` and `y` give the mask's top-left corner relative to the pen
/// origin on the baseline (so `y` is usually negative); `advance` is how far the pen moved.
#[derive(Clone, Debug, Default)]
pub struct TextMask {
    pub advance: f64,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
}

/// What the caller provides so that SVG `text` can be drawn: this crate has no fonts of its own.
pub trait TextRenderer {
    /// The advance width of `text` in device pixels.
    fn measure(&mut self, text: &str, style: &TextStyle) -> f64;
    /// The coverage of `text` at that size, or `None` when it cannot be drawn.
    fn render(&mut self, text: &str, style: &TextStyle) -> Option<TextMask>;
}

type P = (f64, f64);
type Rgba = [f32; 4];

// ------------------------------------------------------------------------------------------------ XML

struct Node {
    tag: String,
    attrs: Vec<(String, String)>,
    kids: Vec<usize>,
    text: String,
}

struct Xml {
    nodes: Vec<Node>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(j) = rest.find(';').filter(|&j| j <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..j];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name.strip_prefix('#').and_then(|n| if let Some(h) = n.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok() } else { n.parse().ok() }).and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[j + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn local_name(n: &str) -> String {
    n.rsplit(':').next().unwrap_or(n).to_string()
}

/// Parse the markup into a tree. Node 0 is a virtual root. Never recurses.
fn parse_xml(s: &str) -> Result<Xml, String> {
    let b = s.as_bytes();
    let mut nodes = vec![Node { tag: String::new(), attrs: Vec::new(), kids: Vec::new(), text: String::new() }];
    let mut stack = vec![0usize];
    let mut i = 0;
    let starts = |i: usize, p: &str| b.get(i..).is_some_and(|r| r.starts_with(p.as_bytes()));
    while i < b.len() {
        if b[i] != b'<' {
            let j = s[i..].find('<').map_or(b.len(), |k| i + k);
            let top = *stack.last().unwrap_or(&0);
            if nodes[top].tag == "style" {
                nodes[top].text.push_str(&s[i..j]);
            } else if matches!(nodes[top].tag.as_str(), "text" | "tspan") && nodes.len() < MAX_ELEMENTS {
                let id = nodes.len();
                nodes.push(Node { tag: "#text".into(), attrs: Vec::new(), kids: Vec::new(), text: decode_entities(&s[i..j]) });
                nodes[top].kids.push(id);
            }
            i = j;
            continue;
        }
        if starts(i, "<!--") {
            i = s[i + 4..].find("-->").map_or(b.len(), |k| i + 4 + k + 3);
        } else if starts(i, "<![CDATA[") {
            let end = s[i + 9..].find("]]>").map_or(b.len(), |k| i + 9 + k);
            let top = *stack.last().unwrap_or(&0);
            if nodes[top].tag == "style" {
                nodes[top].text.push_str(&s[i + 9..end]);
            }
            i = (end + 3).min(b.len());
        } else if starts(i, "<?") {
            i = s[i..].find("?>").map_or(b.len(), |k| i + k + 2);
        } else if starts(i, "<!") {
            // a doctype, possibly with an internal subset in [ ]
            let mut depth = 0i32;
            let mut j = i + 2;
            while j < b.len() {
                match b[j] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    b'>' if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i = (j + 1).min(b.len());
        } else if starts(i, "</") {
            let j = s[i..].find('>').map_or(b.len(), |k| i + k + 1);
            let name = local_name(s[i + 2..j.saturating_sub(1).max(i + 2)].trim());
            // close the nearest open element of that name; a stray end tag is ignored
            if let Some(p) = stack.iter().rposition(|&n| nodes[n].tag == name) {
                if p > 0 {
                    stack.truncate(p);
                }
            }
            i = j;
        } else {
            i += 1;
            let ns = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
                i += 1;
            }
            let tag = local_name(&s[ns..i]);
            let mut attrs = Vec::new();
            let mut selfclose = false;
            loop {
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                match b.get(i) {
                    None => break,
                    Some(b'>') => {
                        i += 1;
                        break;
                    }
                    Some(b'/') => {
                        selfclose = true;
                        i += 1;
                        continue;
                    }
                    _ => {}
                }
                let ks = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'/' | b'>') {
                    i += 1;
                }
                let mut key = s[ks..i].to_string();
                if i == ks {
                    i += 1; // a stray byte: skip it
                    continue;
                }
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut val = String::new();
                if b.get(i) == Some(&b'=') {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    match b.get(i) {
                        Some(&q) if q == b'"' || q == b'\'' => {
                            let vs = i + 1;
                            let ve = s[vs..].find(q as char).map_or(b.len(), |k| vs + k);
                            val = decode_entities(&s[vs..ve]);
                            i = (ve + 1).min(b.len());
                        }
                        _ => {
                            let vs = i;
                            while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                                i += 1;
                            }
                            val = decode_entities(&s[vs..i]);
                        }
                    }
                }
                if key == "xlink:href" {
                    key = "href".into();
                }
                if !attrs.iter().any(|(k, _): &(String, String)| *k == key) {
                    attrs.push((key, val));
                }
            }
            if nodes.len() >= MAX_ELEMENTS {
                return Err("the SVG has too many elements".into());
            }
            let parent = *stack.last().unwrap_or(&0);
            let id = nodes.len();
            nodes.push(Node { tag, attrs, kids: Vec::new(), text: String::new() });
            nodes[parent].kids.push(id);
            if !selfclose {
                if stack.len() >= MAX_XML_DEPTH {
                    return Err("the SVG is nested too deeply".into());
                }
                stack.push(id);
            }
        }
    }
    Ok(Xml { nodes })
}

// ------------------------------------------------------------------------------------------------ numbers, lengths, colours

/// A number at the start of `s` (after spaces and commas); the rest of the string follows it.
fn take_number(s: &str) -> Option<(f64, &str)> {
    let t = s.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
    let b = t.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let ds = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let mut digits = i - ds;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let fs = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        digits += i - fs;
    }
    if digits == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if b.get(j).is_some_and(u8::is_ascii_digit) {
            while b.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            i = j;
        }
    }
    let v: f64 = t[..i].parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some((v, &t[i..]))
}

fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some((v, r)) = take_number(rest) {
        out.push(v);
        rest = r;
        if out.len() > 100_000 {
            break;
        }
    }
    out
}

/// A length in user units; `%` is relative to `refv`.
fn length(s: &str, refv: f64) -> Option<f64> {
    let (v, unit) = take_number(s)?;
    let k = match unit.trim() {
        "" | "px" => 1.0,
        "pt" => 4.0 / 3.0,
        "pc" => 16.0,
        "mm" => 96.0 / 25.4,
        "cm" => 96.0 / 2.54,
        "in" => 96.0,
        "em" => 16.0,
        "ex" => 8.0,
        "%" => refv / 100.0,
        _ => return None,
    };
    Some(v * k)
}

const NAMED: &[(&str, u32)] = &[
    ("black", 0x000000), ("white", 0xffffff), ("red", 0xff0000), ("green", 0x008000), ("blue", 0x0000ff), ("yellow", 0xffff00),
    ("cyan", 0x00ffff), ("aqua", 0x00ffff), ("magenta", 0xff00ff), ("fuchsia", 0xff00ff), ("gray", 0x808080), ("grey", 0x808080),
    ("silver", 0xc0c0c0), ("maroon", 0x800000), ("olive", 0x808000), ("lime", 0x00ff00), ("teal", 0x008080), ("navy", 0x000080),
    ("purple", 0x800080), ("orange", 0xffa500), ("pink", 0xffc0cb), ("brown", 0xa52a2a), ("gold", 0xffd700), ("orangered", 0xff4500),
    ("darkgreen", 0x006400), ("darkblue", 0x00008b), ("darkred", 0x8b0000), ("lightgray", 0xd3d3d3), ("lightgrey", 0xd3d3d3),
    ("darkgray", 0xa9a9a9), ("darkgrey", 0xa9a9a9), ("dimgray", 0x696969), ("dimgrey", 0x696969), ("lightblue", 0xadd8e6),
    ("lightgreen", 0x90ee90), ("skyblue", 0x87ceeb), ("steelblue", 0x4682b4), ("tomato", 0xff6347), ("crimson", 0xdc143c),
    ("indigo", 0x4b0082), ("violet", 0xee82ee), ("coral", 0xff7f50), ("salmon", 0xfa8072), ("khaki", 0xf0e68c), ("beige", 0xf5f5dc),
    ("ivory", 0xfffff0), ("tan", 0xd2b48c), ("royalblue", 0x4169e1), ("slategray", 0x708090), ("whitesmoke", 0xf5f5f5),
    ("gainsboro", 0xdcdcdc), ("limegreen", 0x32cd32), ("forestgreen", 0x228b22), ("dodgerblue", 0x1e90ff), ("deepskyblue", 0x00bfff),
    ("hotpink", 0xff69b4), ("chocolate", 0xd2691e), ("firebrick", 0xb22222), ("turquoise", 0x40e0d0), ("plum", 0xdda0dd),
];

fn hex_rgba(h: &str) -> Option<Rgba> {
    if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize, n: usize| u32::from_str_radix(&h[i..i + n], 16).ok();
    let (r, g, b, a) = match h.len() {
        3 | 4 => {
            let d = |i: usize| v(i, 1).map(|x| (x * 17) as f32 / 255.0);
            (d(0)?, d(1)?, d(2)?, if h.len() == 4 { d(3)? } else { 1.0 })
        }
        6 | 8 => {
            let d = |i: usize| v(i, 2).map(|x| x as f32 / 255.0);
            (d(0)?, d(2)?, d(4)?, if h.len() == 8 { d(6)? } else { 1.0 })
        }
        _ => return None,
    };
    Some([r, g, b, a])
}

fn hsl_rgb(h: f64, s: f64, l: f64) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f64| {
        t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p };
        v as f32
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// `None` = not a colour; `Some(None)` is not used: "none" is handled by the paint parser.
fn parse_color(s: &str) -> Option<Rgba> {
    let t = s.trim();
    let l = t.to_ascii_lowercase();
    if let Some(h) = l.strip_prefix('#') {
        return hex_rgba(h);
    }
    if l == "transparent" {
        return Some([0.0; 4]);
    }
    if let Some(&(_, v)) = NAMED.iter().find(|(n, _)| *n == l) {
        return Some([(v >> 16 & 255) as f32 / 255.0, (v >> 8 & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0]);
    }
    let open = l.find('(')?;
    let close = l.rfind(')')?;
    if close < open {
        return None;
    }
    let name = l[..open].trim();
    let args: Vec<&str> = l[open + 1..close].split([',', ' ', '/']).filter(|p| !p.is_empty()).collect();
    if args.len() < 3 {
        return None;
    }
    let comp = |a: &str, scale: f64| -> Option<f64> {
        let (v, rest) = take_number(a)?;
        Some(if rest.trim() == "%" { v / 100.0 } else { v / scale })
    };
    let alpha = match args.get(3) {
        Some(a) => comp(a, 1.0)?.clamp(0.0, 1.0) as f32,
        None => 1.0,
    };
    match name {
        "rgb" | "rgba" => Some([comp(args[0], 255.0)?.clamp(0.0, 1.0) as f32, comp(args[1], 255.0)?.clamp(0.0, 1.0) as f32, comp(args[2], 255.0)?.clamp(0.0, 1.0) as f32, alpha]),
        "hsl" | "hsla" => {
            let (h, _) = take_number(args[0])?;
            let rgb = hsl_rgb(h, comp(args[1], 100.0)?.clamp(0.0, 1.0), comp(args[2], 100.0)?.clamp(0.0, 1.0));
            Some([rgb[0], rgb[1], rgb[2], alpha])
        }
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ matrices

#[derive(Clone, Copy, Debug)]
struct M([f64; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn mul(&self, o: &M) -> M {
        let (a, b) = (&self.0, &o.0);
        M([a[0] * b[0] + a[2] * b[1], a[1] * b[0] + a[3] * b[1], a[0] * b[2] + a[2] * b[3], a[1] * b[2] + a[3] * b[3], a[0] * b[4] + a[2] * b[5] + a[4], a[1] * b[4] + a[3] * b[5] + a[5]])
    }
    fn apply(&self, p: P) -> P {
        let a = &self.0;
        (a[0] * p.0 + a[2] * p.1 + a[4], a[1] * p.0 + a[3] * p.1 + a[5])
    }
    fn invert(&self) -> Option<M> {
        let a = &self.0;
        let det = a[0] * a[3] - a[1] * a[2];
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        Some(M([a[3] / det, -a[1] / det, -a[2] / det, a[0] / det, (a[2] * a[5] - a[3] * a[4]) / det, (a[1] * a[4] - a[0] * a[5]) / det]))
    }
    fn scale_factor(&self) -> f64 {
        let a = &self.0;
        (a[0] * a[3] - a[1] * a[2]).abs().sqrt().max(1e-6)
    }
    fn finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite() && v.abs() < 1e9)
    }
}

fn parse_transform(s: &str) -> M {
    let mut m = M::ID;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let a = numbers(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let t = match (name, a.len()) {
            ("translate", 1) => M([1.0, 0.0, 0.0, 1.0, a[0], 0.0]),
            ("translate", 2) => M([1.0, 0.0, 0.0, 1.0, a[0], a[1]]),
            ("scale", 1) => M([a[0], 0.0, 0.0, a[0], 0.0, 0.0]),
            ("scale", 2) => M([a[0], 0.0, 0.0, a[1], 0.0, 0.0]),
            ("rotate", 1) | ("rotate", 3) => {
                let (s, c) = a[0].to_radians().sin_cos();
                let r = M([c, s, -s, c, 0.0, 0.0]);
                if a.len() == 3 {
                    M([1.0, 0.0, 0.0, 1.0, a[1], a[2]]).mul(&r).mul(&M([1.0, 0.0, 0.0, 1.0, -a[1], -a[2]]))
                } else {
                    r
                }
            }
            ("skewX", 1) => M([1.0, 0.0, a[0].to_radians().tan(), 1.0, 0.0, 0.0]),
            ("skewY", 1) => M([1.0, a[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
            ("matrix", 6) => M([a[0], a[1], a[2], a[3], a[4], a[5]]),
            _ => continue,
        };
        m = m.mul(&t);
        if !m.finite() {
            return M::ID;
        }
    }
    m
}

// ------------------------------------------------------------------------------------------------ paths

#[derive(Clone, Copy, Debug)]
enum Cmd {
    M(P),
    L(P),
    C(P, P, P),
    Z,
}

fn arc_to(out: &mut Vec<Cmd>, from: P, rx: f64, ry: f64, phi_deg: f64, large: bool, sweep: bool, to: P) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if from == to {
        return;
    }
    if rx < 1e-9 || ry < 1e-9 {
        out.push(Cmd::L(to));
        return;
    }
    let (sp, cp) = phi_deg.to_radians().sin_cos();
    let (dx2, dy2) = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    let (x1p, y1p) = (cp * dx2 + sp * dy2, -sp * dx2 + cp * dy2);
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() } * if large == sweep { -1.0 } else { 1.0 };
    let (cxp, cyp) = (coef * rx * y1p / ry, -coef * ry * x1p / rx);
    let (cx, cy) = (cp * cxp - sp * cyp + (from.0 + to.0) / 2.0, sp * cxp + cp * cyp + (from.1 + to.1) / 2.0);
    let t1 = ((y1p - cyp) / ry).atan2((x1p - cxp) / rx);
    let t2 = ((-y1p - cyp) / ry).atan2((-x1p - cxp) / rx);
    let mut dt = t2 - t1;
    if sweep && dt < 0.0 {
        dt += std::f64::consts::TAU;
    } else if !sweep && dt > 0.0 {
        dt -= std::f64::consts::TAU;
    }
    let n = ((dt.abs() / (std::f64::consts::FRAC_PI_2 - 1e-9)).ceil() as usize).clamp(1, 8);
    let delta = dt / n as f64;
    let k = 4.0 / 3.0 * (delta / 4.0).tan();
    let map = |x: f64, y: f64| (cx + rx * x * cp - ry * y * sp, cy + rx * x * sp + ry * y * cp);
    for i in 0..n {
        let a1 = t1 + i as f64 * delta;
        let a2 = a1 + delta;
        let (s1, c1) = a1.sin_cos();
        let (s2, c2) = a2.sin_cos();
        let end = if i + 1 == n { to } else { map(c2, s2) };
        out.push(Cmd::C(map(c1 - k * s1, s1 + k * c1), map(c2 + k * s2, s2 - k * c2), end));
    }
}

fn parse_path(d: &str) -> Vec<Cmd> {
    let b = d.as_bytes();
    let mut i = 0;
    let mut out: Vec<Cmd> = Vec::new();
    let (mut cur, mut start) = ((0.0, 0.0), (0.0, 0.0));
    let (mut last_c, mut last_q): (Option<P>, Option<P>) = (None, None);
    let mut cmd = 0u8;
    let skip = |i: &mut usize| {
        while *i < b.len() && (b[*i].is_ascii_whitespace() || b[*i] == b',') {
            *i += 1;
        }
    };
    macro_rules! num {
        () => {{
            skip(&mut i);
            match take_number(&d[i..]) {
                Some((v, rest)) => {
                    i = d.len() - rest.len();
                    v
                }
                None => break,
            }
        }};
    }
    macro_rules! flag {
        () => {{
            skip(&mut i);
            match b.get(i) {
                Some(b'0') => {
                    i += 1;
                    false
                }
                Some(b'1') => {
                    i += 1;
                    true
                }
                _ => break,
            }
        }};
    }
    loop {
        skip(&mut i);
        if i >= b.len() || out.len() > MAX_CMDS {
            break;
        }
        if b[i].is_ascii_alphabetic() {
            cmd = b[i];
            i += 1;
            if out.is_empty() && !matches!(cmd, b'M' | b'm') {
                break;
            }
            if matches!(cmd, b'Z' | b'z') {
                out.push(Cmd::Z);
                cur = start;
                last_c = None;
                last_q = None;
                continue;
            }
        } else if cmd == 0 || matches!(cmd, b'Z' | b'z') {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { cur } else { (0.0, 0.0) };
        let (mut nc, mut nq) = (None, None);
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = (num!() + ox, num!() + oy);
                out.push(Cmd::M(p));
                cur = p;
                start = p;
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let p = (num!() + ox, num!() + oy);
                out.push(Cmd::L(p));
                cur = p;
            }
            b'H' => {
                let p = (num!() + ox, cur.1);
                out.push(Cmd::L(p));
                cur = p;
            }
            b'V' => {
                let p = (cur.0, num!() + oy);
                out.push(Cmd::L(p));
                cur = p;
            }
            b'C' => {
                let c1 = (num!() + ox, num!() + oy);
                let c2 = (num!() + ox, num!() + oy);
                let p = (num!() + ox, num!() + oy);
                out.push(Cmd::C(c1, c2, p));
                nc = Some(c2);
                cur = p;
            }
            b'S' => {
                let c1 = last_c.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                let c2 = (num!() + ox, num!() + oy);
                let p = (num!() + ox, num!() + oy);
                out.push(Cmd::C(c1, c2, p));
                nc = Some(c2);
                cur = p;
            }
            b'Q' | b'T' => {
                let q = if cmd.to_ascii_uppercase() == b'Q' { (num!() + ox, num!() + oy) } else { last_q.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1)) };
                let p = (num!() + ox, num!() + oy);
                out.push(Cmd::C((cur.0 + 2.0 / 3.0 * (q.0 - cur.0), cur.1 + 2.0 / 3.0 * (q.1 - cur.1)), (p.0 + 2.0 / 3.0 * (q.0 - p.0), p.1 + 2.0 / 3.0 * (q.1 - p.1)), p));
                nq = Some(q);
                cur = p;
            }
            b'A' => {
                let (rx, ry, phi) = (num!(), num!(), num!());
                let (large, sweep) = (flag!(), flag!());
                let p = (num!() + ox, num!() + oy);
                arc_to(&mut out, cur, rx, ry, phi, large, sweep, p);
                cur = p;
            }
            _ => break,
        }
        last_c = nc;
        last_q = nq;
    }
    out
}

struct Sub {
    pts: Vec<P>,
    closed: bool,
}

/// Straight pieces of a path, fine enough for a device scale of `scale`.
fn flatten(cmds: &[Cmd], scale: f64) -> Vec<Sub> {
    let mut subs: Vec<Sub> = Vec::new();
    let mut start = (0.0, 0.0);
    let mut open = false;
    for c in cmds {
        match *c {
            Cmd::M(p) => {
                subs.push(Sub { pts: vec![p], closed: false });
                start = p;
                open = true;
            }
            Cmd::Z => {
                if let Some(s) = subs.last_mut() {
                    s.closed = true;
                }
                open = false;
            }
            Cmd::L(p) => {
                if !open {
                    subs.push(Sub { pts: vec![start], closed: false });
                    open = true;
                }
                if let Some(s) = subs.last_mut() {
                    s.pts.push(p);
                }
            }
            Cmd::C(c1, c2, p) => {
                if !open {
                    subs.push(Sub { pts: vec![start], closed: false });
                    open = true;
                }
                if let Some(s) = subs.last_mut() {
                    let p0 = *s.pts.last().unwrap_or(&start);
                    let d = |a: P, b: P| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
                    let len = (d(p0, c1) + d(c1, c2) + d(c2, p)) * scale;
                    let n = if len.is_finite() { ((len.sqrt() * 1.5).ceil() as usize).clamp(4, 96) } else { 4 };
                    for k in 1..=n {
                        let t = k as f64 / n as f64;
                        let u = 1.0 - t;
                        let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        s.pts.push((a * p0.0 + b * c1.0 + c * c2.0 + e * p.0, a * p0.1 + b * c1.1 + c * c2.1 + e * p.1));
                    }
                }
            }
        }
    }
    subs
}

// ------------------------------------------------------------------------------------------------ stroking

#[derive(Clone, Copy, PartialEq)]
enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, PartialEq)]
enum Join {
    Miter,
    Round,
    Bevel,
}

fn orient(mut poly: Vec<P>) -> Vec<P> {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        a += p.0 * q.1 - q.0 * p.1;
    }
    if a < 0.0 {
        poly.reverse();
    }
    poly
}

fn circle_poly(c: P, r: f64, scale: f64) -> Vec<P> {
    let n = ((r * scale).sqrt() * 6.0).ceil().clamp(12.0, 96.0) as usize;
    // widen the polygon a little so its area equals the circle's
    let step = std::f64::consts::TAU / n as f64;
    let r = r * (step / step.sin()).sqrt();
    (0..n).map(|i| {
        let (s, co) = (i as f64 * step).sin_cos();
        (c.0 + r * co, c.1 + r * s)
    }).collect()
}

fn split_dashes(subs: &[Sub], dash: &[f64], offset: f64) -> Option<Vec<Sub>> {
    let total: f64 = dash.iter().sum();
    if dash.iter().any(|d| *d < 0.0 || !d.is_finite()) || total <= 1e-9 {
        return None;
    }
    let pattern: Vec<f64> = if dash.len() % 2 == 1 { dash.iter().chain(dash.iter()).copied().collect() } else { dash.to_vec() };
    let ptotal: f64 = pattern.iter().sum();
    let mut out = Vec::new();
    for s in subs {
        let mut pts = s.pts.clone();
        if s.closed {
            if let Some(&f) = pts.first() {
                pts.push(f);
            }
        }
        let mut idx = 0;
        let mut off = offset.rem_euclid(ptotal);
        while off >= pattern[idx] {
            off -= pattern[idx];
            idx = (idx + 1) % pattern.len();
            if off <= 0.0 && pattern[idx] == 0.0 && pattern.iter().all(|d| *d == 0.0) {
                break;
            }
        }
        let mut remain = pattern[idx] - off;
        let mut on = idx % 2 == 0;
        let mut cur: Vec<P> = if on { pts.first().map(|p| vec![*p]).unwrap_or_default() } else { Vec::new() };
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            let mut pos = 0.0;
            while len - pos > remain {
                pos += remain;
                let t = pos / len;
                let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                if on {
                    cur.push(p);
                    out.push(Sub { pts: std::mem::take(&mut cur), closed: false });
                } else {
                    cur = vec![p];
                }
                on = !on;
                idx = (idx + 1) % pattern.len();
                remain = pattern[idx];
                if out.len() > 50_000 {
                    return None;
                }
            }
            remain -= len - pos;
            if on {
                cur.push(b);
            }
        }
        if on && cur.len() > 1 {
            out.push(Sub { pts: cur, closed: false });
        }
    }
    Some(out)
}

/// The outline of a stroke as a set of positively oriented polygons whose union (non-zero fill) is the stroke.
fn stroke_polys(subs: &[Sub], width: f64, cap: Cap, join: Join, miter: f64, scale: f64) -> Vec<Vec<P>> {
    let hw = width / 2.0;
    let mut out: Vec<Vec<P>> = Vec::new();
    for s in subs {
        let mut pts: Vec<P> = Vec::with_capacity(s.pts.len());
        for &p in &s.pts {
            if pts.last().is_none_or(|l| (l.0 - p.0).abs() > 1e-9 || (l.1 - p.1).abs() > 1e-9) {
                pts.push(p);
            }
        }
        if s.closed && pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        if pts.len() == 1 {
            // a dot: only round and square caps show it
            let c = pts[0];
            match cap {
                Cap::Round => out.push(circle_poly(c, hw, scale)),
                Cap::Square => out.push(vec![(c.0 - hw, c.1 - hw), (c.0 + hw, c.1 - hw), (c.0 + hw, c.1 + hw), (c.0 - hw, c.1 + hw)]),
                _ => {}
            }
            continue;
        }
        if pts.len() < 2 {
            continue;
        }
        let n = pts.len();
        let segs = if s.closed { n } else { n - 1 };
        let dir = |i: usize| {
            let (a, b) = (pts[i % n], pts[(i + 1) % n]);
            let l = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt().max(1e-12);
            ((b.0 - a.0) / l, (b.1 - a.1) / l)
        };
        for i in 0..segs {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let d = dir(i);
            let nm = (-d.1 * hw, d.0 * hw);
            out.push(orient(vec![(a.0 + nm.0, a.1 + nm.1), (b.0 + nm.0, b.1 + nm.1), (b.0 - nm.0, b.1 - nm.1), (a.0 - nm.0, a.1 - nm.1)]));
        }
        let joins = if s.closed { 0..n } else { 1..n - 1 };
        for i in joins {
            let v = pts[i];
            let (d0, d1) = (dir((i + n - 1) % n), dir(i));
            let cross = d0.0 * d1.1 - d0.1 * d1.0;
            let dot = d0.0 * d1.0 + d0.1 * d1.1;
            if cross.abs() < 1e-9 && dot > 0.0 {
                continue;
            }
            match join {
                Join::Round => out.push(circle_poly(v, hw, scale)),
                _ => {
                    let sd = if cross > 0.0 { -1.0 } else { 1.0 };
                    let n0 = (-d0.1 * hw * sd, d0.0 * hw * sd);
                    let n1 = (-d1.1 * hw * sd, d1.0 * hw * sd);
                    let (a, b) = ((v.0 + n0.0, v.1 + n0.1), (v.0 + n1.0, v.1 + n1.1));
                    let ratio = 1.0 / ((1.0 + dot) / 2.0).max(1e-12).sqrt();
                    if join == Join::Miter && ratio <= miter && dot > -0.999999 {
                        let k = 1.0 / (1.0 + dot);
                        out.push(orient(vec![v, a, (v.0 + (n0.0 + n1.0) * k, v.1 + (n0.1 + n1.1) * k), b]));
                    } else {
                        out.push(orient(vec![v, a, b]));
                    }
                }
            }
        }
        if !s.closed && cap != Cap::Butt {
            for (p, d) in [(pts[0], { let d = dir(0); (-d.0, -d.1) }), (pts[n - 1], dir(n - 2))] {
                match cap {
                    Cap::Round => out.push(circle_poly(p, hw, scale)),
                    _ => {
                        let nm = (-d.1 * hw, d.0 * hw);
                        let q = (p.0 + d.0 * hw, p.1 + d.1 * hw);
                        out.push(orient(vec![(p.0 + nm.0, p.1 + nm.1), (q.0 + nm.0, q.1 + nm.1), (q.0 - nm.0, q.1 - nm.1), (p.0 - nm.0, p.1 - nm.1)]));
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------ rasteriser

struct Mask {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    data: Vec<u8>,
}

fn add_span(cov: &mut [f32], diff: &mut [f32], xa: f64, xb: f64) {
    if xb <= xa {
        return;
    }
    let ia = (xa.floor().max(0.0) as usize).min(cov.len() - 1);
    let ib = (xb.floor().max(0.0) as usize).min(cov.len() - 1);
    if ia == ib {
        cov[ia] += (xb - xa) as f32;
    } else {
        cov[ia] += ((ia + 1) as f64 - xa) as f32;
        cov[ib] += (xb - ib as f64) as f32;
        diff[ia + 1] += 1.0;
        diff[ib] -= 1.0;
    }
}

/// Coverage of the polygons (device space, implicitly closed) with `SUB` sample rows per pixel and exact horizontal
/// coverage, so edges are smooth. `None` when nothing is inside the canvas or the work budget is spent.
fn fill_mask(polys: &[Vec<P>], evenodd: bool, cw: usize, ch: usize, work: &mut u64) -> Option<Mask> {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in polys {
        for &(x, y) in p {
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    if minx > maxx {
        return None;
    }
    let x0 = minx.floor().max(0.0) as usize;
    let y0 = miny.floor().max(0.0) as usize;
    let x1 = (maxx.ceil().max(0.0) as usize).min(cw);
    let y1 = (maxy.ceil().max(0.0) as usize).min(ch);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (bw, bh) = (x1 - x0, y1 - y0);
    *work += (bw * bh) as u64;
    if *work > MAX_WORK {
        return None;
    }
    let sub = SUB as f64;
    let (klo, khi) = ((y0 * SUB) as f64, (y1 * SUB) as f64);
    let (fx0, fx1) = (x0 as f64, x1 as f64);
    let mut xs: Vec<(u32, f32, i8)> = Vec::new();
    for poly in polys {
        let n = poly.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (mut a, mut b) = (poly[i], poly[(i + 1) % n]);
            if a.1 == b.1 {
                continue;
            }
            let dir = if a.1 < b.1 { 1 } else { -1 };
            if dir < 0 {
                std::mem::swap(&mut a, &mut b);
            }
            let ka = (a.1 * sub - 0.5).ceil().max(klo);
            let kb = (b.1 * sub - 0.5).ceil().min(khi);
            if kb <= ka {
                continue;
            }
            let slope = (b.0 - a.0) / (b.1 - a.1);
            let mut k = ka;
            while k < kb {
                let ys = (k + 0.5) / sub;
                let x = (a.0 + (ys - a.1) * slope).clamp(fx0, fx1);
                xs.push(((k - klo) as u32, (x - fx0) as f32, dir));
                k += 1.0;
            }
            if xs.len() > 40_000_000 {
                return None;
            }
        }
    }
    if xs.is_empty() {
        return None;
    }
    *work += xs.len() as u64 * 2;
    xs.sort_unstable_by(|p, q| p.0.cmp(&q.0).then(p.1.total_cmp(&q.1)));
    let mut data = vec![0u8; bw * bh];
    let mut cov = vec![0f32; bw + 1];
    let mut diff = vec![0f32; bw + 2];
    let mut row = (xs[0].0 as usize) / SUB;
    let flush = |row: usize, cov: &mut Vec<f32>, diff: &mut Vec<f32>, data: &mut Vec<u8>| {
        let mut run = 0f32;
        for x in 0..bw {
            run += diff[x];
            let v = (cov[x] + run) / SUB as f32;
            data[row * bw + x] = (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
        cov.iter_mut().for_each(|v| *v = 0.0);
        diff.iter_mut().for_each(|v| *v = 0.0);
    };
    let mut i = 0;
    while i < xs.len() {
        let k = xs[i].0;
        let r = k as usize / SUB;
        if r != row {
            flush(row, &mut cov, &mut diff, &mut data);
            row = r;
        }
        let (mut wind, mut start) = (0i32, 0f64);
        while i < xs.len() && xs[i].0 == k {
            let inside = |w: i32| if evenodd { w & 1 != 0 } else { w != 0 };
            let was = inside(wind);
            wind += xs[i].2 as i32;
            let now = inside(wind);
            if !was && now {
                start = xs[i].1 as f64;
            } else if was && !now {
                add_span(&mut cov, &mut diff, start, xs[i].1 as f64);
            }
            i += 1;
        }
    }
    flush(row, &mut cov, &mut diff, &mut data);
    Some(Mask { x0, y0, w: bw, h: bh, data })
}

/// Premultiplied RGBA.
#[derive(Clone)]
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![0; w * h * 4] }
    }
    fn blend(&mut self, x: usize, y: usize, rgb: [f32; 3], a: f32) {
        let o = (y * self.w + x) * 4;
        let d = &mut self.px[o..o + 4];
        let inv = 1.0 - a;
        for c in 0..3 {
            d[c] = (rgb[c] * 255.0 * a + d[c] as f32 * inv + 0.5).min(255.0) as u8;
        }
        d[3] = (a * 255.0 + d[3] as f32 * inv + 0.5).min(255.0) as u8;
    }
    fn composite_layer(&mut self, l: &Canvas, opacity: f32) {
        for (d, s) in self.px.chunks_exact_mut(4).zip(l.px.chunks_exact(4)) {
            if s[3] == 0 {
                continue;
            }
            let inv = 1.0 - s[3] as f32 / 255.0 * opacity;
            for c in 0..4 {
                d[c] = (s[c] as f32 * opacity + d[c] as f32 * inv + 0.5).min(255.0) as u8;
            }
        }
    }
}

/// Scale a layer by the alpha of a clip or mask canvas.
fn multiply_alpha(layer: &mut Canvas, k: &Canvas) {
    for (d, s) in layer.px.chunks_exact_mut(4).zip(k.px.chunks_exact(4)) {
        if d[3] == 0 {
            continue;
        }
        let f = s[3] as u32;
        if f == 255 {
            continue;
        }
        for c in d.iter_mut() {
            *c = ((*c as u32 * f + 127) / 255) as u8;
        }
    }
}

/// A Gaussian blur of a layer, as three box blurs (premultiplied channels, so edges stay soft and colours do not bleed).
fn blur_canvas(cv: &mut Canvas, sigma: f64) {
    let d = ((sigma * 3.0 * (2.0 * std::f64::consts::PI).sqrt() / 4.0 + 0.5).floor() as usize).max(1);
    let (w, h) = (cv.w, cv.h);
    let mut a: Vec<f32> = cv.px.iter().map(|&v| v as f32).collect();
    let mut b = a.clone();
    let radii = if d % 2 == 1 { [(d / 2, d / 2); 3] } else { [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)] };
    for (rl, rr) in radii {
        let win = (rl + rr + 1) as f32;
        // horizontal
        for y in 0..h {
            for c in 0..4 {
                let at = |x: isize| -> f32 { if x < 0 || x >= w as isize { 0.0 } else { a[(y * w + x as usize) * 4 + c] } };
                let mut acc: f32 = (-(rl as isize)..=rr as isize).map(at).sum();
                for x in 0..w {
                    b[(y * w + x) * 4 + c] = acc / win;
                    acc += at(x as isize + rr as isize + 1) - at(x as isize - rl as isize);
                }
            }
        }
        // vertical
        for x in 0..w {
            for c in 0..4 {
                let at = |y: isize| -> f32 { if y < 0 || y >= h as isize { 0.0 } else { b[(y as usize * w + x) * 4 + c] } };
                let mut acc: f32 = (-(rl as isize)..=rr as isize).map(at).sum();
                for y in 0..h {
                    a[(y * w + x) * 4 + c] = acc / win;
                    acc += at(y as isize + rr as isize + 1) - at(y as isize - rl as isize);
                }
            }
        }
    }
    for (o, v) in cv.px.iter_mut().zip(a) {
        *o = (v + 0.5).min(255.0) as u8;
    }
}

// ------------------------------------------------------------------------------------------------ paint

#[derive(Clone)]
enum PaintSpec {
    None,
    Color(Rgba),
    Current,
    Url(String, Option<Rgba>),
}

struct GradEval {
    inv: M,
    radial: bool,
    p: [f64; 5],
    spread: u8,
    stops: Vec<(f64, Rgba)>,
}

impl GradEval {
    fn at(&self, x: f64, y: f64) -> Rgba {
        let (gx, gy) = self.inv.apply((x, y));
        let t = if !self.radial {
            let (dx, dy) = (self.p[2] - self.p[0], self.p[3] - self.p[1]);
            let l2 = dx * dx + dy * dy;
            if l2 < 1e-12 { 1.0 } else { ((gx - self.p[0]) * dx + (gy - self.p[1]) * dy) / l2 }
        } else {
            let (cx, cy, r, fx, fy) = (self.p[0], self.p[1], self.p[2], self.p[3], self.p[4]);
            let (dx, dy) = (gx - fx, gy - fy);
            let (ex, ey) = (cx - fx, cy - fy);
            let a = ex * ex + ey * ey - r * r;
            let b = -2.0 * (dx * ex + dy * ey);
            let c = dx * dx + dy * dy;
            if a.abs() < 1e-12 {
                if b.abs() < 1e-12 { 0.0 } else { -c / b }
            } else {
                let disc = b * b - 4.0 * a * c;
                if disc < 0.0 {
                    1.0
                } else {
                    let s = disc.sqrt();
                    ((-b + s) / (2.0 * a)).max((-b - s) / (2.0 * a))
                }
            }
        };
        let t = if t.is_finite() { t } else { 0.0 };
        let t = match self.spread {
            1 => {
                let m = t.rem_euclid(2.0);
                if m > 1.0 { 2.0 - m } else { m }
            }
            2 => t.rem_euclid(1.0),
            _ => t.clamp(0.0, 1.0),
        };
        let s = &self.stops;
        if t <= s[0].0 {
            return s[0].1;
        }
        for w in s.windows(2) {
            if t <= w[1].0 {
                let span = w[1].0 - w[0].0;
                let f = if span < 1e-12 { 1.0 } else { ((t - w[0].0) / span) as f32 };
                let mut o = [0.0; 4];
                for (c, v) in o.iter_mut().enumerate() {
                    *v = w[0].1[c] + (w[1].1[c] - w[0].1[c]) * f;
                }
                return o;
            }
        }
        s[s.len() - 1].1
    }
}

enum Source {
    Solid(Rgba),
    Grad(GradEval),
    Pat(PatEval),
}

/// A pattern paint: one rendered tile, repeated.
struct PatEval {
    inv: M,
    tw: f64,
    th: f64,
    tile: Canvas,
}

impl PatEval {
    fn at(&self, x: f64, y: f64) -> Rgba {
        let (u, v) = self.inv.apply((x, y));
        let (u, v) = (u.rem_euclid(self.tw) / self.tw, v.rem_euclid(self.th) / self.th);
        let tx = ((u * self.tile.w as f64) as usize).min(self.tile.w - 1);
        let ty = ((v * self.tile.h as f64) as usize).min(self.tile.h - 1);
        let o = (ty * self.tile.w + tx) * 4;
        let p = &self.tile.px[o..o + 4];
        if p[3] == 0 {
            return [0.0; 4];
        }
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 / 255.0 / a, p[1] as f32 / 255.0 / a, p[2] as f32 / 255.0 / a, a]
    }
}

fn paint_mask(cv: &mut Canvas, mask: &Mask, src: &Source, opacity: f32) {
    for my in 0..mask.h {
        for mx in 0..mask.w {
            let m = mask.data[my * mask.w + mx];
            if m == 0 {
                continue;
            }
            let (x, y) = (mask.x0 + mx, mask.y0 + my);
            let c = match src {
                Source::Solid(c) => *c,
                Source::Grad(g) => g.at(x as f64 + 0.5, y as f64 + 0.5),
                Source::Pat(p) => p.at(x as f64 + 0.5, y as f64 + 0.5),
            };
            let a = c[3] * opacity * m as f32 / 255.0;
            if a > 0.0 {
                cv.blend(x, y, [c[0], c[1], c[2]], a.min(1.0));
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ style

#[derive(Clone)]
struct Style {
    fill: PaintSpec,
    fill_opacity: f64,
    stroke: PaintSpec,
    stroke_opacity: f64,
    stroke_width: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    evenodd: bool,
    color: Rgba,
    dash: Vec<f64>,
    dash_offset: f64,
    opacity: f64,
    display: bool,
    visible: bool,
    stop_color: PaintSpec,
    stop_opacity: f64,
    font_size: f64,
    bold: bool,
    mono: bool,
    anchor: f64,
}

impl Style {
    fn root() -> Style {
        Style { fill: PaintSpec::Color([0.0, 0.0, 0.0, 1.0]), fill_opacity: 1.0, stroke: PaintSpec::None, stroke_opacity: 1.0, stroke_width: 1.0, cap: Cap::Butt, join: Join::Miter, miter: 4.0, evenodd: false, color: [0.0, 0.0, 0.0, 1.0], dash: Vec::new(), dash_offset: 0.0, opacity: 1.0, display: true, visible: true, stop_color: PaintSpec::Color([0.0, 0.0, 0.0, 1.0]), stop_opacity: 1.0, font_size: 16.0, bold: false, mono: false, anchor: 0.0 }
    }

    fn paint(v: &str) -> Option<PaintSpec> {
        let v = v.trim();
        if v.eq_ignore_ascii_case("none") {
            return Some(PaintSpec::None);
        }
        if v.eq_ignore_ascii_case("currentcolor") {
            return Some(PaintSpec::Current);
        }
        if let Some(rest) = v.strip_prefix("url(") {
            let close = rest.find(')')?;
            let id = rest[..close].trim().trim_matches(['"', '\'']).trim_start_matches('#').to_string();
            let fallback = parse_color(rest[close + 1..].trim());
            return Some(PaintSpec::Url(id, fallback));
        }
        parse_color(v).map(PaintSpec::Color)
    }

    fn set(&mut self, name: &str, v: &str, vw: f64) {
        let v = v.trim().trim_end_matches("!important").trim();
        match name {
            "fill" => {
                if let Some(p) = Style::paint(v) {
                    self.fill = p
                }
            }
            "stroke" => {
                if let Some(p) = Style::paint(v) {
                    self.stroke = p
                }
            }
            "color" => {
                if let Some(c) = parse_color(v) {
                    self.color = c
                }
            }
            "fill-opacity" => self.fill_opacity = opacity_value(v).unwrap_or(self.fill_opacity),
            "stroke-opacity" => self.stroke_opacity = opacity_value(v).unwrap_or(self.stroke_opacity),
            "opacity" => self.opacity = opacity_value(v).unwrap_or(self.opacity),
            "stop-opacity" => self.stop_opacity = opacity_value(v).unwrap_or(self.stop_opacity),
            "stop-color" => {
                if let Some(p) = Style::paint(v) {
                    self.stop_color = p
                }
            }
            "stroke-width" => {
                if let Some(w) = length(v, vw) {
                    self.stroke_width = w.max(0.0)
                }
            }
            "stroke-miterlimit" => {
                if let Some((m, _)) = take_number(v) {
                    self.miter = m.max(1.0)
                }
            }
            "stroke-linecap" => {
                self.cap = match v {
                    "round" => Cap::Round,
                    "square" => Cap::Square,
                    _ => Cap::Butt,
                }
            }
            "stroke-linejoin" => {
                self.join = match v {
                    "round" => Join::Round,
                    "bevel" => Join::Bevel,
                    _ => Join::Miter,
                }
            }
            "fill-rule" => self.evenodd = v == "evenodd",
            "stroke-dasharray" => {
                self.dash = if v == "none" { Vec::new() } else { v.split([',', ' ']).filter(|p| !p.is_empty()).filter_map(|p| length(p, vw)).collect() }
            }
            "stroke-dashoffset" => self.dash_offset = length(v, vw).unwrap_or(0.0),
            "font-size" => {
                if let Some((n, unit)) = take_number(v) {
                    let size = match unit.trim() {
                        "em" => n * self.font_size,
                        "%" => n * self.font_size / 100.0,
                        _ => length(v, self.font_size).unwrap_or(self.font_size),
                    };
                    if size.is_finite() && size > 0.0 {
                        self.font_size = size.min(2000.0);
                    }
                } else {
                    self.font_size = match v {
                        "xx-small" => 9.0,
                        "x-small" => 10.0,
                        "small" => 13.0,
                        "medium" => 16.0,
                        "large" => 18.0,
                        "x-large" => 24.0,
                        "xx-large" => 32.0,
                        _ => self.font_size,
                    };
                }
            }
            "font-weight" => self.bold = v == "bold" || v == "bolder" || v.parse::<u32>().is_ok_and(|w| w >= 600),
            "font-family" => {
                let l = v.to_ascii_lowercase();
                self.mono = l.contains("mono") || l.contains("courier") || l.contains("consolas");
            }
            "text-anchor" => {
                self.anchor = match v {
                    "middle" => 0.5,
                    "end" => 1.0,
                    _ => 0.0,
                }
            }
            "display" => self.display = v != "none",
            "visibility" => self.visible = !matches!(v, "hidden" | "collapse"),
            _ => {}
        }
    }
}

fn opacity_value(v: &str) -> Option<f64> {
    let (n, rest) = take_number(v)?;
    Some(if rest.trim() == "%" { n / 100.0 } else { n }.clamp(0.0, 1.0))
}

struct Rule {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    spec: u32,
    decls: Vec<(String, String)>,
}

fn parse_css(css: &str) -> Vec<Rule> {
    let mut clean = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        clean.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    clean.push_str(rest);
    let mut rules = Vec::new();
    let mut s = clean.as_str();
    while let Some(open) = s.find('{') {
        let sel = s[..open].trim();
        let Some(close) = s[open..].find('}') else { break };
        let body = &s[open + 1..open + close];
        s = &s[open + close + 1..];
        if sel.starts_with('@') || rules.len() > 2000 {
            continue;
        }
        let decls: Vec<(String, String)> = body.split(';').filter_map(|d| d.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
        for one in sel.split(',') {
            let one = one.trim();
            if one.is_empty() || one.contains(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~' | '[' | ':' | '(')) {
                continue;
            }
            let (mut tag, mut id, mut classes) = (None, None, Vec::new());
            let mut spec = 0;
            // split into pieces at . and #
            let mut cur = String::new();
            let mut kind = 't';
            let mut pieces = Vec::new();
            for ch in one.chars().chain(std::iter::once('\0')) {
                if matches!(ch, '.' | '#' | '\0') {
                    if !cur.is_empty() {
                        pieces.push((kind, std::mem::take(&mut cur)));
                    }
                    kind = ch;
                } else {
                    cur.push(ch);
                }
            }
            for (k, v) in pieces {
                match k {
                    't' if v != "*" => {
                        tag = Some(v);
                        spec += 1;
                    }
                    '.' => {
                        classes.push(v);
                        spec += 100;
                    }
                    '#' => {
                        id = Some(v);
                        spec += 10_000;
                    }
                    _ => {}
                }
            }
            rules.push(Rule { tag, id, classes, spec, decls: decls.clone() });
        }
    }
    rules.sort_by_key(|r| r.spec);
    rules
}

// ------------------------------------------------------------------------------------------------ rendering

struct Gradient {
    radial: bool,
    bbox_units: bool,
    tf: M,
    spread: u8,
    coords: [Option<String>; 5],
    stops: Vec<(f64, Rgba)>,
}

struct Renderer<'a> {
    xml: &'a Xml,
    ids: HashMap<String, usize>,
    rules: Vec<Rule>,
    vw: f64,
    vh: f64,
    visits: usize,
    work: u64,
    layer_px: u64,
    text: Option<&'a mut dyn TextRenderer>,
    glyph_runs: usize,
    deadline: std::time::Instant,
    /// Patterns and markers drawn inside each other.
    nest: usize,
}

#[derive(Default, Clone)]
struct TextPos {
    x: Option<f64>,
    y: Option<f64>,
    dx: f64,
    dy: f64,
}

struct TextRun {
    text: String,
    st: Style,
    pos: TextPos,
    ux: f64,
    uy: f64,
    width: f64,
}

fn shape_path(node: &Node, vw: f64, vh: f64) -> Vec<Cmd> {
    let len = |name: &str, refv: f64| node.attr(name).and_then(|v| length(v, refv)).unwrap_or(0.0);
    let diag = ((vw * vw + vh * vh) / 2.0).sqrt();
    let mut out = Vec::new();
    match node.tag.as_str() {
        "path" => return parse_path(node.attr("d").unwrap_or("")),
        "rect" => {
            let (x, y, w, h) = (len("x", vw), len("y", vh), len("width", vw), len("height", vh));
            if w <= 0.0 || h <= 0.0 {
                return out;
            }
            let rxa = node.attr("rx").and_then(|v| length(v, vw));
            let rya = node.attr("ry").and_then(|v| length(v, vh));
            let (mut rx, mut ry) = match (rxa, rya) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, a),
                (None, Some(b)) => (b, b),
                _ => (0.0, 0.0),
            };
            rx = rx.clamp(0.0, w / 2.0);
            ry = ry.clamp(0.0, h / 2.0);
            if rx <= 0.0 || ry <= 0.0 {
                out.extend([Cmd::M((x, y)), Cmd::L((x + w, y)), Cmd::L((x + w, y + h)), Cmd::L((x, y + h)), Cmd::Z]);
            } else {
                out.push(Cmd::M((x + rx, y)));
                out.push(Cmd::L((x + w - rx, y)));
                arc_to(&mut out, (x + w - rx, y), rx, ry, 0.0, false, true, (x + w, y + ry));
                out.push(Cmd::L((x + w, y + h - ry)));
                arc_to(&mut out, (x + w, y + h - ry), rx, ry, 0.0, false, true, (x + w - rx, y + h));
                out.push(Cmd::L((x + rx, y + h)));
                arc_to(&mut out, (x + rx, y + h), rx, ry, 0.0, false, true, (x, y + h - ry));
                out.push(Cmd::L((x, y + ry)));
                arc_to(&mut out, (x, y + ry), rx, ry, 0.0, false, true, (x + rx, y));
                out.push(Cmd::Z);
            }
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (len("cx", vw), len("cy", vh));
            let (rx, ry) = if node.tag == "circle" { let r = len("r", diag); (r, r) } else { (len("rx", vw), len("ry", vh)) };
            if rx <= 0.0 || ry <= 0.0 {
                return out;
            }
            out.push(Cmd::M((cx + rx, cy)));
            arc_to(&mut out, (cx + rx, cy), rx, ry, 0.0, false, true, (cx, cy + ry));
            arc_to(&mut out, (cx, cy + ry), rx, ry, 0.0, false, true, (cx - rx, cy));
            arc_to(&mut out, (cx - rx, cy), rx, ry, 0.0, false, true, (cx, cy - ry));
            arc_to(&mut out, (cx, cy - ry), rx, ry, 0.0, false, true, (cx + rx, cy));
            out.push(Cmd::Z);
        }
        "line" => {
            out.push(Cmd::M((len("x1", vw), len("y1", vh))));
            out.push(Cmd::L((len("x2", vw), len("y2", vh))));
        }
        "polyline" | "polygon" => {
            let n = numbers(node.attr("points").unwrap_or(""));
            for (i, c) in n.chunks_exact(2).enumerate() {
                out.push(if i == 0 { Cmd::M((c[0], c[1])) } else { Cmd::L((c[0], c[1])) });
            }
            if node.tag == "polygon" && !out.is_empty() {
                out.push(Cmd::Z);
            }
        }
        _ => {}
    }
    out
}

impl<'a> Renderer<'a> {
    fn new(xml: &'a Xml, vw: f64, vh: f64) -> Renderer<'a> {
        let mut ids = HashMap::new();
        let mut css = String::new();
        for (i, n) in xml.nodes.iter().enumerate() {
            if let Some(id) = n.attr("id") {
                ids.entry(id.to_string()).or_insert(i);
            }
            if n.tag == "style" && css.len() < 1 << 20 {
                css.push_str(&n.text);
                css.push('\n');
            }
        }
        Renderer { xml, ids, rules: parse_css(&css), vw, vh, visits: 0, work: 0, layer_px: 0, text: None, glyph_runs: 0, deadline: std::time::Instant::now() + TIME_BUDGET, nest: 0 }
    }

    fn cascade(&self, parent: &Style, n: usize) -> Style {
        let node = &self.xml.nodes[n];
        let mut st = parent.clone();
        st.opacity = 1.0;
        st.display = true;
        st.stop_color = PaintSpec::Color([0.0, 0.0, 0.0, 1.0]);
        st.stop_opacity = 1.0;
        let mut decls: Vec<(&str, &str)> = Vec::new();
        for (k, v) in &node.attrs {
            decls.push((k.as_str(), v.as_str()));
        }
        let classes: Vec<&str> = node.attr("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
        for r in &self.rules {
            if r.tag.as_ref().is_some_and(|t| *t != node.tag) || r.id.as_deref().is_some_and(|i| Some(i) != node.attr("id")) || !r.classes.iter().all(|c| classes.contains(&c.as_str())) {
                continue;
            }
            for (k, v) in &r.decls {
                decls.push((k.as_str(), v.as_str()));
            }
        }
        let inline: Vec<(String, String)> = node.attr("style").map(|s| s.split(';').filter_map(|d| d.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect()).unwrap_or_default();
        for (k, v) in &inline {
            decls.push((k.as_str(), v.as_str()));
        }
        for (k, v) in decls.iter().filter(|(k, _)| *k == "color") {
            st.set(k, v, self.vw);
        }
        for (k, v) in &decls {
            st.set(k, v, self.vw);
        }
        st
    }

    fn resolve(&self, p: &PaintSpec, st: &Style) -> PaintSpec {
        match p {
            PaintSpec::Current => PaintSpec::Color(st.color),
            other => other.clone(),
        }
    }

    fn gradient(&self, id: usize) -> Option<Gradient> {
        let head = &self.xml.nodes[id];
        let radial = match head.tag.as_str() {
            "linearGradient" => false,
            "radialGradient" => true,
            _ => return None,
        };
        let names: [&str; 5] = if radial { ["cx", "cy", "r", "fx", "fy"] } else { ["x1", "y1", "x2", "y2", ""] };
        let mut g = Gradient { radial, bbox_units: true, tf: M::ID, spread: 0, coords: Default::default(), stops: Vec::new() };
        let (mut seen_units, mut seen_tf, mut seen_spread, mut seen_stops) = (false, false, false, false);
        let mut cur = id;
        for _ in 0..8 {
            let n = &self.xml.nodes[cur];
            if !seen_units {
                if let Some(u) = n.attr("gradientUnits") {
                    g.bbox_units = u != "userSpaceOnUse";
                    seen_units = true;
                }
            }
            if !seen_tf {
                if let Some(t) = n.attr("gradientTransform") {
                    g.tf = parse_transform(t);
                    seen_tf = true;
                }
            }
            if !seen_spread {
                if let Some(s) = n.attr("spreadMethod") {
                    g.spread = match s {
                        "reflect" => 1,
                        "repeat" => 2,
                        _ => 0,
                    };
                    seen_spread = true;
                }
            }
            if n.tag == head.tag {
                for (i, name) in names.iter().enumerate() {
                    if !name.is_empty() && g.coords[i].is_none() {
                        g.coords[i] = n.attr(name).map(str::to_string);
                    }
                }
            }
            if !seen_stops {
                let stops: Vec<usize> = n.kids.iter().copied().filter(|&k| self.xml.nodes[k].tag == "stop").collect();
                if !stops.is_empty() {
                    seen_stops = true;
                    let mut last = 0.0f64;
                    for k in stops {
                        let sn = &self.xml.nodes[k];
                        let off = sn.attr("offset").and_then(opacity_value).unwrap_or(0.0).max(last);
                        last = off;
                        let st = self.cascade(&Style::root(), k);
                        let mut c = match self.resolve(&st.stop_color, &st) {
                            PaintSpec::Color(c) => c,
                            _ => [0.0, 0.0, 0.0, 1.0],
                        };
                        c[3] *= st.stop_opacity as f32;
                        g.stops.push((off, c));
                    }
                }
            }
            match n.attr("href").and_then(|h| h.strip_prefix('#')).and_then(|h| self.ids.get(h)) {
                Some(&next) if next != cur => cur = next,
                _ => break,
            }
        }
        Some(g)
    }

    fn make_source(&mut self, spec: &PaintSpec, st: &Style, bbox: (f64, f64, f64, f64), m: &M) -> Option<Source> {
        match self.resolve(spec, st) {
            PaintSpec::None | PaintSpec::Current => None,
            PaintSpec::Color(c) => Some(Source::Solid(c)),
            PaintSpec::Url(id, fallback) => {
                if let Some(&pi) = self.ids.get(&id).filter(|&&i| self.xml.nodes[i].tag == "pattern") {
                    return self.pattern_source(pi, st, bbox, m).or(fallback.map(Source::Solid));
                }
                let Some(g) = self.ids.get(&id).and_then(|&i| self.gradient(i)) else { return fallback.map(Source::Solid) };
                if g.stops.is_empty() {
                    return None;
                }
                if g.stops.len() == 1 {
                    return Some(Source::Solid(g.stops[0].1));
                }
                let (bw, bh) = (bbox.2 - bbox.0, bbox.3 - bbox.1);
                let mut full = *m;
                if g.bbox_units {
                    if bw <= 1e-9 || bh <= 1e-9 {
                        return None;
                    }
                    full = full.mul(&M([bw, 0.0, 0.0, bh, bbox.0, bbox.1]));
                }
                full = full.mul(&g.tf);
                let inv = full.invert()?;
                let coord = |i: usize, def: f64, refv: f64| -> f64 {
                    match g.coords[i].as_deref() {
                        Some(s) if g.bbox_units => {
                            let t = s.trim();
                            if t.ends_with('%') { take_number(t).map_or(def, |(v, _)| v / 100.0) } else { take_number(t).map_or(def, |(v, _)| v) }
                        }
                        Some(s) => length(s, refv).unwrap_or(def * refv),
                        None if g.bbox_units => def,
                        None => def * refv,
                    }
                };
                let (rx, ry) = (self.vw, self.vh);
                let p = if g.radial {
                    let cx = coord(0, 0.5, rx);
                    let cy = coord(1, 0.5, ry);
                    let r = coord(2, 0.5, (rx + ry) / 2.0);
                    let fx = if g.coords[3].is_some() { coord(3, 0.5, rx) } else { cx };
                    let fy = if g.coords[4].is_some() { coord(4, 0.5, ry) } else { cy };
                    [cx, cy, r, fx, fy]
                } else {
                    [coord(0, 0.0, rx), coord(1, 0.0, ry), coord(2, 1.0, rx), coord(3, 0.0, ry), 0.0]
                };
                Some(Source::Grad(GradEval { inv, radial: g.radial, p, spread: g.spread, stops: g.stops }))
            }
        }
    }

    fn render_node(&mut self, cv: &mut Canvas, n: usize, parent: &Style, m: &M, depth: usize) {
        self.visits += 1;
        if depth > MAX_RENDER_DEPTH || self.visits > MAX_VISITS || self.work > MAX_WORK || (self.visits % 16 == 0 && std::time::Instant::now() > self.deadline) {
            return;
        }
        let xml = self.xml;
        let node = &xml.nodes[n];
        if !matches!(node.tag.as_str(), "g" | "a" | "svg" | "switch" | "use" | "text" | "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon") {
            return;
        }
        let st = self.cascade(parent, n);
        if !st.display {
            return;
        }
        let mut m2 = *m;
        if let Some(t) = node.attr("transform") {
            m2 = m.mul(&parse_transform(t));
        }
        if !m2.finite() {
            return;
        }
        let area = (cv.w * cv.h) as u64;
        let clip = self.reference(node, "clip-path", "clipPath");
        let mask = self.reference(node, "mask", "mask");
        let filter = self.reference(node, "filter", "filter").filter(|&f| self.has_primitives(f));
        let layered = st.opacity < 0.999 || clip.is_some() || mask.is_some() || filter.is_some();
        if layered && self.layer_px + area <= MAX_LAYER_PIXELS {
            self.layer_px += area;
            let mut layer = Canvas::new(cv.w, cv.h);
            let mut st2 = st.clone();
            st2.opacity = 1.0;
            self.render_inner(&mut layer, n, &st2, &m2, depth);
            if let Some(f) = filter {
                self.apply_filter(&mut layer, f, &m2);
            }
            if let Some(c) = clip {
                if self.layer_px + area <= MAX_LAYER_PIXELS {
                    self.layer_px += area;
                    let k = self.clip_canvas(c, &m2, cv.w, cv.h, depth, false);
                    multiply_alpha(&mut layer, &k);
                }
            }
            if let Some(mk) = mask {
                if self.layer_px + area <= MAX_LAYER_PIXELS {
                    self.layer_px += area;
                    let k = self.clip_canvas(mk, &m2, cv.w, cv.h, depth, true);
                    multiply_alpha(&mut layer, &k);
                }
            }
            cv.composite_layer(&layer, st.opacity as f32);
        } else {
            self.render_inner(cv, n, &st, &m2, depth);
        }
    }

    /// The element a property such as `clip-path="url(#id)"` points to, when it is a `tag` element.
    fn reference(&self, node: &Node, prop: &str, tag: &str) -> Option<usize> {
        let v = node.attr(prop).map(str::to_string).or_else(|| {
            node.attr("style").and_then(|s| s.split(';').filter_map(|d| d.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case(prop)).map(|(_, v)| v.trim().to_string()))
        })?;
        let rest = &v[v.find("url(")? + 4..];
        let id = rest[..rest.find(')')?].trim().trim_matches(['\'', '"']).strip_prefix('#')?.to_string();
        self.ids.get(&id).copied().filter(|&t| self.xml.nodes[t].tag == tag)
    }

    fn has_primitives(&self, f: usize) -> bool {
        self.xml.nodes[f].kids.iter().any(|&k| FILTER_PRIMS.contains(&self.xml.nodes[k].tag.as_str()))
    }

    /// Runs the primitives of a `filter` element over a layer, in order, and leaves the last result in the layer.
    fn apply_filter(&mut self, layer: &mut Canvas, f: usize, m: &M) {
        let xml = self.xml;
        let k = m.scale_factor();
        // the primitives work in linearRGB unless the filter says `color-interpolation-filters="sRGB"` (the property's default, SVG 1.1 and
        // Filter Effects): the picture goes in as linear light, flood colours are made linear, and the result goes back to sRGB at the end.
        // (A different value on one primitive is not honoured: the value of the filter element holds for all of them.)
        let linear = !filter_is_srgb(xml, f);
        // the filter region: nothing is drawn outside it, a flood fills only it, and the picture is cut to it first
        let region = self.filter_region(f, layer, m);
        let mut src = layer.clone();
        clip_to_region(&mut src, region);
        if linear {
            convert_space(&mut src, true);
        }
        let mut named: HashMap<String, Canvas> = HashMap::new();
        let mut cur = src.clone();
        let mut steps = 0;
        for &p in &xml.nodes[f].kids {
            let node = &xml.nodes[p];
            if !FILTER_PRIMS.contains(&node.tag.as_str()) {
                continue;
            }
            steps += 1;
            if steps > 24 || self.work > MAX_WORK {
                break;
            }
            self.work += (src.w * src.h) as u64 * 6;
            let input = |name: Option<&str>, cur: &Canvas, named: &HashMap<String, Canvas>| -> Canvas {
                match name.map(str::trim) {
                    Some("SourceGraphic") => src.clone(),
                    Some("SourceAlpha") => alpha_only(&src),
                    Some(n) if named.contains_key(n) => named[n].clone(),
                    _ => cur.clone(),
                }
            };
            let a = input(node.attr("in"), &cur, &named);
            let num = |name: &str, def: f64| node.attr(name).and_then(|v| numbers(v).first().copied()).filter(|v| v.is_finite()).unwrap_or(def);
            let flood = |node: &Node| -> Rgba {
                let mut c = node.attr("flood-color").and_then(parse_color).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                if let Some(o) = node.attr("flood-opacity").and_then(|v| numbers(v).first().copied()) {
                    c[3] *= o.clamp(0.0, 1.0) as f32;
                }
                if linear {
                    for ch in c.iter_mut().take(3) {
                        *ch = srgb_to_linear(*ch);
                    }
                }
                c
            };
            let out = match node.tag.as_str() {
                "feGaussianBlur" => {
                    let v = node.attr("stdDeviation").map(numbers).unwrap_or_default();
                    let sd = if v.len() > 1 { (v[0] + v[1]) / 2.0 } else { v.first().copied().unwrap_or(0.0) };
                    let mut c = a;
                    if sd.is_finite() && sd * k > 0.05 {
                        blur_canvas(&mut c, (sd * k).min(60.0));
                    }
                    c
                }
                "feOffset" => offset_canvas(&a, (num("dx", 0.0) * k).round() as i64, (num("dy", 0.0) * k).round() as i64),
                "feFlood" => flood_canvas(a.w, a.h, flood(node)),
                "feColorMatrix" => {
                    let vals = node.attr("values").map(numbers).unwrap_or_default();
                    let mut mx = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0f64];
                    match node.attr("type").unwrap_or("matrix") {
                        "saturate" => {
                            let s = vals.first().copied().unwrap_or(1.0).max(0.0);
                            mx = [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.0, 0.0, 0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.0, 0.0, 0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
                        }
                        "hueRotate" => {
                            let (sn, cs) = vals.first().copied().unwrap_or(0.0).to_radians().sin_cos();
                            mx = [
                                0.213 + cs * 0.787 - sn * 0.213, 0.715 - cs * 0.715 - sn * 0.715, 0.072 - cs * 0.072 + sn * 0.928, 0.0, 0.0,
                                0.213 - cs * 0.213 + sn * 0.143, 0.715 + cs * 0.285 + sn * 0.140, 0.072 - cs * 0.072 - sn * 0.283, 0.0, 0.0,
                                0.213 - cs * 0.213 - sn * 0.787, 0.715 - cs * 0.715 + sn * 0.715, 0.072 + cs * 0.928 + sn * 0.072, 0.0, 0.0,
                                0.0, 0.0, 0.0, 1.0, 0.0,
                            ];
                        }
                        "luminanceToAlpha" => {
                            mx = [0.0; 20];
                            mx[15] = 0.2125;
                            mx[16] = 0.7154;
                            mx[17] = 0.0721;
                        }
                        _ => {
                            if vals.len() == 20 {
                                mx.copy_from_slice(&vals);
                            }
                        }
                    }
                    color_matrix(a, &mx)
                }
                "feComposite" => {
                    let b = input(node.attr("in2"), &cur, &named);
                    let op = node.attr("operator").unwrap_or("over").trim().to_string();
                    let kk = [num("k1", 0.0) as f32, num("k2", 0.0) as f32, num("k3", 0.0) as f32, num("k4", 0.0) as f32];
                    combine(&a, &b, |x, y| match op.as_str() {
                        "in" => scale4(x, y[3]),
                        "out" => scale4(x, 1.0 - y[3]),
                        "atop" => add4(scale4(x, y[3]), scale4(y, 1.0 - x[3])),
                        "xor" => add4(scale4(x, 1.0 - y[3]), scale4(y, 1.0 - x[3])),
                        "arithmetic" => {
                            let mut o = [0.0; 4];
                            for c in 0..4 {
                                o[c] = (kk[0] * x[c] * y[c] + kk[1] * x[c] + kk[2] * y[c] + kk[3]).clamp(0.0, 1.0);
                            }
                            for c in 0..3 {
                                o[c] = o[c].min(o[3]);
                            }
                            o
                        }
                        _ => add4(x, scale4(y, 1.0 - x[3])),
                    })
                }
                "feBlend" => {
                    let b = input(node.attr("in2"), &cur, &named);
                    let mode = node.attr("mode").unwrap_or("normal").trim().to_string();
                    combine(&a, &b, |x, y| {
                        let mut o = [0.0; 4];
                        o[3] = 1.0 - (1.0 - x[3]) * (1.0 - y[3]);
                        for c in 0..3 {
                            let (ca, cb) = (x[c], y[c]);
                            o[c] = match mode.as_str() {
                                "multiply" => (1.0 - y[3]) * ca + (1.0 - x[3]) * cb + ca * cb,
                                "screen" => ca + cb - ca * cb,
                                "darken" => (1.0 - y[3]) * ca + (1.0 - x[3]) * cb + (ca * y[3]).min(cb * x[3]),
                                "lighten" => (1.0 - y[3]) * ca + (1.0 - x[3]) * cb + (ca * y[3]).max(cb * x[3]),
                                _ => ca + cb * (1.0 - x[3]),
                            };
                        }
                        o
                    })
                }
                "feMerge" => {
                    let mut acc = Canvas::new(a.w, a.h);
                    for &mn in node.kids.iter().filter(|&&mn| xml.nodes[mn].tag == "feMergeNode").take(16) {
                        let layer = input(xml.nodes[mn].attr("in"), &cur, &named);
                        acc = combine(&layer, &acc, |x, y| add4(x, scale4(y, 1.0 - x[3])));
                    }
                    acc
                }
                "feDropShadow" => {
                    let v = node.attr("stdDeviation").map(numbers).unwrap_or_default();
                    let sd = if v.len() > 1 { (v[0] + v[1]) / 2.0 } else { v.first().copied().unwrap_or(2.0) };
                    let fl = flood(node);
                    let mut sh = offset_canvas(&alpha_only(&a), (num("dx", 2.0) * k).round() as i64, (num("dy", 2.0) * k).round() as i64);
                    if sd.is_finite() && sd * k > 0.05 {
                        blur_canvas(&mut sh, (sd * k).min(60.0));
                    }
                    let tint = flood_canvas(a.w, a.h, fl);
                    let sh = combine(&tint, &sh, |t, s| scale4(t, s[3]));
                    combine(&a, &sh, |x, y| add4(x, scale4(y, 1.0 - x[3])))
                }
                _ => a,
            };
            if let Some(name) = node.attr("result") {
                if named.len() < 24 {
                    named.insert(name.trim().to_string(), out.clone());
                }
            }
            let mut out = out;
            clip_to_region(&mut out, region);
            cur = out;
        }
        if linear {
            convert_space(&mut cur, false);
        }
        *layer = cur;
    }

    /// The filter region in picture pixels (left, top, right, bottom). Without `filterUnits="userSpaceOnUse"` it is given as fractions of
    /// the element's box (here the extent of what the element painted, strokes included, in picture pixels), by default from -10 % to 110 %.
    /// An element that painted nothing has no box: the region is empty and the filter shows nothing, as in other renderers.
    fn filter_region(&self, f: usize, layer: &Canvas, m: &M) -> (usize, usize, usize, usize) {
        let node = &self.xml.nodes[f];
        let user = node.attr("filterUnits").is_some_and(|u| u.trim() == "userSpaceOnUse");
        let frac = |name: &str, def: f64| -> f64 {
            match node.attr(name).map(str::trim) {
                Some(t) if t.ends_with('%') => take_number(t).map_or(def, |(v, _)| v / 100.0),
                Some(t) => take_number(t).map_or(def, |(v, _)| v),
                None => def,
            }
        };
        let (w, h) = (layer.w, layer.h);
        if user {
            let len = |name: &str, def: f64, refv: f64| -> f64 { node.attr(name).and_then(|v| length(v, refv)).unwrap_or(def * refv) };
            let (x, y) = (len("x", -0.1, self.vw), len("y", -0.1, self.vh));
            let (rw, rh) = (len("width", 1.2, self.vw), len("height", 1.2, self.vh));
            let pts = [m.apply((x, y)), m.apply((x + rw, y)), m.apply((x, y + rh)), m.apply((x + rw, y + rh))];
            let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
            let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
            return region_px(x0, y0, x1, y1, w, h);
        }
        let (mut bx0, mut by0, mut bx1, mut by1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for (i, p) in layer.px.chunks_exact(4).enumerate() {
            if p[3] > 0 {
                let (x, y) = (i % w, i / w);
                bx0 = bx0.min(x);
                by0 = by0.min(y);
                bx1 = bx1.max(x + 1);
                by1 = by1.max(y + 1);
            }
        }
        if bx0 == usize::MAX {
            return (0, 0, 0, 0);
        }
        let (bw, bh) = ((bx1 - bx0) as f64, (by1 - by0) as f64);
        let (fx, fy, fw, fh) = (frac("x", -0.1), frac("y", -0.1), frac("width", 1.2), frac("height", 1.2));
        region_px(bx0 as f64 + fx * bw, by0 as f64 + fy * bh, bx0 as f64 + (fx + fw) * bw, by0 as f64 + (fy + fh) * bh, w, h)
    }

    /// A `pattern` paint: the tile is drawn once, then sampled over the shape.
    fn pattern_source(&mut self, pi: usize, st: &Style, bbox: (f64, f64, f64, f64), m: &M) -> Option<Source> {
        if self.nest >= 2 {
            return None;
        }
        let xml = self.xml;
        let pn = &xml.nodes[pi];
        let (bw, bh) = (bbox.2 - bbox.0, bbox.3 - bbox.1);
        let bbox_units = pn.attr("patternUnits").map_or(true, |u| u.trim() != "userSpaceOnUse");
        let content_bbox = pn.attr("patternContentUnits").is_some_and(|u| u.trim() == "objectBoundingBox");
        let coord = |name: &str, refv: f64, bb_off: f64, bb_size: f64| -> f64 {
            let raw = pn.attr(name).map(str::trim).unwrap_or("0");
            if bbox_units {
                let v = if raw.ends_with('%') { take_number(raw).map_or(0.0, |(v, _)| v / 100.0) } else { take_number(raw).map_or(0.0, |(v, _)| v) };
                bb_off + v * bb_size
            } else {
                length(raw, refv).unwrap_or(0.0)
            }
        };
        let tx = coord("x", self.vw, bbox.0, bw);
        let ty = coord("y", self.vh, bbox.1, bh);
        let tw = coord("width", self.vw, 0.0, bw);
        let th = coord("height", self.vh, 0.0, bh);
        if !(tw.is_finite() && th.is_finite()) || tw <= 1e-6 || th <= 1e-6 {
            return None;
        }
        let ptf = pn.attr("patternTransform").map(parse_transform).unwrap_or(M::ID);
        let to_dev = m.mul(&ptf).mul(&M([1.0, 0.0, 0.0, 1.0, tx, ty]));
        let inv = to_dev.invert()?;
        let sc = to_dev.scale_factor();
        let (pw, ph) = (((tw * sc).ceil() as usize).clamp(1, 512), ((th * sc).ceil() as usize).clamp(1, 512));
        if self.layer_px + (pw * ph) as u64 > MAX_LAYER_PIXELS {
            return None;
        }
        self.layer_px += (pw * ph) as u64;
        let mut tile = Canvas::new(pw, ph);
        let mut content = M([pw as f64 / tw, 0.0, 0.0, ph as f64 / th, 0.0, 0.0]);
        if let Some(vb) = view_box(pn) {
            content = content.mul(&fit_view_box(&vb, tw, th, pn.attr("preserveAspectRatio").unwrap_or("")));
        } else if content_bbox {
            content = content.mul(&M([bw, 0.0, 0.0, bh, 0.0, 0.0]));
        }
        let pst = self.cascade(&Style::root(), pi);
        let _ = st;
        self.nest += 1;
        for &k in &xml.nodes[pi].kids {
            self.render_node(&mut tile, k, &pst, &content, 2);
        }
        self.nest -= 1;
        Some(Source::Pat(PatEval { inv, tw, th, tile }))
    }

    /// `marker-start`, `marker-mid` and `marker-end` of a shape, drawn on its vertices.
    fn draw_markers(&mut self, cv: &mut Canvas, node: &Node, st: &Style, m: &M, cmds: &[Cmd]) {
        if self.nest >= 2 {
            return;
        }
        let find = |prop: &str| self.reference(node, prop, "marker").or_else(|| self.reference(node, "marker", "marker"));
        let (ms, mm, me) = (find("marker-start"), find("marker-mid"), find("marker-end"));
        if ms.is_none() && mm.is_none() && me.is_none() {
            return;
        }
        let vs = marker_vertices(cmds);
        let last = vs.len().saturating_sub(1);
        for (i, v) in vs.iter().enumerate().take(512) {
            let which = if i == 0 { ms } else if i == last { me } else { mm };
            if let Some(mk) = which {
                self.draw_marker(cv, mk, st, m, v, i == 0);
            }
        }
    }

    fn draw_marker(&mut self, cv: &mut Canvas, mk: usize, st: &Style, m: &M, v: &Vtx, at_start: bool) {
        let xml = self.xml;
        let mn = &xml.nodes[mk];
        let mw = mn.attr("markerWidth").and_then(|x| length(x, self.vw)).unwrap_or(3.0);
        let mh = mn.attr("markerHeight").and_then(|x| length(x, self.vh)).unwrap_or(3.0);
        if mw <= 0.0 || mh <= 0.0 {
            return;
        }
        let refx = mn.attr("refX").and_then(|x| numbers(x).first().copied()).unwrap_or(0.0);
        let refy = mn.attr("refY").and_then(|x| numbers(x).first().copied()).unwrap_or(0.0);
        let units = if mn.attr("markerUnits").is_some_and(|u| u.trim() == "userSpaceOnUse") { 1.0 } else { st.stroke_width };
        let auto = v.angle;
        let ang = match mn.attr("orient").map(str::trim) {
            Some("auto") => auto,
            Some("auto-start-reverse") => if at_start { auto + 180.0 } else { auto },
            Some(o) => numbers(o).first().copied().unwrap_or(0.0),
            None => 0.0,
        };
        let (sn, cs) = ang.to_radians().sin_cos();
        let mut mt = m.mul(&M([1.0, 0.0, 0.0, 1.0, v.p.0, v.p.1])).mul(&M([cs, sn, -sn, cs, 0.0, 0.0])).mul(&M([units, 0.0, 0.0, units, 0.0, 0.0]));
        if let Some(vb) = view_box(mn) {
            let f = fit_view_box(&vb, mw, mh, mn.attr("preserveAspectRatio").unwrap_or(""));
            // the reference point is in viewBox units; the view box's own offset is already inside `f`
            let fm = M([f.0[0], f.0[1], f.0[2], f.0[3], 0.0, 0.0]);
            mt = mt.mul(&fm);
        }
        mt = mt.mul(&M([1.0, 0.0, 0.0, 1.0, -refx, -refy]));
        if !mt.finite() {
            return;
        }
        let mst = self.cascade(&Style::root(), mk);
        self.nest += 1;
        for &k in &xml.nodes[mk].kids {
            self.render_node(cv, k, &mst, &mt, 2);
        }
        self.nest -= 1;
    }

    /// The shapes of a `clipPath` (drawn solid) or a `mask` (drawn as they are, then read as luminance) as a canvas whose
    /// alpha says how much of the clipped picture stays.
    fn clip_canvas(&mut self, t: usize, m: &M, w: usize, h: usize, depth: usize, luminance: bool) -> Canvas {
        let mut c = Canvas::new(w, h);
        let mut mt = *m;
        if let Some(tf) = self.xml.nodes[t].attr("transform") {
            mt = m.mul(&parse_transform(tf));
        }
        let kids = self.xml.nodes[t].kids.clone();
        for k in kids {
            self.render_node(&mut c, k, &Style::root(), &mt, depth + 2);
        }
        if luminance {
            for p in c.px.chunks_exact_mut(4) {
                // premultiplied colour: the sum is luminance times alpha
                let l = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).min(255.0);
                p[3] = l as u8;
            }
        }
        c
    }

    fn render_kids(&mut self, cv: &mut Canvas, n: usize, st: &Style, m: &M, depth: usize) {
        let kids = self.xml.nodes[n].kids.clone();
        for k in kids {
            self.render_node(cv, k, st, m, depth + 1);
        }
    }

    fn render_inner(&mut self, cv: &mut Canvas, n: usize, st: &Style, m: &M, depth: usize) {
        let xml = self.xml;
        let node = &xml.nodes[n];
        match node.tag.as_str() {
            "g" | "a" | "switch" => self.render_kids(cv, n, st, m, depth),
            "text" => self.draw_text(cv, n, st, m),
            "svg" => {
                let mut m2 = *m;
                if depth > 0 {
                    let x = node.attr("x").and_then(|v| length(v, self.vw)).unwrap_or(0.0);
                    let y = node.attr("y").and_then(|v| length(v, self.vh)).unwrap_or(0.0);
                    m2 = m.mul(&M([1.0, 0.0, 0.0, 1.0, x, y]));
                    if let Some(vb) = view_box(node) {
                        let w = node.attr("width").and_then(|v| length(v, self.vw)).filter(|v| *v > 0.0).unwrap_or(self.vw - x);
                        let h = node.attr("height").and_then(|v| length(v, self.vh)).filter(|v| *v > 0.0).unwrap_or(self.vh - y);
                        m2 = m2.mul(&fit_view_box(&vb, w, h, node.attr("preserveAspectRatio").unwrap_or("")));
                    }
                }
                self.render_kids(cv, n, st, &m2, depth);
            }
            "use" => {
                let Some(&t) = node.attr("href").and_then(|h| h.strip_prefix('#')).and_then(|h| self.ids.get(h)) else { return };
                let x = node.attr("x").and_then(|v| length(v, self.vw)).unwrap_or(0.0);
                let y = node.attr("y").and_then(|v| length(v, self.vh)).unwrap_or(0.0);
                let m2 = m.mul(&M([1.0, 0.0, 0.0, 1.0, x, y]));
                if matches!(xml.nodes[t].tag.as_str(), "symbol" | "svg") {
                    let sst = self.cascade(st, t);
                    // the symbol (or svg) is a viewport of its own: the size comes from the `use`, else from the svg itself, else the whole picture;
                    // its viewBox is fitted into it (without a clip to the viewport: what a symbol draws outside it is still drawn)
                    let tn = &xml.nodes[t];
                    let size = |name: &str, refv: f64| -> f64 {
                        node.attr(name).or_else(|| tn.attr(name).filter(|_| tn.tag == "svg")).and_then(|v| length(v, refv)).filter(|v| *v > 0.0).unwrap_or(refv)
                    };
                    let m3 = match view_box(tn) {
                        Some(vb) => m2.mul(&fit_view_box(&vb, size("width", self.vw), size("height", self.vh), tn.attr("preserveAspectRatio").unwrap_or(""))),
                        None => m2,
                    };
                    self.render_kids(cv, t, &sst, &m3, depth);
                } else {
                    self.render_node(cv, t, st, &m2, depth + 1);
                }
            }
            _ => self.draw_shape(cv, node, st, m),
        }
    }

    /// Gathers the pieces of text inside a `text` element, each with its own style and position.
    fn collect_text(&self, n: usize, parent: &Style, pend: &mut TextPos, runs: &mut Vec<TextRun>, depth: usize) {
        if depth > MAX_XML_DEPTH || runs.len() > 2000 {
            return;
        }
        let node = &self.xml.nodes[n];
        let st = self.cascade(parent, n);
        if !st.display {
            return;
        }
        let first = |name: &str| node.attr(name).and_then(|v| numbers(v).first().copied());
        let len = |name: &str| node.attr(name).and_then(|v| v.split([',', ' ']).find(|p| !p.is_empty()).and_then(|p| length(p, self.vw)));
        if let Some(x) = len("x").or(first("x")) {
            pend.x = Some(x);
        }
        if let Some(y) = len("y").or(first("y")) {
            pend.y = Some(y);
        }
        pend.dx += len("dx").unwrap_or(0.0);
        pend.dy += len("dy").unwrap_or(0.0);
        for &k in &node.kids {
            let kn = &self.xml.nodes[k];
            match kn.tag.as_str() {
                "#text" => {
                    let t: String = kn.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if t.is_empty() {
                        continue;
                    }
                    let lead = kn.text.starts_with(char::is_whitespace) && !runs.is_empty() && pend.x.is_none();
                    let trail = kn.text.ends_with(char::is_whitespace);
                    let t = format!("{}{}{}", if lead { " " } else { "" }, t, if trail { " " } else { "" });
                    runs.push(TextRun { text: t, st: st.clone(), pos: std::mem::take(pend), ux: 0.0, uy: 0.0, width: 0.0 });
                }
                "tspan" | "a" => self.collect_text(k, &st, pend, runs, depth + 1),
                _ => {}
            }
        }
    }

    fn draw_text(&mut self, cv: &mut Canvas, n: usize, st: &Style, m: &M) {
        if self.text.is_none() || !st.visible {
            return;
        }
        let mut runs: Vec<TextRun> = Vec::new();
        let mut pend = TextPos::default();
        self.collect_text(n, st, &mut pend, &mut runs, 0);
        if runs.is_empty() {
            return;
        }
        let sc = m.scale_factor();
        self.glyph_runs += runs.len();
        if self.glyph_runs > 5000 {
            return;
        }
        // lay the runs out in user space, one chunk per absolute x
        let (mut px, mut py) = (0.0f64, 0.0f64);
        let mut chunk_start = 0usize;
        let mut total_chars = 0usize;
        for i in 0..runs.len() {
            if runs[i].pos.x.is_some() && i > chunk_start {
                Self::finish_chunk(&mut runs[chunk_start..i]);
                chunk_start = i;
            }
            if let Some(x) = runs[i].pos.x {
                px = x;
            }
            if let Some(y) = runs[i].pos.y {
                py = y;
            }
            px += runs[i].pos.dx;
            py += runs[i].pos.dy;
            total_chars += runs[i].text.chars().count();
            if total_chars > 20_000 {
                runs.truncate(i);
                break;
            }
            let ts = TextStyle { size: (runs[i].st.font_size * sc).clamp(1.0, 600.0), bold: runs[i].st.bold, mono: runs[i].st.mono };
            let w = self.text.as_mut().map_or(0.0, |t| t.measure(&runs[i].text, &ts)) / sc;
            runs[i].ux = px;
            runs[i].uy = py;
            runs[i].width = if w.is_finite() { w } else { 0.0 };
            px += runs[i].width;
        }
        let len = runs.len();
        if chunk_start < len {
            Self::finish_chunk(&mut runs[chunk_start..len]);
        }
        for r in &runs {
            if self.work > MAX_WORK {
                return;
            }
            let ts = TextStyle { size: (r.st.font_size * sc).clamp(1.0, 600.0), bold: r.st.bold, mono: r.st.mono };
            let (ox, oy) = m.apply((r.ux, r.uy));
            if !(ox.is_finite() && oy.is_finite()) || ox.abs() > 1e6 || oy.abs() > 1e6 {
                continue;
            }
            let bbox = (r.ux, r.uy - r.st.font_size, r.ux + r.width, r.uy + r.st.font_size * 0.3);
            let Some(src) = self.make_source(&r.st.fill, &r.st, bbox, m) else { continue };
            let Some(tm) = self.text.as_mut().and_then(|t| t.render(&r.text, &ts)) else { continue };
            if tm.w == 0 || tm.h == 0 || tm.alpha.len() != tm.w as usize * tm.h as usize || tm.w as u64 * tm.h as u64 > 4_000_000 {
                continue;
            }
            self.work += tm.w as u64 * tm.h as u64;
            let (gx, gy) = ((ox + tm.x as f64).round() as i64, (oy + tm.y as f64).round() as i64);
            // clip to the canvas
            let (x0, y0) = (gx.max(0), gy.max(0));
            let (x1, y1) = ((gx + tm.w as i64).min(cv.w as i64), (gy + tm.h as i64).min(cv.h as i64));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let (mw, mh) = ((x1 - x0) as usize, (y1 - y0) as usize);
            let mut data = Vec::with_capacity(mw * mh);
            for y in y0..y1 {
                let row = ((y - gy) as usize) * tm.w as usize + (x0 - gx) as usize;
                data.extend_from_slice(&tm.alpha[row..row + mw]);
            }
            paint_mask(cv, &Mask { x0: x0 as usize, y0: y0 as usize, w: mw, h: mh, data }, &src, r.st.fill_opacity as f32);
        }
    }

    /// Applies `text-anchor` to a chunk of runs that belong together.
    fn finish_chunk(chunk: &mut [TextRun]) {
        let Some(first) = chunk.first() else { return };
        let anchor = first.st.anchor;
        if anchor == 0.0 {
            return;
        }
        let total: f64 = chunk.iter().map(|r| r.width).sum();
        for r in chunk.iter_mut() {
            r.ux -= total * anchor;
        }
    }

    fn draw_shape(&mut self, cv: &mut Canvas, node: &Node, st: &Style, m: &M) {
        if !st.visible {
            return;
        }
        let cmds = shape_path(node, self.vw, self.vh);
        if cmds.is_empty() {
            return;
        }
        let scale = m.scale_factor();
        let subs = flatten(&cmds, scale);
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for s in &subs {
            for &(x, y) in &s.pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        if x0 > x1 {
            return;
        }
        let bbox = (x0, y0, x1, y1);
        let to_dev = |polys: Vec<Vec<P>>| -> Vec<Vec<P>> { polys.into_iter().map(|p| p.into_iter().map(|q| m.apply(q)).collect()).collect() };
        let line_like = node.tag == "line";
        if !line_like {
            if let Some(src) = self.make_source(&st.fill, st, bbox, m) {
                let polys = to_dev(subs.iter().filter(|s| s.pts.len() >= 3).map(|s| s.pts.clone()).collect());
                if let Some(mask) = fill_mask(&polys, st.evenodd, cv.w, cv.h, &mut self.work) {
                    paint_mask(cv, &mask, &src, st.fill_opacity as f32);
                }
            }
        }
        if st.stroke_width > 0.0 {
            if let Some(src) = self.make_source(&st.stroke, st, bbox, m) {
                let dashed;
                let use_subs: &[Sub] = if st.dash.is_empty() {
                    &subs
                } else {
                    match split_dashes(&subs, &st.dash, st.dash_offset) {
                        Some(d) => {
                            dashed = d;
                            &dashed
                        }
                        None => &subs,
                    }
                };
                let polys = to_dev(stroke_polys(use_subs, st.stroke_width, st.cap, st.join, st.miter, scale));
                if let Some(mask) = fill_mask(&polys, false, cv.w, cv.h, &mut self.work) {
                    paint_mask(cv, &mask, &src, st.stroke_opacity as f32);
                }
            }
        }
        if matches!(node.tag.as_str(), "path" | "line" | "polyline" | "polygon") {
            self.draw_markers(cv, node, st, m, &cmds);
        }
    }
}

const FILTER_PRIMS: [&str; 8] = ["feGaussianBlur", "feOffset", "feFlood", "feColorMatrix", "feComposite", "feBlend", "feMerge", "feDropShadow"];

/// The matrix that fits a `viewBox` into a `w` x `h` viewport at its origin, by the `preserveAspectRatio` rules.
fn fit_view_box(vb: &[f64], w: f64, h: f64, aspect: &str) -> M {
    let (ax, ay, slice) = parse_aspect(aspect);
    if ax < 0.0 {
        let (sx, sy) = (w / vb[2], h / vb[3]);
        return M([sx, 0.0, 0.0, sy, -vb[0] * sx, -vb[1] * sy]);
    }
    let (sx, sy) = (w / vb[2], h / vb[3]);
    let s = if slice { sx.max(sy) } else { sx.min(sy) };
    M([s, 0.0, 0.0, s, (w - vb[2] * s) * ax - vb[0] * s, (h - vb[3] * s) * ay - vb[1] * s])
}

struct Vtx {
    p: P,
    din: Option<P>,
    dout: Option<P>,
    angle: f64,
}

/// The vertices of a path with the direction of the line through each one, in degrees.
fn marker_vertices(cmds: &[Cmd]) -> Vec<Vtx> {
    let mut vs: Vec<Vtx> = Vec::new();
    let mut start: P = (0.0, 0.0);
    let mut cur: P = (0.0, 0.0);
    let dir = |a: P, b: P| -> Option<P> { ((b.0 - a.0).abs() + (b.1 - a.1).abs() > 1e-9).then_some((b.0 - a.0, b.1 - a.1)) };
    let seg = |vs: &mut Vec<Vtx>, from: P, d0: Option<P>, d1: Option<P>, to: P| {
        if let Some(l) = vs.last_mut() {
            l.dout = d0;
        }
        vs.push(Vtx { p: to, din: d1, dout: None, angle: 0.0 });
        let _ = from;
    };
    for c in cmds.iter().take(4096) {
        match *c {
            Cmd::M(p) => {
                vs.push(Vtx { p, din: None, dout: None, angle: 0.0 });
                start = p;
                cur = p;
            }
            Cmd::L(p) => {
                let d = dir(cur, p);
                seg(&mut vs, cur, d, d, p);
                cur = p;
            }
            Cmd::C(a, b, p) => {
                let d0 = dir(cur, a).or(dir(cur, b)).or(dir(cur, p));
                let d1 = dir(b, p).or(dir(a, p)).or(dir(cur, p));
                seg(&mut vs, cur, d0, d1, p);
                cur = p;
            }
            Cmd::Z => {
                let d = dir(cur, start);
                seg(&mut vs, cur, d, d, start);
                cur = start;
            }
        }
    }
    for v in vs.iter_mut() {
        let unit = |d: P| {
            let l = (d.0 * d.0 + d.1 * d.1).sqrt();
            (d.0 / l, d.1 / l)
        };
        v.angle = match (v.din, v.dout) {
            (Some(a), Some(b)) => {
                let (a, b) = (unit(a), unit(b));
                let s = (a.0 + b.0, a.1 + b.1);
                if s.0.abs() + s.1.abs() < 1e-9 { a.1.atan2(a.0).to_degrees() } else { s.1.atan2(s.0).to_degrees() }
            }
            (Some(d), None) | (None, Some(d)) => d.1.atan2(d.0).to_degrees(),
            (None, None) => 0.0,
        };
    }
    vs
}

fn px4(p: &[u8]) -> [f32; 4] {
    [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
}

fn put4(d: &mut [u8], v: [f32; 4]) {
    let fin = |x: f32| if x.is_finite() { x } else { 0.0 };
    let a = fin(v[3]).clamp(0.0, 1.0);
    for c in 0..3 {
        d[c] = (fin(v[c]).clamp(0.0, a) * 255.0 + 0.5) as u8;
    }
    d[3] = (a * 255.0 + 0.5) as u8;
}

fn scale4(v: [f32; 4], k: f32) -> [f32; 4] {
    [v[0] * k, v[1] * k, v[2] * k, v[3] * k]
}

fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

/// Combines two premultiplied canvases pixel by pixel.
fn combine(a: &Canvas, b: &Canvas, f: impl Fn([f32; 4], [f32; 4]) -> [f32; 4]) -> Canvas {
    let mut o = Canvas::new(a.w, a.h);
    for ((d, x), y) in o.px.chunks_exact_mut(4).zip(a.px.chunks_exact(4)).zip(b.px.chunks_exact(4)) {
        put4(d, f(px4(x), px4(y)));
    }
    o
}

/// A rectangle of picture pixels from corners that may lie outside the picture or be NaN.
fn region_px(x0: f64, y0: f64, x1: f64, y1: f64, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let c = |v: f64, max: usize| if v.is_finite() { v.round().clamp(0.0, max as f64) as usize } else { 0 };
    let (l, t, r, b) = (c(x0, w), c(y0, h), c(x1, w), c(y1, h));
    (l, t, r.max(l), b.max(t))
}

/// Everything outside the region becomes transparent.
fn clip_to_region(c: &mut Canvas, r: (usize, usize, usize, usize)) {
    let (w, h) = (c.w, c.h);
    for y in 0..h {
        for x in 0..w {
            if x < r.0 || x >= r.2 || y < r.1 || y >= r.3 {
                let o = (y * w + x) * 4;
                c.px[o..o + 4].fill(0);
            }
        }
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Does the filter element ask for sRGB? (`color-interpolation-filters` as an attribute or in `style`; anything else is linearRGB, the default.)
fn filter_is_srgb(xml: &Xml, f: usize) -> bool {
    let n = &xml.nodes[f];
    let from_style = n.attr("style").and_then(|s| s.split(';').filter_map(|d| d.split_once(':')).find(|(k, _)| k.trim() == "color-interpolation-filters").map(|(_, v)| v.trim().to_string()));
    let v = from_style.or_else(|| n.attr("color-interpolation-filters").map(|v| v.trim().to_string()));
    v.is_some_and(|v| v.eq_ignore_ascii_case("sRGB"))
}

/// Premultiplied 8-bit pixels from sRGB to linear light (`to_linear`) or back, colour taken out of the alpha and put back.
fn convert_space(c: &mut Canvas, to_linear: bool) {
    let table: Vec<u8> = (0..256).map(|i| {
        let v = i as f32 / 255.0;
        ((if to_linear { srgb_to_linear(v) } else { linear_to_srgb(v) }) * 255.0 + 0.5) as u8
    }).collect();
    for p in c.px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a == 0 {
            continue;
        }
        for ch in p.iter_mut().take(3) {
            let straight = ((*ch as u32 * 255 + a / 2) / a).min(255) as usize;
            *ch = ((table[straight] as u32 * a + 127) / 255) as u8;
        }
    }
}

fn alpha_only(c: &Canvas) -> Canvas {
    let mut o = Canvas::new(c.w, c.h);
    for (d, s) in o.px.chunks_exact_mut(4).zip(c.px.chunks_exact(4)) {
        d[3] = s[3];
    }
    o
}

fn flood_canvas(w: usize, h: usize, c: Rgba) -> Canvas {
    let mut o = Canvas::new(w, h);
    let v = [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
    for d in o.px.chunks_exact_mut(4) {
        put4(d, v);
    }
    o
}

fn offset_canvas(c: &Canvas, dx: i64, dy: i64) -> Canvas {
    let mut o = Canvas::new(c.w, c.h);
    for y in 0..c.h as i64 {
        let sy = y - dy;
        if sy < 0 || sy >= c.h as i64 {
            continue;
        }
        for x in 0..c.w as i64 {
            let sx = x - dx;
            if sx < 0 || sx >= c.w as i64 {
                continue;
            }
            let (s, d) = ((sy as usize * c.w + sx as usize) * 4, (y as usize * c.w + x as usize) * 4);
            o.px[d..d + 4].copy_from_slice(&c.px[s..s + 4]);
        }
    }
    o
}

fn color_matrix(mut c: Canvas, mx: &[f64; 20]) -> Canvas {
    for p in c.px.chunks_exact_mut(4) {
        let v = px4(p);
        let a = v[3];
        let (r, g, b) = if a > 0.0 { (v[0] / a, v[1] / a, v[2] / a) } else { (0.0, 0.0, 0.0) };
        let row = |i: usize| (mx[i] as f32 * r + mx[i + 1] as f32 * g + mx[i + 2] as f32 * b + mx[i + 3] as f32 * a + mx[i + 4] as f32).clamp(0.0, 1.0);
        let (nr, ng, nb, na) = (row(0), row(5), row(10), row(15));
        put4(p, [nr * na, ng * na, nb * na, na]);
    }
    c
}

// ------------------------------------------------------------------------------------------------ entry points

/// Does this look like an SVG file? (Optional byte-order mark, spaces, then `<svg`, `<?xml`, `<!--` or `<!DOCTYPE svg`, and an
/// `<svg` tag near the start.)
pub fn looks_like_svg(b: &[u8]) -> bool {
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    let head = String::from_utf8_lossy(&b[..b.len().min(4096)]);
    let t = head.trim_start().to_ascii_lowercase();
    (t.starts_with("<svg") || t.starts_with("<?xml") || t.starts_with("<!--") || t.starts_with("<!doctype svg")) && t.contains("<svg")
}

fn parse_aspect(s: &str) -> (f64, f64, bool) {
    // (x alignment 0..1, y alignment 0..1, slice); "none" is reported as x = -1
    let mut it = s.split_whitespace().filter(|p| *p != "defer");
    let align = it.next().unwrap_or("xMidYMid");
    let slice = it.next() == Some("slice");
    if align == "none" {
        return (-1.0, -1.0, false);
    }
    let ax = if align.contains("xMin") { 0.0 } else if align.contains("xMax") { 1.0 } else { 0.5 };
    let ay = if align.contains("YMin") { 0.0 } else if align.contains("YMax") { 1.0 } else { 0.5 };
    (ax, ay, slice)
}

fn view_box(rn: &Node) -> Option<Vec<f64>> {
    rn.attr("viewBox").map(numbers).filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0)
}

/// The document's own size from `width`, `height` and `viewBox`, with the web's 300x150 default.
fn doc_size(rn: &Node, vb: &Option<Vec<f64>>) -> (f64, f64) {
    let wattr = rn.attr("width").filter(|v| !v.trim().ends_with('%')).and_then(|v| length(v, 0.0)).filter(|v| *v > 0.0);
    let hattr = rn.attr("height").filter(|v| !v.trim().ends_with('%')).and_then(|v| length(v, 0.0)).filter(|v| *v > 0.0);
    match (wattr, hattr, vb) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(v)) => (w, w * v[3] / v[2]),
        (None, Some(h), Some(v)) => (h * v[2] / v[3], h),
        (None, None, Some(v)) => (v[2], v[3]),
        (Some(w), None, None) => (w, 150.0),
        (None, Some(h), None) => (300.0, h),
        (None, None, None) => (300.0, 150.0),
    }
}

/// The size an SVG document asks for (before any caps), or why it cannot be read.
pub fn natural_size(bytes: &[u8]) -> Result<(f64, f64), String> {
    if bytes.len() > MAX_INPUT {
        return Err("the SVG is too large".into());
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let xml = parse_xml(&String::from_utf8_lossy(bytes))?;
    let root = xml.nodes[0].kids.iter().copied().find(|&k| xml.nodes[k].tag == "svg").ok_or("this is not an SVG picture")?;
    Ok(doc_size(&xml.nodes[root], &view_box(&xml.nodes[root])))
}

/// Draw an SVG document. With `size` the picture is scaled into that many pixels (keeping the aspect ratio rules of
/// `preserveAspectRatio`); without it the document's own size is used.
pub fn render(bytes: &[u8], size: Option<(u32, u32)>) -> Result<Bitmap, String> {
    render_with(bytes, size, None)
}

/// [`render`] with a way to draw `text` (see [`TextRenderer`]).
pub fn render_with(bytes: &[u8], size: Option<(u32, u32)>, tr: Option<&mut dyn TextRenderer>) -> Result<Bitmap, String> {
    if bytes.len() > MAX_INPUT {
        return Err("the SVG is too large".into());
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = String::from_utf8_lossy(bytes);
    let xml = parse_xml(&text)?;
    let root = xml.nodes[0].kids.iter().copied().find(|&k| xml.nodes[k].tag == "svg").ok_or("this is not an SVG picture")?;
    let rn = &xml.nodes[root];
    let vb = view_box(rn);
    let (mut w, mut h) = doc_size(rn, &vb);
    let (doc_w, doc_h) = (w, h);
    if let Some((sw, sh)) = size {
        w = sw as f64;
        h = sh as f64;
    }
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return Err("the SVG has no size".into());
    }
    let k = (MAX_SIDE / w).min(MAX_SIDE / h).min((MAX_PIXELS / (w * h)).sqrt()).min(1.0);
    let (pw, ph) = ((w * k).round().max(1.0), (h * k).round().max(1.0));
    let (cw, ch) = (pw as usize, ph as usize);
    let base = if let Some(v) = &vb {
        let (ax, ay, slice) = rn.attr("preserveAspectRatio").map_or((0.5, 0.5, false), parse_aspect);
        if ax < 0.0 {
            M([pw / v[2], 0.0, 0.0, ph / v[3], -v[0] * pw / v[2], -v[1] * ph / v[3]])
        } else {
            let (sx, sy) = (pw / v[2], ph / v[3]);
            let s = if slice { sx.max(sy) } else { sx.min(sy) };
            let (tx, ty) = ((pw - v[2] * s) * ax - v[0] * s, (ph - v[3] * s) * ay - v[1] * s);
            M([s, 0.0, 0.0, s, tx, ty])
        }
    } else {
        // no viewBox: the document's own units are scaled into the size asked for
        M([pw / doc_w, 0.0, 0.0, ph / doc_h, 0.0, 0.0])
    };
    let (vw, vh) = vb.as_ref().map_or((doc_w, doc_h), |v| (v[2], v[3]));
    let mut r = Renderer::new(&xml, vw, vh);
    let mut tr = tr;
    r.text = tr.as_mut().map(|t| &mut **t as &mut dyn TextRenderer);
    let mut cv = Canvas::new(cw, ch);
    r.render_node(&mut cv, root, &Style::root(), &base, 0);
    let mut rgba = cv.px;
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a == 0 {
            p.fill(0);
        } else if a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(Bitmap { width: cw as u32, height: ch as u32, rgba })
}

/// Decode an SVG file at its own size.
pub fn decode(bytes: &[u8]) -> Result<Bitmap, String> {
    render(bytes, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * b.width + x) * 4) as usize;
        [b.rgba[o], b.rgba[o + 1], b.rgba[o + 2], b.rgba[o + 3]]
    }

    /// Sum of coverage (alpha / 255) over the picture: the painted area in pixels.
    fn area(b: &Bitmap) -> f64 {
        b.rgba.chunks_exact(4).map(|p| p[3] as f64 / 255.0).sum()
    }

    fn svg(inner: &str) -> Bitmap {
        decode(format!("<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100' viewBox='0 0 100 100'>{inner}</svg>").as_bytes()).unwrap()
    }

    #[test]
    fn a_filled_rectangle_has_the_right_area_and_colour() {
        let b = svg("<rect x='10' y='20' width='30' height='40' fill='#ff0000'/>");
        assert_eq!((b.width, b.height), (100, 100));
        assert_eq!(px(&b, 20, 30), [255, 0, 0, 255]);
        assert_eq!(px(&b, 5, 5)[3], 0);
        assert_eq!(px(&b, 41, 30)[3], 0);
        assert!((area(&b) - 1200.0).abs() < 12.0, "area {}", area(&b));
    }

    #[test]
    fn a_fractional_edge_is_anti_aliased() {
        let b = svg("<rect x='10.5' y='10' width='10' height='10' fill='black'/>");
        let a = px(&b, 10, 15)[3] as i32;
        assert!((a - 128).abs() <= 4, "half covered pixel has alpha {a}");
        assert_eq!(px(&b, 12, 15)[3], 255);
        assert!((area(&b) - 100.0).abs() < 1.0);
    }

    #[test]
    fn circle_and_ellipse_areas_are_within_one_percent() {
        let c = svg("<circle cx='50' cy='50' r='40' fill='blue'/>");
        let want = std::f64::consts::PI * 1600.0;
        assert!((area(&c) - want).abs() / want < 0.01, "circle {} vs {want}", area(&c));
        let e = svg("<ellipse cx='50' cy='50' rx='45' ry='20'/>");
        let want = std::f64::consts::PI * 45.0 * 20.0;
        assert!((area(&e) - want).abs() / want < 0.01, "ellipse {} vs {want}", area(&e));
        assert_eq!(px(&c, 50, 50), [0, 0, 255, 255]);
    }

    #[test]
    fn strokes_have_the_asked_width_and_caps() {
        let b = svg("<line x1='10' y1='50' x2='90' y2='50' stroke='black' stroke-width='10'/>");
        assert!((area(&b) - 800.0).abs() < 8.0, "butt line area {}", area(&b));
        assert_eq!(px(&b, 50, 45)[3], 255);
        assert_eq!(px(&b, 50, 44)[3], 0);
        let sq = svg("<line x1='10' y1='50' x2='90' y2='50' stroke='black' stroke-width='10' stroke-linecap='square'/>");
        assert!((area(&sq) - 900.0).abs() < 9.0, "square caps {}", area(&sq));
        let rd = svg("<line x1='30' y1='50' x2='70' y2='50' stroke='black' stroke-width='20' stroke-linecap='round'/>");
        let want = 40.0 * 20.0 + std::f64::consts::PI * 100.0;
        assert!((area(&rd) - want).abs() / want < 0.01, "round caps {} vs {want}", area(&rd));
    }

    #[test]
    fn a_stroked_square_has_mitered_corners_unless_round_or_bevel() {
        let sq = |j: &str| svg(&format!("<rect x='20' y='20' width='60' height='60' fill='none' stroke='black' stroke-width='10' stroke-linejoin='{j}'/>"));
        let miter = area(&sq("miter"));
        assert!((miter - (70.0 * 70.0 - 50.0 * 50.0)).abs() < 10.0, "miter {miter}");
        let round = area(&sq("round"));
        assert!((round - (miter - 4.0 * 25.0 + 4.0 * std::f64::consts::PI * 25.0 / 4.0)).abs() < 10.0, "round {round}");
        let bevel = area(&sq("bevel"));
        assert!((bevel - (miter - 4.0 * 12.5)).abs() < 10.0, "bevel {bevel}");
    }

    #[test]
    fn path_commands_relative_forms_and_close() {
        let b = svg("<path d='M10 10 h30 v30 h-30 z' fill='lime'/>");
        assert!((area(&b) - 900.0).abs() < 9.0);
        let curve = svg("<path d='M10 50 C10 10 90 10 90 50 S 10 90 10 50 Z'/>");
        assert!(area(&curve) > 1500.0 && area(&curve) < 4500.0);
        let quad = svg("<path d='M0 100 Q50 -100 100 100 Z'/>");
        // area under a parabola-like quadratic: 2/3 of bounding triangle-box -> base 100, height 100: 2/3*100*100
        assert!((area(&quad) - 6666.7).abs() / 6666.7 < 0.01, "quad {}", area(&quad));
        let t = svg("<path d='M0 100 Q50 0 100 100 T200 100 Z'/>");
        assert!(area(&t) > 100.0);
        // arc: half disc of radius 40
        let arc = svg("<path d='M10 50 A40 40 0 0 1 90 50 Z'/>");
        let want = std::f64::consts::PI * 1600.0 / 2.0;
        assert!((area(&arc) - want).abs() / want < 0.01, "arc {} vs {want}", area(&arc));
        assert!(px(&arc, 50, 30)[3] == 255 && px(&arc, 50, 70)[3] == 0, "sweep=1 bulges upwards");
        let arc2 = svg("<path d='M10 50 a40 40 0 0 0 80 0 z'/>");
        assert!(px(&arc2, 50, 70)[3] == 255 && px(&arc2, 50, 30)[3] == 0, "sweep=0 bulges downwards");
        // compact flags
        let flags = svg("<path d='M10 50a40 40 0 0090 0z'/>");
        assert!(area(&flags) > 1000.0);
    }

    #[test]
    fn fill_rules_decide_holes_and_winding() {
        let ring = "M10 10 H90 V90 H10 Z M30 30 H70 V70 H30 Z";
        let eo = svg(&format!("<path d='{ring}' fill-rule='evenodd'/>"));
        assert_eq!(px(&eo, 50, 50)[3], 0, "even-odd leaves a hole");
        assert!((area(&eo) - (6400.0 - 1600.0)).abs() < 20.0);
        let nz = svg(&format!("<path d='{ring}'/>"));
        assert_eq!(px(&nz, 50, 50)[3], 255, "same winding: no hole with non-zero");
        let opposite = svg("<path d='M10 10 H90 V90 H10 Z M30 30 V70 H70 V30 Z'/>");
        assert_eq!(px(&opposite, 50, 50)[3], 0, "opposite winding makes a hole even with non-zero");
    }

    #[test]
    fn shapes_polygons_rounded_rects_and_dashes() {
        let tri = svg("<polygon points='0,0 100,0 0,100'/>");
        assert!((area(&tri) - 5000.0).abs() < 50.0);
        let pl = svg("<polyline points='10,10 90,10 90,90' fill='none' stroke='red' stroke-width='4'/>");
        assert!(area(&pl) > 600.0 && area(&pl) < 700.0, "{}", area(&pl));
        let rr = svg("<rect width='100' height='100' rx='20'/>");
        let want = 10000.0 - (4.0 - std::f64::consts::PI) * 400.0;
        assert!((area(&rr) - want).abs() / want < 0.005, "{} vs {want}", area(&rr));
        assert_eq!(px(&rr, 0, 0)[3], 0);
        let dash = svg("<line x1='0' y1='50' x2='100' y2='50' stroke='black' stroke-width='10' stroke-dasharray='10 10'/>");
        assert!((area(&dash) - 500.0).abs() < 10.0, "dashed {}", area(&dash));
    }

    #[test]
    fn transforms_compose_and_move_things() {
        let t = svg("<g transform='translate(50 50) scale(2) rotate(90)'><rect x='0' y='0' width='10' height='20'/></g>");
        // rotate(90) maps (x,y) -> (-y,x); the 10x20 rect covers x in [-20,0], y in [0,10]; then x2 and +50
        assert_eq!(px(&t, 30, 60)[3], 255);
        assert_eq!(px(&t, 60, 60)[3], 0);
        assert!((area(&t) - 800.0).abs() < 8.0);
        let m = svg("<rect width='10' height='10' transform='matrix(2 0 0 2 10 10)'/>");
        assert!((area(&m) - 400.0).abs() < 4.0);
        assert_eq!(px(&m, 29, 29)[3], 255);
        assert_eq!(px(&m, 31, 31)[3], 0);
    }

    #[test]
    fn view_box_and_preserve_aspect_ratio() {
        let doc = |extra: &str| decode(format!("<svg xmlns='http://www.w3.org/2000/svg' width='200' height='100' viewBox='0 0 10 10' {extra}><rect width='10' height='10'/></svg>").as_bytes()).unwrap();
        let meet = doc("");
        assert_eq!((meet.width, meet.height), (200, 100));
        assert_eq!(px(&meet, 100, 50)[3], 255);
        assert_eq!(px(&meet, 40, 50)[3], 0, "centred square, bars at the sides");
        assert!((area(&meet) - 10000.0).abs() < 50.0);
        let left = doc("preserveAspectRatio='xMinYMid meet'");
        assert_eq!(px(&left, 10, 50)[3], 255);
        assert_eq!(px(&left, 150, 50)[3], 0);
        let none = doc("preserveAspectRatio='none'");
        assert!((area(&none) - 20000.0).abs() < 100.0);
        let slice = doc("preserveAspectRatio='xMidYMid slice'");
        assert!((area(&slice) - 20000.0).abs() < 100.0);
        let only_vb = decode(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 12'/>").unwrap();
        assert_eq!((only_vb.width, only_vb.height), (24, 12));
        let only_w = decode(b"<svg width='40' viewBox='0 0 20 10'/>").unwrap();
        assert_eq!((only_w.width, only_w.height), (40, 20));
        let sized = render(b"<svg viewBox='0 0 10 10'><rect width='10' height='10'/></svg>", Some((64, 64))).unwrap();
        assert!((area(&sized) - 4096.0).abs() < 20.0);
    }

    #[test]
    fn colours_opacity_and_current_color() {
        let c = |s: &str| px(&svg(&format!("<rect width='100' height='100' fill='{s}'/>")), 50, 50);
        assert_eq!(c("#f80"), [255, 136, 0, 255]);
        assert_eq!(c("#0000ff"), [0, 0, 255, 255]);
        assert_eq!(c("rgb(10, 20, 30)"), [10, 20, 30, 255]);
        assert_eq!(c("rgb(100%, 0%, 0%)"), [255, 0, 0, 255]);
        assert_eq!(c("Navy"), [0, 0, 128, 255]);
        assert_eq!(c("hsl(120, 100%, 50%)"), [0, 255, 0, 255]);
        assert_eq!(c("#ff000080")[3], 128);
        assert_eq!(c("none")[3], 0);
        assert_eq!(c("nonsense"), [0, 0, 0, 255], "an unknown colour keeps the inherited fill");
        let cc = px(&svg("<g color='#00ff00'><rect width='100' height='100' fill='currentColor'/></g>"), 5, 5);
        assert_eq!(cc, [0, 255, 0, 255]);
        let fo = px(&svg("<rect width='100' height='100' fill='#fff' fill-opacity='0.5'/>"), 5, 5);
        assert_eq!(fo[3], 128);
        assert_eq!(&fo[..3], &[255, 255, 255], "straight colour survives the alpha");
        let op = px(&svg("<rect width='100' height='100' fill='black' opacity='0.25'/>"), 5, 5);
        assert!((op[3] as i32 - 64).abs() <= 1);
    }

    #[test]
    fn group_opacity_does_not_double_darken_overlaps() {
        let b = svg("<g opacity='0.5'><rect width='60' height='100'/><rect x='40' width='60' height='100'/></g>");
        let a = px(&b, 50, 50)[3] as i32;
        let single = px(&b, 10, 50)[3] as i32;
        assert!((a - single).abs() <= 1, "overlap {a} vs single {single}");
        assert!((single - 128).abs() <= 1);
    }

    #[test]
    fn linear_gradient_endpoints_and_middle() {
        let b = svg("<defs><linearGradient id='g'><stop offset='0' stop-color='#ff0000'/><stop offset='1' stop-color='#0000ff'/></linearGradient></defs><rect width='100' height='100' fill='url(#g)'/>");
        let l = px(&b, 0, 50);
        let r = px(&b, 99, 50);
        let m = px(&b, 50, 50);
        assert!(l[0] >= 250 && l[2] <= 6, "{l:?}");
        assert!(r[2] >= 250 && r[0] <= 6, "{r:?}");
        assert!((m[0] as i32 - 127).abs() <= 6 && (m[2] as i32 - 127).abs() <= 6, "{m:?}");
        // user space units, vertical, with an inherited stop list and a stop offset in percent
        let v = svg("<linearGradient id='base'><stop offset='0%' stop-color='white'/><stop offset='100%' stop-color='black'/></linearGradient><linearGradient id='g' href='#base' gradientUnits='userSpaceOnUse' x1='0' y1='0' x2='0' y2='100'/><rect width='100' height='100' fill='url(#g)'/>");
        assert!(px(&v, 50, 0)[0] >= 250 && px(&v, 50, 99)[0] <= 5);
        assert!((px(&v, 50, 50)[0] as i32 - 127).abs() <= 6);
        let rep = svg("<linearGradient id='g' x2='0.5' spreadMethod='repeat'><stop offset='0' stop-color='black'/><stop offset='1' stop-color='white'/></linearGradient><rect width='100' height='100' fill='url(#g)'/>");
        assert!(px(&rep, 1, 5)[0] < 10 && px(&rep, 48, 5)[0] > 240 && px(&rep, 52, 5)[0] < 30, "repeats at x=50");
    }

    #[test]
    fn radial_gradient_goes_from_centre_to_edge() {
        let b = svg("<radialGradient id='g'><stop offset='0' stop-color='white'/><stop offset='1' stop-color='black'/></radialGradient><rect width='100' height='100' fill='url(#g)'/>");
        assert!(px(&b, 50, 50)[0] >= 250);
        assert!(px(&b, 99, 50)[0] <= 8);
        assert!((px(&b, 75, 50)[0] as i32 - 127).abs() <= 8);
        let missing = svg("<rect width='100' height='100' fill='url(#nope) red'/>");
        assert_eq!(px(&missing, 5, 5), [255, 0, 0, 255], "fallback colour");
    }

    #[test]
    fn use_style_attribute_and_class_rules() {
        let u = svg("<defs><rect id='r' width='20' height='20' fill='red'/></defs><use href='#r' x='10' y='10'/><use xlink:href='#r' x='60' y='60' fill='blue'/>");
        assert_eq!(px(&u, 15, 15), [255, 0, 0, 255]);
        assert_eq!(px(&u, 5, 5)[3], 0, "defs draw nothing by themselves");
        assert_eq!(px(&u, 65, 65), [255, 0, 0, 255], "the referenced element keeps its own fill");
        let s = svg("<rect width='50' height='50' style='fill: #00f; stroke:none'/>");
        assert_eq!(px(&s, 5, 5), [0, 0, 255, 255]);
        let css = svg("<style>.a{fill:#f00} rect.b, #c{fill:#0f0} /* x */ .a.d{fill:#00f}</style><rect class='a' width='10' height='10'/><rect class='b' x='20' width='10' height='10'/><rect id='c' x='40' width='10' height='10'/><rect class='a d' x='60' width='10' height='10'/><rect class='a' x='80' width='10' height='10' fill='yellow'/>");
        assert_eq!(px(&css, 5, 5), [255, 0, 0, 255]);
        assert_eq!(px(&css, 25, 5), [0, 255, 0, 255]);
        assert_eq!(px(&css, 45, 5), [0, 255, 0, 255]);
        assert_eq!(px(&css, 65, 5), [0, 0, 255, 255]);
        assert_eq!(px(&css, 85, 5), [255, 0, 0, 255], "a rule beats a presentation attribute");
        let sym = svg("<symbol id='s'><rect width='10' height='10'/></symbol><use href='#s' x='30' y='30'/>");
        assert_eq!(px(&sym, 35, 35)[3], 255);
    }

    #[test]
    fn text_without_a_font_and_unknown_elements_draw_nothing_and_display_none_hides() {
        let b = svg("<text x='10' y='50'>Hello</text><foreignObject/><rect width='10' height='10' display='none'/><g visibility='hidden'><rect width='10' height='10'/></g>");
        assert_eq!(area(&b), 0.0);
    }

    #[test]
    fn xml_prolog_comments_entities_and_cdata_are_handled() {
        let src = "\u{feff}<?xml version='1.0'?>\n<!-- hi -->\n<!DOCTYPE svg PUBLIC '-//W3C//DTD SVG 1.1//EN' 'x.dtd' [ <!ENTITY a 'b'> ]>\n<svg:svg xmlns:svg='http://www.w3.org/2000/svg' width='10' height='10'><svg:style><![CDATA[ .k > x { } rect{fill:#123456} ]]></svg:style><svg:rect width='10' height='10' id=\"a&amp;b\"/></svg:svg>";
        assert!(looks_like_svg(src.as_bytes()));
        let b = decode(src.as_bytes()).unwrap();
        assert_eq!(px(&b, 3, 3), [0x12, 0x34, 0x56, 255]);
        assert!(!looks_like_svg(b"<!DOCTYPE html><html><body><svg></svg></body></html>"));
        assert!(!looks_like_svg(b"hello <svg"));
        assert!(decode(b"<html></html>").is_err());
        assert!(decode(b"").is_err());
    }

    #[test]
    fn sizes_are_capped() {
        let big = decode(b"<svg width='100000' height='50000'><rect width='5' height='5'/></svg>").unwrap();
        assert!(big.width <= 4096 && big.height <= 4096 && (big.width as u64 * big.height as u64) <= 16_000_000, "{}x{}", big.width, big.height);
        assert_eq!(big.width as usize * big.height as usize * 4, big.rgba.len());
        assert!(render(b"<svg viewBox='0 0 10 10'/>", Some((4000, 4000))).map(|b| b.width as u64 * b.height as u64).unwrap() <= 16_000_000);
        assert!(decode(b"<svg width='0' height='0' viewBox='0 0 0 0'/>").is_ok());
    }

    #[test]
    fn hostile_input_never_panics_or_runs_away() {
        let t0 = std::time::Instant::now();
        // use loops and billion-laughs style expansion
        let mut s = String::from("<svg width='50' height='50'><defs><g id='a0'><rect width='5' height='5'/></g>");
        for i in 1..30 {
            s.push_str(&format!("<g id='a{i}'><use href='#a{}'/><use href='#a{}'/></g>", i - 1, i - 1));
        }
        s.push_str("<g id='loop'><use href='#loop'/></g></defs><use href='#a29'/><use href='#loop'/></svg>");
        decode(s.as_bytes()).unwrap();
        // deep nesting, many elements, giant numbers, nan-ish text
        let deep = format!("<svg>{}{}</svg>", "<g>".repeat(5000), "</g>".repeat(5000));
        assert!(decode(deep.as_bytes()).is_err());
        let many = format!("<svg>{}</svg>", "<rect width='1' height='1'/>".repeat(30_000));
        assert!(decode(many.as_bytes()).is_err());
        for hostile in [
            "<svg width='50' height='50'><path d='M1e308 1e308 L-1e308 5 C1e400 1 2 3 4 5 A1e308 1e308 0 1 1 5 5 Z' stroke='red' stroke-width='1e300'/></svg>",
            "<svg width='50' height='50'><rect width='1e9' height='1e9' stroke='black' stroke-dasharray='0.0001'/></svg>",
            "<svg width='50' height='50'><circle r='1e300' stroke-width='1e300' stroke='blue' stroke-dasharray='1e-9 1e-9'/></svg>",
            "<svg width='50' height='50'><g transform='scale(1e200) scale(1e200)'><rect width='5' height='5'/></g></svg>",
            "<svg width='50' height='50'><g transform='matrix(0 0 0 0 0 0)'><rect width='5' height='5' fill='url(#g)'/></g><linearGradient id='g'/></svg>",
            "<svg width='50' height='50'><path d='M 0 0 L 10'/><path d='M'/><path d='A'/><path d='M0 0 A 1 1 0 2 2 5 5'/><polyline points='1,2,3'/></svg>",
            "<svg width='50' height='50'><rect width='10' height='10' fill='url(' stroke='rgb(' style='fill:;;:;a:b:c'/></svg>",
            "<svg width='nan' height='inf'><rect/></svg>",
            "<svg><linearGradient id='a' href='#b'/><linearGradient id='b' href='#a'/><rect width='9' height='9' fill='url(#a)'/></svg>",
            "<svg><style>{{{{ }} .a{ ;;; } @media x { .b { fill: red } }</style><rect class='a' width='9' height='9'/></svg>",
            "<svg width='50' height='50' viewBox='0 0 1e-300 1e-300'><rect width='9' height='9'/></svg>",
            "<svg width='50' height='50' viewBox='0 0 1e300 1e300'><rect width='9' height='9'/></svg>",
            "<svg <<<>>> =\"=\" width='10",
            "<svg><![CDATA[ <!-- <?",
        ] {
            let _ = decode(hostile.as_bytes());
        }
        // random byte soup in the shape of markup
        let mut seed = 12345u64;
        let alphabet = b"<>/=\"' abcdsvgrectpathM0123456789.-e#()%;:,";
        for _ in 0..300 {
            let mut v = b"<svg width='20' height='20'>".to_vec();
            for _ in 0..200 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                v.push(alphabet[(seed >> 33) as usize % alphabet.len()]);
            }
            let _ = decode(&v);
        }
        assert!(t0.elapsed().as_secs() < 60, "took {:?}", t0.elapsed());
    }

    /// A stand-in font: every character is a solid box 0.6 em wide and 0.7 em tall, standing on the baseline.
    struct Boxes {
        drawn: Vec<(String, f64)>,
    }

    impl TextRenderer for Boxes {
        fn measure(&mut self, text: &str, style: &TextStyle) -> f64 {
            text.chars().count() as f64 * style.size * 0.6
        }
        fn render(&mut self, text: &str, style: &TextStyle) -> Option<TextMask> {
            self.drawn.push((text.to_string(), style.size));
            let w = self.measure(text, style).ceil() as u32;
            let h = (style.size * 0.7).ceil() as u32;
            Some(TextMask { advance: w as f64, x: 0, y: -(h as i32), w, h, alpha: vec![255; (w * h) as usize] })
        }
    }

    fn draw_text(inner: &str) -> (Bitmap, Vec<(String, f64)>) {
        let mut t = Boxes { drawn: Vec::new() };
        let src = format!("<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100' viewBox='0 0 100 100'>{inner}</svg>");
        let b = render_with(src.as_bytes(), None, Some(&mut t)).unwrap();
        (b, t.drawn)
    }

    #[test]
    fn text_is_drawn_through_the_hook_with_position_size_and_colour() {
        let (b, drawn) = draw_text("<text x='10' y='50' font-size='20' fill='#ff0000'>Hi</text>");
        assert_eq!(drawn, vec![("Hi".to_string(), 20.0)]);
        // two boxes 12 wide, 14 tall, standing on y = 50
        assert_eq!(px(&b, 15, 45), [255, 0, 0, 255]);
        assert_eq!(px(&b, 31, 45), [255, 0, 0, 255]);
        assert_eq!(px(&b, 40, 45)[3], 0, "past the end");
        assert_eq!(px(&b, 15, 30)[3], 0, "above the box");
        assert_eq!(px(&b, 15, 52)[3], 0, "below the baseline");
        // without a text renderer nothing is drawn
        assert_eq!(area(&svg("<text x='10' y='50'>Hello</text>")), 0.0);
    }

    #[test]
    fn text_anchor_tspan_dx_and_style_inheritance() {
        let (b, drawn) = draw_text("<g fill='blue' font-size='10'><text x='50' y='40' text-anchor='middle'>abcd<tspan fill='lime' dy='20'>ef</tspan></text></g>");
        assert_eq!(drawn.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), vec!["abcd", "ef"]);
        // 6 chars * 6 px = 36 wide, centred on 50: starts at 32; "abcd" covers 32..56, "ef" 56..68 and 20 lower
        assert_eq!(px(&b, 34, 37), [0, 0, 255, 255]);
        assert_eq!(px(&b, 60, 57), [0, 255, 0, 255]);
        assert_eq!(px(&b, 25, 37)[3], 0);
        let (b2, _) = draw_text("<text x='90' y='40' text-anchor='end' font-size='10'>abcd</text>");
        assert_eq!(px(&b2, 70, 37)[3], 255);
        assert_eq!(px(&b2, 60, 37)[3], 0);
        let (b3, d3) = draw_text("<text y='40' dx='5' font-size='10' font-weight='bold' font-family='Courier'>a b</text>");
        assert_eq!(d3[0].0, "a b");
        assert_eq!(px(&b3, 7, 37)[3], 255);
    }

    #[test]
    fn text_follows_translate_and_scale_and_opacity() {
        let (b, drawn) = draw_text("<g transform='translate(20 10) scale(2)'><text x='5' y='20' font-size='10' opacity='0.5'>a</text></g>");
        assert_eq!(drawn[0].1, 20.0, "scaled font size");
        // the box: x 30..42, y 30..58 baseline at 50, 14 tall
        let p = px(&b, 35, 45);
        assert!(p[3] > 120 && p[3] < 135, "{p:?}");
        assert_eq!(px(&b, 35, 20)[3], 0);
    }

    #[test]
    fn hostile_text_and_use_expansion_stay_capped() {
        let t0 = std::time::Instant::now();
        let mut inner = String::from("<defs><g id='a0'><text x='1' y='9'>boom</text><rect width='5' height='5'/></g>");
        for i in 1..12 {
            inner.push_str(&format!("<g id='a{i}'>{}</g>", format!("<use href='#a{}'/>", i - 1).repeat(10)));
        }
        inner.push_str("</defs><use href='#a11'/>");
        let (b, drawn) = draw_text(&inner);
        assert!(drawn.len() <= 5000, "{}", drawn.len());
        assert_eq!((b.width, b.height), (100, 100));
        // a huge size and a huge font are held to the caps
        let mut t = Boxes { drawn: Vec::new() };
        let big = render_with(b"<svg width='100000' height='100000'><text x='1' y='99' font-size='1e9'>x</text></svg>", None, Some(&mut t)).unwrap();
        assert!(big.width <= 4096 && big.height <= 4096);
        assert!(t.drawn.iter().all(|d| d.1 <= 600.0));
        let long = format!("<text x='1' y='9'>{}</text>", "word ".repeat(100_000));
        let _ = draw_text(&long);
        assert!(t0.elapsed().as_secs() < 30, "{:?}", t0.elapsed());
        assert_eq!(natural_size(b"<svg viewBox='0 0 40 30'/>").unwrap(), (40.0, 30.0));
        assert_eq!(natural_size(b"<svg/>").unwrap(), (300.0, 150.0));
        assert!(natural_size(b"<html/>").is_err());
    }

    #[test]
    fn a_requested_size_scales_a_drawing_that_has_no_view_box() {
        let src = b"<svg xmlns='http://www.w3.org/2000/svg' width='10' height='5'><rect width='10' height='5' fill='red'/></svg>";
        let b = render(src, Some((40, 20))).unwrap();
        assert_eq!((b.width, b.height), (40, 20));
        assert_eq!(px(&b, 39, 19), [255, 0, 0, 255], "the rectangle grew with the picture");
        assert_eq!(px(&b, 1, 1), [255, 0, 0, 255]);
    }
}

#[cfg(test)]
mod clip_tests {
    use super::*;

    fn draw(inner: &str) -> Bitmap {
        render(format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>{inner}</svg>").as_bytes(), Some((100, 100))).unwrap()
    }
    fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * b.width + x) * 4) as usize;
        [b.rgba[o], b.rgba[o + 1], b.rgba[o + 2], b.rgba[o + 3]]
    }

    #[test]
    fn a_clip_path_keeps_only_what_is_inside_it() {
        let b = draw("<defs><clipPath id='c'><circle cx='50' cy='50' r='20'/></clipPath></defs><rect width='100' height='100' fill='#ff0000' clip-path='url(#c)'/>");
        assert_eq!(px(&b, 50, 50), [255, 0, 0, 255]);
        assert_eq!(px(&b, 5, 5)[3], 0, "outside the circle nothing is drawn");
        assert_eq!(px(&b, 95, 50)[3], 0);
        // a group clipped as a whole, and the quoted form of the reference
        let b = draw("<defs><clipPath id='c'><rect x='0' y='0' width='50' height='100'/></clipPath></defs><g clip-path=\"url('#c')\"><rect width='100' height='100' fill='#00ff00'/><circle cx='80' cy='50' r='15' fill='#0000ff'/></g>");
        assert_eq!(px(&b, 20, 50), [0, 255, 0, 255]);
        assert_eq!(px(&b, 80, 50)[3], 0, "the right half is clipped away");
    }

    #[test]
    fn a_blur_filter_softens_edges_and_keeps_the_total() {
        let sharp = draw("<rect x='40' y='40' width='20' height='20' fill='#ffffff'/>");
        let soft = draw("<defs><filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'><feGaussianBlur stdDeviation='4'/></filter></defs><rect x='40' y='40' width='20' height='20' fill='#ffffff' filter='url(#f)'/>");
        assert_eq!(px(&sharp, 35, 50)[3], 0);
        assert!(px(&soft, 35, 50)[3] > 10, "the glow spreads outside the square: {:?}", px(&soft, 35, 50));
        assert!(px(&soft, 50, 50)[3] > 200);
        let sum = |b: &Bitmap| b.rgba.chunks(4).map(|p| p[3] as u64).sum::<u64>() as f64;
        assert!((sum(&soft) / sum(&sharp) - 1.0).abs() < 0.05, "blur keeps the amount of paint");
    }

    #[test]
    fn a_luminance_mask_fades_a_picture() {
        let b = draw("<defs><linearGradient id='g' x1='0' y1='0' x2='1' y2='0'><stop offset='0' stop-color='#000'/><stop offset='1' stop-color='#fff'/></linearGradient><mask id='m'><rect width='100' height='100' fill='url(#g)'/></mask></defs><rect width='100' height='100' fill='#ff0000' mask='url(#m)'/>");
        assert!(px(&b, 5, 50)[3] < 40 && px(&b, 95, 50)[3] > 215, "{:?} {:?}", px(&b, 5, 50), px(&b, 95, 50));
    }
}

#[cfg(test)]
mod more_svg_tests {
    use super::*;

    fn draw(inner: &str) -> Bitmap {
        render(format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'>{inner}</svg>").as_bytes(), Some((100, 100))).unwrap()
    }
    fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * b.width + x) * 4) as usize;
        [b.rgba[o], b.rgba[o + 1], b.rgba[o + 2], b.rgba[o + 3]]
    }
    fn filtered(prims: &str) -> Bitmap {
        draw(&format!("<defs><filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100'>{prims}</filter></defs><rect x='10' y='10' width='20' height='20' fill='#ff0000' filter='url(#f)'/>"))
    }

    #[test]
    fn an_offset_filter_moves_the_picture() {
        let b = filtered("<feOffset dx='40' dy='10'/>");
        assert_eq!(px(&b, 20, 20)[3], 0, "the old place is empty");
        assert_eq!(px(&b, 60, 30), [255, 0, 0, 255]);
    }

    #[test]
    fn a_flood_and_a_composite_in_cut_the_colour_to_the_shape() {
        let b = filtered("<feFlood flood-color='#00ff00'/><feComposite in2='SourceGraphic' operator='in'/>");
        assert_eq!(px(&b, 20, 20), [0, 255, 0, 255]);
        assert_eq!(px(&b, 70, 70)[3], 0, "outside the shape the flood is cut away");
    }

    #[test]
    fn a_colour_matrix_changes_colours_and_alpha() {
        // swap red into blue
        let b = filtered("<feColorMatrix type='matrix' values='0 0 0 0 0  0 1 0 0 0  1 0 0 0 0  0 0 0 1 0'/>");
        assert_eq!(px(&b, 20, 20), [0, 0, 255, 255]);
        // saturate 0 turns red grey: in sRGB the grey is the luminance of the sRGB numbers, in linear light (the default) it is the luminance
        // of the light and then looks brighter, as every other renderer shows it (a real Chrome gives 127 here)
        let srgb = |prims: &str| draw(&format!("<defs><filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100' color-interpolation-filters='sRGB'>{prims}</filter></defs><rect x='10' y='10' width='20' height='20' fill='#ff0000' filter='url(#f)'/>"));
        let p = px(&srgb("<feColorMatrix type='saturate' values='0'/>"), 20, 20);
        assert!(p[0] == p[1] && p[1] == p[2] && p[0] > 40 && p[0] < 80, "sRGB: {p:?}");
        let g = filtered("<feColorMatrix type='saturate' values='0'/>");
        let p = px(&g, 20, 20);
        assert!(p[0] == p[1] && p[1] == p[2] && p[0] > 120 && p[0] < 135, "linearRGB, the default: {p:?}");
        // the choice can also come in a style
        let st = draw("<defs><filter id='f' filterUnits='userSpaceOnUse' x='0' y='0' width='100' height='100' style='color-interpolation-filters: sRGB'><feColorMatrix type='saturate' values='0'/></filter></defs><rect x='10' y='10' width='20' height='20' fill='#ff0000' filter='url(#f)'/>");
        assert!(px(&st, 20, 20)[0] < 80);
        // a flood keeps its colour through the conversions, and so does a picture that no primitive changes
        let fl = filtered("<feFlood flood-color='#336699'/><feComposite in2='SourceGraphic' operator='in'/>");
        let p = px(&fl, 20, 20);
        assert!((p[0] as i32 - 0x33).abs() <= 1 && (p[1] as i32 - 0x66).abs() <= 1 && (p[2] as i32 - 0x99).abs() <= 1, "{p:?}");
        let same = filtered("<feOffset dx='0' dy='0'/>");
        assert_eq!(px(&same, 20, 20), [255, 0, 0, 255]);
        // hue rotate by 120 degrees takes red towards green
        let h = filtered("<feColorMatrix type='hueRotate' values='120'/>");
        let p = px(&h, 20, 20);
        assert!(p[1] > p[0] && p[1] > p[2], "{p:?}");
    }

    #[test]
    fn the_default_filter_region_is_the_box_with_a_tenth_more_on_every_side() {
        // a flood fills only the region: -10 % to 110 % of the 20 x 20 box at 10,10 is 8 to 32 (a real Chrome gives the same)
        let d = |attrs: &str| draw(&format!("<defs><filter id='f' {attrs}><feFlood flood-color='#00ff00'/></filter></defs><rect x='10' y='10' width='20' height='20' fill='#ff0000' filter='url(#f)'/>"));
        let b = d("");
        assert_eq!(px(&b, 20, 20), [0, 255, 0, 255]);
        assert_eq!(px(&b, 9, 20), [0, 255, 0, 255], "inside the extra tenth");
        assert_eq!(px(&b, 5, 20)[3], 0, "outside the region nothing is drawn");
        assert_eq!(px(&b, 60, 60)[3], 0);
        // the region can be given as fractions of the box ...
        let b = d("x='0' y='0' width='1' height='1'");
        assert_eq!(px(&b, 9, 20)[3], 0);
        assert_eq!(px(&b, 25, 25), [0, 255, 0, 255]);
        // ... or in the picture's own units
        let b = d("filterUnits='userSpaceOnUse' x='50' y='50' width='30' height='30'");
        assert_eq!(px(&b, 60, 60), [0, 255, 0, 255]);
        assert_eq!(px(&b, 20, 20)[3], 0);
        // a drawing that painted nothing has no box: nothing is shown
        let none = draw("<defs><filter id='f'><feFlood flood-color='#00ff00'/></filter></defs><rect x='10' y='10' width='0' height='20' fill='#ff0000' filter='url(#f)'/>");
        assert!(none.rgba.chunks(4).all(|p| p[3] == 0));
    }

    #[test]
    fn blend_merge_and_named_results_chain() {
        let b = filtered("<feFlood flood-color='#0000ff' result='blue'/><feBlend in='SourceGraphic' in2='blue' mode='multiply'/>");
        assert_eq!(px(&b, 20, 20), [0, 0, 0, 255], "red times blue is black");
        let m = filtered("<feOffset in='SourceAlpha' dx='5' dy='5' result='o'/><feFlood flood-color='#000000' flood-opacity='1' result='k'/><feComposite in='k' in2='o' operator='in' result='shadow'/><feMerge><feMergeNode in='shadow'/><feMergeNode in='SourceGraphic'/></feMerge>");
        assert_eq!(px(&m, 20, 20), [255, 0, 0, 255], "the picture is on top");
        assert_eq!(px(&m, 33, 33), [0, 0, 0, 255], "the shadow peeks out behind it");
    }

    #[test]
    fn a_drop_shadow_goes_behind_and_blur_still_works() {
        let b = filtered("<feDropShadow dx='10' dy='10' stdDeviation='0' flood-color='#0000ff'/>");
        assert_eq!(px(&b, 15, 15), [255, 0, 0, 255]);
        assert_eq!(px(&b, 35, 35), [0, 0, 255, 255]);
        let soft = filtered("<feGaussianBlur stdDeviation='4'/>");
        assert!(px(&soft, 8, 20)[3] > 5);
    }

    #[test]
    fn an_unknown_filter_input_falls_back_and_hostile_filters_do_not_run_away() {
        let b = filtered("<feOffset in='nothing' dx='0' dy='0'/>");
        assert_eq!(px(&b, 20, 20), [255, 0, 0, 255]);
        let many: String = (0..200).map(|i| format!("<feOffset dx='1' result='r{i}'/>")).collect();
        let b = filtered(&many);
        assert!(b.width == 100);
        let b = filtered("<feColorMatrix values='nan inf'/><feComposite operator='arithmetic' k1='1e308' k2='1e308'/>");
        assert!(b.width == 100);
    }

    #[test]
    fn a_pattern_repeats_its_tile_in_user_space_and_in_bounding_box_units() {
        let user = draw("<defs><pattern id='p' width='20' height='20' patternUnits='userSpaceOnUse'><rect width='10' height='20' fill='#ff0000'/></pattern></defs><rect width='100' height='100' fill='url(#p)'/>");
        assert_eq!(px(&user, 5, 50), [255, 0, 0, 255]);
        assert_eq!(px(&user, 15, 50)[3], 0, "the gap in the tile stays empty");
        assert_eq!(px(&user, 25, 50), [255, 0, 0, 255], "the tile repeats");
        assert_eq!(px(&user, 85, 50), [255, 0, 0, 255]);
        let bb = draw("<defs><pattern id='p' width='0.5' height='0.5'><rect width='5' height='5' fill='#00ff00'/><rect x='5' y='5' width='5' height='5' fill='#0000ff'/></pattern></defs><rect x='0' y='0' width='100' height='100' fill='url(#p)'/>");
        assert_eq!(px(&bb, 2, 2), [0, 255, 0, 255]);
        assert_eq!(px(&bb, 8, 8), [0, 0, 255, 255]);
        assert_eq!(px(&bb, 52, 2), [0, 255, 0, 255], "a second tile starts at half the box");
        // a stroked shape and a viewBox in the pattern
        let vb = draw("<defs><pattern id='p' width='20' height='20' patternUnits='userSpaceOnUse' viewBox='0 0 2 2'><rect width='1' height='2' fill='#ff0000'/></pattern></defs><circle cx='50' cy='50' r='40' fill='url(#p)'/>");
        assert_eq!(px(&vb, 45, 50), [255, 0, 0, 255]);
        assert_eq!(px(&vb, 55, 50)[3], 0);
    }

    #[test]
    fn a_pattern_that_uses_itself_or_has_no_size_does_not_loop() {
        let b = draw("<defs><pattern id='p' width='10' height='10' patternUnits='userSpaceOnUse'><rect width='10' height='10' fill='url(#p)'/></pattern><pattern id='z' width='0' height='0'><rect width='1' height='1'/></pattern></defs><rect width='50' height='50' fill='url(#p)'/><rect x='50' width='50' height='50' fill='url(#z)'/>");
        assert_eq!(b.width, 100);
    }

    #[test]
    fn markers_sit_on_the_vertices_and_turn_with_the_line() {
        let defs = "<defs><marker id='m' markerWidth='10' markerHeight='10' refX='5' refY='5' orient='auto' markerUnits='userSpaceOnUse'><rect x='0' y='3' width='10' height='4' fill='#0000ff'/></marker></defs>";
        // a horizontal line: the marker at the end is not turned
        let b = draw(&format!("{defs}<path d='M10 50 L90 50' stroke='#000' stroke-width='1' marker-end='url(#m)'/>"));
        assert_eq!(px(&b, 92, 50), [0, 0, 255, 255]);
        assert_eq!(px(&b, 90, 55)[3], 0, "the marker is flat, so nothing above and below");
        // a vertical line turns it a quarter
        let v = draw(&format!("{defs}<path d='M50 10 L50 90' stroke='#000' marker-end='url(#m)'/>"));
        assert_eq!(px(&v, 50, 92), [0, 0, 255, 255]);
        assert_eq!(px(&v, 55, 90)[3], 0);
        let w = draw(&format!("{defs}<path d='M50 10 L50 90' stroke='none' marker-end='url(#m)'/>"));
        assert_eq!(px(&w, 51, 92)[3], 255, "the marker is turned: it is tall, not wide");
        assert_eq!(px(&w, 54, 90)[3], 0);
    }

    #[test]
    fn marker_start_mid_end_and_stroke_width_units() {
        let defs = "<defs><marker id='a' markerWidth='2' markerHeight='2' refX='1' refY='1'><rect width='2' height='2' fill='#ff0000'/></marker><marker id='b' markerWidth='2' markerHeight='2' refX='1' refY='1'><rect width='2' height='2' fill='#00ff00'/></marker><marker id='c' markerWidth='2' markerHeight='2' refX='1' refY='1'><rect width='2' height='2' fill='#0000ff'/></marker></defs>";
        let b = draw(&format!("{defs}<polyline points='20,50 50,50 80,50' fill='none' stroke='#fff' stroke-width='10' marker-start='url(#a)' marker-mid='url(#b)' marker-end='url(#c)'/>"));
        // stroke width 10 scales the 2-unit marker to 20 units
        assert_eq!(px(&b, 22, 40), [255, 0, 0, 255]);
        assert_eq!(px(&b, 50, 42), [0, 255, 0, 255]);
        assert_eq!(px(&b, 78, 58), [0, 0, 255, 255]);
        // the shorthand `marker` and no marker element
        let s = draw(&format!("{defs}<path d='M20 50 L80 50' stroke='#fff' stroke-width='10' marker='url(#a)'/>"));
        assert_eq!(px(&s, 22, 45), [255, 0, 0, 255]);
        let none = draw("<path d='M20 50 L80 50' stroke='#fff' marker-end='url(#missing)'/>");
        assert_eq!(none.width, 100);
    }

    #[test]
    fn a_marker_that_draws_itself_stops() {
        let b = draw("<defs><marker id='m' markerWidth='10' markerHeight='10'><path d='M0 0 L5 5' stroke='#000' marker-end='url(#m)'/></marker></defs><path d='M10 10 L90 90' stroke='#000' marker-start='url(#m)' marker-end='url(#m)'/>");
        assert_eq!(b.width, 100);
    }

    #[test]
    fn a_nested_svg_scales_its_view_box_into_its_own_box() {
        let b = draw("<svg x='50' y='0' width='40' height='40' viewBox='0 0 10 10'><rect width='5' height='5' fill='#ff0000'/></svg>");
        assert_eq!(px(&b, 60, 10), [255, 0, 0, 255], "5 view-box units become 20 pixels");
        assert_eq!(px(&b, 75, 10)[3], 0);
        assert_eq!(px(&b, 55, 25)[3], 0);
        // meet keeps the aspect ratio and centres
        let m = draw("<svg x='0' y='0' width='80' height='40' viewBox='0 0 10 10'><rect width='10' height='10' fill='#00ff00'/></svg>");
        assert_eq!(px(&m, 40, 20), [0, 255, 0, 255]);
        assert_eq!(px(&m, 5, 20)[3], 0, "bars either side");
        let n = draw("<svg width='80' height='40' viewBox='0 0 10 10' preserveAspectRatio='none'><rect width='10' height='10' fill='#00ff00'/></svg>");
        assert_eq!(px(&n, 5, 20), [0, 255, 0, 255], "none stretches");
    }
}

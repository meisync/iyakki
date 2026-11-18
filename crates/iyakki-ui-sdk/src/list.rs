//! The display list: what a frame is when it leaves the app for a browser, a client or a test. [`ListCanvas`] records it from `Canvas`
//! calls; [`DisplayList::replay`] draws it on any other `Canvas`; [`DisplayList::to_text`] / [`DisplayList::parse`] are the wire format
//! (plain text, one operation per line, versioned and additive: a list newer than [`DISPLAY_LIST_VERSION`] is refused by name, and an
//! operation this version does not know is an error naming the line, never silently skipped).

use crate::canvas::{Canvas, Hit, Image, ImageFormat, Role, TextStyle};
use crate::geom::{Color, Rect};
use crate::typeface::{Family, TextOpts};

/// The wire version of the display list. Additive changes only; a bump needs a fixture folder (CORE-COMPATIBILITY-DESIGN.md).
/// 2 (alpha.5) adds `textx` (family, letter spacing, tabular figures of one run) and `blur`; a list that uses neither is still written as
/// version 1, so older readers keep reading what they could read.
pub const DISPLAY_LIST_VERSION: u32 = 2;
const MAGIC: &str = "iyakki-display-list";
/// Limits of a list a reader accepts.
pub const MAX_OPS: usize = 100_000;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Rect(Rect, Color),
    RoundRect(Rect, f32, Color),
    Outline(Rect, f32, f32, Color),
    Line([f32; 4], f32, Color),
    Gradient(Rect, Color, Color),
    Shadow(Rect, f32, f32, Color),
    Text { x: f32, y: f32, size: f32, color: Color, style: TextStyle, text: String },
    /// Version 2: a run set with the person's family, letter spacing and tabular figures.
    TextEx { x: f32, y: f32, size: f32, color: Color, style: TextStyle, text: String, opts: TextOpts },
    /// Version 2: blur what is drawn so far inside the rectangle.
    Blur(Rect, f32),
    PushClip(Rect),
    PopClip,
    Hit(Rect, Hit),
    /// A picture (F-SDK-164): `image X Y W H FORMAT PIXELS_W PIXELS_H SEQ BASE64`. Additive in version 1.
    Image(Rect, Image),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayList {
    pub width: f32,
    pub height: f32,
    pub ops: Vec<Op>,
}

/// One entry of the accessibility tree: a named, roled region.
#[derive(Clone, Debug, PartialEq)]
pub struct A11yNode {
    pub name: String,
    pub role: Role,
    pub rect: Rect,
}

impl DisplayList {
    /// The topmost clickable region at a point (the last one drawn wins).
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&Hit> {
        let mut clips: Vec<Rect> = Vec::new();
        let mut found = None;
        for op in &self.ops {
            match op {
                Op::PushClip(r) => clips.push(*r),
                Op::PopClip => {
                    clips.pop();
                }
                Op::Hit(r, h) if r.contains(x, y) && clips.iter().all(|c| c.contains(x, y)) => found = Some(h),
                _ => {}
            }
        }
        found
    }

    /// The accessibility tree, in drawing order.
    pub fn a11y(&self) -> Vec<A11yNode> {
        self.ops.iter().filter_map(|o| if let Op::Hit(r, h) = o { Some(A11yNode { name: h.name.clone(), role: h.role, rect: *r }) } else { None }).collect()
    }

    /// Every text drawn, in order (tests and the screen reader's fallback).
    pub fn texts(&self) -> Vec<&str> {
        self.ops.iter().filter_map(|o| match o { Op::Text { text, .. } | Op::TextEx { text, .. } => Some(text.as_str()), _ => None }).collect()
    }

    /// The same list moved by (`dx`, `dy`) with every hit region's action raised by `action_base`; a hit whose action is `action_cap` or
    /// more is dropped (the drawing stays). An app that embeds a list made by someone else (the core's browser frame) uses it to put the
    /// picture inside its own layout without its actions clashing with its own.
    pub fn translated(&self, dx: f32, dy: f32, action_base: u32, action_cap: u32) -> DisplayList {
        let mv = |r: &Rect| Rect::new(r.x + dx, r.y + dy, r.w, r.h);
        let ops = self
            .ops
            .iter()
            .filter_map(|o| {
                Some(match o {
                    Op::Rect(r, c) => Op::Rect(mv(r), *c),
                    Op::RoundRect(r, rad, c) => Op::RoundRect(mv(r), *rad, *c),
                    Op::Outline(r, rad, w, c) => Op::Outline(mv(r), *rad, *w, *c),
                    Op::Line(p, w, c) => Op::Line([p[0] + dx, p[1] + dy, p[2] + dx, p[3] + dy], *w, *c),
                    Op::Gradient(r, a, b) => Op::Gradient(mv(r), *a, *b),
                    Op::Shadow(r, rad, sp, c) => Op::Shadow(mv(r), *rad, *sp, *c),
                    Op::Text { x, y, size, color, style, text } => Op::Text { x: x + dx, y: y + dy, size: *size, color: *color, style: *style, text: text.clone() },
                    Op::TextEx { x, y, size, color, style, text, opts } => Op::TextEx { x: x + dx, y: y + dy, size: *size, color: *color, style: *style, text: text.clone(), opts: *opts },
                    Op::Blur(r, rad) => Op::Blur(mv(r), *rad),
                    Op::PushClip(r) => Op::PushClip(mv(r)),
                    Op::PopClip => Op::PopClip,
                    Op::Hit(r, h) if h.action < action_cap => Op::Hit(mv(r), Hit { action: h.action + action_base, name: h.name.clone(), role: h.role }),
                    Op::Hit(..) => return None,
                    Op::Image(r, i) => Op::Image(mv(r), i.clone()),
                })
            })
            .collect();
        DisplayList { width: self.width, height: self.height, ops }
    }

    /// Draw this list on another canvas (the pixel canvas of the device, or a test canvas).
    pub fn replay(&self, c: &mut dyn Canvas) {
        replay_ops(&self.ops, c);
    }

    pub fn to_text(&self) -> String {
        let v2 = self.ops.iter().any(|o| matches!(o, Op::TextEx { .. } | Op::Blur(..)));
        let mut s = format!("{MAGIC} {} {} {}\n", if v2 { DISPLAY_LIST_VERSION } else { 1 }, self.width, self.height);
        for op in &self.ops {
            match op {
                Op::Rect(r, c) => s += &format!("rect {} {} {} {} {}\n", r.x, r.y, r.w, r.h, hex(*c)),
                Op::RoundRect(r, rad, c) => s += &format!("rrect {} {} {} {} {} {}\n", r.x, r.y, r.w, r.h, rad, hex(*c)),
                Op::Outline(r, rad, w, c) => s += &format!("outline {} {} {} {} {} {} {}\n", r.x, r.y, r.w, r.h, rad, w, hex(*c)),
                Op::Line(p, w, c) => s += &format!("line {} {} {} {} {} {}\n", p[0], p[1], p[2], p[3], w, hex(*c)),
                Op::Gradient(r, a, b) => s += &format!("grad {} {} {} {} {} {}\n", r.x, r.y, r.w, r.h, hex(*a), hex(*b)),
                Op::Shadow(r, rad, sp, c) => s += &format!("shadow {} {} {} {} {} {} {}\n", r.x, r.y, r.w, r.h, rad, sp, hex(*c)),
                Op::Text { x, y, size, color, style, text } => s += &format!("text {x} {y} {size} {} {} {}\n", hex(*color), style_key(*style), esc(text)),
                Op::TextEx { x, y, size, color, style, text, opts } => {
                    s += &format!("textx {x} {y} {size} {} {} {} {} {} {}\n", hex(*color), style_key(*style), opts.family.key(), opts.tracking, u8::from(opts.tabular), esc(text))
                }
                Op::Blur(r, rad) => s += &format!("blur {} {} {} {} {}\n", r.x, r.y, r.w, r.h, rad),
                Op::PushClip(r) => s += &format!("clip {} {} {} {}\n", r.x, r.y, r.w, r.h),
                Op::PopClip => s += "endclip\n",
                Op::Hit(r, h) => s += &format!("hit {} {} {} {} {} {} {}\n", r.x, r.y, r.w, r.h, h.action, h.role.key(), esc(&h.name)),
                Op::Image(r, i) => s += &format!("image {} {} {} {} {} {} {} {} {}\n", r.x, r.y, r.w, r.h, i.format.key(), i.width, i.height, i.seq, b64_encode(&i.data)),
            }
        }
        s
    }

    pub fn parse(text: &str) -> Result<DisplayList, String> {
        if text.len() > MAX_BYTES {
            return Err("the display list is too big".into());
        }
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().ok_or("empty display list")?.split(' ').collect();
        if head.len() != 4 || head[0] != MAGIC {
            return Err("not a display list".into());
        }
        let version: u32 = head[1].parse().map_err(|_| "bad version".to_string())?;
        if version > DISPLAY_LIST_VERSION {
            return Err(format!("display list version {version} is newer than {DISPLAY_LIST_VERSION}"));
        }
        let (width, height) = (num(head[2], 1)?, num(head[3], 1)?);
        let mut ops = Vec::new();
        let mut depth = 0usize;
        for (i, line) in lines.enumerate() {
            let n = i + 2;
            if ops.len() >= MAX_OPS {
                return Err(format!("line {n}: too many operations"));
            }
            if line.is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split(' ').collect();
            let g = |k: usize| -> Result<f32, String> { num(f.get(k).copied().unwrap_or(""), n) };
            let r = |k: usize| -> Result<Rect, String> { Ok(Rect::new(g(k)?, g(k + 1)?, g(k + 2)?, g(k + 3)?)) };
            let col = |k: usize| -> Result<Color, String> { unhex(f.get(k).copied().unwrap_or(""), n) };
            let op = match (f[0], f.len()) {
                ("rect", 6) => Op::Rect(r(1)?, col(5)?),
                ("rrect", 7) => Op::RoundRect(r(1)?, g(5)?, col(6)?),
                ("outline", 8) => Op::Outline(r(1)?, g(5)?, g(6)?, col(7)?),
                ("line", 7) => Op::Line([g(1)?, g(2)?, g(3)?, g(4)?], g(5)?, col(6)?),
                ("grad", 7) => Op::Gradient(r(1)?, col(5)?, col(6)?),
                ("shadow", 8) => Op::Shadow(r(1)?, g(5)?, g(6)?, col(7)?),
                ("text", 7) => Op::Text { x: g(1)?, y: g(2)?, size: g(3)?, color: col(4)?, style: style_from(f[5]).ok_or(format!("line {n}: unknown text style"))?, text: unesc(f[6], n)? },
                ("textx", 10) => Op::TextEx {
                    x: g(1)?,
                    y: g(2)?,
                    size: g(3)?,
                    color: col(4)?,
                    style: style_from(f[5]).ok_or(format!("line {n}: unknown text style"))?,
                    opts: TextOpts {
                        family: Family::from_key(f[6]).ok_or(format!("line {n}: unknown font family"))?,
                        tracking: g(7)?,
                        tabular: match f[8] {
                            "0" => false,
                            "1" => true,
                            _ => return Err(format!("line {n}: bad tabular flag")),
                        },
                    }
                    .clamped(),
                    text: unesc(f[9], n)?,
                },
                ("blur", 6) => Op::Blur(r(1)?, g(5)?),
                ("clip", 5) => {
                    depth += 1;
                    Op::PushClip(r(1)?)
                }
                ("endclip", 1) => {
                    depth = depth.checked_sub(1).ok_or(format!("line {n}: endclip without clip"))?;
                    Op::PopClip
                }
                ("image", 10) => {
                    let bad = |m: &str| format!("line {n}: {m}");
                    let format = ImageFormat::from_key(f[5]).ok_or_else(|| bad("unknown image format"))?;
                    let int = |k: usize| f[k].parse::<u32>().map_err(|_| bad("bad number"));
                    if f[9].len() > crate::canvas::MAX_IMAGE_BYTES / 3 * 4 + 8 {
                        return Err(bad("the image is too big"));
                    }
                    let data = b64_decode(f[9]).ok_or_else(|| bad("bad image data"))?;
                    let img = Image { format, width: int(6)?, height: int(7)?, seq: f[8].parse::<u64>().map_err(|_| bad("bad number"))?, data: std::sync::Arc::new(data) };
                    img.check().map_err(|e| bad(&e))?;
                    Op::Image(r(1)?, img)
                }
                ("hit", 8) => Op::Hit(r(1)?, Hit { action: f[5].parse().map_err(|_| format!("line {n}: bad action"))?, role: Role::from_key(f[6]).ok_or(format!("line {n}: unknown role"))?, name: unesc(f[7], n)? }),
                (other, _) => return Err(format!("line {n}: unknown or malformed operation '{other}'")),
            };
            ops.push(op);
        }
        if depth != 0 {
            return Err("a clip is never closed".into());
        }
        Ok(DisplayList { width, height, ops })
    }
}

fn replay_ops(ops: &[Op], c: &mut dyn Canvas) {
    let mut i = 0;
    while i < ops.len() {
        match &ops[i] {
            Op::Rect(r, col) => c.rect(*r, *col),
            Op::RoundRect(r, rad, col) => c.rrect(*r, *rad, *col),
            Op::Outline(r, rad, w, col) => c.outline(*r, *rad, *w, *col),
            Op::Line(p, w, col) => c.line(p[0], p[1], p[2], p[3], *w, *col),
            Op::Gradient(r, a, b) => c.gradient(*r, *a, *b),
            Op::Shadow(r, rad, sp, col) => c.shadow(*r, *rad, *sp, *col),
            Op::Text { x, y, size, color, style, text } => {
                c.text(*x, *y, *size, *color, text, *style);
            }
            Op::TextEx { x, y, size, color, style, text, opts } => {
                let prev = c.type_opts();
                c.set_type(*opts);
                c.text(*x, *y, *size, *color, text, *style);
                c.set_type(prev);
            }
            Op::Blur(r, rad) => c.blur(*r, *rad),
            Op::Hit(r, h) => c.hit(*r, h.clone()),
            Op::Image(r, img) => c.image(*r, img),
            Op::PushClip(r) => {
                // the matching PopClip: nested clips count up and down
                let (mut depth, mut j) = (1, i + 1);
                while j < ops.len() {
                    match ops[j] {
                        Op::PushClip(_) => depth += 1,
                        Op::PopClip => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let inner = &ops[i + 1..j.min(ops.len())];
                c.clipped(*r, &mut |cc| replay_ops(inner, cc));
                i = j;
            }
            Op::PopClip => {}
        }
        i += 1;
    }
}

/// Records the calls as a [`DisplayList`]. Text is measured by the function given (the device's real font through
/// [`ListCanvas::with_measure`], or a fixed approximation by default so a list can be made anywhere).
pub struct ListCanvas {
    w: f32,
    h: f32,
    ops: Vec<Op>,
    opts: TextOpts,
    measure: Box<dyn FnMut(f32, &str, TextStyle) -> f32>,
}

impl ListCanvas {
    pub fn new(w: f32, h: f32) -> ListCanvas {
        ListCanvas::with_measure(w, h, Box::new(approx_measure))
    }

    pub fn with_measure(w: f32, h: f32, measure: Box<dyn FnMut(f32, &str, TextStyle) -> f32>) -> ListCanvas {
        ListCanvas { w, h, ops: Vec::new(), opts: TextOpts::default(), measure }
    }

    pub fn finish(self) -> DisplayList {
        DisplayList { width: self.w, height: self.h, ops: self.ops }
    }
}

/// A fixed-width estimate of text width: the same everywhere, so a list made on any machine lays out the same.
pub fn approx_measure(size: f32, s: &str, style: TextStyle) -> f32 {
    let k = match style {
        TextStyle::Regular => 0.55,
        TextStyle::Bold => 0.58,
        TextStyle::Mono => 0.6,
    };
    s.chars().count() as f32 * size * k
}

impl Canvas for ListCanvas {
    fn size(&self) -> (f32, f32) {
        (self.w, self.h)
    }
    fn rect(&mut self, r: Rect, c: Color) {
        self.ops.push(Op::Rect(r, c));
    }
    fn rrect(&mut self, r: Rect, radius: f32, c: Color) {
        self.ops.push(Op::RoundRect(r, radius, c));
    }
    fn outline(&mut self, r: Rect, radius: f32, width: f32, c: Color) {
        self.ops.push(Op::Outline(r, radius, width, c));
    }
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        self.ops.push(Op::Line([x0, y0, x1, y1], width, c));
    }
    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        self.ops.push(Op::Gradient(r, top, bottom));
    }
    fn shadow(&mut self, r: Rect, radius: f32, spread: f32, c: Color) {
        self.ops.push(Op::Shadow(r, radius, spread, c));
    }
    fn text(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle) -> f32 {
        let adv = self.measure(size, s, style);
        if self.opts.is_default() {
            self.ops.push(Op::Text { x, y, size, color: c, style, text: s.to_string() });
        } else {
            self.ops.push(Op::TextEx { x, y, size, color: c, style, text: s.to_string(), opts: self.opts });
        }
        adv
    }
    fn measure(&mut self, size: f32, s: &str, style: TextStyle) -> f32 {
        (self.measure)(size, s, style) + tracking_extra(self.opts.tracking, size, s)
    }
    fn set_type(&mut self, o: TextOpts) {
        self.opts = o.clamped();
    }
    fn type_opts(&self) -> TextOpts {
        self.opts
    }
    fn blur(&mut self, r: Rect, radius: f32) {
        if radius > 0.0 {
            self.ops.push(Op::Blur(r, radius));
        }
    }
    fn clipped(&mut self, r: Rect, f: &mut dyn FnMut(&mut dyn Canvas)) {
        self.ops.push(Op::PushClip(r));
        f(self);
        self.ops.push(Op::PopClip);
    }
    fn hit(&mut self, r: Rect, hit: Hit) {
        self.ops.push(Op::Hit(r, hit));
    }
    fn image(&mut self, r: Rect, img: &Image) {
        // an image the reader would refuse is not recorded: a list this canvas makes always parses
        if img.check().is_ok() {
            self.ops.push(Op::Image(r, img.clone()));
        }
    }
}

/// The width letter spacing adds to a run: after every letter of a script that is not connected (a browser does the same).
pub fn tracking_extra(tracking: f32, size: f32, s: &str) -> f32 {
    if tracking <= 0.0 {
        return 0.0;
    }
    s.chars().filter(|c| !matches!(crate::typeface::script_of(*c), crate::typeface::Script::Arabic)).count() as f32 * tracking * size
}

fn hex(c: Color) -> String {
    format!("{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
}

fn unhex(s: &str, line: usize) -> Result<Color, String> {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("line {line}: bad colour '{s}'"));
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| format!("line {line}: bad colour"));
    Ok([b(0)?, b(2)?, b(4)?, b(6)?])
}

fn num(s: &str, line: usize) -> Result<f32, String> {
    match s.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("line {line}: bad number '{s}'")),
    }
}

fn style_key(s: TextStyle) -> &'static str {
    match s {
        TextStyle::Regular => "regular",
        TextStyle::Bold => "bold",
        TextStyle::Mono => "mono",
    }
}

fn style_from(k: &str) -> Option<TextStyle> {
    [TextStyle::Regular, TextStyle::Bold, TextStyle::Mono].into_iter().find(|s| style_key(*s) == k)
}

pub fn esc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
            o.push(b as char);
        } else {
            o += &format!("%{b:02X}");
        }
    }
    if o.is_empty() {
        o.push('~'); // an empty string (a literal ~ is always written %7E)
    }
    o
}

pub(crate) fn unesc(s: &str, line: usize) -> Result<String, String> {
    if s == "~" {
        return Ok(String::new());
    }
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = s.get(i + 1..i + 3).ok_or(format!("line {line}: bad escape"))?;
            out.push(u8::from_str_radix(h, 16).map_err(|_| format!("line {line}: bad escape"))?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("line {line}: text is not UTF-8"))
}

/// The version of the `image` operation inside display list version 1 (listed in the compat table; additive).
pub const IMAGE_OP_VERSION: u32 = 1;

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard Base64 with padding (the image operation's data).
pub fn b64_encode(b: &[u8]) -> String {
    let mut o = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(B64[(n >> 18) as usize & 63] as char);
        o.push(B64[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    o
}

/// The bytes of standard padded Base64; `None` for anything else.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| B64.iter().position(|x| *x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, q) in b.chunks(4).enumerate() {
        let last = i == b.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (k, c) in q.iter().enumerate() {
            n = n << 6 | if k >= 4 - pad { 0 } else { val(*c)? };
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

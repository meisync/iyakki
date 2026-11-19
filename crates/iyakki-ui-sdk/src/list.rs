//! The display list: what a frame is when it leaves the app for a browser, a client or a test. [`ListCanvas`] records it from `Canvas`
//! calls; [`DisplayList::replay`] draws it on any other `Canvas`; [`DisplayList::to_text`] / [`DisplayList::parse`] are the wire format
//! (plain text, one operation per line, versioned and additive: a list newer than [`DISPLAY_LIST_VERSION`] is refused by name, and an
//! operation this version does not know is an error naming the line, never silently skipped).

use crate::canvas::{Canvas, Hit, Image, ImageFormat, Role, TextStyle};
use crate::geom::{Color, Rect};
use crate::path::{self, Seg};
use crate::typeface::{Family, TextOpts};

/// The wire version of the display list. Additive changes only; a bump needs a fixture folder (CORE-COMPATIBILITY-DESIGN.md).
/// 2 (alpha.5) adds `textx` (family, letter spacing, tabular figures of one run) and `blur`; a list that uses neither is still written as
/// version 1, so older readers keep reading what they could read.
/// 3 (alpha.7) adds `path` (fill and stroke of straight and curved segments), `layer`/`endlayer` (opacity of a group) and `textc` (a run
/// with the advance of every cluster); a list that uses none of them is written as version 2 or 1 as before.
pub const DISPLAY_LIST_VERSION: u32 = 3;
const MAGIC: &str = "iyakki-display-list";
/// Limits of a list a reader accepts.
pub const MAX_OPS: usize = 100_000;
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// The deepest nesting of clips and layers together.
pub const MAX_NEST: usize = 64;

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
    /// Version 3: a shape of straight and curved segments, filled (non-zero winding) and/or stroked.
    Path { fill: Option<Color>, stroke: Option<(Color, f32)>, segs: Vec<Seg> },
    /// Version 3: everything up to the matching `PopLayer` is drawn at this opacity (0 to 1).
    PushLayer(f32),
    PopLayer,
    /// Version 3: a run with the advance of each cluster (a letter with its marks), so a renderer without the font places the clusters
    /// where the device's text shaper did. `advances` has one entry per cluster of `text`.
    TextC { x: f32, y: f32, size: f32, color: Color, style: TextStyle, text: String, opts: TextOpts, advances: Vec<f32> },
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
        self.ops.iter().filter_map(|o| match o { Op::Text { text, .. } | Op::TextEx { text, .. } | Op::TextC { text, .. } => Some(text.as_str()), _ => None }).collect()
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
                    Op::Path { fill, stroke, segs } => Op::Path { fill: *fill, stroke: *stroke, segs: segs.iter().map(|g| move_seg(g, dx, dy)).collect() },
                    Op::PushLayer(a) => Op::PushLayer(*a),
                    Op::PopLayer => Op::PopLayer,
                    Op::TextC { x, y, size, color, style, text, opts, advances } => {
                        Op::TextC { x: x + dx, y: y + dy, size: *size, color: *color, style: *style, text: text.clone(), opts: *opts, advances: advances.clone() }
                    }
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
        let v3 = self.ops.iter().any(|o| matches!(o, Op::Path { .. } | Op::PushLayer(_) | Op::PopLayer | Op::TextC { .. }));
        let v2 = self.ops.iter().any(|o| matches!(o, Op::TextEx { .. } | Op::Blur(..)));
        let mut s = format!("{MAGIC} {} {} {}\n", if v3 { 3 } else if v2 { 2 } else { 1 }, self.width, self.height);
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
                Op::Path { fill, stroke, segs } => {
                    let f = fill.map_or("-".to_string(), hex);
                    let (sc, sw) = stroke.map_or(("-".to_string(), 0.0), |(c, w)| (hex(c), w));
                    s += &format!("path {f} {sc} {sw} {}\n", path::encode(segs))
                }
                Op::PushLayer(a) => s += &format!("layer {a}\n"),
                Op::PopLayer => s += "endlayer\n",
                Op::TextC { x, y, size, color, style, text, opts, advances } => {
                    let adv = if advances.is_empty() { "-".to_string() } else { advances.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",") };
                    s += &format!("textc {x} {y} {size} {} {} {} {} {} {adv} {}\n", hex(*color), style_key(*style), opts.family.key(), opts.tracking, u8::from(opts.tabular), esc(text));
                }
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
        let mut stack: Vec<bool> = Vec::new(); // true: a clip, false: a layer
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
                    stack.push(true);
                    if stack.len() > MAX_NEST {
                        return Err(format!("line {n}: groups are nested too deep"));
                    }
                    Op::PushClip(r(1)?)
                }
                ("endclip", 1) => {
                    if stack.pop() != Some(true) {
                        return Err(format!("line {n}: endclip without clip"));
                    }
                    Op::PopClip
                }
                ("layer", 2) => {
                    stack.push(false);
                    if stack.len() > MAX_NEST {
                        return Err(format!("line {n}: groups are nested too deep"));
                    }
                    Op::PushLayer(g(1)?.clamp(0.0, 1.0))
                }
                ("endlayer", 1) => {
                    if stack.pop() != Some(false) {
                        return Err(format!("line {n}: endlayer without layer"));
                    }
                    Op::PopLayer
                }
                ("path", 5) => {
                    let side = |k: usize| -> Result<Option<Color>, String> { if f[k] == "-" { Ok(None) } else { unhex(f[k], n).map(Some) } };
                    let (fill, sc, sw) = (side(1)?, side(2)?, g(3)?);
                    if fill.is_none() && sc.is_none() {
                        return Err(format!("line {n}: a path needs a fill or a stroke"));
                    }
                    Op::Path { fill, stroke: sc.map(|c| (c, sw.clamp(0.0, 256.0))), segs: path::decode(f[4]).map_err(|e| format!("line {n}: {e}"))? }
                }
                ("textc", 11) => {
                    let text = unesc(f[10], n)?;
                    let advances: Vec<f32> = if f[9] == "-" { Vec::new() } else { f[9].split(',').map(|a| num(a, n).map(|v| v.clamp(0.0, 4096.0))).collect::<Result<_, _>>()? };
                    if advances.len() != path::clusters(&text).len() {
                        return Err(format!("line {n}: a cluster run needs one advance for every cluster"));
                    }
                    Op::TextC {
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
                        text,
                        advances,
                    }
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
        if !stack.is_empty() {
            return Err("a clip or a layer is never closed".into());
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
            Op::Path { fill, stroke, segs } => c.path(*fill, *stroke, segs),
            Op::TextC { x, y, size, color, style, text, opts, advances } => {
                let prev = c.type_opts();
                c.set_type(*opts);
                c.text_clusters(*x, *y, *size, *color, text, *style, advances);
                c.set_type(prev);
            }
            Op::Hit(r, h) => c.hit(*r, h.clone()),
            Op::Image(r, img) => c.image(*r, img),
            Op::PushClip(_) | Op::PushLayer(_) => {
                // the matching pop: nested groups count up and down
                let (mut depth, mut j) = (1, i + 1);
                while j < ops.len() {
                    match ops[j] {
                        Op::PushClip(_) | Op::PushLayer(_) => depth += 1,
                        Op::PopClip | Op::PopLayer => {
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
                match &ops[i] {
                    Op::PushClip(r) => c.clipped(*r, &mut |cc| replay_ops(inner, cc)),
                    Op::PushLayer(a) => c.layer(*a, &mut |cc| replay_ops(inner, cc)),
                    _ => {}
                }
                i = j;
            }
            Op::PopClip | Op::PopLayer => {}
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
        if s.chars().count() > 1 && path::clusters(s).iter().any(|r| s[r.clone()].chars().count() > 1) && !s.is_empty() {
            // a run with marks, joined emoji or flags: the advance of every cluster travels with it
            let mut m = |t: &str| (self.measure)(size, t, style) + tracking_extra(self.opts.tracking, size, t);
            let advances = path::cluster_advances(s, &mut m);
            self.ops.push(Op::TextC { x, y, size, color: c, style, text: s.to_string(), opts: self.opts, advances });
            return adv;
        }
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
    fn path(&mut self, fill: Option<Color>, stroke: Option<(Color, f32)>, segs: &[Seg]) {
        // a path the reader would refuse is not recorded: a list this canvas makes always parses
        if (fill.is_some() || stroke.is_some()) && !segs.is_empty() && segs.len() <= path::MAX_SEGS && matches!(segs[0], Seg::Move(..)) && segs.iter().all(seg_finite) {
            self.ops.push(Op::Path { fill, stroke: stroke.map(|(c, w)| (c, w.clamp(0.0, 256.0))), segs: segs.to_vec() });
        }
    }
    fn layer(&mut self, alpha: f32, f: &mut dyn FnMut(&mut dyn Canvas)) {
        self.ops.push(Op::PushLayer(alpha.clamp(0.0, 1.0)));
        f(self);
        self.ops.push(Op::PopLayer);
    }
    fn text_clusters(&mut self, x: f32, y: f32, size: f32, c: Color, s: &str, style: TextStyle, advances: &[f32]) -> f32 {
        let adv = self.measure(size, s, style);
        if advances.len() == path::clusters(s).len() && !advances.is_empty() {
            self.ops.push(Op::TextC { x, y, size, color: c, style, text: s.to_string(), opts: self.opts, advances: advances.iter().map(|a| a.clamp(0.0, 4096.0)).collect() });
            adv
        } else {
            self.text(x, y, size, c, s, style)
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

fn seg_finite(g: &Seg) -> bool {
    match *g {
        Seg::Move(a, b) | Seg::Line(a, b) => a.is_finite() && b.is_finite(),
        Seg::Quad(a, b, c, d) => [a, b, c, d].iter().all(|v| v.is_finite()),
        Seg::Cubic(a, b, c, d, e, f) => [a, b, c, d, e, f].iter().all(|v| v.is_finite()),
        Seg::Close => true,
    }
}

fn move_seg(g: &Seg, dx: f32, dy: f32) -> Seg {
    match *g {
        Seg::Move(a, b) => Seg::Move(a + dx, b + dy),
        Seg::Line(a, b) => Seg::Line(a + dx, b + dy),
        Seg::Quad(a, b, c, d) => Seg::Quad(a + dx, b + dy, c + dx, d + dy),
        Seg::Cubic(a, b, c, d, e, f) => Seg::Cubic(a + dx, b + dy, c + dx, d + dy, e + dx, f + dy),
        Seg::Close => Seg::Close,
    }
}

/// A text measure a client can have without a font file: the advance of each letter, as the page's own fonts drew it at a reference size,
/// scaled to the size asked (text widths grow in a straight line with the size). A letter the table does not know is measured like
/// [`approx_measure`]; a measure from a table is what makes a client (a browser tile, the engine) wrap and fit text as the device does.
#[derive(Clone, Debug, Default)]
pub struct AdvanceTable {
    /// `(style, letter)` to the advance at one unit of size.
    per_unit: std::collections::BTreeMap<(u8, char), f32>,
}

impl AdvanceTable {
    pub fn new() -> AdvanceTable {
        AdvanceTable::default()
    }

    fn code(s: TextStyle) -> u8 {
        match s {
            TextStyle::Regular => 0,
            TextStyle::Bold => 1,
            TextStyle::Mono => 2,
        }
    }

    /// Record that `c` is `advance` wide when set at `size`. A size or advance that is not a positive finite number is ignored.
    pub fn put(&mut self, style: TextStyle, c: char, size: f32, advance: f32) {
        if size.is_finite() && size > 0.0 && advance.is_finite() && advance >= 0.0 {
            self.per_unit.insert((Self::code(style), c), advance / size);
        }
    }

    pub fn len(&self) -> usize {
        self.per_unit.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_unit.is_empty()
    }

    /// The width of `s` (a letter the table does not have counts as the fixed estimate).
    pub fn width(&self, size: f32, s: &str, style: TextStyle) -> f32 {
        s.chars()
            .map(|c| match self.per_unit.get(&(Self::code(style), c)) {
                Some(u) => u * size,
                None => approx_measure(size, "x", style),
            })
            .sum()
    }

    /// The measure to hand to [`ListCanvas::with_measure`].
    pub fn into_measure(self) -> Box<dyn FnMut(f32, &str, TextStyle) -> f32> {
        Box::new(move |size, s, style| self.width(size, s, style))
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

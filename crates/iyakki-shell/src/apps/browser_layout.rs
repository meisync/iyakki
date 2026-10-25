//! Laying a page out: words in lines, headings, lists, code blocks, tables and form controls, in a column of a given
//! width. The result is a flat list of drawing pieces ([`Prim`]) with positions relative to the column's top left, plus
//! where each link and control ended up, so the browser can draw, scroll, outline and click without laying out again.

use crate::safeclamp::SafeClamp;
use iyakki_web::css::{Gradient, Length, Rgba};
use iyakki_web::doc::{Block, BoxStyle, Control, ControlKind, Doc, ImageRef, InlineBox, RowBlock, Span, Table, TextBlock};
use iyakki_web::style::{contrast, luminance, over, Align, Clear, Fit, Float, Justify, Pos, VAlign};

use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{DIM, MUTED, R, TEXT};

const LINK: [u8; 4] = rgb(120, 175, 255);
const ITALIC: [u8; 4] = rgb(196, 208, 255);

pub enum Kind {
    Text { text: String, size: f32, face: Face, color: [u8; 4], link: Option<usize> },
    Rect { color: [u8; 4], radius: f32 },
    /// A form control; drawn by the app, which knows its state.
    Control { idx: usize },
    /// A picture; drawn by the app, which has the pixels (or says why not). `link` makes it clickable.
    /// `fit` is `object-fit`; `alpha` the `opacity` of the box it sits in.
    Image { idx: usize, link: Option<usize>, fit: Fit, alpha: f32 },
}

/// What layout needs to know about a picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImgInfo {
    /// The picture's own size once it has arrived.
    pub size: Option<(u32, u32)>,
    pub failed: bool,
}

/// What a page's text can contain besides words, for sizing.
struct Env<'a> {
    controls: &'a [Control],
    images: &'a [ImageRef],
    infos: &'a [ImgInfo],
    inlines: &'a [InlineBox],
    /// How deep inline boxes are being measured or laid out (they are measured, then laid out, so nesting costs).
    depth: std::cell::Cell<u8>,
    s: f32,
}

pub struct Prim {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: Kind,
}

/// Something Tab can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Link(usize),
    Control(usize),
}

/// A page laid out for one width and scale. Positions are relative to the top left of the content column.
pub struct Layout {
    pub prims: Vec<Prim>,
    pub height: f32,
    /// The first rectangle of each link, to scroll to and outline.
    pub links: Vec<Option<R>>,
    /// Where each control is.
    pub controls: Vec<Option<R>>,
    /// Links and controls in reading order.
    pub focus: Vec<Focus>,
    pub key: (i32, i32),
    /// How many pictures had settled (arrived or failed) when this was laid out.
    pub rev: u32,
    /// The page's own background, for the whole view.
    pub canvas: Option<[u8; 4]>,
}

/// What text and rules are drawn on, which decides their default colours.
#[derive(Clone, Copy)]
struct Ctx {
    bg: [u8; 4],
    fg: [u8; 4],
    link: [u8; 4],
    light: bool,
    /// Inside a table cell or an inline box: no gap after the last paragraph.
    tight: bool,
}

/// The colours of the reader's own dark theme.
const THEME_BG: [u8; 4] = rgb(14, 18, 34);

impl Ctx {
    fn on(bg: Rgba, themed: bool) -> Ctx {
        let light = luminance(bg) > 0.4;
        if !light && themed {
            return Ctx { bg, fg: TEXT, link: LINK, light: false, tight: false };
        }
        if light {
            Ctx { bg, fg: rgb(24, 24, 28), link: rgb(0, 76, 190), light: true, tight: false }
        } else {
            Ctx { bg, fg: rgb(236, 238, 245), link: rgb(130, 180, 255), light: false, tight: false }
        }
    }

    /// Like [`Ctx::readable`] against some other background (a highlight behind the words).
    fn readable_on(&self, c: [u8; 4], bg: [u8; 4]) -> [u8; 4] {
        Ctx { bg, ..*self }.readable(c)
    }

    /// A border or fill colour as it will look: blended over the background, never changed otherwise.
    fn readable_fill(&self, c: [u8; 4]) -> [u8; 4] {
        if c[3] < 255 { over(c, self.bg) } else { c }
    }

    /// A colour that can be read on this background: unchanged when it already contrasts enough.
    fn readable(&self, c: [u8; 4]) -> [u8; 4] {
        let c = if c[3] < 255 { over(c, self.bg) } else { c };
        if contrast(c, self.bg) >= 3.0 {
            c
        } else if contrast(self.fg, self.bg) >= 4.5 {
            self.fg
        } else if luminance(self.bg) > 0.4 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    }
}

struct Word {
    text: String,
    face: Face,
    size: f32,
    color: [u8; 4],
    link: Option<usize>,
    /// There was white space before this word (in the page).
    space_before: bool,
    w: f32,
    bg: Option<[u8; 4]>,
    underline: bool,
    strike: bool,
}

enum Tok {
    Word(Word),
    /// A form control; `m` is its margin (top, right, bottom, left).
    Control { idx: usize, w: f32, h: f32, space_before: bool, m: [f32; 4], va: VAlign },
    /// An atomic inline box (`display:inline-block`), measured: border-box width and height.
    Inline { idx: usize, w: f32, h: f32, space_before: bool, m: [f32; 4], va: VAlign },
    /// Room an inline element's padding (with its background) or margin takes beside its words.
    Gap { w: f32, bg: Option<[u8; 4]>, size: f32 },
    Image { idx: usize, link: Option<usize>, w: f32, h: f32, space_before: bool, fit: Fit },
    /// A picture with `float: left|right`: taken out of the line and put at the edge.
    FloatImg { idx: usize, link: Option<usize>, w: f32, h: f32, fit: Fit, left: bool },
    Break,
}

struct Sink<'a> {
    prims: &'a mut Vec<Prim>,
    links: &'a mut Vec<Option<R>>,
    controls: &'a mut Vec<Option<R>>,
    focus: &'a mut Vec<Focus>,
    /// Left edge of the column being laid out.
    ox: f32,
    /// Floats placed so far (their margin boxes), which text lines and later floats make room for.
    floats: Vec<FloatBox>,
    /// Top of the last float: a float never sits higher than the one before it.
    float_top: f32,
    /// Positioned boxes taken out of the flow (`position:absolute`, or any with a `z-index`), drawn after the flow.
    layers: Vec<Layer>,
    /// The nearest positioned ancestors, innermost last; the first is the page.
    anc: Vec<Anc>,
}

/// The margin box of a float.
#[derive(Clone, Copy)]
struct FloatBox {
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    left: bool,
}

/// A box drawn above the flow, with what is needed to move it once its container's height is known.
struct Layer {
    z: i32,
    prims: Vec<Prim>,
    focus: Vec<Focus>,
    /// `bottom` offset, where it was laid out and how tall it is, when it hangs from the bottom of its container.
    bottom: Option<(f32, f32, f32)>,
    /// The layers made while this one was laid out (they move with it).
    inner: std::ops::Range<usize>,
}

/// A positioned ancestor: its padding box, so far.
struct Anc {
    x: f32,
    y: f32,
    w: f32,
    /// Layers waiting for the height.
    hanging: Vec<usize>,
}

/// The size a control takes in a line of text of `size`.
pub fn control_size(g: &mut Gfx, c: &Control, size: f32, s: f32, max_w: f32) -> (f32, f32) {
    let field_h = size * 1.95;
    let (w, h) = match c.kind {
        ControlKind::Text | ControlKind::Password => ((c.size as f32 * size * 0.52).clamp_safe(130.0 * s, 900.0 * s), field_h),
        ControlKind::Textarea => ((c.size as f32 * size * 0.52).clamp_safe(220.0 * s, 700.0 * s), c.rows as f32 * size * 1.45 + 14.0 * s),
        ControlKind::Checkbox | ControlKind::Radio => (size * 1.25, size * 1.25),
        ControlKind::Submit | ControlKind::Button => (g.text_width(size, &c.label, Face::Bold) + size * 2.0, field_h),
        ControlKind::Select => {
            let widest = c.options.iter().map(|o| g.text_width(size, &o.label, Face::Sans)).fold(0.0f32, f32::max);
            ((widest + size * 2.6).max(120.0 * s), field_h)
        }
        ControlKind::Hidden => (0.0, 0.0),
    };
    (w.min(max_w.max(40.0)), h)
}

/// The size a picture takes: its own size (or the one the page gave it), scaled with the screen, and never wider than
/// the column. A picture that has not arrived keeps a placeholder; one that failed shrinks to its description.
fn image_size(g: &mut Gfx, img: &ImageRef, info: &ImgInfo, size: f32, s: f32, max_w: f32) -> (f32, f32) {
    if info.failed {
        let label = if img.alt.is_empty() { "image".to_string() } else { img.alt.clone() };
        let w = (g.text_width(size * 0.9, &label, Face::Sans) + 20.0 * s).max(48.0 * s).min(max_w.max(1.0));
        return (w, size * 1.7);
    }
    let natural = info.size.map(|(w, h)| (w as f32, h as f32));
    let (mut w, mut h) = match (img.width, img.height, natural) {
        (Some(w), Some(h), _) => (w as f32, h as f32),
        (Some(w), None, Some((nw, nh))) => (w as f32, w as f32 * nh / nw),
        (None, Some(h), Some((nw, nh))) => (h as f32 * nw / nh, h as f32),
        (_, _, Some((nw, nh))) => (nw, nh),
        (Some(w), None, None) => (w as f32, w as f32 * 0.66),
        (None, Some(h), None) => (h as f32 * 1.5, h as f32),
        (None, None, None) => (120.0, 80.0),
    };
    let k = s.clamp(0.75, 3.0);
    (w, h) = (w * k, h * k);
    let cap = max_w.max(40.0);
    if w > cap {
        h *= cap / w;
        w = cap;
    }
    if h > 900.0 * s {
        w *= 900.0 * s / h;
        h = 900.0 * s;
    }
    (w.max(8.0).round(), h.max(8.0).round())
}

fn tokens(g: &mut Gfx, spans: &[Span], size: f32, bold: bool, ctx: &Ctx, env: &Env, max_w: f32) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut space = false;
    for sp in spans {
        if let Some(i) = sp.control {
            if let Some(c) = env.controls.get(i) {
                let (mut w, mut h) = control_size(g, c, size, env.s, max_w);
                // a size from the page's style sheet wins over the one the control would pick
                if !matches!(c.kind, ControlKind::Checkbox | ControlKind::Radio) {
                    if let Some(cw) = px(c.width, env.s, max_w).filter(|v| *v >= 1.0) {
                        w = cw.min(max_w.max(40.0)).max(12.0);
                    }
                    if let Length::Px(p) = c.height {
                        if p >= 1.0 {
                            h = (p * env.s).max(10.0);
                        }
                    }
                }
                let m = c.margin.map(|v| v.max(0.0) * env.s);
                out.push(Tok::Control { idx: i, w, h, space_before: space, m, va: c.valign });
                space = false;
            }
            continue;
        }
        if let Some(i) = sp.inline {
            if i < env.inlines.len() {
                let (w, h, m, va) = inline_metrics(g, i, env, ctx, max_w);
                out.push(Tok::Inline { idx: i, w, h, space_before: space, m, va });
                space = false;
            }
            continue;
        }
        if sp.style.gap > 0 && sp.text.is_empty() {
            let size = if sp.style.size_pct > 0 { 16.0 * env.s * sp.style.size_pct as f32 / 100.0 } else { size };
            out.push(Tok::Gap { w: sp.style.gap as f32 * env.s, bg: sp.style.bg.map(|b| over(b, ctx.bg)), size });
            continue;
        }
        if let Some(i) = sp.image {
            if let Some(img) = env.images.get(i) {
                let info = env.infos.get(i).copied().unwrap_or_default();
                let (w, h) = image_size(g, img, &info, size, env.s, max_w);
                if img.float != Float::None && !info.failed {
                    out.push(Tok::FloatImg { idx: i, link: sp.link, w, h, fit: img.fit, left: img.float == Float::Left });
                    continue;
                }
                out.push(Tok::Image { idx: i, link: sp.link, w, h, space_before: space, fit: img.fit });
                space = false;
            }
            continue;
        }
        let face = if sp.style.code { Face::Mono } else if sp.style.bold || bold { Face::Bold } else { Face::Sans };
        let base = if sp.style.size_pct > 0 { 16.0 * env.s * sp.style.size_pct as f32 / 100.0 } else { size };
        let sz = if sp.style.code { base * 0.92 } else { base };
        // the page's colour when it gave one; otherwise the reader's own colours for links, notes and italics
        let wanted = match sp.style.color {
            Some(c) => c,
            None if sp.link.is_some() => ctx.link,
            None if sp.style.dim => DIM,
            None if sp.style.italic && !ctx.light => ITALIC,
            None => ctx.fg,
        };
        let bg = sp.style.bg.map(|b| over(b, ctx.bg));
        let col = ctx.readable_on(wanted, bg.unwrap_or(ctx.bg));
        let underline = match sp.style.underline {
            Some(u) => u,
            None => sp.link.is_some(),
        };
        let strike = sp.style.strike;
        let mut word = String::new();
        let flush = |g: &mut Gfx, word: &mut String, space: &mut bool, out: &mut Vec<Tok>| {
            if !word.is_empty() {
                let w = g.text_width(sz, word, face);
                out.push(Tok::Word(Word { text: std::mem::take(word), face, size: sz, color: col, link: sp.link, space_before: *space, w, bg, underline, strike }));
                *space = false;
            }
        };
        for c in sp.text.chars() {
            if c == '\n' {
                flush(g, &mut word, &mut space, &mut out);
                out.push(Tok::Break);
                space = false;
            } else if c.is_whitespace() && c != '\u{a0}' {
                flush(g, &mut word, &mut space, &mut out);
                space = true;
            } else {
                word.push(if c == '\u{a0}' { ' ' } else { c });
            }
        }
        flush(g, &mut word, &mut space, &mut out);
    }
    out
}

enum What {
    Word(Word),
    Control { idx: usize, h: f32, m: [f32; 4], va: VAlign },
    Inline { idx: usize, h: f32, m: [f32; 4], va: VAlign },
    Gap { bg: Option<[u8; 4]>, size: f32 },
    Image { idx: usize, link: Option<usize>, h: f32, fit: Fit },
}

struct Item {
    x: f32,
    w: f32,
    /// The right margin after it (counts when a line is centred).
    tail: f32,
    what: What,
}

/// Where a box of height `h` with margins `m` sits in a line `lh` tall.
fn v_off(va: VAlign, lh: f32, h: f32, m: [f32; 4]) -> f32 {
    match va {
        VAlign::Top => m[0],
        VAlign::Bottom => lh - h - m[2],
        VAlign::Middle => (lh - m[0] - h - m[2]) / 2.0 + m[0],
    }
}

/// Put a finished line on the page, `align`ed within `width`.
#[allow(clippy::too_many_arguments)]
fn emit_line(g: &mut Gfx, env: &Env, ctx: &Ctx, line: &mut Vec<Item>, lh: f32, size: f32, x0: f32, y0: f32, y: f32, width: f32, align: Align, sink: &mut Sink) {
    let used = line.last().map_or(0.0, |it| it.x + it.w + it.tail);
    let shift = match align {
        Align::Left => 0.0,
        Align::Center => ((width - used) / 2.0).max(0.0),
        Align::Right => (width - used).max(0.0),
    };
    let x0 = x0 + sink.ox + shift;
    // a background that runs on from the word before is drawn through the space between them
    let mut prev: Option<(f32, [u8; 4])> = None;
    for it in line.drain(..) {
        match it.what {
            What::Word(wd) => {
                let r = R::new(x0 + it.x, y0 + y, it.w, lh);
                let ty = r.y + (lh - wd.size * 1.25) / 2.0;
                if let Some(bg) = wd.bg {
                    let start = match prev {
                        Some((end, c)) if c == bg && r.x - end < wd.size => end,
                        _ => r.x,
                    };
                    sink.prims.push(Prim { x: start - 2.0, y: ty - 1.0, w: r.x + r.w - start + 4.0, h: wd.size * 1.4, kind: Kind::Rect { color: bg, radius: 3.0 } });
                    prev = Some((r.x + r.w, bg));
                } else {
                    prev = None;
                }
                if let Some(l) = wd.link {
                    if let Some(slot) = sink.links.get_mut(l) {
                        if slot.is_none() {
                            *slot = Some(r);
                            sink.focus.push(Focus::Link(l));
                        }
                    }
                }
                if wd.underline {
                    sink.prims.push(Prim { x: r.x, y: r.y + lh * 0.82, w: it.w, h: (size * 0.07).max(1.0), kind: Kind::Rect { color: wd.color, radius: 0.0 } });
                }
                if wd.strike {
                    sink.prims.push(Prim { x: r.x, y: ty + wd.size * 0.7, w: it.w, h: (wd.size * 0.07).max(1.0), kind: Kind::Rect { color: wd.color, radius: 0.0 } });
                }
                sink.prims.push(Prim { x: r.x, y: ty, w: it.w, h: lh, kind: Kind::Text { text: wd.text, size: wd.size, face: wd.face, color: wd.color, link: wd.link } });
            }
            What::Image { idx, link, h, fit } => {
                prev = None;
                let r = R::new(x0 + it.x, y0 + y + (lh - h) / 2.0, it.w, h);
                if let Some(l) = link {
                    if let Some(slot) = sink.links.get_mut(l) {
                        if slot.is_none() {
                            *slot = Some(r);
                            sink.focus.push(Focus::Link(l));
                        }
                    }
                }
                sink.prims.push(Prim { x: r.x, y: r.y, w: r.w, h: r.h, kind: Kind::Image { idx, link, fit, alpha: 1.0 } });
            }
            What::Gap { bg, size: gs } => {
                if let Some(bg) = bg {
                    let ty = y0 + y + (lh - gs * 1.25) / 2.0;
                    let start = match prev {
                        Some((end, c)) if c == bg && x0 + it.x - end < gs => end,
                        _ => x0 + it.x,
                    };
                    sink.prims.push(Prim { x: start, y: ty - 1.0, w: x0 + it.x + it.w - start, h: gs * 1.4, kind: Kind::Rect { color: bg, radius: 0.0 } });
                    prev = Some((x0 + it.x + it.w, bg));
                } else {
                    prev = None;
                }
            }
            What::Inline { idx, h, m, va } => {
                prev = None;
                if let Some(ib) = env.inlines.get(idx) {
                    let top = y0 + y + v_off(va, lh, h, m);
                    place_inline(g, ib, env, ctx, x0 + it.x, top, it.w, h, sink);
                }
            }
            What::Control { idx, h, m, va } => {
                prev = None;
                let r = R::new(x0 + it.x, y0 + y + v_off(va, lh, h, m), it.w, h);
                if let Some(slot) = sink.controls.get_mut(idx) {
                    *slot = Some(r);
                }
                sink.focus.push(Focus::Control(idx));
                sink.prims.push(Prim { x: r.x, y: r.y, w: r.w, h: r.h, kind: Kind::Control { idx } });
            }
        }
    }
}

/// Place words and controls into lines `width` wide, starting at (`x0`, `y0`). Returns the height used.
#[allow(clippy::too_many_arguments)]
/// The room beside the floats for a line (or a float) at `y` that is `h` tall in a column whose left edge is `left` and
/// that is `width` wide: how far in the line starts and how wide it is.
fn band(sink: &Sink, left: f32, width: f32, y: f32, h: f32) -> (f32, f32) {
    let (mut l, mut r) = (left, left + width);
    for f in &sink.floats {
        if f.y0 < y + h && f.y1 > y {
            if f.left {
                l = l.max(f.x1);
            } else {
                r = r.min(f.x0);
            }
        }
    }
    (l - left, (r - l).max(0.0))
}

/// The bottom of the nearest float beside a line at `y` (where the line can go to get more room).
fn next_bottom(sink: &Sink, y: f32, h: f32) -> Option<f32> {
    sink.floats.iter().filter(|f| f.y0 < y + h && f.y1 > y).map(|f| f.y1).fold(None, |a, b| Some(a.map_or(b, |a: f32| a.min(b))))
}

/// Where a float `outer` wide goes: not above the float before it, and low enough to have room beside the others.
/// Returns its top and the left and right edge of the room there.
fn settle(sink: &Sink, left: f32, width: f32, y: f32, outer: f32) -> (f32, f32, f32) {
    let mut y = y.max(sink.float_top);
    for _ in 0..200 {
        let (_, room) = band(sink, left, width, y, 1.0);
        let beside = sink.floats.iter().any(|f| f.y0 < y + 1.0 && f.y1 > y);
        if outer <= room + 0.5 || !beside {
            break;
        }
        match next_bottom(sink, y, 1.0) {
            Some(b) if b > y => y = b,
            _ => break,
        }
    }
    let (li, room) = band(sink, left, width, y, 1.0);
    (y, left + li, left + li + room)
}

/// Something that must start below the floats on the given side(s).
fn clear_y(sink: &Sink, clear: Clear, y: f32) -> f32 {
    sink.floats.iter().filter(|f| match clear {
        Clear::Both => true,
        Clear::Left => f.left,
        Clear::Right => !f.left,
        Clear::None => false,
    }).fold(y, |a, f| a.max(f.y1))
}

/// A picture with `float` in the middle of text: it takes the left or right edge from `abs_y` down, and the lines
/// after it get shorter beside it. Returns where the lines may go on when the column is too narrow to leave room for
/// text beside it (the picture then sits on a line of its own).
#[allow(clippy::too_many_arguments)]
fn float_image(sink: &mut Sink, left: f32, width: f32, abs_y: f32, size: f32, idx: usize, link: Option<usize>, fit: Fit, w: f32, h: f32, is_left: bool) -> Option<f32> {
    let gap = size * 0.6;
    let outer = (w + gap).min(width);
    let (y, cl, cr) = settle(sink, left, width, abs_y, outer);
    let x0 = if is_left { cl } else { (cr - outer).max(cl) };
    let w = w.min(outer - gap).max(1.0);
    let x = if is_left { x0 } else { x0 + gap };
    let r = R::new(x, y, w, h);
    if let Some(l) = link {
        if let Some(slot) = sink.links.get_mut(l) {
            if slot.is_none() {
                *slot = Some(r);
                sink.focus.push(Focus::Link(l));
            }
        }
    }
    sink.prims.push(Prim { x: r.x, y: r.y, w: r.w, h: r.h, kind: Kind::Image { idx, link, fit, alpha: 1.0 } });
    let y1 = y + h + gap * 0.5;
    sink.floats.push(FloatBox { x0, x1: x0 + outer, y0: y, y1, left: is_left });
    sink.float_top = y;
    if (cr - cl) - outer < size * 3.0 { Some(y1) } else { None }
}

/// Place words and controls into lines `width` wide, starting at (`x0`, `y0`). Returns the height used.
#[allow(clippy::too_many_arguments)]
fn flow(g: &mut Gfx, env: &Env, ctx: &Ctx, toks: Vec<Tok>, x0: f32, y0: f32, width: f32, size: f32, align: Align, sink: &mut Sink) -> f32 {
    let space_w = g.text_width(size, " ", Face::Sans);
    let min_h = size * 1.45;
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut line: Vec<Item> = Vec::new();
    let mut lh = min_h;
    // the room beside floats for the line being filled
    let (mut lx, mut lw) = band(sink, sink.ox + x0, width, y0, min_h);
    // floats met in the middle of a line, put down when it ends
    let mut pending: Vec<(usize, Option<usize>, f32, f32, Fit, bool)> = Vec::new();
    macro_rules! reband {
        () => {{
            let b = band(sink, sink.ox + x0, width, y0 + y, min_h);
            lx = b.0;
            lw = b.1;
        }};
    }
    macro_rules! put_floats {
        () => {{
            for (idx, link, w, h, fit, left) in pending.drain(..) {
                if let Some(b) = float_image(sink, sink.ox + x0, width, y0 + y, size, idx, link, fit, w, h, left) {
                    y = y.max(b - y0);
                }
            }
        }};
    }
    macro_rules! end_line {
        () => {{
            emit_line(g, env, ctx, &mut line, lh, size, x0 + lx, y0, y, lw, align, sink);
            y += lh;
            x = 0.0;
            lh = min_h;
            put_floats!();
            reband!();
        }};
    }
    // an empty line too narrow for the next thing moves down to below the float beside it
    macro_rules! room {
        ($w:expr) => {{
            while line.is_empty() && $w > lw && lw < width {
                match next_bottom(sink, y0 + y, min_h) {
                    Some(b) if b > y0 + y => {
                        y = b - y0;
                        reband!();
                    }
                    _ => break,
                }
            }
        }};
    }
    // a box or control in the line: wraps to the next line when it does not fit, with its margins around it
    macro_rules! atom {
        ($space:expr, $w:expr, $m:expr, $h:expr, $pad:expr, $what:expr) => {{
            let m: [f32; 4] = $m;
            let outer = $w + m[1] + m[3];
            room!(outer.min(width));
            let gap = if line.is_empty() || !$space { 0.0 } else { space_w };
            if !line.is_empty() && x + gap + outer > lw {
                end_line!();
                room!(outer.min(width));
            }
            let gap = if line.is_empty() { 0.0 } else { gap };
            let w = $w.min((lw - m[1] - m[3]).max(40.0));
            line.push(Item { x: x + gap + m[3], w, tail: m[1], what: $what });
            x += gap + m[3] + w + m[1];
            lh = lh.max($h + $pad + m[0] + m[2]);
        }};
    }
    for t in toks {
        match t {
            Tok::Break => {
                if line.is_empty() {
                    y += min_h;
                    x = 0.0;
                    lh = min_h;
                    reband!();
                } else {
                    end_line!();
                }
            }
            Tok::FloatImg { idx, link, w, h, fit, left } => {
                if line.is_empty() {
                    if let Some(b) = float_image(sink, sink.ox + x0, width, y0 + y, size, idx, link, fit, w, h, left) {
                        y = y.max(b - y0);
                    }
                    reband!();
                } else {
                    pending.push((idx, link, w, h, fit, left));
                }
            }
            Tok::Control { idx, w, h, space_before, m, va } => atom!(space_before, w, m, h, 6.0, What::Control { idx, h, m, va }),
            Tok::Inline { idx, w, h, space_before, m, va } => atom!(space_before, w, m, h, 0.0, What::Inline { idx, h, m, va }),
            Tok::Gap { w, bg, size: gs } => {
                line.push(Item { x, w, tail: 0.0, what: What::Gap { bg, size: gs } });
                x += w;
                lh = lh.max(gs * 1.45);
            }
            Tok::Image { idx, link, w, h, space_before, fit } => {
                room!(w.min(width));
                let gap = if line.is_empty() || !space_before { 0.0 } else { space_w };
                if !line.is_empty() && x + gap + w > lw {
                    end_line!();
                    room!(w.min(width));
                }
                let gap = if line.is_empty() { 0.0 } else { gap };
                let w = w.min(lw.max(40.0));
                line.push(Item { x: x + gap, w, tail: 0.0, what: What::Image { idx, link, h, fit } });
                x += gap + w;
                lh = lh.max(h + 4.0);
            }
            Tok::Word(wd) => {
                room!(wd.w.min(width));
                let gap = if line.is_empty() || !wd.space_before { 0.0 } else { space_w };
                if !line.is_empty() && x + gap + wd.w > lw {
                    end_line!();
                    room!(wd.w.min(width));
                }
                let gap = if line.is_empty() { 0.0 } else { gap };
                lh = lh.max(wd.size * 1.45);
                if wd.w <= lw - x - gap {
                    let w = wd.w;
                    x += gap + w;
                    line.push(Item { x: x - w, w, tail: 0.0, what: What::Word(wd) });
                } else {
                    // a word wider than the line (an address, a long identifier): cut it into pieces
                    let mut piece = String::new();
                    let piece_word = |text: String, w: f32| Word { text, face: wd.face, size: wd.size, color: wd.color, link: wd.link, space_before: false, w, bg: wd.bg, underline: wd.underline, strike: wd.strike };
                    for c in wd.text.chars() {
                        let mut next = piece.clone();
                        next.push(c);
                        if g.text_width(wd.size, &next, wd.face) > lw - x && !piece.is_empty() {
                            let pw = g.text_width(wd.size, &piece, wd.face);
                            line.push(Item { x, w: pw, tail: 0.0, what: What::Word(piece_word(std::mem::take(&mut piece), pw)) });
                            end_line!();
                        }
                        piece.push(c);
                    }
                    if !piece.is_empty() {
                        let pw = g.text_width(wd.size, &piece, wd.face);
                        line.push(Item { x, w: pw, tail: 0.0, what: What::Word(piece_word(piece, pw)) });
                        x += pw;
                    }
                }
            }
        }
    }
    if !line.is_empty() {
        emit_line(g, env, ctx, &mut line, lh, size, x0 + lx, y0, y, lw, align, sink);
        y += lh;
    }
    put_floats!();
    y
}

fn heading_size(level: u8, s: f32) -> f32 {
    s * match level {
        1 => 30.0,
        2 => 25.0,
        3 => 21.0,
        4 => 18.0,
        _ => 16.5,
    }
}

pub fn layout(g: &mut Gfx, doc: &Doc, width: f32, s: f32, infos: &[ImgInfo], rev: u32) -> Layout {
    let env = Env { controls: &doc.controls, images: &doc.images, infos, inlines: &doc.inlines, depth: std::cell::Cell::new(0), s };
    let mut prims: Vec<Prim> = Vec::new();
    let mut links: Vec<Option<R>> = vec![None; doc.links.len()];
    let mut controls: Vec<Option<R>> = vec![None; doc.controls.len()];
    let mut focus: Vec<Focus> = Vec::new();
    let canvas = doc.canvas;
    let ctx = match canvas {
        Some(c) => Ctx::on(c, false),
        None => Ctx::on(THEME_BG, true),
    };
    let base = 16.0 * s;
    let mut sink = Sink { prims: &mut prims, links: &mut links, controls: &mut controls, focus: &mut focus, ox: 0.0, floats: Vec::new(), float_top: 0.0, layers: Vec::new(), anc: vec![Anc { x: 0.0, y: 0.0, w: width, hanging: Vec::new() }] };
    let mut prev_margin = 0.0;
    let y = blocks(g, &doc.blocks, &env, 0.0, width, 0.0, &ctx, &mut prev_margin, &mut sink);
    // floats may hang below the last line
    let y = sink.floats.iter().fold(y, |a, f| a.max(f.y1));
    if let Some(page) = sink.anc.pop() {
        finish_anc(&mut sink, page, 0.0, y);
    }
    // boxes taken out of the flow are drawn over it, in `z-index` order (below it when negative)
    let mut layers = std::mem::take(&mut sink.layers);
    layers.sort_by_key(|l| l.z);
    let (under, over_flow): (Vec<Layer>, Vec<Layer>) = layers.into_iter().partition(|l| l.z < 0);
    let mut below: Vec<Prim> = under.into_iter().flat_map(|l| l.prims).collect();
    let above: Vec<Prim> = over_flow.into_iter().flat_map(|l| l.prims).collect();
    drop(sink);
    below.append(&mut prims);
    prims = below;
    prims.extend(above);
    Layout { prims, height: y + base, links, controls, focus, key: key_of(width, s), rev, canvas }
}

/// Lay blocks one under the other in a column `width` wide whose left edge is at `ox`. Returns where the next one starts.
#[allow(clippy::too_many_arguments)]
fn blocks(g: &mut Gfx, list: &[Block], env: &Env, ox: f32, width: f32, y: f32, ctx: &Ctx, prev_margin: &mut f32, sink: &mut Sink) -> f32 {
    let s = env.s;
    let base = 16.0 * s;
    let mut y = y;
    let saved_ox = sink.ox;
    sink.ox = ox;
    for block in list {
        match block {
            Block::Text(t) => {
                y = text_block(g, t, env, width, y, base, ctx, sink);
                *prev_margin = 0.0;
            }
            Block::Pre(text) => {
                let size = 14.0 * s;
                let lh = size * 1.4;
                let pad = 10.0 * s;
                let (pre_bg, pre_fg) = if ctx.light { (rgb(240, 241, 245), rgb(30, 34, 48)) } else { (rgb(12, 16, 30), rgb(200, 215, 240)) };
                let first = sink.prims.len();
                let mut ly = y + pad;
                for line in text.lines().take(5000) {
                    let mut rest: Vec<char> = line.chars().collect();
                    if rest.is_empty() {
                        ly += lh;
                        continue;
                    }
                    while !rest.is_empty() {
                        let n = chars_that_fit(g, &rest, size, width - 2.0 * pad);
                        let chunk: String = rest[..n].iter().collect();
                        sink.prims.push(Prim { x: ox + pad, y: ly, w: width - 2.0 * pad, h: lh, kind: Kind::Text { text: chunk, size, face: Face::Mono, color: pre_fg, link: None } });
                        rest.drain(..n);
                        ly += lh;
                    }
                }
                let h = ly + pad - y;
                sink.prims.insert(first, Prim { x: ox, y, w: width, h, kind: Kind::Rect { color: pre_bg, radius: 8.0 * s } });
                y += h + base * 0.8;
                *prev_margin = 0.0;
            }
            Block::Rule => {
                y += base * 0.4;
                let c = if ctx.light { rgb(200, 204, 214) } else { rgb(60, 70, 100) };
                sink.prims.push(Prim { x: ox, y, w: width, h: (2.0 * s).max(1.0), kind: Kind::Rect { color: c, radius: 0.0 } });
                y += base * 1.2;
                *prev_margin = 0.0;
            }
            Block::Table(t) => {
                let held = std::mem::take(&mut sink.floats);
                y = table(g, t, env, width, y, base, ctx, sink);
                sink.floats = held;
                *prev_margin = 0.0;
            }
            Block::Box(b) => {
                y = place_box(g, b, env, ox, width, y, ctx, prev_margin, sink);
            }
            Block::Row(r) => {
                let held = std::mem::take(&mut sink.floats);
                y = row_block(g, r, env, ox, width, y, ctx, prev_margin, sink);
                sink.floats = held;
            }
        }
    }
    sink.ox = saved_ox;
    y
}

/// How the pieces that make up a box's frame move when the box is made taller than its content.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stretch {
    Grow,
    Shift,
}

struct BoxOut {
    y_end: f32,
    /// Top of the box's frame and its height (without margins).
    top: f32,
    height: f32,
    decor: Vec<(usize, Stretch)>,
}

fn px(l: Length, s: f32, of: f32) -> Option<f32> {
    l.resolve(of, s)
}

/// A box: margins, border, padding and background around blocks. The width counts the border and padding (the common
/// `border-box` convention), and `max-width` with automatic side margins centres it. `min_h` stretches the frame (the
/// columns of a row are as tall as the tallest).
#[allow(clippy::too_many_arguments)]
fn box_block(g: &mut Gfx, st: &BoxStyle, children: &[Block], env: &Env, ox: f32, width: f32, y: f32, ctx: &Ctx, prev_margin: &mut f32, sink: &mut Sink, min_h: Option<f32>) -> BoxOut {
    let s = env.s;
    let m = st.margin.map(|v| v * s);
    let pad = st.padding.map(|v| (v * s).max(0.0));
    let bw = st.border_w.map(|v| if v > 0.0 { (v * s).max(1.0) } else { 0.0 });
    // the box's own width, then the room left of it for margins
    let mut outer = px(st.width, s, width).unwrap_or(width - m[1].max(0.0) - m[3].max(0.0));
    if let Some(mx) = px(st.max_width, s, width) {
        outer = outer.min(mx);
    }
    if let Some(mn) = px(st.min_width, s, width) {
        outer = outer.max(mn);
    }
    let frame = pad[1] + pad[3] + bw[1] + bw[3];
    // a box that asked for a width keeps it however small; the floor is for boxes squeezed by their column
    let floor = if matches!(st.width, Length::Px(_)) { 8.0 } else { 40.0 };
    outer = outer.min(width).max(frame + floor);
    let free = (width - outer).max(0.0);
    let ml = match (st.auto_left, st.auto_right) {
        (true, true) => free / 2.0,
        (true, false) => (free - m[1].max(0.0)).max(0.0),
        _ => m[3].max(0.0).min(free),
    };
    let inner_x = ox + ml + bw[3] + pad[3];
    let inner_w = (outer - frame).max(floor);
    // the top margin merges with the bottom margin of what came before
    let top_gap = (m[0] - *prev_margin).max(0.0);
    let top = y + top_gap;
    let cstart = top + bw[0] + pad[0];
    // a gradient is what the box looks like; its average is what text on it is read against
    let bg = match &st.fx.gradient {
        Some(gr) => Some(over(gr.mean(), ctx.bg)),
        None => st.bg.map(|c| over(c, ctx.bg)),
    };
    let inner_ctx = match bg {
        Some(b) => Ctx { tight: ctx.tight, ..Ctx::on(b, false) },
        None => *ctx,
    };
    let first = sink.prims.len();
    let nf = sink.floats.len();
    let positioned = st.fx.pos != Pos::Static;
    if positioned {
        // what is positioned inside is placed against this box's padding box
        sink.anc.push(Anc { x: ox + ml + bw[3], y: top + bw[0], w: (outer - bw[1] - bw[3]).max(0.0), hanging: Vec::new() });
    }
    let mut inner_margin = 0.0;
    let mut y_end = blocks(g, children, env, inner_x, inner_w, cstart, &inner_ctx, &mut inner_margin, sink);
    // a box with a frame (or one that floats or is positioned) holds the floats inside it; others let them hang out
    let framed = bg.is_some() || bw.iter().any(|w| *w > 0.0) || st.fx.shadow.is_some() || st.fx.float != Float::None || st.fx.pos == Pos::Absolute;
    if framed {
        y_end = sink.floats[nf..].iter().fold(y_end, |a, f| a.max(f.y1));
    }
    // a child's bottom margin stays inside a box that has padding or a border, and leaks out otherwise
    let leak = if pad[2] > 0.0 || bw[2] > 0.0 { 0.0 } else { inner_margin };
    y_end -= leak;
    let mut height = (y_end - top) + pad[2] + bw[2];
    if let Some(mh) = min_h {
        height = height.max(mh);
    }
    let fx = ox + ml;
    if positioned {
        if let Some(a) = sink.anc.pop() {
            finish_anc(sink, a, top + bw[0], (height - bw[0] - bw[2]).max(0.0));
        }
    }
    let mut decor: Vec<(usize, Stretch)> = Vec::new();
    let mut at = first;
    let radius = (st.radius * s).min(outer / 2.0).min(height / 2.0);
    if let Some(sh) = &st.fx.shadow {
        for p in shadow_prims(sh, fx, top, outer, height, radius, s) {
            sink.prims.insert(at, p);
            decor.push((at, Stretch::Grow));
            at += 1;
        }
    }
    let uniform = bw.iter().all(|w| *w == bw[0]) && st.border_c.iter().all(|c| *c == st.border_c[0]);
    if bw.iter().any(|w| *w > 0.0) && uniform && radius > 0.0 {
        // a rounded frame: the border colour, then the fill inside it
        sink.prims.insert(at, Prim { x: fx, y: top, w: outer, h: height, kind: Kind::Rect { color: ctx.readable_fill(st.border_c[0]), radius } });
        decor.push((at, Stretch::Grow));
        at += 1;
        let fill = if bg.is_some() { fill_prims(st.fx.gradient.as_ref(), bg, fx + bw[3], top + bw[0], outer - bw[1] - bw[3], height - bw[0] - bw[2], (radius - bw[0]).max(0.0), ctx.bg) } else { vec![(Prim { x: fx + bw[3], y: top + bw[0], w: outer - bw[1] - bw[3], h: height - bw[0] - bw[2], kind: Kind::Rect { color: ctx.bg, radius: (radius - bw[0]).max(0.0) } }, Some(Stretch::Grow))] };
        for (p, k) in fill {
            sink.prims.insert(at, p);
            if let Some(k) = k {
                decor.push((at, k));
            }
            at += 1;
        }
    } else {
        if bg.is_some() {
            for (p, k) in fill_prims(st.fx.gradient.as_ref(), bg, fx, top, outer, height, radius, ctx.bg) {
                sink.prims.insert(at, p);
                if let Some(k) = k {
                    decor.push((at, k));
                }
                at += 1;
            }
        }
        let mut put = |p: Prim, k: Option<Stretch>, at: &mut usize, decor: &mut Vec<(usize, Stretch)>| {
            sink.prims.insert(*at, p);
            if let Some(k) = k {
                decor.push((*at, k));
            }
            *at += 1;
        };
        let c = |i: usize| ctx.readable_fill(st.border_c[i]);
        if bw[0] > 0.0 {
            put(Prim { x: fx, y: top, w: outer, h: bw[0], kind: Kind::Rect { color: c(0), radius: 0.0 } }, None, &mut at, &mut decor);
        }
        if bw[1] > 0.0 {
            put(Prim { x: fx + outer - bw[1], y: top, w: bw[1], h: height, kind: Kind::Rect { color: c(1), radius: 0.0 } }, Some(Stretch::Grow), &mut at, &mut decor);
        }
        if bw[2] > 0.0 {
            put(Prim { x: fx, y: top + height - bw[2], w: outer, h: bw[2], kind: Kind::Rect { color: c(2), radius: 0.0 } }, Some(Stretch::Shift), &mut at, &mut decor);
        }
        if bw[3] > 0.0 {
            put(Prim { x: fx, y: top, w: bw[3], h: height, kind: Kind::Rect { color: c(3), radius: 0.0 } }, Some(Stretch::Grow), &mut at, &mut decor);
        }
    }
    if st.fx.opacity < 1.0 {
        fade(&mut sink.prims[first..], st.fx.opacity, ctx.bg);
    }
    let mb = m[2].max(0.0) + leak;
    *prev_margin = mb;
    BoxOut { y_end: top + height + mb, top, height, decor }
}

/// A positioned box's container is finished: boxes that hang from its bottom edge can now be placed.
fn finish_anc(sink: &mut Sink, a: Anc, top: f32, height: f32) {
    for idx in a.hanging {
        let Some((b, prov_y, box_h)) = sink.layers[idx].bottom.take() else { continue };
        let dy = (top + height - b - box_h) - prov_y;
        shift_layer(sink, idx, 0.0, dy);
    }
}

/// Move one layer (and the links and controls that were put in it).
fn shift_one(sink: &mut Sink, idx: usize, dx: f32, dy: f32) {
    for p in sink.layers[idx].prims.iter_mut() {
        p.x += dx;
        p.y += dy;
    }
    let focus = sink.layers[idx].focus.clone();
    shift_focus(sink, &focus, dx, dy);
}

fn shift_focus(sink: &mut Sink, focus: &[Focus], dx: f32, dy: f32) {
    for f in focus {
        let slot = match f {
            Focus::Link(l) => sink.links.get_mut(*l),
            Focus::Control(c) => sink.controls.get_mut(*c),
        };
        if let Some(Some(r)) = slot {
            r.x += dx;
            r.y += dy;
        }
    }
}

/// Move a layer and the layers that were made inside it.
fn shift_layer(sink: &mut Sink, idx: usize, dx: f32, dy: f32) {
    let inner = sink.layers[idx].inner.clone();
    shift_one(sink, idx, dx, dy);
    for k in inner {
        shift_one(sink, k, dx, dy);
    }
}

/// Everything laid out since `p0` moves (`position: relative`).
fn shift_since(sink: &mut Sink, p0: usize, f0: usize, l0: usize, dx: f32, dy: f32) {
    for p in sink.prims[p0..].iter_mut() {
        p.x += dx;
        p.y += dy;
    }
    let focus: Vec<Focus> = sink.focus[f0..].to_vec();
    shift_focus(sink, &focus, dx, dy);
    for k in l0..sink.layers.len() {
        shift_one(sink, k, dx, dy);
    }
}

/// The soft edge of a `box-shadow` as a few nested translucent rectangles, widest first.
fn shadow_prims(sh: &iyakki_web::css::Shadow, x: f32, y: f32, w: f32, h: f32, radius: f32, s: f32) -> Vec<Prim> {
    let (dx, dy, blur, spread) = (sh.dx * s, sh.dy * s, sh.blur * s, sh.spread * s);
    let n = if blur <= 0.5 { 1 } else { ((blur / (2.0 * s)).ceil() as usize).clamp(2, 6) };
    let a = sh.color[3] as f32 / 255.0;
    // each layer adds a share, so that the middle ends up as dark as the colour asked for
    let per = 1.0 - (1.0 - a).powf(1.0 / n as f32);
    let alpha = ((per * 255.0).round() as u8).max(if a > 0.0 { 1 } else { 0 });
    let mut out = Vec::new();
    for k in 0..n {
        let grow = if n == 1 { spread } else { spread + blur * (0.5 - k as f32 / n as f32) };
        let (lw, lh) = (w + 2.0 * grow, h + 2.0 * grow);
        if lw <= 0.0 || lh <= 0.0 || alpha == 0 {
            continue;
        }
        out.push(Prim { x: x + dx - grow, y: y + dy - grow, w: lw, h: lh, kind: Kind::Rect { color: [sh.color[0], sh.color[1], sh.color[2], alpha], radius: (radius + grow).max(0.0) } });
    }
    out
}

/// A box's fill: the gradient as bands across the box (rows for a mostly vertical one, columns otherwise; a radial one
/// is its average colour), or the plain colour. The second part says how the piece follows a box made taller.
#[allow(clippy::too_many_arguments)]
fn fill_prims(gr: Option<&Gradient>, bg: Option<[u8; 4]>, x: f32, y: f32, w: f32, h: f32, radius: f32, under: [u8; 4]) -> Vec<(Prim, Option<Stretch>)> {
    let Some(gr) = gr else {
        return bg.map(|c| vec![(Prim { x, y, w, h, kind: Kind::Rect { color: c, radius } }, Some(Stretch::Grow))]).unwrap_or_default();
    };
    if gr.radial || w <= 0.0 || h <= 0.0 {
        return vec![(Prim { x, y, w, h, kind: Kind::Rect { color: over(gr.mean(), under), radius } }, Some(Stretch::Grow))];
    }
    let th = gr.angle.to_radians();
    let (dx, dy) = (th.sin(), -th.cos());
    let len = (w * dx.abs() + h * dy.abs()).max(1.0);
    let rows = dy.abs() >= dx.abs();
    let extent = if rows { h } else { w };
    let n = ((extent / 3.0).ceil() as usize).clamp(1, 48);
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let mut out = Vec::new();
    for k in 0..n {
        let (a, b) = (extent * k as f32 / n as f32, extent * (k + 1) as f32 / n as f32);
        let mid = (a + b) / 2.0;
        let t = if rows { 0.5 + (y + mid - cy) * dy / len } else { 0.5 + (x + mid - cx) * dx / len };
        let color = over(gr.at(t.clamp(0.0, 1.0)), under);
        // bands overlap by a pixel so that rounding never leaves a seam
        let size = (b - a) + if k + 1 < n { 1.0 } else { 0.0 };
        // rounded corners: the bands near the ends are shorter
        let d = mid.min(extent - mid);
        let inset = if radius > 0.0 && d < radius { radius - (radius * radius - (radius - d) * (radius - d)).max(0.0).sqrt() } else { 0.0 };
        if rows {
            out.push((Prim { x: x + inset, y: y + a, w: (w - 2.0 * inset).max(1.0), h: size, kind: Kind::Rect { color, radius: 0.0 } }, if k + 1 == n { Some(Stretch::Grow) } else { None }));
        } else {
            out.push((Prim { x: x + a, y: y + inset, w: size, h: (h - 2.0 * inset).max(1.0), kind: Kind::Rect { color, radius: 0.0 } }, Some(Stretch::Grow)));
        }
    }
    out
}

/// `opacity`: colours move toward what is behind the box.
fn fade(prims: &mut [Prim], opacity: f32, under: [u8; 4]) {
    let mix = |c: &mut [u8; 4]| *c = over([c[0], c[1], c[2], (c[3] as f32 * opacity).round() as u8], under);
    for p in prims {
        match &mut p.kind {
            Kind::Rect { color, .. } | Kind::Text { color, .. } => mix(color),
            Kind::Image { alpha, .. } => *alpha *= opacity,
            Kind::Control { .. } => {}
        }
    }
}

/// Where `object-fit` puts a picture of `nw` by `nh` pixels in the box `r`: the part of the box it paints, then its
/// scale and where its top left corner goes.
pub fn fit_picture(fit: Fit, r: R, nw: f32, nh: f32) -> (R, f32, f32, f32, f32) {
    let fit = match fit {
        Fit::ScaleDown if nw <= r.w && nh <= r.h => Fit::None,
        Fit::ScaleDown => Fit::Contain,
        f => f,
    };
    let (nw, nh) = (nw.max(1.0), nh.max(1.0));
    match fit {
        Fit::Fill => (r, r.w / nw, r.h / nh, r.x, r.y),
        Fit::Contain | Fit::ScaleDown => {
            let k = (r.w / nw).min(r.h / nh);
            let (w, h) = (nw * k, nh * k);
            let (tx, ty) = (r.x + (r.w - w) / 2.0, r.y + (r.h - h) / 2.0);
            (R::new(tx, ty, w, h), k, k, tx, ty)
        }
        Fit::Cover => {
            let k = (r.w / nw).max(r.h / nh);
            (r, k, k, r.x + (r.w - nw * k) / 2.0, r.y + (r.h - nh * k) / 2.0)
        }
        Fit::None => {
            let (tx, ty) = (r.x + (r.w - nw) / 2.0, r.y + (r.h - nh) / 2.0);
            let (cx, cy) = (tx.max(r.x), ty.max(r.y));
            (R::new(cx, cy, (tx + nw).min(r.x + r.w) - cx, (ty + nh).min(r.y + r.h) - cy), 1.0, 1.0, tx, ty)
        }
    }
}

/// A block-level box, wherever its style puts it: in the flow (maybe shifted, if `position: relative`), at the edge
/// (`float`), or out of the flow (`position: absolute`). `fixed` and `sticky` stay where the flow puts them.
#[allow(clippy::too_many_arguments)]
fn place_box(g: &mut Gfx, b: &iyakki_web::doc::BoxBlock, env: &Env, ox: f32, width: f32, y: f32, ctx: &Ctx, prev_margin: &mut f32, sink: &mut Sink) -> f32 {
    let fx = &b.style.fx;
    let s = env.s;
    let mut y = y;
    if fx.clear != Clear::None {
        let c = clear_y(sink, fx.clear, y);
        if c > y {
            y = c;
            *prev_margin = 0.0;
        }
    }
    if fx.float != Float::None {
        return float_box(g, b, env, ox, width, y, ctx, sink);
    }
    if fx.pos == Pos::Absolute {
        abs_box(g, b, env, ox, width, y, ctx, sink);
        return y;
    }
    let (p0, f0, l0) = (sink.prims.len(), sink.focus.len(), sink.layers.len());
    let out = box_block(g, &b.style, &b.children, env, ox, width, y, ctx, prev_margin, sink, None);
    if fx.pos == Pos::Relative {
        let dx = fx.off[3].map(|v| v * s).or(fx.off[1].map(|v| -v * s)).unwrap_or(0.0);
        let dy = fx.off[0].map(|v| v * s).or(fx.off[2].map(|v| -v * s)).unwrap_or(0.0);
        if dx != 0.0 || dy != 0.0 {
            shift_since(sink, p0, f0, l0, dx, dy);
        }
    }
    if fx.pos != Pos::Static && fx.z != 0 {
        let prims: Vec<Prim> = sink.prims.drain(p0..).collect();
        let focus = sink.focus[f0..].to_vec();
        let idx = sink.layers.len();
        sink.layers.push(Layer { z: fx.z, prims, focus, bottom: None, inner: l0..idx });
    }
    out.y_end
}

/// The width a floated or absolutely placed box takes when it did not ask for one: as wide as its content, up to `avail`.
fn shrink_width(g: &mut Gfx, b: &iyakki_web::doc::BoxBlock, env: &Env, ctx: &Ctx, avail: f32, parent: f32) -> f32 {
    let s = env.s;
    let st = &b.style;
    let frame = (st.padding[1] + st.padding[3]) * s + st.border_w[1].max(0.0) * s + st.border_w[3].max(0.0) * s;
    let mut w = px(st.width, s, parent).unwrap_or_else(|| (intrinsic(g, &b.children, env, ctx, avail) + frame + 2.0).min(avail));
    if let Some(mx) = px(st.max_width, s, parent) {
        w = w.min(mx);
    }
    if let Some(mn) = px(st.min_width, s, parent) {
        w = w.max(mn);
    }
    w.min(avail).max(frame + 40.0)
}

/// `float: left|right` on a box: it takes the edge of the column (under any floats already there), and the text after
/// it flows beside it. When the column is too narrow to leave room for text, it takes a line of its own.
#[allow(clippy::too_many_arguments)]
fn float_box(g: &mut Gfx, b: &iyakki_web::doc::BoxBlock, env: &Env, ox: f32, width: f32, y: f32, ctx: &Ctx, sink: &mut Sink) -> f32 {
    let s = env.s;
    let st = &b.style;
    let (ml, mr) = (st.margin[3].max(0.0) * s, st.margin[1].max(0.0) * s);
    let avail = (width - ml - mr).max(60.0);
    let bw = shrink_width(g, b, env, ctx, avail, width);
    let outer = bw + ml + mr;
    let (ty, cl, cr) = settle(sink, ox, width, y, outer);
    let left = st.fx.float == Float::Left;
    let x = if left { cl } else { (cr - outer).max(cl) };
    let mut st2 = st.clone();
    st2.width = Length::Px(bw / s);
    st2.auto_left = false;
    st2.auto_right = false;
    st2.max_width = Length::Auto;
    st2.min_width = Length::Auto;
    // a float is its own world: text beside the floats outside does not shorten its lines
    let held = std::mem::take(&mut sink.floats);
    let held_top = std::mem::replace(&mut sink.float_top, 0.0);
    let mut pm = 0.0;
    let out = box_block(g, &st2, &b.children, env, x, outer, ty, ctx, &mut pm, sink, None);
    sink.floats = held;
    sink.float_top = held_top;
    sink.floats.push(FloatBox { x0: x, x1: x + outer, y0: ty, y1: out.y_end, left });
    sink.float_top = ty;
    if (cr - cl) - outer < 48.0 * s { y.max(out.y_end) } else { y }
}

/// `position: absolute`: laid out against the nearest positioned ancestor (or the page) and drawn over the flow; it
/// takes no room. Without offsets it stays where the flow would have put it.
#[allow(clippy::too_many_arguments)]
fn abs_box(g: &mut Gfx, b: &iyakki_web::doc::BoxBlock, env: &Env, ox: f32, width: f32, y: f32, ctx: &Ctx, sink: &mut Sink) {
    let s = env.s;
    let st = &b.style;
    let off = st.fx.off.map(|o| o.map(|v| v * s));
    let (ax, ay, aw) = sink.anc.last().map_or((0.0, 0.0, width), |a| (a.x, a.y, a.w));
    let (ml, mr) = (st.margin[3].max(0.0) * s, st.margin[1].max(0.0) * s);
    let avail = (aw - off[3].unwrap_or(0.0) - off[1].unwrap_or(0.0) - ml - mr).max(60.0);
    let mut bw = shrink_width(g, b, env, ctx, avail, aw);
    if matches!(st.width, Length::Auto) && off[3].is_some() && off[1].is_some() {
        // both sides given: the box spans between them
        bw = avail;
    }
    let outer = bw + ml + mr;
    let x = match (off[3], off[1]) {
        (Some(l), _) => ax + l,
        (None, Some(r)) => ax + aw - r - outer,
        _ => ox,
    };
    let y_top = match (off[0], off[2]) {
        (Some(t), _) => ay + t,
        (None, Some(_)) => ay,
        _ => y,
    };
    let mut st2 = st.clone();
    st2.width = Length::Px(bw / s);
    st2.auto_left = false;
    st2.auto_right = false;
    st2.max_width = Length::Auto;
    st2.min_width = Length::Auto;
    let held = std::mem::take(&mut sink.floats);
    let held_top = std::mem::replace(&mut sink.float_top, 0.0);
    let (p0, f0, l0) = (sink.prims.len(), sink.focus.len(), sink.layers.len());
    let mut pm = 0.0;
    let out = box_block(g, &st2, &b.children, env, x, outer, y_top, ctx, &mut pm, sink, None);
    sink.floats = held;
    sink.float_top = held_top;
    let prims: Vec<Prim> = sink.prims.drain(p0..).collect();
    let focus = sink.focus[f0..].to_vec();
    let bottom = if off[0].is_none() { off[2].map(|bo| (bo, y_top, out.y_end - y_top)) } else { None };
    let idx = sink.layers.len();
    let hangs = bottom.is_some();
    sink.layers.push(Layer { z: st.fx.z, prims, focus, bottom, inner: l0..idx });
    if hangs {
        if let Some(a) = sink.anc.last_mut() {
            a.hanging.push(idx);
        }
    }
}

/// Make a box taller after the fact (see [`Stretch`]).
fn stretch(prims: &mut [Prim], out: &BoxOut, to: f32) {
    let extra = to - out.height;
    if extra <= 0.5 {
        return;
    }
    for (i, k) in &out.decor {
        match k {
            Stretch::Grow => prims[*i].h += extra,
            Stretch::Shift => prims[*i].y += extra,
        }
    }
}

/// The width a run of blocks would take on one line.
fn intrinsic(g: &mut Gfx, list: &[Block], env: &Env, ctx: &Ctx, cap: f32) -> f32 {
    let s = env.s;
    let base = 16.0 * s;
    let mut best = 0.0f32;
    for b in list {
        let w = match b {
            Block::Text(t) => {
                let (size, bold) = if t.level > 0 { (heading_size(t.level, s), true) } else { (base, false) };
                let toks = tokens(g, &t.spans, size, bold, ctx, env, cap);
                let space = g.text_width(size, " ", Face::Sans);
                let mut w = 0.0;
                let mut line = 0.0f32;
                for tk in &toks {
                    match tk {
                        Tok::Word(wd) => line += wd.w + if wd.space_before && line > 0.0 { space } else { 0.0 },
                        Tok::Control { w, space_before, m, .. } | Tok::Inline { w, space_before, m, .. } => line += w + m[1] + m[3] + if *space_before && line > 0.0 { space } else { 0.0 },
                        Tok::Image { w, space_before, .. } => line += w + if *space_before && line > 0.0 { space } else { 0.0 },
                        Tok::Gap { w, .. } | Tok::FloatImg { w, .. } => line += w,
                        Tok::Break => {
                            w = f32::max(w, line);
                            line = 0.0;
                        }
                    }
                }
                f32::max(w, line) + t.indent as f32 * 24.0 * s + if t.marker.is_empty() { 0.0 } else { 30.0 * s }
            }
            Block::Pre(text) => text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32 * 14.0 * s * 0.6 + 20.0 * s,
            Block::Rule => 0.0,
            Block::Table(t) => match px(t.width, s, cap) {
                Some(w) if matches!(t.width, Length::Px(_)) => w,
                _ => {
                    let grid = grid_of(t);
                    let cols = table_cols(g, t, &grid, env, ctx, cap, false, true);
                    auto_width(&cols, t.spacing * s * (grid.ncols as f32 + 1.0))
                }
            },
            Block::Box(b) => {
                let inner = intrinsic(g, &b.children, env, ctx, cap);
                let st = &b.style;
                let frame = (st.padding[1] + st.padding[3] + st.border_w[1] + st.border_w[3] + st.margin[1].max(0.0) + st.margin[3].max(0.0)) * s;
                px(st.width, s, cap).unwrap_or(inner + frame)
            }
            Block::Row(r) => {
                let sum: f32 = r.cols.iter().map(|c| col_basis(g, c, env, ctx, cap)).sum();
                sum + r.gap * s * (r.cols.len().saturating_sub(1)) as f32
            }
        };
        best = best.max(w);
    }
    best.min(cap)
}

/// The narrowest a run of blocks can be without breaking a word: its longest word.
fn min_content(g: &mut Gfx, list: &[Block], env: &Env, ctx: &Ctx) -> f32 {
    let s = env.s;
    let mut best = 0.0f32;
    for b in list {
        let w = match b {
            Block::Text(t) => {
                let (size, bold) = if t.level > 0 { (heading_size(t.level, s), true) } else { (16.0 * s, false) };
                let toks = tokens(g, &t.spans, size, bold, ctx, env, 1e6);
                // a line can break between boxes, words and pictures: the widest one alone is the least it needs
                let mut best = 0.0f32;
                for tk in &toks {
                    best = best.max(match tk {
                        Tok::Word(w) => w.w,
                        Tok::Control { idx, w, m, .. } => {
                            let fixed = env.controls.get(*idx).is_some_and(|c| matches!(c.width, Length::Px(_)));
                            (if fixed { *w } else { w.min(120.0 * s) }) + m[1] + m[3]
                        }
                        Tok::Image { w, .. } | Tok::FloatImg { w, .. } => w.min(120.0 * s),
                        Tok::Inline { idx, m, .. } => inline_min(g, *idx, env, ctx) + m[1] + m[3],
                        Tok::Gap { .. } | Tok::Break => 0.0,
                    });
                }
                best
            }
            Block::Table(t) => {
                let grid = grid_of(t);
                let cols = table_cols(g, t, &grid, env, ctx, 1e6, true, false);
                cols.min.iter().sum::<f32>() + t.spacing * s * (grid.ncols as f32 + 1.0)
            }
            Block::Box(b) => {
                let st = &b.style;
                min_content(g, &b.children, env, ctx) + (st.padding[1] + st.padding[3] + st.border_w[1] + st.border_w[3]) * s
            }
            Block::Row(r) => r.cols.iter().map(|c| min_content(g, &c.blocks, env, ctx)).fold(0.0, f32::max),
            _ => 0.0,
        };
        best = best.max(w);
    }
    best
}

fn col_basis(g: &mut Gfx, c: &iyakki_web::doc::Column, env: &Env, ctx: &Ctx, cap: f32) -> f32 {
    if let Some(w) = px(c.basis, env.s, cap) {
        return w.min(cap);
    }
    let st = &c.style;
    let frame = (st.padding[1] + st.padding[3] + st.border_w[1] + st.border_w[3] + st.margin[1].max(0.0) + st.margin[3].max(0.0)) * env.s;
    (intrinsic(g, &c.blocks, env, ctx, cap) + frame + 2.0).min(cap)
}

/// A flex item's `width`, `min-width` and `max-width` given as percentages (or `calc`) refer to the row it sits in, but
/// the column is laid out in its slot of that row; the same style with those lengths turned into pixels of the row.
/// `None` when nothing needs changing.
fn row_relative_style(st: &BoxStyle, row_w: f32, s: f32) -> Option<BoxStyle> {
    let relative = |l: Length| !matches!(l, Length::Px(_) | Length::Auto);
    if !(relative(st.width) || relative(st.max_width) || relative(st.min_width)) {
        return None;
    }
    let fix = |l: Length| if relative(l) { px(l, s, row_w).map_or(Length::Auto, |v| Length::Px(v / s)) } else { l };
    let mut o = st.clone();
    o.width = fix(st.width);
    o.max_width = fix(st.max_width);
    o.min_width = fix(st.min_width);
    Some(o)
}

/// Columns side by side: a flex row (each column as wide as its content, or as it asked, with spare room shared out
/// by `flex-grow` or by `justify-content`) or a grid (equal columns, as many rows as it takes).
#[allow(clippy::too_many_arguments)]
fn row_block(g: &mut Gfx, r: &RowBlock, env: &Env, ox: f32, width: f32, y: f32, ctx: &Ctx, prev_margin: &mut f32, sink: &mut Sink) -> f32 {
    let s = env.s;
    let gap = (r.gap * s).max(0.0);
    let row_gap = (r.row_gap * s).max(0.0);
    let n = r.cols.len();
    // which columns go on which line, and how wide each is
    let mut lines: Vec<Vec<(usize, f32)>> = Vec::new();
    let mut offsets_spread: Vec<(f32, f32)> = Vec::new(); // (start offset, extra gap) per line
    if r.grid {
        let per = if r.grid_cols > 0 {
            r.grid_cols as usize
        } else {
            let min = (r.grid_min * s).max(80.0 * s);
            (((width + gap) / (min + gap)).floor() as usize).clamp_safe(1, n.max(1))
        }
        .min(n.max(1));
        // never so many columns that each is too narrow to read
        let per = per.min((((width + gap) / (140.0 * s + gap)).floor() as usize).max(1));
        let w = ((width - gap * (per as f32 - 1.0)) / per as f32).max(40.0);
        for chunk in (0..n).collect::<Vec<_>>().chunks(per) {
            lines.push(chunk.iter().map(|&i| (i, w)).collect());
            offsets_spread.push((0.0, 0.0));
        }
    } else {
        let want: Vec<f32> = r.cols.iter().map(|c| col_basis(g, c, env, ctx, width).max(30.0 * s)).collect();
        // columns that would come out narrower than a readable line go on several lines instead
        let wrap = r.wrap || (n > 1 && (width - gap * (n as f32 - 1.0)) / (n as f32) < 150.0 * s && want.iter().sum::<f32>() + gap * (n as f32 - 1.0) > width);
        // greedy lines when wrapping; one line otherwise
        let mut cur: Vec<usize> = Vec::new();
        let mut used = 0.0;
        for (i, w) in want.iter().enumerate() {
            let add = w + if cur.is_empty() { 0.0 } else { gap };
            if wrap && !cur.is_empty() && used + add > width {
                lines.push(std::mem::take(&mut cur).into_iter().map(|k| (k, want[k])).collect());
                used = 0.0;
            }
            used += w + if cur.is_empty() { 0.0 } else { gap };
            cur.push(i);
        }
        if !cur.is_empty() {
            lines.push(cur.into_iter().map(|k| (k, want[k])).collect());
        }
        for line in lines.iter_mut() {
            let k = line.len() as f32;
            let total: f32 = line.iter().map(|(_, w)| *w).sum::<f32>() + gap * (k - 1.0);
            if total > width {
                // too wide: every column gives up the same share, but keeps something readable
                let room = (width - gap * (k - 1.0)).max(40.0 * k);
                let sum: f32 = line.iter().map(|(_, w)| *w).sum();
                // no column narrower than its longest word, as far as the room allows
                let floors: Vec<f32> = line.iter().map(|(i, _)| min_content(g, &r.cols[*i].blocks, env, ctx).max(40.0)).collect();
                let floor_sum: f32 = floors.iter().sum();
                let scale = if floor_sum >= room { room / floor_sum } else { 1.0 };
                let mut widths: Vec<f32> = line.iter().zip(&floors).map(|((_, w), f)| (*w * room / sum).max(f * scale)).collect();
                // what the floors took beyond their share comes out of the columns that have room to spare
                let over_by = widths.iter().sum::<f32>() - room;
                if over_by > 0.5 {
                    let spare: f32 = widths.iter().zip(&floors).map(|(w, f)| (w - f * scale).max(0.0)).sum();
                    if spare > 0.0 {
                        for (w, f) in widths.iter_mut().zip(&floors) {
                            *w -= (*w - f * scale).max(0.0) / spare * over_by;
                        }
                    }
                }
                for ((_, w), nw) in line.iter_mut().zip(widths) {
                    *w = nw;
                }
                offsets_spread.push((0.0, 0.0));
            } else {
                let free = width - total;
                let grow: f32 = line.iter().map(|(i, _)| r.cols[*i].grow).sum();
                if grow > 0.0 {
                    for (i, w) in line.iter_mut() {
                        *w += free * r.cols[*i].grow / grow;
                    }
                    offsets_spread.push((0.0, 0.0));
                } else {
                    offsets_spread.push(match r.justify {
                        Justify::Start => (0.0, 0.0),
                        Justify::Center => (free / 2.0, 0.0),
                        Justify::End => (free, 0.0),
                        Justify::Between if k > 1.0 => (0.0, free / (k - 1.0)),
                        Justify::Between => (0.0, 0.0),
                        Justify::Around => (free / (2.0 * k), free / k),
                    });
                }
            }
        }
    }
    let mut y = y;
    for (li, line) in lines.iter().enumerate() {
        let (mut x, extra) = offsets_spread[li];
        let first = sink.prims.len();
        let mut outs: Vec<(BoxOut, usize, usize)> = Vec::new();
        let mut bottom = y;
        for (i, w) in line {
            let col = &r.cols[*i];
            let start = sink.prims.len();
            let mut pm = 0.0;
            // percentages in a column's own style (`width:50%`, `max-width:50%`) are shares of the row, not of the column's slot
            let own = row_relative_style(&col.style, width, s);
            let out = box_block(g, own.as_ref().unwrap_or(&col.style), &col.blocks, env, ox + x, *w, y, ctx, &mut pm, sink, None);
            outs.push((out, start, sink.prims.len()));
            bottom = bottom.max(outs.last().map_or(y, |o| o.0.y_end));
            x += w + gap + extra;
        }
        // columns of a row are as tall as the tallest
        let tallest = outs.iter().map(|(o, _, _)| o.height + (o.top - y)).fold(0.0f32, f32::max);
        for (o, _, _) in &outs {
            stretch(&mut sink.prims[first..], &BoxOut { y_end: o.y_end, top: o.top, height: o.height, decor: o.decor.iter().map(|(i, k)| (i - first, *k)).collect() }, tallest - (o.top - y));
        }
        y = bottom + if li + 1 < lines.len() { row_gap } else { 0.0 };
    }
    *prev_margin = 0.0;
    y
}

/// How many of these characters (at least one) fit in `max_w` in the monospace face.
fn chars_that_fit(g: &mut Gfx, chars: &[char], size: f32, max_w: f32) -> usize {
    let (mut lo, mut hi) = (1usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if g.text_width(size, &chars[..mid].iter().collect::<String>(), Face::Mono) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

pub fn key_of(width: f32, s: f32) -> (i32, i32) {
    (width.round() as i32, (s * 100.0).round() as i32)
}

#[allow(clippy::too_many_arguments)]
fn text_block(g: &mut Gfx, t: &TextBlock, env: &Env, width: f32, y: f32, base: f32, ctx: &Ctx, sink: &mut Sink) -> f32 {
    let s = env.s;
    let (size, bold) = if t.level > 0 { (heading_size(t.level, s), true) } else { (base, false) };
    let indent = t.indent as f32 * 24.0 * s + if t.quote { 18.0 * s } else { 0.0 };
    let marker_w = if t.marker.is_empty() { 0.0 } else { 30.0 * s };
    let mut y = y;
    if t.level > 0 {
        y += size * 0.5;
    }
    let col_w = (width - indent - marker_w).max(if ctx.tight { 4.0 } else { 40.0 });
    let toks = tokens(g, &t.spans, size, bold, ctx, env, col_w);
    let first = sink.prims.len();
    let ox = sink.ox;
    if !t.marker.is_empty() {
        let mw = g.text_width(size, &t.marker, Face::Bold);
        sink.prims.push(Prim { x: ox + (indent + marker_w - 6.0 * s - mw).max(0.0), y, w: mw, h: size * 1.45, kind: Kind::Text { text: t.marker.clone(), size, face: Face::Bold, color: ctx.readable(MUTED), link: None } });
    }
    let h = flow(g, env, ctx, toks, indent + marker_w, y, col_w, size, t.align, sink);
    if t.quote {
        sink.prims.insert(first, Prim { x: ox + 4.0 * s, y, w: 3.0 * s, h, kind: Kind::Rect { color: if ctx.light { rgb(170, 180, 205) } else { rgb(70, 90, 140) }, radius: 1.5 * s } });
    }
    let after = if ctx.tight { 0.0 } else if t.level > 0 { size * 0.3 } else if t.marker.is_empty() { base * 0.7 } else { base * 0.25 };
    y + h + after
}

/// A scratch sink for measuring: what is laid out in it is thrown away.
fn with_scratch<T>(w: f32, f: impl FnOnce(&mut Sink) -> T) -> T {
    let (mut prims, mut links, mut controls, mut focus) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut sink = Sink { prims: &mut prims, links: &mut links, controls: &mut controls, focus: &mut focus, ox: 0.0, floats: Vec::new(), float_top: 0.0, layers: Vec::new(), anc: vec![Anc { x: 0.0, y: 0.0, w, hanging: Vec::new() }] };
    f(&mut sink)
}

/// The style an inline box is laid out with: exactly `w` wide, its margins left to the line.
fn inline_style(ib: &InlineBox, w: f32, s: f32) -> BoxStyle {
    let mut st = ib.style.clone();
    st.margin = [0.0; 4];
    st.auto_left = false;
    st.auto_right = false;
    st.width = Length::Px(w / s);
    st.max_width = Length::Auto;
    st.min_width = Length::Auto;
    st.fx.float = Float::None;
    st.fx.clear = Clear::None;
    st
}

/// The height an inline box asked for (`height`, `min-height` in pixels), if any.
fn inline_min_h(ib: &InlineBox, s: f32) -> Option<f32> {
    let px_of = |l: Length| if let Length::Px(p) = l { Some(p * s) } else { None };
    match (px_of(ib.height), px_of(ib.min_height)) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// An inline box's least width: its own, or what its content needs at the narrowest.
fn inline_min(g: &mut Gfx, i: usize, env: &Env, ctx: &Ctx) -> f32 {
    let Some(ib) = env.inlines.get(i) else { return 0.0 };
    let s = env.s;
    let st = &ib.style;
    if let Length::Px(p) = st.width {
        return p * s;
    }
    let depth = env.depth.get();
    if depth > 4 {
        return 40.0 * s;
    }
    env.depth.set(depth + 1);
    let w = min_content(g, &ib.children, env, ctx) + (st.padding[1] + st.padding[3] + st.border_w[1] + st.border_w[3]) * s;
    env.depth.set(depth);
    w
}

/// Border-box width and height of an inline box in a line `avail` wide, with its margins and vertical alignment: as wide
/// as its content (or the width it asked for), as tall as what is inside it (or the height it asked for).
fn inline_metrics(g: &mut Gfx, i: usize, env: &Env, ctx: &Ctx, avail: f32) -> (f32, f32, [f32; 4], VAlign) {
    let ib = &env.inlines[i];
    let s = env.s;
    let st = &ib.style;
    let m = st.margin.map(|v| v.max(0.0) * s);
    let frame = (st.padding[1] + st.padding[3]) * s + st.border_w[1].max(0.0) * s + st.border_w[3].max(0.0) * s;
    let room = (avail - m[1] - m[3]).max(40.0);
    let depth = env.depth.get();
    if depth > 4 {
        // nested too deep to measure twice: a plausible box
        return ((60.0 * s).min(room), inline_min_h(ib, s).unwrap_or(24.0 * s), m, ib.valign);
    }
    let explicit = px(st.width, s, avail).filter(|_| !matches!(st.width, Length::Auto));
    env.depth.set(depth + 1);
    let mut w = match explicit {
        Some(w) => w,
        None => intrinsic(g, &ib.children, env, ctx, room) + frame + 2.0,
    };
    if let Some(mx) = px(st.max_width, s, avail) {
        w = w.min(mx);
    }
    if let Some(mn) = px(st.min_width, s, avail) {
        w = w.max(mn);
    }
    w = w.min(room).max(frame + if explicit.is_some() { 1.0 } else { 0.0 });
    let styled = inline_style(ib, w, s);
    let tight = Ctx { tight: true, ..*ctx };
    let min_h = inline_min_h(ib, s);
    let h = with_scratch(w, |sink| {
        let mut pm = 0.0;
        box_block(g, &styled, &ib.children, env, 0.0, w, 0.0, &tight, &mut pm, sink, min_h).height
    });
    env.depth.set(depth);
    (w, h.max(min_h.unwrap_or(0.0)), m, ib.valign)
}

/// Lay an inline box out for real: `w` by `h` with its top left corner at (`x`, `y`).
#[allow(clippy::too_many_arguments)]
fn place_inline(g: &mut Gfx, ib: &InlineBox, env: &Env, ctx: &Ctx, x: f32, y: f32, w: f32, h: f32, sink: &mut Sink) {
    let styled = inline_style(ib, w, env.s);
    let tight = Ctx { tight: true, ..*ctx };
    // an atomic box is its own world: floats outside do not shorten its lines
    let held = std::mem::take(&mut sink.floats);
    let held_top = std::mem::replace(&mut sink.float_top, 0.0);
    let depth = env.depth.get();
    env.depth.set(depth + 1);
    let mut pm = 0.0;
    box_block(g, &styled, &ib.children, env, x, w, y, &tight, &mut pm, sink, Some(h));
    env.depth.set(depth);
    sink.floats = held;
    sink.float_top = held_top;
}

/// Which grid column each cell of each row starts in (cells above that span down take their columns), and how many
/// columns there are.
struct Grid {
    place: Vec<Vec<usize>>,
    ncols: usize,
}

fn grid_of(t: &Table) -> Grid {
    let mut occupied: Vec<u32> = Vec::new();
    let mut place = Vec::new();
    let mut ncols = 0;
    for row in &t.rows {
        let mut col = 0;
        let mut here = Vec::new();
        for c in row {
            while occupied.get(col).copied().unwrap_or(0) > 0 {
                col += 1;
            }
            here.push(col);
            let span = c.colspan.clamp(1, 100) as usize;
            if occupied.len() < col + span {
                occupied.resize(col + span, 0);
            }
            if c.rowspan > 1 {
                for o in &mut occupied[col..col + span] {
                    *o = c.rowspan.min(100);
                }
            }
            col += span;
            ncols = ncols.max(col);
        }
        for o in &mut occupied {
            *o = o.saturating_sub(1);
        }
        place.push(here);
    }
    Grid { place, ncols: ncols.clamp(1, 200) }
}

/// What the columns of a table need: the least and the most width of each, and the share or size the page asked for.
struct Cols {
    min: Vec<f32>,
    max: Vec<f32>,
    /// Percent of the table (0: none) and pixels (0: none).
    pct: Vec<f32>,
    fix: Vec<f32>,
}

#[allow(clippy::too_many_arguments)]
fn table_cols(g: &mut Gfx, t: &Table, grid: &Grid, env: &Env, ctx: &Ctx, cap: f32, need_min: bool, need_max: bool) -> Cols {
    let s = env.s;
    let n = grid.ncols;
    let mut cols = Cols { min: vec![0.0; n], max: vec![0.0; n], pct: vec![0.0; n], fix: vec![0.0; n] };
    let tight = Ctx { tight: true, ..*ctx };
    // cells that span columns are shared out after the single ones
    let mut spanning: Vec<(usize, usize, f32, f32)> = Vec::new();
    for (ri, row) in t.rows.iter().enumerate() {
        for (ci, cell) in row.iter().enumerate() {
            let c0 = grid.place[ri][ci];
            let span = (cell.colspan.clamp(1, 100) as usize).min(n - c0);
            let pad = (cell.pad[1] + cell.pad[3]) * s;
            let mx = if need_max || cell.nowrap && need_min { intrinsic(g, &cell.blocks, env, &tight, cap) + pad + if cell.blocks.is_empty() { 0.0 } else { 2.0 } } else { 0.0 };
            let mn = if cell.nowrap { mx } else if need_min { min_content(g, &cell.blocks, env, &tight) + pad } else { 0.0 };
            if span == 1 {
                cols.min[c0] = cols.min[c0].max(mn);
                cols.max[c0] = cols.max[c0].max(mx.max(mn));
                match cell.width {
                    Length::Percent(p) if p > 0.0 => cols.pct[c0] = cols.pct[c0].max(p),
                    Length::Px(v) if v > 0.0 => cols.fix[c0] = cols.fix[c0].max(v * s),
                    _ => {}
                }
            } else {
                spanning.push((c0, span, mn, mx.max(mn)));
            }
        }
    }
    for (c0, span, mn, mx) in spanning {
        let (have_min, have_max): (f32, f32) = (cols.min[c0..c0 + span].iter().sum(), cols.max[c0..c0 + span].iter().sum());
        for k in c0..c0 + span {
            if mn > have_min {
                cols.min[k] += (mn - have_min) / span as f32;
            }
            if mx > have_max {
                cols.max[k] += (mx - have_max) / span as f32;
            }
        }
    }
    cols
}

/// How wide a table is when its page did not say: as wide as its columns want, and wide enough that columns that asked
/// for a share of it get that share of what the others need.
fn auto_width(cols: &Cols, spacing_total: f32) -> f32 {
    let n = cols.max.len();
    let share: f32 = cols.pct.iter().sum();
    let (mut rest, mut from_pct, mut all) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        let want = if cols.fix[i] > 0.0 { cols.fix[i].max(cols.min[i]) } else { cols.max[i] };
        all += want;
        if cols.pct[i] > 0.0 {
            from_pct = from_pct.max(want / (cols.pct[i] / 100.0));
        } else {
            rest += want;
        }
    }
    let mut w = all + spacing_total;
    if share > 0.0 && share < 100.0 {
        w = w.max((rest + spacing_total) / (1.0 - share / 100.0)).max(from_pct);
    }
    w
}

/// Move what was laid out in these ranges (a table cell's content).
fn shift_range(sink: &mut Sink, prims: std::ops::Range<usize>, focus: std::ops::Range<usize>, layers: std::ops::Range<usize>, dy: f32) {
    if dy == 0.0 {
        return;
    }
    for p in sink.prims[prims].iter_mut() {
        p.y += dy;
    }
    let f: Vec<Focus> = sink.focus[focus].to_vec();
    shift_focus(sink, &f, 0.0, dy);
    for k in layers {
        shift_one(sink, k, 0.0, dy);
    }
}

#[allow(clippy::too_many_arguments)]
fn table(g: &mut Gfx, t: &Table, env: &Env, width: f32, y: f32, base: f32, ctx: &Ctx, sink: &mut Sink) -> f32 {
    let s = env.s;
    let ox = sink.ox;
    let grid = grid_of(t);
    let n = grid.ncols;
    let spacing = t.spacing * s;
    let total_sp = spacing * (n as f32 + 1.0);
    let cols = table_cols(g, t, &grid, env, ctx, width, true, true);
    // the table's width: what it asked for, else what its columns want, never more than the column it is in
    let want = match t.width {
        Length::Auto => auto_width(&cols, total_sp),
        l => px(l, s, width).unwrap_or(width),
    };
    let tw = want.min(width).max(40.0);
    // each column's width: the asked-for ones first, the rest share what is left between their least and most
    let avail = (tw - total_sp).max(n as f32 * 4.0);
    let mut w: Vec<f32> = (0..n).map(|i| if cols.pct[i] > 0.0 { (cols.pct[i] / 100.0 * tw).max(cols.min[i]) } else if cols.fix[i] > 0.0 { cols.fix[i].max(cols.min[i]) } else { 0.0 }).collect();
    let auto: Vec<usize> = (0..n).filter(|&i| w[i] == 0.0).collect();
    let used: f32 = w.iter().sum();
    let left = avail - used;
    let (sum_min, sum_max): (f32, f32) = (auto.iter().map(|&i| cols.min[i]).sum(), auto.iter().map(|&i| cols.max[i]).sum());
    for &i in &auto {
        w[i] = if left >= sum_max {
            // spare room goes to the columns with the most to say
            cols.max[i] + if sum_max > 0.0 { (left - sum_max) * cols.max[i] / sum_max } else { (left - sum_max) / auto.len() as f32 }
        } else if left > sum_min && sum_max > sum_min {
            cols.min[i] + (cols.max[i] - cols.min[i]) * (left - sum_min) / (sum_max - sum_min)
        } else {
            cols.min[i]
        };
    }
    // too wide for the column: columns above their least give way first, then all shrink
    let total: f32 = w.iter().sum();
    if total > avail + 0.5 {
        let slack: f32 = (0..n).map(|i| (w[i] - cols.min[i]).max(0.0)).sum();
        let take = (total - avail).min(slack);
        if slack > 0.0 {
            for i in 0..n {
                w[i] -= (w[i] - cols.min[i]).max(0.0) * take / slack;
            }
        }
        let total: f32 = w.iter().sum();
        if total > avail + 0.5 {
            let k = avail / total;
            w.iter_mut().for_each(|v| *v *= k);
        }
    }
    let table_w = w.iter().sum::<f32>() + total_sp;
    let left_edge = ox + match t.align {
        Align::Left => 0.0,
        Align::Center => ((width - table_w) / 2.0).max(0.0),
        Align::Right => (width - table_w).max(0.0),
    };
    let mut xs = Vec::with_capacity(n);
    let mut x = left_edge + spacing;
    for v in &w {
        xs.push(x);
        x += v + spacing;
    }
    // text on a table's (or a cell's) own background is read against that background
    let on = |bg: Option<[u8; 4]>, under: &Ctx| match bg {
        Some(b) => Ctx { tight: true, ..Ctx::on(over(b, under.bg), false) },
        None => Ctx { tight: true, ..*under },
    };
    let tight = on(t.bg, ctx);
    let line = if ctx.light { rgb(214, 218, 228) } else { rgb(44, 52, 80) };
    let first = sink.prims.len();
    let held = std::mem::take(&mut sink.floats);
    let held_top = std::mem::replace(&mut sink.float_top, 0.0);
    let top = y;
    let mut y = y + spacing;
    for (ri, row) in t.rows.iter().enumerate() {
        let row_first = sink.prims.len();
        // lay each cell out against the top of the row, then move the ones that are not as tall as the row
        let mut placed: Vec<(usize, f32, std::ops::Range<usize>, std::ops::Range<usize>, std::ops::Range<usize>)> = Vec::new();
        let mut row_h = 0.0f32;
        for (ci, cell) in row.iter().enumerate() {
            let c0 = grid.place[ri][ci];
            let span = (cell.colspan.clamp(1, 100) as usize).min(n - c0);
            let cw: f32 = w[c0..c0 + span].iter().sum::<f32>() + spacing * (span as f32 - 1.0);
            let pad = cell.pad.map(|v| v * s);
            let (p0, f0, l0) = (sink.prims.len(), sink.focus.len(), sink.layers.len());
            let mut pm = 0.0;
            let inner_w = (cw - pad[1] - pad[3]).max(4.0);
            let cell_ctx = on(cell.bg, &tight);
            let end = blocks(g, &cell.blocks, env, xs[c0] + pad[3], inner_w, y + pad[0], &cell_ctx, &mut pm, sink);
            let h = (end - y - pad[0]).max(0.0) + pad[0] + pad[2];
            row_h = row_h.max(h);
            placed.push((ci, h, p0..sink.prims.len(), f0..sink.focus.len(), l0..sink.layers.len()));
        }
        row_h = row_h.max(base * 0.2);
        let mut fills: Vec<Prim> = Vec::new();
        for (ci, h, pr, fr, lr) in placed {
            let cell = &row[ci];
            let dy = match cell.valign {
                VAlign::Top => 0.0,
                VAlign::Middle => (row_h - h) / 2.0,
                VAlign::Bottom => row_h - h,
            };
            shift_range(sink, pr, fr, lr, dy);
            if let Some(bg) = cell.bg {
                let c0 = grid.place[ri][ci];
                let span = (cell.colspan.clamp(1, 100) as usize).min(n - c0);
                let cw: f32 = w[c0..c0 + span].iter().sum::<f32>() + spacing * (span as f32 - 1.0);
                fills.push(Prim { x: xs[c0], y, w: cw, h: row_h, kind: Kind::Rect { color: ctx.readable_fill(bg), radius: 0.0 } });
            }
        }
        for (k, p) in fills.into_iter().enumerate() {
            sink.prims.insert(row_first + k, p);
        }
        y += row_h;
        if t.lines {
            sink.prims.push(Prim { x: left_edge, y: y + spacing / 2.0, w: table_w, h: (1.0 * s).max(1.0), kind: Kind::Rect { color: line, radius: 0.0 } });
        }
        y += spacing;
    }
    sink.floats = held;
    sink.float_top = held_top;
    if let Some(bg) = t.bg {
        sink.prims.insert(first, Prim { x: left_edge, y: top, w: table_w, h: y - top, kind: Kind::Rect { color: bg, radius: 0.0 } });
    }
    y + if ctx.tight { 0.0 } else if t.lines { base * 0.6 } else { base * 0.3 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_web::css::{parse_sheet, Viewport};
    use iyakki_web::style::StyleSheets;

    fn lay(html: &str, css: &str, width: f32) -> Layout {
        let tree = iyakki_web::html::parse(html);
        let vp = Viewport::default();
        let mut n = 0;
        let sheet = parse_sheet(css, &vp, &mut n);
        let doc = iyakki_web::doc::build_styled(&tree, &StyleSheets::new(&[sheet]), &vp);
        let infos = vec![ImgInfo { size: Some((100, 100)), failed: false }; doc.images.len()];
        let mut g = Gfx::new(100, 100);
        layout(&mut g, &doc, width, 1.0, &infos, 0)
    }

    /// Every word, as (text, x, y, width).
    fn words(l: &Layout) -> Vec<(String, f32, f32, f32)> {
        l.prims.iter().filter_map(|p| if let Kind::Text { text, .. } = &p.kind { Some((text.clone(), p.x, p.y, p.w)) } else { None }).collect()
    }

    fn word(l: &Layout, w: &str) -> (f32, f32, f32) {
        words(l).into_iter().find(|t| t.0 == w).map(|t| (t.1, t.2, t.3)).unwrap_or_else(|| panic!("no word {w}"))
    }

    fn rects(l: &Layout, color: [u8; 4]) -> Vec<(f32, f32, f32, f32)> {
        l.prims.iter().filter_map(|p| match p.kind {
            Kind::Rect { color: c, .. } if c == color => Some((p.x, p.y, p.w, p.h)),
            _ => None,
        }).collect()
    }

    fn lorem(n: usize) -> String {
        (0..n).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ")
    }

    /// The page, its `<style>` text applied: the document and its layout.
    fn lay_page(html: &str, width: f32) -> (Doc, Layout) {
        let tree = iyakki_web::html::parse(html);
        let vp = Viewport::default();
        let mut css = String::new();
        let mut rest = html;
        while let Some(i) = rest.find("<style") {
            let body = &rest[i..];
            let start = body.find('>').unwrap() + 1;
            let end = body.find("</style>").unwrap();
            css.push_str(&body[start..end]);
            css.push('\n');
            rest = &body[end + 8..];
        }
        let mut n = 0;
        let sheet = parse_sheet(&css, &vp, &mut n);
        let doc = iyakki_web::doc::build_styled(&tree, &StyleSheets::new(&[sheet]), &vp);
        let infos = vec![ImgInfo { size: Some((100, 100)), failed: false }; doc.images.len()];
        let mut g = Gfx::new(100, 100);
        let l = layout(&mut g, &doc, width, 1.0, &infos, 0);
        (doc, l)
    }

    fn control_rect(doc: &Doc, l: &Layout, name: &str) -> R {
        let i = doc.controls.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no control {name}"));
        l.controls[i].unwrap_or_else(|| panic!("control {name} not placed"))
    }

    const GOOGLE: &str = include_str!("testdata/google_home.html");

    #[test]
    fn two_half_width_flex_columns_each_get_half_the_row_whatever_their_padding() {
        let page = "<style>*{box-sizing:border-box} .card{margin:0 200px;padding:0 36px;background:#ffffff} .main{display:flex;flex-direction:row;flex-wrap:wrap} \
            .l{flex-basis:50%;flex-grow:1;max-width:50%;padding-right:24px} .r{flex-basis:50%;flex-grow:1;max-width:50%;padding-left:24px} h1{font-size:36px;margin:24px 0 0}</style>\
            <div class=card><main class=main><div class=l><c-wiz><div>logo</div></c-wiz><h1>Sign in</h1><div>Use your Google Account</div></div>\
            <div class=r><div><div><input name=identifier></div><button>Forgot email?</button><p>Not your computer? Use Guest mode to sign in privately.</p></div></div></div></main></div>";
        let (doc, l) = lay_page(page, 940.0);
        let (sx, sy, _) = word(&l, "Sign");
        let (ix, iy, _) = word(&l, "in");
        assert!(ix > sx && (iy - sy).abs() < 1.0, "the heading stays on one line in a 210px column: {sx},{sy} {ix},{iy}");
        let (ux, uy, _) = word(&l, "Use");
        let (gx, gy, _) = word(&l, "Google");
        assert!(gx > ux && (gy - uy).abs() < 1.0, "Use your Google fits a column half of 468px wide: {ux},{uy} {gx},{gy}");
        let field = control_rect(&doc, &l, "identifier");
        assert!(field.x >= 236.0 + 234.0 + 20.0 && field.w >= 150.0, "the right column starts at the middle and the field is wide: {field:?}");
    }

    /// Developer probe: `IYAKKI_PAGE=/path/page.html cargo test -p iyakki-shell --lib layout_probe -- --ignored --nocapture`
    /// prints where the words of a saved page land.
    #[test]
    #[ignore]
    fn layout_probe() {
        let Ok(path) = std::env::var("IYAKKI_PAGE") else { return };
        let html = std::fs::read_to_string(path).unwrap();
        let w: f32 = std::env::var("IYAKKI_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1263.0);
        let l = if std::env::var("IYAKKI_STYLED").is_ok() {
            let tree = iyakki_web::html::parse(&html);
            let u = iyakki_web::Url::parse("https://accounts.google.com/").unwrap();
            let doc = iyakki_web::style::styled_doc(&tree, &u, &Viewport::default(), &|_: &iyakki_web::Url| None);
            let infos = vec![ImgInfo { size: Some((100, 100)), failed: false }; doc.images.len()];
            let mut g = Gfx::new(100, 100);
            layout(&mut g, &doc, w, 1.0, &infos, 0)
        } else {
            lay_page(&html, w).1
        };
        for (t, x, y, tw) in words(&l).into_iter().take(40) {
            eprintln!("{t:<14} x={x:>7.1} y={y:>7.1} w={tw:>6.1}");
        }
    }

    #[test]
    fn google_home_search_buttons_sit_side_by_side_and_the_box_is_as_wide_as_the_page_says() {
        let (doc, l) = lay_page(GOOGLE, 1200.0);
        let (q, g, lucky) = (control_rect(&doc, &l, "q"), control_rect(&doc, &l, "btnG"), control_rect(&doc, &l, "btnI"));
        assert!((g.y - lucky.y).abs() < 1.5, "one row: {g:?} {lucky:?}");
        assert!(lucky.x >= g.x + g.w, "left to right: {g:?} {lucky:?}");
        assert!((q.w - 496.0).abs() < 2.0, ".lst{{width:496px}}: {}", q.w);
        assert!(g.y > q.y + q.h - 2.0, "the buttons are under the box");
        // the box is in the middle cell of the three-column table with 25% sides: centred on the page
        assert!((q.x + q.w / 2.0 - 600.0).abs() < 20.0, "centred: {}", q.x + q.w / 2.0);
        let (adv_x, adv_y, _) = word(&l, "Advanced");
        assert!(adv_x > q.x + q.w && adv_y < q.y + q.h + 12.0 && adv_y > q.y - 12.0, "the link is in the right cell, beside the box: {adv_x} {adv_y}");
    }

    #[test]
    fn google_home_logo_is_centred_and_the_footer_links_have_room_between_them() {
        let (_, l) = lay_page(GOOGLE, 1200.0);
        let logo = l.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).expect("the logo");
        assert!((logo.w - 272.0).abs() < 1.0 && (logo.x + logo.w / 2.0 - 600.0).abs() < 2.0, "centred above the box: {} {}", logo.x, logo.w);
        let (ax, _, aw) = word(&l, "Advertising");
        let (bx, by, _) = word(&l, "Business");
        let (cx, cy, _) = word(&l, "About");
        let (_, ay, _) = word(&l, "Advertising");
        assert!(bx - (ax + aw) >= 20.0, "margin:0 12px on both sides: {}", bx - (ax + aw));
        assert!((ay - by).abs() < 1.0 && (by - cy).abs() < 1.0 && cx > bx, "one row");
    }

    #[test]
    fn google_home_header_links_share_a_row_and_sign_in_is_a_pill() {
        let (_, l) = lay_page(GOOGLE, 1200.0);
        let (gx, gy, _) = word(&l, "Gmail");
        let (ix, iy, _) = word(&l, "Images");
        assert!((gy - iy).abs() < 1.0 && ix > gx + 20.0, "Gmail and Images in a row: {gy} {iy} {gx} {ix}");
        let pill = rects(&l, [0x0b, 0x57, 0xd0, 255]);
        assert_eq!(pill.len(), 1, "{pill:?}");
        let (_, _, tw) = word(&l, "Sign");
        assert!(pill[0].2 >= tw + 24.0 && pill[0].2 >= 85.0 && pill[0].3 >= 40.0, "padding:10px 12px, min-width:85px, min-height:40px: {:?}", pill[0]);
        let (sx, sy, _) = word(&l, "Sign");
        assert!(sx >= pill[0].0 + 12.0 && sy > pill[0].1 && sy < pill[0].1 + pill[0].3, "the words are inside the pill: {sx} {sy} {:?}", pill[0]);
    }

    #[test]
    fn inline_blocks_sit_in_the_line_with_their_margins_padding_and_background() {
        let css = ".b{display:inline-block;margin:0 10px;padding:4px 12px;background:#ff0000}";
        let l = lay("<p><span class=b>one</span><span class=b>two</span> after</p>", css, 600.0);
        let boxes = rects(&l, [255, 0, 0, 255]);
        assert_eq!(boxes.len(), 2);
        assert!((boxes[0].1 - boxes[1].1).abs() < 0.5, "same row");
        assert!(boxes[1].0 - (boxes[0].0 + boxes[0].2) >= 20.0 - 0.5, "20px of margin between: {boxes:?}");
        let (x, _, w) = word(&l, "one");
        assert!((boxes[0].2 - (w + 24.0)).abs() < 4.0 && x >= boxes[0].0 + 12.0, "padding around the words: {boxes:?} {x}");
        assert!(word(&l, "after").0 > boxes[1].0 + boxes[1].2, "text goes on after");
    }

    #[test]
    fn inline_blocks_wrap_to_the_next_line_when_they_do_not_fit_and_centre_with_text_align() {
        let css = ".b{display:inline-block;width:100px;height:30px;background:#00ff00} .c{text-align:center}";
        let l = lay("<div class=c><span class=b></span><span class=b></span><span class=b></span></div>", css, 250.0);
        let b = rects(&l, [0, 255, 0, 255]);
        assert_eq!(b.len(), 3);
        assert!((b[0].1 - b[1].1).abs() < 0.5 && b[2].1 >= b[0].1 + 30.0, "two fit, the third wraps: {b:?}");
        assert!((b[0].2 - 100.0).abs() < 0.5 && (b[0].3 - 30.0).abs() < 0.5, "width and height honoured: {b:?}");
        let left = b[0].0.min(b[1].0);
        let right = (b[0].0 + b[0].2).max(b[1].0 + b[1].2);
        assert!((left + right) / 2.0 - 125.0 < 1.0 && ((left + right) / 2.0 - 125.0).abs() < 1.0, "the full line is centred: {b:?}");
        assert!((b[2].0 + 50.0 - 125.0).abs() < 1.0, "so is the lone one: {b:?}");
    }

    #[test]
    fn inline_block_has_its_blocks_inside_and_takes_the_widest_of_them() {
        let css = ".n{display:inline-block;vertical-align:top;border:1px solid #0000ff} .m{display:inline-block;vertical-align:top;border:1px solid #0000ff}";
        let l = lay("<div><span class=n><p>short</p><p>a much longer second paragraph</p></span><span class=m>x</span></div>", css, 600.0);
        let (_, y1, _) = word(&l, "short");
        let (_, y2, _) = word(&l, "a");
        assert!(y2 > y1 + 10.0, "the paragraphs inside stack");
        let (xs, ys, _) = word(&l, "x");
        let (xl, _, _) = word(&l, "second");
        assert!(xs > xl, "the next box is to the right of the whole first one");
        assert!(ys <= y1 + 1.0, "vertical-align:top lines them up at the top");
    }

    #[test]
    fn inline_padding_and_margin_leave_room_and_the_background_covers_the_padding() {
        let css = ".t{padding:0 10px;margin:0 6px;background:#ffff00}";
        let l = lay("<p>aa<span class=t>bb</span>cc</p>", css, 600.0);
        let plain = lay("<p>aa<span>bb</span>cc</p>", "", 600.0);
        let (cx, _, _) = word(&l, "cc");
        let (px_, _, _) = word(&plain, "cc");
        assert!((cx - px_ - 32.0).abs() < 1.5, "10+10 padding and 6+6 margin: {}", cx - px_);
        let hl = rects(&l, [255, 255, 0, 255]);
        let (bx, _, bw) = word(&l, "bb");
        let (from, to) = (hl.iter().map(|r| r.0).fold(f32::MAX, f32::min), hl.iter().map(|r| r.0 + r.2).fold(0.0, f32::max));
        assert!(from <= bx - 9.0 && to >= bx + bw + 9.0, "highlight spans the padding: {hl:?}");
    }

    #[test]
    fn form_controls_take_a_size_from_style_sheets_and_flow_in_lines() {
        let l = lay_page("<style>.w{width:300px;height:40px}</style><form><input class=w name=a><input name=b size=30><input type=submit value=Go name=c></form>", 800.0);
        let (doc, l) = l;
        let (a, b, c) = (control_rect(&doc, &l, "a"), control_rect(&doc, &l, "b"), control_rect(&doc, &l, "c"));
        assert!((a.w - 300.0).abs() < 1.0 && (a.h - 40.0).abs() < 1.0, "{a:?}");
        assert!(b.w > 200.0, "size=30 is wider than the default: {}", b.w);
        assert!((a.y + a.h / 2.0 - (b.y + b.h / 2.0)).abs() < 3.0 || b.y > a.y, "fields are inline: {a:?} {b:?}");
        assert!(c.x > b.x || c.y > b.y);
    }

    #[test]
    fn pictures_in_centred_text_are_centred() {
        let l = lay("<center><img src=a.png width=100 height=50></center>", "", 600.0);
        let img = l.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).unwrap();
        assert!((img.x + img.w / 2.0 - 300.0).abs() < 1.0, "{}", img.x);
        let l = lay("<div style=\"text-align:center\"><p><img src=a.png width=100 height=50> <img src=a.png width=100 height=50></p></div>", "", 600.0);
        let xs: Vec<f32> = l.prims.iter().filter(|p| matches!(p.kind, Kind::Image { .. })).map(|p| p.x).collect();
        assert!(xs[0] > 100.0 && xs[1] > xs[0], "{xs:?}");
        let l = lay("<div align=center><img src=a.png width=100 height=50></div>", "", 600.0);
        let img = l.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).unwrap();
        assert!((img.x + img.w / 2.0 - 300.0).abs() < 1.0, "align=center: {}", img.x);
    }

    #[test]
    fn tables_size_columns_to_their_content_and_honour_percent_widths_and_alignment() {
        let l = lay("<table width=100%><tr><td width=25%>left<td align=center>middle<td width=25% align=right>right</table>", "", 800.0);
        let (lx, _, _) = word(&l, "left");
        let (mx, _, mw) = word(&l, "middle");
        let (rx, _, rw) = word(&l, "right");
        assert!(lx < 20.0 && (mx + mw / 2.0 - 400.0).abs() < 12.0 && (rx + rw - 800.0).abs() < 12.0, "{lx} {mx} {rx}");
        let narrow = lay("<center><table><tr><td>a<td>b</table></center>", "", 800.0);
        let (ax, _, _) = word(&narrow, "a");
        let (bx, _, _) = word(&narrow, "b");
        assert!(ax > 300.0 && bx - ax < 100.0, "a table with no width shrinks to its content and the centre tag centres it: {ax} {bx}");
        let nested = lay("<table><tr><td><table><tr><td>in1<tr><td>in2</table><td>side</table>", "", 800.0);
        assert!(word(&nested, "in2").1 > word(&nested, "in1").1 + 5.0, "a table inside a cell is laid out as rows");
        let span = lay("<table border=1 width=300><tr><td colspan=2>wide<tr><td>l<td>r</table>", "", 800.0);
        assert!(word(&span, "r").0 > word(&span, "l").0 + 30.0);
    }

    #[test]
    fn grid_areas_put_children_in_the_columns_their_areas_name_whatever_the_source_order() {
        let css = ".g{display:grid;grid-template:auto auto / 100px 1fr;grid-template-areas:'top top' 'side main'} .t{grid-area:top} .s{grid-area:side} .m{grid-area:main}";
        let l = lay("<div class=g><div class=m>MAIN</div><div class=s>SIDE</div><div class=t>TOP</div></div>", css, 600.0);
        let (sx, sy, _) = word(&l, "SIDE");
        let (mx, my, _) = word(&l, "MAIN");
        let (tx, ty, _) = word(&l, "TOP");
        assert!(sx < 5.0 && mx >= 100.0 - 1.0 && (sy - my).abs() < 1.0, "side left, main right, one row: {sx} {mx} {sy} {my}");
        assert!(ty < sy && tx < 5.0, "the area that spans both columns is a row above: {ty} {sy}");
    }

    #[test]
    fn br_clear_all_starts_below_the_floats_and_old_markup_sets_alignment_and_colours() {
        let l = lay("<div style=\"float:right;width:100px;height:60px;background:#00ff00\"><p>f1</p><p>f2</p><p>f3</p></div><br clear=all><p>after</p>", "", 600.0);
        let f = rects(&l, [0, 255, 0, 255])[0];
        assert!(word(&l, "after").1 >= f.1 + f.3 - 1.0, "below the float: {} {:?}", word(&l, "after").1, f);
        let l = lay("<font color=#ff0000 size=5>red</font> <u>under</u> <p align=right>r</p><table bgcolor=#112233><tr><td>c</table>", "", 600.0);
        let colour = |w: &str| l.prims.iter().find_map(|p| if let Kind::Text { text, color, .. } = &p.kind { (text == w).then_some(*color) } else { None }).unwrap();
        assert_eq!(colour("red"), [255, 0, 0, 255]);
        let (rx, _, rw) = word(&l, "r");
        assert!((rx + rw - 600.0).abs() < 2.0, "align=right: {rx}");
        assert_eq!(rects(&l, [0x11, 0x22, 0x33, 255]).len(), 1, "bgcolor on a table paints it");
    }

    const SIDE: &str = ".f{float:left;width:150px;background:#336699} .r{float:right;width:150px;background:#996633}";
    const FOUR: &str = "<p>f1</p><p>f2</p><p>f3</p><p>f4</p>";

    #[test]
    fn a_left_float_takes_the_edge_and_text_wraps_beside_it_then_under_it() {
        let page = format!("<div class=f>{FOUR}</div><p>{}</p>", lorem(160));
        let l = lay(&page, SIDE, 600.0);
        let f = rects(&l, [0x33, 0x66, 0x99, 255])[0];
        assert!(f.0 < 1.0 && (f.2 - 150.0).abs() < 1.0, "float at the left edge: {f:?}");
        let (x0, y0, _) = word(&l, "w0");
        assert!(x0 >= 150.0, "the first line of text is beside the float: {x0}");
        assert!(y0 < f.1 + f.3, "and starts beside it");
        let ws = words(&l);
        let beside = ws.iter().filter(|w| w.0.starts_with('w') && w.2 < f.1 + f.3 - 20.0).count();
        assert!(beside > 3 && ws.iter().filter(|w| w.0.starts_with('w') && w.2 < f.1 + f.3 - 20.0).all(|w| w.1 >= 150.0), "{beside}");
        let under = ws.iter().filter(|w| w.0.starts_with('w') && w.2 >= f.1 + f.3 + 1.0).collect::<Vec<_>>();
        assert!(!under.is_empty() && under.iter().any(|w| w.1 < 5.0), "after the float's bottom the lines use the full width");
    }

    #[test]
    fn a_right_float_shortens_lines_from_the_right() {
        let page = format!("<div class=r>{FOUR}</div><p>{}</p>", lorem(160));
        let l = lay(&page, SIDE, 600.0);
        let f = rects(&l, [0x99, 0x66, 0x33, 255])[0];
        assert!((f.0 + f.2 - 600.0).abs() < 1.0, "float at the right edge: {f:?}");
        let beside: Vec<_> = words(&l).into_iter().filter(|w| w.0.starts_with('w') && w.2 < f.1 + f.3 - 20.0).collect();
        assert!(beside.len() > 3);
        assert!(beside.iter().all(|w| w.1 + w.3 <= 600.0 - 150.0 + 1.0), "text stops before the float");
        assert!(beside.iter().any(|w| w.1 < 5.0), "and starts at the left edge");
    }

    #[test]
    fn a_floated_picture_has_text_wrapping_around_it() {
        let page = format!("<p><img src=a.png width=100 height=100 style=\"float:left\">{}</p>", lorem(120));
        let l = lay(&page, "", 600.0);
        let img = l.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).expect("the picture");
        assert!(img.x < 1.0 && (img.w - 100.0).abs() < 1.0);
        let (x0, y0, _) = word(&l, "w0");
        assert!(x0 >= 100.0 && y0 < img.y + img.h, "text starts beside the picture: {x0}");
        let ws = words(&l);
        assert!(ws.iter().filter(|w| w.2 < img.y + img.h - 20.0).all(|w| w.1 >= 100.0), "every line beside it is shortened");
        assert!(ws.iter().any(|w| w.2 > img.y + img.h && w.1 < 5.0), "and lines below it are not");
        let right = lay(&format!("<p><img src=a.png width=100 height=100 style=\"float:right\">{}</p>", lorem(120)), "", 600.0);
        let img = right.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).unwrap();
        assert!((img.x + img.w - 600.0).abs() < 1.0, "right edge: {}", img.x + img.w);
    }

    #[test]
    fn clear_moves_a_block_below_the_floats() {
        let css = format!("{SIDE} .c{{clear:both}}");
        let free = lay(&format!("<div class=f>{FOUR}</div><div>after</div>"), &css, 600.0);
        let f = rects(&free, [0x33, 0x66, 0x99, 255])[0];
        assert!(word(&free, "after").1 < f.1 + f.3, "without clear it sits beside the float");
        let cleared = lay(&format!("<div class=f>{FOUR}</div><div class=c>after</div>"), &css, 600.0);
        let f = rects(&cleared, [0x33, 0x66, 0x99, 255])[0];
        assert!(word(&cleared, "after").1 >= f.1 + f.3 - 1.0 && word(&cleared, "after").0 < 5.0);
        let left_only = lay(&format!("<div class=r>{FOUR}</div><div style=\"clear:left\">after</div>"), &css, 600.0);
        let f = rects(&left_only, [0x99, 0x66, 0x33, 255])[0];
        assert!(word(&left_only, "after").1 < f.1 + f.3, "clear:left ignores a right float");
        assert!(left_only.height >= f.1 + f.3, "the page is as tall as its floats");
    }

    #[test]
    fn floats_never_overlap_and_a_float_that_does_not_fit_goes_under() {
        let css = ".a{float:left;width:200px;background:#ff0000} .b{float:left;width:200px;background:#00ff00} .c{float:left;width:300px;background:#0000ff}";
        let l = lay("<div class=a><p>one</p></div><div class=b><p>two</p></div><div class=c><p>three</p></div>", css, 600.0);
        let a = rects(&l, [255, 0, 0, 255])[0];
        let b = rects(&l, [0, 255, 0, 255])[0];
        let c = rects(&l, [0, 0, 255, 255])[0];
        assert!(b.0 >= a.0 + a.2 - 0.5 && (b.1 - a.1).abs() < 1.0, "the second sits beside the first: {a:?} {b:?}");
        assert!(c.1 >= a.1 + a.3.min(b.3) - 1.0, "no room left for 300 more: {c:?}");
        let overlap = |p: (f32, f32, f32, f32), q: (f32, f32, f32, f32)| p.0 < q.0 + q.2 - 0.5 && q.0 < p.0 + p.2 - 0.5 && p.1 < q.1 + q.3 - 0.5 && q.1 < p.1 + p.3 - 0.5;
        assert!(!overlap(a, b) && !overlap(a, c) && !overlap(b, c));
    }

    #[test]
    fn a_float_in_a_narrow_column_takes_a_line_of_its_own() {
        let css = ".f{float:left;width:180px;background:#336699}";
        let l = lay(&format!("<div class=f>{FOUR}</div><p>{}</p>", lorem(20)), css, 200.0);
        let f = rects(&l, [0x33, 0x66, 0x99, 255])[0];
        let (_, y0, _) = word(&l, "w0");
        assert!(y0 >= f.1 + f.3 - 1.0, "no room beside a 180px float in 200px: text at {y0}, float ends at {}", f.1 + f.3);
    }

    #[test]
    fn a_box_with_a_frame_holds_its_floats_and_one_without_lets_them_hang() {
        let css = ".f{float:left;width:100px;background:#336699} .frame{background:#eeeeee}";
        let held = lay(&format!("<div class=frame><div class=f>{FOUR}</div></div><p>next</p>"), css, 600.0);
        let frame = rects(&held, [0xee, 0xee, 0xee, 255])[0];
        let f = rects(&held, [0x33, 0x66, 0x99, 255])[0];
        assert!(frame.1 + frame.3 >= f.1 + f.3 - 1.0, "the frame goes round the float: {frame:?} {f:?}");
        let hung = lay(&format!("<div><div class=f>{FOUR}</div></div><p>next</p>"), css, 600.0);
        let f = rects(&hung, [0x33, 0x66, 0x99, 255])[0];
        assert!(word(&hung, "next").0 >= 100.0 && word(&hung, "next").1 < f.1 + f.3, "the text after wraps beside it");
    }

    #[test]
    fn box_shadows_are_soft_translucent_rectangles_under_the_box() {
        let l = lay("<div class=b>card</div>", ".b{width:200px;padding:10px;background:#ffffff;box-shadow:0 6px 12px rgba(0,0,0,0.4)}", 600.0);
        let body = rects(&l, [255, 255, 255, 255])[0];
        let shadow: Vec<&Prim> = l.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { color, .. } if color[3] < 255 && color[0] == 0)).collect();
        assert!(shadow.len() >= 2, "a few layers make the edge soft");
        let widest = shadow.iter().map(|p| p.w).fold(0.0, f32::max);
        assert!(widest > body.2, "the shadow spreads past the box");
        assert!(shadow.iter().all(|p| p.y + p.h >= body.1 + body.3 - 0.5), "and is pushed down by its offset");
        let first_shadow = l.prims.iter().position(|p| matches!(p.kind, Kind::Rect { color, .. } if color[3] < 255)).unwrap();
        let first_body = l.prims.iter().position(|p| matches!(p.kind, Kind::Rect { color, .. } if color == [255, 255, 255, 255])).unwrap();
        assert!(first_shadow < first_body, "drawn first, under the box");
        let hard = lay("<div class=b>x</div>", ".b{width:100px;background:#ffffff;box-shadow:4px 4px #000000}", 600.0);
        let one: Vec<&Prim> = hard.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { color, .. } if color == [0, 0, 0, 255] || color[3] > 0 && color[0] == 0)).collect();
        assert_eq!(one.len(), 1, "no blur: one offset rectangle");
        assert!((one[0].x - 4.0).abs() < 0.5);
    }

    #[test]
    fn a_linear_gradient_is_bands_from_the_first_colour_to_the_last() {
        let l = lay("<div class=b>x</div>", ".b{width:300px;padding:20px;background:linear-gradient(to right,#ff0000,#0000ff)}", 600.0);
        let bands: Vec<&Prim> = l.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { .. })).collect();
        assert!(bands.len() >= 8, "many bands: {}", bands.len());
        let col = |p: &Prim| if let Kind::Rect { color, .. } = p.kind { color } else { [0; 4] };
        let first = bands.iter().min_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        let last = bands.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        assert!(col(first)[0] > 230 && col(first)[2] < 25, "{:?}", col(first));
        assert!(col(last)[2] > 230 && col(last)[0] < 25, "{:?}", col(last));
        let mid = bands.iter().find(|p| (p.x + p.w / 2.0 - 150.0).abs() < 8.0).unwrap();
        assert!(col(mid)[0] > 90 && col(mid)[2] > 90, "the middle is a mix: {:?}", col(mid));
        let top = bands.iter().all(|p| (p.y - bands[0].y).abs() < 0.5);
        assert!(top, "a horizontal gradient is columns of the same height");
    }

    #[test]
    fn a_vertical_gradient_is_rows_and_an_angle_picks_the_direction() {
        let down = lay("<div class=b>x</div>", ".b{width:200px;padding:30px;background:linear-gradient(#ffffff,#000000)}", 600.0);
        let rows: Vec<&Prim> = down.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { .. })).collect();
        let col = |p: &Prim| if let Kind::Rect { color, .. } = p.kind { color } else { [0; 4] };
        let top = rows.iter().min_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
        let bottom = rows.iter().max_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
        assert!(col(top)[0] > 235 && col(bottom)[0] < 20, "light on top, dark at the bottom");
        let up = lay("<div class=b>x</div>", ".b{width:200px;padding:30px;background:linear-gradient(0deg,#ffffff,#000000)}", 600.0);
        let rows: Vec<&Prim> = up.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { .. })).collect();
        let top = rows.iter().min_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
        assert!(col(top)[0] < 20, "0deg runs upward, so the first colour is at the bottom");
        let radial = lay("<div class=b>x</div>", ".b{width:200px;background:radial-gradient(#ffffff,#000000)}", 600.0);
        let fills = radial.prims.iter().filter(|p| matches!(p.kind, Kind::Rect { .. })).count();
        assert_eq!(fills, 1, "a radial gradient is filled with a mix");
    }

    #[test]
    fn white_text_on_a_dark_gradient_stays_readable() {
        let l = lay("<div class=b>hello</div>", ".b{width:300px;color:#ffffff;background:linear-gradient(#101040,#202080)}", 600.0);
        let c = l.prims.iter().find_map(|p| if let Kind::Text { color, .. } = &p.kind { Some(*color) } else { None }).unwrap();
        assert_eq!(c, [255, 255, 255, 255]);
    }

    #[test]
    fn relative_shifts_the_box_without_changing_the_flow() {
        let base = lay("<div class=b>one</div><p>two</p>", ".b{width:200px;padding:4px}", 600.0);
        let moved = lay("<div class=b>one</div><p>two</p>", ".b{width:200px;padding:4px;position:relative;left:30px;top:10px}", 600.0);
        let (x, y, _) = word(&base, "one");
        let (mx, my, _) = word(&moved, "one");
        assert!((mx - x - 30.0).abs() < 0.5 && (my - y - 10.0).abs() < 0.5);
        assert_eq!(word(&base, "two").1, word(&moved, "two").1, "the next block does not move");
        let right = lay("<div class=b>one</div>", ".b{position:relative;right:20px;bottom:5px}", 600.0);
        let plain = lay("<div class=b>one</div>", "", 600.0);
        assert!((word(&plain, "one").0 - word(&right, "one").0 - 20.0).abs() < 0.5 && (word(&plain, "one").1 - word(&right, "one").1 - 5.0).abs() < 0.5);
    }

    #[test]
    fn absolute_boxes_sit_in_their_positioned_ancestor_and_take_no_room() {
        let css = ".host{position:relative;width:400px;padding:10px;background:#eeeeee} .abs{position:absolute;top:20px;left:50px;width:100px}";
        let with = lay("<p>before</p><div class=host><p>text</p><div class=abs>floating</div></div><p>after</p>", css, 600.0);
        let without = lay("<p>before</p><div class=host><p>text</p></div><p>after</p>", css, 600.0);
        let host = rects(&with, [0xee, 0xee, 0xee, 255])[0];
        let (x, y, _) = word(&with, "floating");
        assert!((x - (host.0 + 10.0 + 50.0 - 10.0 + 10.0)).abs() < 14.0 && x >= host.0 + 50.0, "left:50px from the host's padding edge: {x} in {host:?}");
        assert!(y >= host.1 + 20.0 && y < host.1 + 20.0 + 30.0, "top:20px: {y} in {host:?}");
        assert!((word(&with, "after").1 - word(&without, "after").1).abs() < 0.5, "it takes no room in the flow");
        let idx_of = |l: &Layout, w: &str| l.prims.iter().position(|p| matches!(&p.kind, Kind::Text { text, .. } if text == w)).unwrap();
        assert!(idx_of(&with, "floating") > idx_of(&with, "after"), "drawn over everything in the flow");
    }

    #[test]
    fn absolute_boxes_can_hang_from_the_bottom_and_right_and_default_to_the_page() {
        let css = ".host{position:relative;width:400px;padding:10px;background:#eeeeee} .abs{position:absolute;bottom:0;right:0;width:100px;background:#ff00ff}";
        let l = lay("<div class=host><p>a</p><p>b</p><p>c</p><div class=abs>corner</div></div>", css, 600.0);
        let host = rects(&l, [0xee, 0xee, 0xee, 255])[0];
        let abs = rects(&l, [255, 0, 255, 255])[0];
        assert!((abs.0 + abs.2 - (host.0 + host.2)).abs() < 1.5, "right edges meet: {abs:?} {host:?}");
        assert!((abs.1 + abs.3 - (host.1 + host.3)).abs() < 1.5, "bottom edges meet: {abs:?} {host:?}");
        let page = lay("<p>x</p><p>y</p><div class=abs2>top</div>", ".abs2{position:absolute;top:5px;left:7px}", 600.0);
        let (x, y, _) = word(&page, "top");
        assert!((x - 7.0).abs() < 1.0 && (y - 5.0).abs() < 10.0, "{x} {y}");
    }

    #[test]
    fn z_index_orders_the_drawing_of_positioned_boxes() {
        let css = ".a,.b{position:absolute;top:0;left:0;width:100px} .a{z-index:5;background:#ff0000} .b{z-index:1;background:#0000ff}";
        let l = lay("<div class=a>aaa</div><div class=b>bbb</div>", css, 600.0);
        let at = |c: [u8; 4]| l.prims.iter().position(|p| matches!(p.kind, Kind::Rect { color, .. } if color == c)).unwrap();
        assert!(at([0, 0, 255, 255]) < at([255, 0, 0, 255]), "the higher z-index is drawn later, so it is on top");
        let under = lay("<div class=n>flow</div><div class=a>aaa</div>", ".a{position:absolute;top:0;left:0;z-index:-1} .n{background:#00ff00}", 600.0);
        let w = under.prims.iter().position(|p| matches!(&p.kind, Kind::Text { text, .. } if text == "aaa")).unwrap();
        let n = under.prims.iter().position(|p| matches!(p.kind, Kind::Rect { color, .. } if color == [0, 255, 0, 255]));
        assert!(n.is_none_or(|n| w < n), "a negative z-index goes under the flow");
    }

    #[test]
    fn fixed_and_sticky_stay_in_the_flow() {
        let plain = lay("<p>a</p><div class=x>fixed</div><p>b</p>", "", 600.0);
        for pos in ["fixed", "sticky"] {
            let l = lay("<p>a</p><div class=x>fixed</div><p>b</p>", &format!(".x{{position:{pos};top:0;left:40px}}"), 600.0);
            assert_eq!(word(&l, "fixed"), word(&plain, "fixed"), "{pos}");
            assert_eq!(word(&l, "b"), word(&plain, "b"), "{pos}");
        }
    }

    #[test]
    fn opacity_blends_the_colours_toward_the_background() {
        let solid = lay("<body><div class=b>text</div></body>", "body{background:#ffffff} .b{background:#f0f0f0;color:#000000;width:200px}", 600.0);
        let faded = lay("<body><div class=b>text</div></body>", "body{background:#ffffff} .b{background:#f0f0f0;color:#000000;width:200px;opacity:0.5}", 600.0);
        let tc = |l: &Layout| l.prims.iter().find_map(|p| if let Kind::Text { color, .. } = &p.kind { Some(*color) } else { None }).unwrap();
        assert_eq!(tc(&solid)[0], 0);
        assert!((tc(&faded)[0] as i32 - 128).abs() < 70 && tc(&faded)[0] > 40, "{:?}", tc(&faded));
        let zero = lay("<div class=b>text</div>", ".b{opacity:0}", 600.0);
        assert!(words(&zero).iter().any(|w| w.0 == "text"), "opacity:0 still shows the text (pages fade content in)");
    }

    #[test]
    fn object_fit_reaches_the_picture_and_places_it_in_its_box() {
        let l = lay("<img src=a.png width=200 height=100 style=\"object-fit:cover\">", "", 600.0);
        assert!(l.prims.iter().any(|p| matches!(p.kind, Kind::Image { fit: Fit::Cover, .. })));
        let l = lay("<img src=a.png width=200 height=100>", "img{object-fit:contain}", 600.0);
        assert!(l.prims.iter().any(|p| matches!(p.kind, Kind::Image { fit: Fit::Contain, .. })));
        let sized = lay("<img src=a.png>", "img{width:250px;height:60px}", 600.0);
        let img = sized.prims.iter().find(|p| matches!(p.kind, Kind::Image { .. })).unwrap();
        assert_eq!((img.w, img.h), (250.0, 60.0), "a size from the style sheet counts");
        let r = R::new(0.0, 0.0, 200.0, 100.0);
        let (clip, sx, _, tx, _) = fit_picture(Fit::Contain, r, 100.0, 100.0);
        assert!((sx - 1.0).abs() < 1e-3 && (tx - 50.0).abs() < 1e-3 && (clip.w - 100.0).abs() < 1e-3, "a square in a wide box is centred");
        let (clip, sx, _, _, ty) = fit_picture(Fit::Cover, r, 100.0, 100.0);
        assert!((sx - 2.0).abs() < 1e-3 && (ty + 50.0).abs() < 1e-3 && clip.w == 200.0, "cover fills the box and crops the rest");
        let (clip, sx, ..) = fit_picture(Fit::None, r, 100.0, 40.0);
        assert!(sx == 1.0 && clip.w == 100.0 && clip.h == 40.0);
        let (_, sx, ..) = fit_picture(Fit::ScaleDown, r, 400.0, 100.0);
        assert!((sx - 0.5).abs() < 1e-3, "too big: scaled down");
        let (_, sx, ..) = fit_picture(Fit::ScaleDown, r, 50.0, 50.0);
        assert_eq!(sx, 1.0, "small enough: left alone");
    }
}

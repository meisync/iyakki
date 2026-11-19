//! The SDK's display list (floats, version 1) onto the tile ABI's (whole numbers, ABI 1), operation by operation.
//!
//! | SDK operation | tile ABI operation |
//! |---|---|
//! | `rect`, `rrect`, `line`, `clip`, `endclip` | `Rect`, `RoundRect`, `Line`, `PushClip`, `PopClip` (coordinates rounded) |
//! | `outline` (radius, width) | `Outline` (thickness; the ABI has no rounded outline, so the radius is dropped) |
//! | `grad` (top to bottom) | bands of `Rect`: one per 2 units of height, at most 48, each the colour at its middle |
//! | `shadow` | three `RoundRect` of growing size and falling alpha (the same glow the device draws) |
//! | `text` | `Text` (face 0 sans, 1 mono, 2 bold; size rounded, 1 to 512; cut at 1024 bytes) |
//! | `hit` (action, name, role) | `Hit` with id = action + 1 (id 0 is "no hit" to the ABI's access nodes), and an `Access` node with the same id, the hit's name and role, so a screen reader hears the control (the list is then ABI 2; at most 256 nodes, the rest stay as plain hits) |
//!
//! | `path` (version 3) | the stroke as `Line` pieces, the fill as one `Rect` per row-run (the shape flattened to quarter-unit pieces; at most 512 rows) |
//! | `layer`, `endlayer` (version 3) | the colours of the operations inside are made that much fainter (each shape fades on its own) |
//! | `textc` (version 3) | a plain `Text` (the ABI has no cluster advances) |
//!
//! A list the ABI cannot hold (too many operations, clips nested deeper than 16) is refused whole, with the reason.

use iyakki_tile_abi::{limits, Color, DisplayList as Tile, Op, Rect};
use iyakki_ui_sdk::canvas::{Role as SdkRole, TextStyle};
use iyakki_ui_sdk::geom::{Color as SdkColor, Rect as SdkRect};
use iyakki_ui_sdk::list::{DisplayList as Sdk, Op as SdkOp};

/// Bands a gradient is cut into, at most.
pub const MAX_BANDS: i32 = 48;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConvertError {
    TooManyOps,
    ClipsTooDeep,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::TooManyOps => write!(f, "the frame has more than {} drawing operations", limits::MAX_OPS),
            ConvertError::ClipsTooDeep => write!(f, "the frame nests clips deeper than {}", limits::MAX_CLIP_DEPTH),
        }
    }
}

fn n(v: f32) -> i32 {
    if v.is_finite() {
        (v.round() as i64).clamp(-(limits::MAX_COORD as i64), limits::MAX_COORD as i64) as i32
    } else {
        0
    }
}

fn rect(r: &SdkRect) -> Rect {
    let (x, y) = (n(r.x), n(r.y));
    Rect::new(x, y, (n(r.x + r.w) - x).max(0), (n(r.y + r.h) - y).max(0))
}

fn col(c: SdkColor) -> Color {
    Color(c[0], c[1], c[2], c[3])
}

fn lerp(a: SdkColor, b: SdkColor, t: f32) -> Color {
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color(m(0), m(1), m(2), m(3))
}

pub fn face(s: TextStyle) -> u8 {
    match s {
        TextStyle::Regular => 0,
        TextStyle::Mono => 1,
        TextStyle::Bold => 2,
    }
}

fn cut(s: &str) -> String {
    if s.len() <= limits::MAX_TEXT {
        return s.to_string();
    }
    let mut end = limits::MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The ABI's picture of an SDK frame.
pub fn to_tile(list: &Sdk) -> Result<Tile, ConvertError> {
    let mut out = Tile::new();
    let mut depth = 0usize;
    let mut access_nodes = 0usize;
    let mut layers: Vec<(usize, f32)> = Vec::new();
    for op in &list.ops {
        match op {
            SdkOp::PushLayer(a) => layers.push((out.ops.len(), *a)),
            SdkOp::PopLayer => {
                if let Some((from, a)) = layers.pop() {
                    for o in out.ops[from..].iter_mut() {
                        fade(o, a);
                    }
                }
            }
            SdkOp::Path { fill, stroke, segs } => {
                let polys = iyakki_ui_sdk::path::flatten(segs, 0.25);
                if let (Some(c), Some((x0, y0, x1, y1))) = (fill, iyakki_ui_sdk::path::bounds(&polys, 0.0)) {
                    let (px, py) = (n(x0.floor()), n(y0.floor()));
                    let (w, h) = ((n(x1.ceil()) - px).clamp(0, 2048) as usize, (n(y1.ceil()) - py).clamp(0, 512) as usize);
                    let mask = iyakki_ui_sdk::path::fill_mask(&polys, px, py, w, h);
                    for row in 0..h {
                        let mut x = 0;
                        while x < w {
                            if mask[row * w + x] >= 0.5 {
                                let from = x;
                                while x < w && mask[row * w + x] >= 0.5 {
                                    x += 1;
                                }
                                out.push(Op::Rect { r: Rect::new(px + from as i32, py + row as i32, (x - from) as i32, 1), color: col(*c) });
                            } else {
                                x += 1;
                            }
                        }
                        if out.ops.len() > limits::MAX_OPS {
                            return Err(ConvertError::TooManyOps);
                        }
                    }
                }
                if let Some((c, wd)) = stroke {
                    for p in &polys {
                        let m = p.pts.len();
                        for i in 0..if p.closed { m } else { m.saturating_sub(1) } {
                            let (a, b) = (p.pts[i], p.pts[(i + 1) % m]);
                            out.push(Op::Line { x1: n(a.0), y1: n(a.1), x2: n(b.0), y2: n(b.1), thickness: n(*wd).max(1), color: col(*c) });
                            if out.ops.len() > limits::MAX_OPS {
                                return Err(ConvertError::TooManyOps);
                            }
                        }
                    }
                }
            }
            SdkOp::Rect(r, c) => out.push(Op::Rect { r: rect(r), color: col(*c) }),
            SdkOp::RoundRect(r, rad, c) => out.push(Op::RoundRect { r: rect(r), radius: n(*rad).max(0), color: col(*c) }),
            SdkOp::Outline(r, _radius, w, c) => out.push(Op::Outline { r: rect(r), thickness: n(*w).max(1), color: col(*c) }),
            SdkOp::Line(p, w, c) => out.push(Op::Line { x1: n(p[0]), y1: n(p[1]), x2: n(p[2]), y2: n(p[3]), thickness: n(*w).max(1), color: col(*c) }),
            SdkOp::Gradient(r, top, bottom) => {
                let b = rect(r);
                if b.w > 0 && b.h > 0 {
                    let bands = (b.h / 2).clamp(1, MAX_BANDS);
                    for i in 0..bands {
                        let (y0, y1) = (b.y + b.h * i / bands, b.y + b.h * (i + 1) / bands);
                        out.push(Op::Rect { r: Rect::new(b.x, y0, b.w, y1 - y0), color: lerp(*top, *bottom, (i as f32 + 0.5) / bands as f32) });
                    }
                }
            }
            SdkOp::Shadow(r, rad, spread, c) => {
                let b = rect(r);
                for (k, f) in [(1.0f32, 0.35f32), (0.66, 0.6), (0.33, 1.0)] {
                    let d = n(spread * k);
                    out.push(Op::RoundRect { r: Rect::new(b.x - d, b.y - d, b.w + 2 * d, b.h + 2 * d), radius: n(*rad) + d, color: Color(c[0], c[1], c[2], (c[3] as f32 * f * 0.5) as u8) });
                }
            }
            // the tile ABI has no family, letter spacing or blur: a version 2 run is drawn as a plain run, a blur is left out
            SdkOp::Blur(..) => {}
            SdkOp::Text { x, y, size, color, style, text } | SdkOp::TextEx { x, y, size, color, style, text, .. } | SdkOp::TextC { x, y, size, color, style, text, .. } => {
                if text.is_empty() {
                    continue;
                }
                out.push(Op::Text { x: n(*x), y: n(*y), face: face(*style), size: n(*size).clamp(1, limits::MAX_FONT_SIZE as i32) as u32, color: col(*color), text: cut(text) })
            }
            SdkOp::PushClip(r) => {
                depth += 1;
                if depth > limits::MAX_CLIP_DEPTH {
                    return Err(ConvertError::ClipsTooDeep);
                }
                out.push(Op::PushClip { r: rect(r) })
            }
            SdkOp::PopClip => {
                depth = depth.saturating_sub(1);
                out.push(Op::PopClip)
            }
            // tile ABI 1 has no picture operation (F-SDK-166): a picture is drawn as a dark block so the layout stays whole
            SdkOp::Image(r, _) => out.push(Op::Rect { r: rect(r), color: Color(24, 24, 28, 255) }),
            SdkOp::Hit(r, h) => {
                let id = h.action.saturating_add(1);
                out.push(Op::Hit { id, r: rect(r) });
                if access_nodes < limits::MAX_ACCESS && !h.name.is_empty() {
                    access_nodes += 1;
                    let name: String = h.name.chars().take(limits::MAX_ACCESS_TEXT / 4).collect();
                    out.push(Op::Access { id, role: role(h.role), r: rect(r), name, value: String::new(), flags: 0 });
                }
            }
        }
        if out.ops.len() > limits::MAX_OPS {
            return Err(ConvertError::TooManyOps);
        }
    }
    Ok(out)
}

fn fade(o: &mut Op, a: f32) {
    let f = |c: &mut Color| c.3 = (c.3 as f32 * a).round() as u8;
    match o {
        Op::Rect { color, .. } | Op::RoundRect { color, .. } | Op::Outline { color, .. } | Op::Line { color, .. } | Op::Text { color, .. } => f(color),
        _ => {}
    }
}

fn role(r: SdkRole) -> iyakki_tile_abi::Role {
    use iyakki_tile_abi::Role;
    match r {
        SdkRole::Button => Role::Button,
        SdkRole::Link => Role::Link,
        SdkRole::ListItem => Role::ListItem,
        SdkRole::Tab => Role::Tab,
        SdkRole::Field => Role::Field,
        SdkRole::Text | SdkRole::Image => Role::Text,
    }
}

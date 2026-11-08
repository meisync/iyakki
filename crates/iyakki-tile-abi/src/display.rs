//! The display list: what a tile draws, as plain data.
use crate::codec::{DecodeError, Reader, Writer};
use crate::limits::*;
use crate::{ABI_VERSION, LIST_MAGIC};
use alloc::{string::String, vec::Vec};

/// RGBA, straight (not premultiplied) alpha. On the wire: four bytes r, g, b, a.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color(r, g, b, 255)
    }
}

/// A rectangle in logical pixels; `w` and `h` are never negative after decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    /// Intersection; empty (w = h = 0) when they do not overlap.
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        if x1 <= x0 || y1 <= y0 {
            Rect::new(x0, y0, 0, 0)
        } else {
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }
}

/// One drawing operation. Later operations paint over earlier ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Rect { r: Rect, color: Color },
    RoundRect { r: Rect, radius: i32, color: Color },
    /// A rectangle border `thickness` pixels wide, drawn inside `r`.
    Outline { r: Rect, thickness: i32, color: Color },
    Line { x1: i32, y1: i32, x2: i32, y2: i32, thickness: i32, color: Color },
    /// `(x, y)` is the top-left of the text box; the box is `measure_text` wide and `line_height` high.
    Text { x: i32, y: i32, face: u8, size: u32, color: Color, text: String },
    /// An image the host knows by `handle`, stretched to `r`.
    Image { handle: u32, r: Rect },
    PushClip { r: Rect },
    PopClip,
    /// An invisible region; a pointer release on the topmost one comes back as `Event::Hit { id }`.
    Hit { id: u32, r: Rect },
}

const T_RECT: u8 = 1;
const T_ROUND: u8 = 2;
const T_OUTLINE: u8 = 3;
const T_LINE: u8 = 4;
const T_TEXT: u8 = 5;
const T_IMAGE: u8 = 6;
const T_PUSH: u8 = 7;
const T_POP: u8 = 8;
const T_HIT: u8 = 9;

/// An ordered list of operations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayList {
    pub ops: Vec<Op>,
}

fn wrect(w: &mut Writer, r: &Rect) {
    w.zz(r.x);
    w.zz(r.y);
    w.var(r.w.max(0) as u32);
    w.var(r.h.max(0) as u32);
}
fn wcolor(w: &mut Writer, c: Color) {
    w.u8(c.0);
    w.u8(c.1);
    w.u8(c.2);
    w.u8(c.3);
}
fn rrect(r: &mut Reader) -> Result<Rect, DecodeError> {
    let x = r.coord()?;
    let y = r.coord()?;
    let w = r.var()?;
    let h = r.var()?;
    if w > MAX_COORD as u32 || h > MAX_COORD as u32 {
        return Err(DecodeError::OutOfRange);
    }
    Ok(Rect::new(x, y, w as i32, h as i32))
}
fn rcolor(r: &mut Reader) -> Result<Color, DecodeError> {
    Ok(Color(r.u8()?, r.u8()?, r.u8()?, r.u8()?))
}
fn rsmall(r: &mut Reader) -> Result<i32, DecodeError> {
    let v = r.var()?;
    if v > MAX_COORD as u32 {
        return Err(DecodeError::OutOfRange);
    }
    Ok(v as i32)
}

impl DisplayList {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, op: Op) {
        self.ops.push(op);
    }

    /// Encode. Fails (never panics) when a limit would be exceeded.
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        if self.ops.len() > MAX_OPS {
            return Err(DecodeError::TooManyItems);
        }
        let mut w = Writer::default();
        w.buf.extend_from_slice(&LIST_MAGIC);
        w.u16le(ABI_VERSION as u16);
        w.var(self.ops.len() as u32);
        for op in &self.ops {
            match op {
                Op::Rect { r, color } => {
                    w.u8(T_RECT);
                    wrect(&mut w, r);
                    wcolor(&mut w, *color);
                }
                Op::RoundRect { r, radius, color } => {
                    w.u8(T_ROUND);
                    wrect(&mut w, r);
                    w.var((*radius).max(0) as u32);
                    wcolor(&mut w, *color);
                }
                Op::Outline { r, thickness, color } => {
                    w.u8(T_OUTLINE);
                    wrect(&mut w, r);
                    w.var((*thickness).max(0) as u32);
                    wcolor(&mut w, *color);
                }
                Op::Line { x1, y1, x2, y2, thickness, color } => {
                    w.u8(T_LINE);
                    w.zz(*x1);
                    w.zz(*y1);
                    w.zz(*x2);
                    w.zz(*y2);
                    w.var((*thickness).max(0) as u32);
                    wcolor(&mut w, *color);
                }
                Op::Text { x, y, face, size, color, text } => {
                    if text.len() > MAX_TEXT {
                        return Err(DecodeError::TooLarge);
                    }
                    w.u8(T_TEXT);
                    w.zz(*x);
                    w.zz(*y);
                    w.u8(*face);
                    w.var(*size);
                    wcolor(&mut w, *color);
                    w.bytes(text.as_bytes());
                }
                Op::Image { handle, r } => {
                    w.u8(T_IMAGE);
                    w.var(*handle);
                    wrect(&mut w, r);
                }
                Op::PushClip { r } => {
                    w.u8(T_PUSH);
                    wrect(&mut w, r);
                }
                Op::PopClip => w.u8(T_POP),
                Op::Hit { id, r } => {
                    w.u8(T_HIT);
                    w.var(*id);
                    wrect(&mut w, r);
                }
            }
            if w.buf.len() > MAX_LIST_BYTES {
                return Err(DecodeError::TooLarge);
            }
        }
        Ok(w.buf)
    }

    /// Decode and check. Total on any input: errors, never panics, allocation bounded by the limits.
    pub fn decode(bytes: &[u8]) -> Result<DisplayList, DecodeError> {
        if bytes.len() > MAX_LIST_BYTES {
            return Err(DecodeError::TooLarge);
        }
        let mut r = Reader::new(bytes);
        if r.take(4)? != LIST_MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let v = r.u16le()?;
        if v as u32 != ABI_VERSION {
            return Err(DecodeError::BadVersion(v));
        }
        let n = r.var()? as usize;
        if n > MAX_OPS {
            return Err(DecodeError::TooManyItems);
        }
        // every op takes at least one byte, so a count beyond the remaining bytes is a lie
        let mut ops = Vec::with_capacity(n.min(bytes.len()));
        let mut depth = 0usize;
        for _ in 0..n {
            let op = match r.u8()? {
                T_RECT => Op::Rect { r: rrect(&mut r)?, color: rcolor(&mut r)? },
                T_ROUND => Op::RoundRect { r: rrect(&mut r)?, radius: rsmall(&mut r)?, color: rcolor(&mut r)? },
                T_OUTLINE => Op::Outline { r: rrect(&mut r)?, thickness: rsmall(&mut r)?, color: rcolor(&mut r)? },
                T_LINE => Op::Line {
                    x1: r.coord()?,
                    y1: r.coord()?,
                    x2: r.coord()?,
                    y2: r.coord()?,
                    thickness: rsmall(&mut r)?,
                    color: rcolor(&mut r)?,
                },
                T_TEXT => {
                    let x = r.coord()?;
                    let y = r.coord()?;
                    let face = r.u8()?;
                    let size = r.var()?;
                    if size == 0 || size > MAX_FONT_SIZE {
                        return Err(DecodeError::OutOfRange);
                    }
                    let color = rcolor(&mut r)?;
                    Op::Text { x, y, face, size, color, text: r.string()? }
                }
                T_IMAGE => Op::Image { handle: r.var()?, r: rrect(&mut r)? },
                T_PUSH => {
                    depth += 1;
                    if depth > MAX_CLIP_DEPTH {
                        return Err(DecodeError::TooManyItems);
                    }
                    Op::PushClip { r: rrect(&mut r)? }
                }
                T_POP => {
                    depth = depth.checked_sub(1).ok_or(DecodeError::UnbalancedClip)?;
                    Op::PopClip
                }
                T_HIT => Op::Hit { id: r.var()?, r: rrect(&mut r)? },
                t => return Err(DecodeError::BadTag(t)),
            };
            ops.push(op);
        }
        if depth != 0 {
            return Err(DecodeError::UnbalancedClip);
        }
        if !r.done() {
            return Err(DecodeError::TrailingBytes);
        }
        Ok(DisplayList { ops })
    }

    /// Hit regions are checked against the surface by the host; this finds the topmost one at a point
    /// (the last `Hit` in draw order whose rectangle contains it), ignoring clips.
    pub fn hit_test(&self, x: i32, y: i32) -> Option<u32> {
        self.ops.iter().rev().find_map(|op| match op {
            Op::Hit { id, r } if r.contains(x, y) => Some(*id),
            _ => None,
        })
    }

    /// The host's surface check: every drawing rectangle must lie within one surface size of the
    /// surface on each side (room for scrolled content, not for nonsense). Returns the index of the
    /// first offending operation.
    pub fn check_bounds(&self, width: i32, height: i32) -> Result<(), usize> {
        let (mx, my) = (width.max(1), height.max(1));
        let (lo_x, hi_x, lo_y, hi_y) = (-mx as i64, 2 * mx as i64, -my as i64, 2 * my as i64);
        let ok = |x: i64, y: i64, w: i64, h: i64| x >= lo_x && y >= lo_y && x + w <= hi_x && y + h <= hi_y;
        for (i, op) in self.ops.iter().enumerate() {
            let good = match op {
                Op::Rect { r, .. }
                | Op::RoundRect { r, .. }
                | Op::Outline { r, .. }
                | Op::Image { r, .. }
                | Op::PushClip { r }
                | Op::Hit { r, .. } => ok(r.x as i64, r.y as i64, r.w as i64, r.h as i64),
                Op::Line { x1, y1, x2, y2, .. } => {
                    ok(*x1 as i64, *y1 as i64, 0, 0) && ok(*x2 as i64, *y2 as i64, 0, 0)
                }
                Op::Text { x, y, face, size, text, .. } => {
                    let w = crate::measure_text(*face, *size, text) as i64;
                    ok(*x as i64, *y as i64, w, crate::line_height(*size) as i64)
                }
                Op::PopClip => true,
            };
            if !good {
                return Err(i);
            }
        }
        Ok(())
    }
}

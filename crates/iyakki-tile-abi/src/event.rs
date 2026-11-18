//! Events a tile receives.
use crate::codec::{DecodeError, Reader, Writer};
use crate::limits::*;
use crate::{Color, ABI_VERSION, EVENT_MAGIC};
use alloc::{string::String, vec::Vec};

/// One input to the tile. Pointer positions are in logical pixels of the tile's surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    PointerDown { x: i32, y: i32, button: u8 },
    PointerUp { x: i32, y: i32, button: u8 },
    PointerMove { x: i32, y: i32 },
    /// A key with a stable code (docs/development/TILE-ABI.md lists them), modifier bits
    /// (1 shift, 2 ctrl, 4 alt, 8 meta) and whether it went down or up.
    Key { code: u32, mods: u8, down: bool },
    /// Text typed (already composed by the host), UTF-8.
    TextInput { text: String },
    /// Time since the last tick, in milliseconds. The only clock a tile has.
    Tick { dt_ms: u32 },
    /// Surface size in logical pixels and the device pixel ratio times 100 (200 = 2x).
    Resize { w: i32, h: i32, scale_pct: u32 },
    Theme { dark: bool, accent: Color, background: Color, foreground: Color },
    /// A pointer release landed on the topmost `Op::Hit` with this id.
    Hit { id: u32 },
}

/// Events delivered together, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventBatch {
    pub events: Vec<Event>,
}

fn wcol(w: &mut Writer, c: Color) {
    w.u8(c.0);
    w.u8(c.1);
    w.u8(c.2);
    w.u8(c.3);
}
fn rcol(r: &mut Reader) -> Result<Color, DecodeError> {
    Ok(Color(r.u8()?, r.u8()?, r.u8()?, r.u8()?))
}

impl EventBatch {
    pub fn new(events: Vec<Event>) -> Self {
        EventBatch { events }
    }

    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        if self.events.len() > MAX_EVENTS {
            return Err(DecodeError::TooManyItems);
        }
        let mut w = Writer::default();
        w.buf.extend_from_slice(&EVENT_MAGIC);
        w.u16le(ABI_VERSION as u16);
        w.var(self.events.len() as u32);
        for e in &self.events {
            match e {
                Event::PointerDown { x, y, button } => {
                    w.u8(1);
                    w.zz(*x);
                    w.zz(*y);
                    w.u8(*button);
                }
                Event::PointerUp { x, y, button } => {
                    w.u8(2);
                    w.zz(*x);
                    w.zz(*y);
                    w.u8(*button);
                }
                Event::PointerMove { x, y } => {
                    w.u8(3);
                    w.zz(*x);
                    w.zz(*y);
                }
                Event::Key { code, mods, down } => {
                    w.u8(4);
                    w.var(*code);
                    w.u8(*mods);
                    w.u8(u8::from(*down));
                }
                Event::TextInput { text } => {
                    if text.len() > MAX_TEXT {
                        return Err(DecodeError::TooLarge);
                    }
                    w.u8(5);
                    w.bytes(text.as_bytes());
                }
                Event::Tick { dt_ms } => {
                    w.u8(6);
                    w.var(*dt_ms);
                }
                Event::Resize { w: ww, h, scale_pct } => {
                    w.u8(7);
                    w.var((*ww).max(0) as u32);
                    w.var((*h).max(0) as u32);
                    w.var(*scale_pct);
                }
                Event::Theme { dark, accent, background, foreground } => {
                    w.u8(8);
                    w.u8(u8::from(*dark));
                    wcol(&mut w, *accent);
                    wcol(&mut w, *background);
                    wcol(&mut w, *foreground);
                }
                Event::Hit { id } => {
                    w.u8(9);
                    w.var(*id);
                }
            }
            if w.buf.len() > MAX_EVENT_BYTES {
                return Err(DecodeError::TooLarge);
            }
        }
        Ok(w.buf)
    }

    pub fn decode(bytes: &[u8]) -> Result<EventBatch, DecodeError> {
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(DecodeError::TooLarge);
        }
        let mut r = Reader::new(bytes);
        if r.take(4)? != EVENT_MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let v = r.u16le()?;
        if v as u32 != ABI_VERSION {
            return Err(DecodeError::BadVersion(v));
        }
        let n = r.var()? as usize;
        if n > MAX_EVENTS {
            return Err(DecodeError::TooManyItems);
        }
        let mut events = Vec::with_capacity(n.min(bytes.len()));
        for _ in 0..n {
            let e = match r.u8()? {
                1 => Event::PointerDown { x: r.coord()?, y: r.coord()?, button: r.u8()? },
                2 => Event::PointerUp { x: r.coord()?, y: r.coord()?, button: r.u8()? },
                3 => Event::PointerMove { x: r.coord()?, y: r.coord()? },
                4 => Event::Key { code: r.var()?, mods: r.u8()?, down: r.u8()? != 0 },
                5 => Event::TextInput { text: r.string()? },
                6 => Event::Tick { dt_ms: r.var()? },
                7 => {
                    let (w, h, s) = (r.var()?, r.var()?, r.var()?);
                    if w > MAX_COORD as u32 || h > MAX_COORD as u32 {
                        return Err(DecodeError::OutOfRange);
                    }
                    Event::Resize { w: w as i32, h: h as i32, scale_pct: s }
                }
                8 => Event::Theme { dark: r.u8()? != 0, accent: rcol(&mut r)?, background: rcol(&mut r)?, foreground: rcol(&mut r)? },
                9 => Event::Hit { id: r.var()? },
                t => return Err(DecodeError::BadTag(t)),
            };
            events.push(e);
        }
        if !r.done() {
            return Err(DecodeError::TrailingBytes);
        }
        Ok(EventBatch { events })
    }
}

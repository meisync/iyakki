//! Events a tile receives.
use crate::codec::{DecodeError, Reader, Writer};
use crate::limits::*;
use crate::{Color, ABI_VERSION, ABI_VERSION_EVENTS, EVENT_MAGIC};
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
    /// An app event of the core (F-SDK-202, ABI 3): `seq` only grows per tile, `name` is the catalogue name (`connectivity.changed`, or
    /// `missed` for the marker that says events were dropped), `late` and `replayed` are as the core says them, `payload` is the event's
    /// payload as JSON text (names and numbers, never a secret). Sent only to a tile that reports ABI 3 and listens for the event.
    App { seq: u64, late: bool, replayed: bool, name: String, payload: String },
    /// ABI 3: two fingers moved; `scale_milli` is the pinch scale times 1000 (1000 = unchanged), `x`/`y` the midpoint.
    Pinch { scale_milli: u32, x: i32, y: i32 },
    /// ABI 3: a finger of the pinch lifted; the last scale times 1000.
    PinchEnd { scale_milli: u32 },
}

/// The largest pinch scale times 1000 an event may carry (a hundredfold).
pub const MAX_PINCH_MILLI: u32 = 100_000;

impl Event {
    /// Whether this event is one the host may send only to a tile of ABI 3 or newer (the app events and the pinch).
    pub fn needs_events_abi(&self) -> bool {
        matches!(self, Event::App { .. } | Event::Pinch { .. } | Event::PinchEnd { .. })
    }
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
        // a batch with an app event is written as ABI 3; any other batch stays version 1, so a tile of ABI 1 or 2 reads what it always read
        w.u16le(if self.events.iter().any(Event::needs_events_abi) { ABI_VERSION_EVENTS } else { ABI_VERSION } as u16);
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
                Event::Pinch { scale_milli, x, y } => {
                    if *scale_milli > MAX_PINCH_MILLI {
                        return Err(DecodeError::OutOfRange);
                    }
                    w.u8(11);
                    w.var(*scale_milli);
                    w.zz(*x);
                    w.zz(*y);
                }
                Event::PinchEnd { scale_milli } => {
                    if *scale_milli > MAX_PINCH_MILLI {
                        return Err(DecodeError::OutOfRange);
                    }
                    w.u8(12);
                    w.var(*scale_milli);
                }
                Event::App { seq, late, replayed, name, payload } => {
                    if name.len() > MAX_APP_EVENT_NAME || payload.len() > MAX_APP_EVENT_PAYLOAD {
                        return Err(DecodeError::TooLarge);
                    }
                    w.u8(10);
                    w.buf.extend_from_slice(&seq.to_le_bytes());
                    w.u8(u8::from(*late) | (u8::from(*replayed) << 1));
                    w.bytes(name.as_bytes());
                    w.bytes(payload.as_bytes());
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
        if v as u32 != ABI_VERSION && v as u32 != ABI_VERSION_EVENTS {
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
                11 if v as u32 == ABI_VERSION_EVENTS => {
                    let (s, x, y) = (r.var()?, r.coord()?, r.coord()?);
                    if s > MAX_PINCH_MILLI {
                        return Err(DecodeError::OutOfRange);
                    }
                    Event::Pinch { scale_milli: s, x, y }
                }
                12 if v as u32 == ABI_VERSION_EVENTS => {
                    let s = r.var()?;
                    if s > MAX_PINCH_MILLI {
                        return Err(DecodeError::OutOfRange);
                    }
                    Event::PinchEnd { scale_milli: s }
                }
                10 if v as u32 == ABI_VERSION_EVENTS => {
                    let seq = u64::from_le_bytes(<[u8; 8]>::try_from(r.take(8)?).map_err(|_| DecodeError::Truncated)?);
                    let flags = r.u8()?;
                    let (name, payload) = (r.string()?, r.string()?);
                    if name.len() > MAX_APP_EVENT_NAME || payload.len() > MAX_APP_EVENT_PAYLOAD {
                        return Err(DecodeError::TooLarge);
                    }
                    Event::App { seq, late: flags & 1 != 0, replayed: flags & 2 != 0, name, payload }
                }
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

//! The wire format between the shell and the helper: `u32 LE length` (of everything after it), `u8 type`, payload.
//! All integers are little-endian; a string is `u32 length` + UTF-8 bytes. Messages from the shell have types below 0x80,
//! messages from the helper 0x80 and above. Extra bytes after the known fields of a message are ignored (room to grow), and
//! a message of an unknown type is skipped. See `docs/WEBVIEW-PROTOCOL.md`.

use std::io::{self, Read};

pub const VERSION: u32 = 1;
/// The largest message either side accepts (length field value), so a broken peer cannot make the other allocate gigabytes.
pub const MAX_MESSAGE: usize = 4 << 20;
/// Strings longer than this (in bytes) are refused when decoding.
pub const MAX_STRING: usize = 1 << 20;
pub const MAX_RECTS: usize = 64;
pub const MAX_HOSTS: usize = 4096;

/// Modifier bits (`mods` fields).
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_ALT: u32 = 4;
pub const MOD_SUPER: u32 = 8;

/// Mouse button numbers and the `buttons` bit mask.
pub const BUTTON_NONE: u8 = 0;
pub const BUTTON_LEFT: u8 = 1;
pub const BUTTON_MIDDLE: u8 = 2;
pub const BUTTON_RIGHT: u8 = 3;
pub const HELD_LEFT: u8 = 1;
pub const HELD_MIDDLE: u8 = 2;
pub const HELD_RIGHT: u8 = 4;

/// Cursor kinds in [`FromHelper::Cursor`].
pub mod cursor {
    pub const DEFAULT: u8 = 0;
    pub const POINTER: u8 = 1;
    pub const TEXT: u8 = 2;
    pub const WAIT: u8 = 3;
    pub const CROSSHAIR: u8 = 4;
    pub const MOVE: u8 = 5;
    pub const NOT_ALLOWED: u8 = 6;
    pub const RESIZE_EW: u8 = 7;
    pub const RESIZE_NS: u8 = 8;
    pub const GRAB: u8 = 9;
    pub const HELP: u8 = 10;
    pub const NONE: u8 = 11;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    /// The smallest rectangle holding both.
    pub fn union(self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        let (x0, y0) = (self.x.min(o.x), self.y.min(o.y));
        let (x1, y1) = ((self.x + self.w).max(o.x + o.w), (self.y + self.h).max(o.y + o.h));
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    /// The part inside a `w` x `h` picture (possibly empty).
    pub fn clip(self, w: u32, h: u32) -> Rect {
        let (x, y) = (self.x.min(w), self.y.min(h));
        Rect { x, y, w: (self.x.saturating_add(self.w)).min(w) - x, h: (self.y.saturating_add(self.h)).min(h) - y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    Down,
    Up,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    pub version: u32,
    /// The size of the picture to render, in device pixels.
    pub w: u32,
    pub h: u32,
    /// Device scale in percent (100 = one pixel per CSS pixel, 200 = HiDPI).
    pub scale_pct: u32,
    /// The largest picture the shared file has room for (a `Resize` is clamped to it by the shell).
    pub max_w: u32,
    pub max_h: u32,
    /// Added to the engine's own user agent (a modern browser name that sites accept). Empty = leave the default.
    pub user_agent: String,
    /// The folder where cookies, storage and cache live (the helper creates what it needs inside).
    pub profile: String,
    /// BCP 47 language tag for `Accept-Language` and the page, e.g. `en-US`. Empty = the engine's default.
    pub locale: String,
    /// Hosts the account may open; `*` = any. The helper should refuse top-level navigation to others (the shell checks again).
    pub allow_hosts: Vec<String>,
}

impl Default for Hello {
    fn default() -> Hello {
        Hello { version: VERSION, w: 1280, h: 720, scale_pct: 100, max_w: 4096, max_h: 2304, user_agent: String::new(), profile: String::new(), locale: String::new(), allow_hosts: vec!["*".into()] }
    }
}

/// Shell to helper.
#[derive(Clone, Debug, PartialEq)]
pub enum ToHelper {
    Hello(Hello),
    Navigate(String),
    Reload { hard: bool },
    Stop,
    Back,
    Forward,
    Resize { w: u32, h: u32 },
    Mouse { kind: MouseKind, x: i32, y: i32, button: u8, buttons: u8, mods: u32, clicks: u8 },
    Wheel { x: i32, y: i32, dx: i32, dy: i32, mods: u32 },
    Key { down: bool, keysym: u32, mods: u32, text: String },
    Text(String),
    Zoom(u32),
    Focus(bool),
    SetVisible(bool),
    /// The shell has copied the pixels of this slot; the helper may draw into it again.
    Ack(u8),
    Quit,
}

/// Helper to shell.
#[derive(Clone, Debug, PartialEq)]
pub enum FromHelper {
    Ready { version: u32, engine: String, engine_version: String },
    Frame { slot: u8, w: u32, h: u32, rects: Vec<Rect> },
    Title(String),
    Url(String),
    Loading { loading: bool, progress: u8 },
    History { can_back: bool, can_forward: bool },
    Cursor(u8),
    Download { url: String, name: String },
    Crashed(String),
    Log(String),
    /// The page copied text (the shell puts it on its clipboard).
    Clipboard(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadUtf8,
    TooLong,
    Unknown(u8),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "message too short"),
            DecodeError::BadUtf8 => write!(f, "text is not UTF-8"),
            DecodeError::TooLong => write!(f, "text or list too long"),
            DecodeError::Unknown(t) => write!(f, "unknown message type {t:#04x}"),
        }
    }
}

// ----------------------------------------------------------------------------------------------- writing

struct W(Vec<u8>);

impl W {
    fn new(ty: u8) -> W {
        W(vec![0, 0, 0, 0, ty])
    }
    fn u8(&mut self, v: u8) -> &mut W {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut W {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn str(&mut self, s: &str) -> &mut W {
        // never produce a string the other side would refuse
        let mut end = s.len().min(MAX_STRING);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u32(end as u32);
        self.0.extend_from_slice(&s.as_bytes()[..end]);
        self
    }
    fn finish(mut self) -> Vec<u8> {
        let n = (self.0.len() - 4) as u32;
        self.0[..4].copy_from_slice(&n.to_le_bytes());
        self.0
    }
}

// ----------------------------------------------------------------------------------------------- reading

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bool(&mut self) -> Result<bool, DecodeError> {
        Ok(self.u8()? != 0)
    }
    fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.u32()? as usize;
        if n > MAX_STRING {
            return Err(DecodeError::TooLong);
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
}

impl ToHelper {
    pub const HELLO: u8 = 0x01;
    pub const NAVIGATE: u8 = 0x02;
    pub const RELOAD: u8 = 0x03;
    pub const STOP: u8 = 0x04;
    pub const BACK: u8 = 0x05;
    pub const FORWARD: u8 = 0x06;
    pub const RESIZE: u8 = 0x07;
    pub const MOUSE: u8 = 0x08;
    pub const WHEEL: u8 = 0x09;
    pub const KEY: u8 = 0x0A;
    pub const TEXT: u8 = 0x0B;
    pub const ZOOM: u8 = 0x0C;
    pub const FOCUS: u8 = 0x0D;
    pub const SET_VISIBLE: u8 = 0x0E;
    pub const ACK: u8 = 0x0F;
    pub const QUIT: u8 = 0x10;

    /// The whole framed message, ready to write to the socket.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ToHelper::Hello(h) => {
                let mut w = W::new(Self::HELLO);
                w.u32(h.version).u32(h.w).u32(h.h).u32(h.scale_pct).u32(h.max_w).u32(h.max_h).str(&h.user_agent).str(&h.profile).str(&h.locale);
                let hosts = &h.allow_hosts[..h.allow_hosts.len().min(MAX_HOSTS)];
                w.u32(hosts.len() as u32);
                for s in hosts {
                    w.str(s);
                }
                w.finish()
            }
            ToHelper::Navigate(u) => {
                let mut w = W::new(Self::NAVIGATE);
                w.str(u);
                w.finish()
            }
            ToHelper::Reload { hard } => {
                let mut w = W::new(Self::RELOAD);
                w.u8(*hard as u8);
                w.finish()
            }
            ToHelper::Stop => W::new(Self::STOP).finish(),
            ToHelper::Back => W::new(Self::BACK).finish(),
            ToHelper::Forward => W::new(Self::FORWARD).finish(),
            ToHelper::Resize { w: pw, h: ph } => {
                let mut w = W::new(Self::RESIZE);
                w.u32(*pw).u32(*ph);
                w.finish()
            }
            ToHelper::Mouse { kind, x, y, button, buttons, mods, clicks } => {
                let mut w = W::new(Self::MOUSE);
                w.u8(match kind {
                    MouseKind::Move => 0,
                    MouseKind::Down => 1,
                    MouseKind::Up => 2,
                })
                .i32(*x)
                .i32(*y)
                .u8(*button)
                .u8(*buttons)
                .u32(*mods)
                .u8(*clicks);
                w.finish()
            }
            ToHelper::Wheel { x, y, dx, dy, mods } => {
                let mut w = W::new(Self::WHEEL);
                w.i32(*x).i32(*y).i32(*dx).i32(*dy).u32(*mods);
                w.finish()
            }
            ToHelper::Key { down, keysym, mods, text } => {
                let mut w = W::new(Self::KEY);
                w.u8(*down as u8).u32(*keysym).u32(*mods).str(text);
                w.finish()
            }
            ToHelper::Text(t) => {
                let mut w = W::new(Self::TEXT);
                w.str(t);
                w.finish()
            }
            ToHelper::Zoom(p) => {
                let mut w = W::new(Self::ZOOM);
                w.u32(*p);
                w.finish()
            }
            ToHelper::Focus(b) => {
                let mut w = W::new(Self::FOCUS);
                w.u8(*b as u8);
                w.finish()
            }
            ToHelper::SetVisible(b) => {
                let mut w = W::new(Self::SET_VISIBLE);
                w.u8(*b as u8);
                w.finish()
            }
            ToHelper::Ack(s) => {
                let mut w = W::new(Self::ACK);
                w.u8(*s);
                w.finish()
            }
            ToHelper::Quit => W::new(Self::QUIT).finish(),
        }
    }

    pub fn decode(ty: u8, payload: &[u8]) -> Result<ToHelper, DecodeError> {
        let mut r = Rd(payload);
        Ok(match ty {
            Self::HELLO => {
                let (version, w, h, scale_pct, max_w, max_h) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
                let (user_agent, profile, locale) = (r.str()?, r.str()?, r.str()?);
                let n = r.u32()? as usize;
                if n > MAX_HOSTS {
                    return Err(DecodeError::TooLong);
                }
                let mut allow_hosts = Vec::with_capacity(n.min(64));
                for _ in 0..n {
                    allow_hosts.push(r.str()?);
                }
                ToHelper::Hello(Hello { version, w, h, scale_pct, max_w, max_h, user_agent, profile, locale, allow_hosts })
            }
            Self::NAVIGATE => ToHelper::Navigate(r.str()?),
            Self::RELOAD => ToHelper::Reload { hard: r.bool()? },
            Self::STOP => ToHelper::Stop,
            Self::BACK => ToHelper::Back,
            Self::FORWARD => ToHelper::Forward,
            Self::RESIZE => ToHelper::Resize { w: r.u32()?, h: r.u32()? },
            Self::MOUSE => {
                let kind = match r.u8()? {
                    0 => MouseKind::Move,
                    1 => MouseKind::Down,
                    _ => MouseKind::Up,
                };
                ToHelper::Mouse { kind, x: r.i32()?, y: r.i32()?, button: r.u8()?, buttons: r.u8()?, mods: r.u32()?, clicks: r.u8()? }
            }
            Self::WHEEL => ToHelper::Wheel { x: r.i32()?, y: r.i32()?, dx: r.i32()?, dy: r.i32()?, mods: r.u32()? },
            Self::KEY => ToHelper::Key { down: r.bool()?, keysym: r.u32()?, mods: r.u32()?, text: r.str()? },
            Self::TEXT => ToHelper::Text(r.str()?),
            Self::ZOOM => ToHelper::Zoom(r.u32()?),
            Self::FOCUS => ToHelper::Focus(r.bool()?),
            Self::SET_VISIBLE => ToHelper::SetVisible(r.bool()?),
            Self::ACK => ToHelper::Ack(r.u8()?),
            Self::QUIT => ToHelper::Quit,
            t => return Err(DecodeError::Unknown(t)),
        })
    }
}

impl FromHelper {
    pub const READY: u8 = 0x81;
    pub const FRAME: u8 = 0x82;
    pub const TITLE: u8 = 0x83;
    pub const URL: u8 = 0x84;
    pub const LOADING: u8 = 0x85;
    pub const HISTORY: u8 = 0x86;
    pub const CURSOR: u8 = 0x87;
    pub const DOWNLOAD: u8 = 0x88;
    pub const CRASHED: u8 = 0x89;
    pub const LOG: u8 = 0x8A;
    pub const CLIPBOARD: u8 = 0x8B;

    pub fn encode(&self) -> Vec<u8> {
        match self {
            FromHelper::Ready { version, engine, engine_version } => {
                let mut w = W::new(Self::READY);
                w.u32(*version).str(engine).str(engine_version);
                w.finish()
            }
            FromHelper::Frame { slot, w: fw, h: fh, rects } => {
                let mut w = W::new(Self::FRAME);
                let rects = &rects[..rects.len().min(MAX_RECTS)];
                w.u8(*slot).u32(*fw).u32(*fh).u32(rects.len() as u32);
                for r in rects {
                    w.u32(r.x).u32(r.y).u32(r.w).u32(r.h);
                }
                w.finish()
            }
            FromHelper::Title(s) => str_msg(Self::TITLE, s),
            FromHelper::Url(s) => str_msg(Self::URL, s),
            FromHelper::Loading { loading, progress } => {
                let mut w = W::new(Self::LOADING);
                w.u8(*loading as u8).u8(*progress);
                w.finish()
            }
            FromHelper::History { can_back, can_forward } => {
                let mut w = W::new(Self::HISTORY);
                w.u8(*can_back as u8).u8(*can_forward as u8);
                w.finish()
            }
            FromHelper::Cursor(k) => {
                let mut w = W::new(Self::CURSOR);
                w.u8(*k);
                w.finish()
            }
            FromHelper::Download { url, name } => {
                let mut w = W::new(Self::DOWNLOAD);
                w.str(url).str(name);
                w.finish()
            }
            FromHelper::Crashed(s) => str_msg(Self::CRASHED, s),
            FromHelper::Log(s) => str_msg(Self::LOG, s),
            FromHelper::Clipboard(s) => str_msg(Self::CLIPBOARD, s),
        }
    }

    pub fn decode(ty: u8, payload: &[u8]) -> Result<FromHelper, DecodeError> {
        let mut r = Rd(payload);
        Ok(match ty {
            Self::READY => FromHelper::Ready { version: r.u32()?, engine: r.str()?, engine_version: r.str()? },
            Self::FRAME => {
                let (slot, w, h) = (r.u8()?, r.u32()?, r.u32()?);
                let n = r.u32()? as usize;
                if n > MAX_RECTS {
                    return Err(DecodeError::TooLong);
                }
                let mut rects = Vec::with_capacity(n);
                for _ in 0..n {
                    rects.push(Rect { x: r.u32()?, y: r.u32()?, w: r.u32()?, h: r.u32()? });
                }
                FromHelper::Frame { slot, w, h, rects }
            }
            Self::TITLE => FromHelper::Title(r.str()?),
            Self::URL => FromHelper::Url(r.str()?),
            Self::LOADING => FromHelper::Loading { loading: r.bool()?, progress: r.u8()?.min(100) },
            Self::HISTORY => FromHelper::History { can_back: r.bool()?, can_forward: r.bool()? },
            Self::CURSOR => FromHelper::Cursor(r.u8()?),
            Self::DOWNLOAD => FromHelper::Download { url: r.str()?, name: r.str()? },
            Self::CRASHED => FromHelper::Crashed(r.str()?),
            Self::LOG => FromHelper::Log(r.str()?),
            Self::CLIPBOARD => FromHelper::Clipboard(r.str()?),
            t => return Err(DecodeError::Unknown(t)),
        })
    }
}

fn str_msg(ty: u8, s: &str) -> Vec<u8> {
    let mut w = W::new(ty);
    w.str(s);
    w.finish()
}

/// Read one framed message: `Ok(None)` at a clean end of stream (the peer closed between messages), an error for a stream that
/// stops in the middle of a message or announces a length above [`MAX_MESSAGE`]. Returns the type and the payload.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stream ended inside a message length")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("message length {n} is not allowed")));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    let ty = body[0];
    body.remove(0);
    Ok(Some((ty, body)))
}

/// A byte buffer that turns whatever a socket delivered into whole messages (for readers with timeouts, such as the fake helper).
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }
    /// The next whole message, `Ok(None)` when more bytes are needed, an error for an impossible length.
    pub fn next(&mut self) -> Result<Option<(u8, Vec<u8>)>, String> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let n = u32::from_le_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if n == 0 || n > MAX_MESSAGE {
            return Err(format!("message length {n} is not allowed"));
        }
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let ty = self.buf[4];
        let payload = self.buf[5..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Ok(Some((ty, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(framed: &[u8]) -> (u8, Vec<u8>) {
        let n = u32::from_le_bytes(framed[..4].try_into().unwrap()) as usize;
        assert_eq!(n, framed.len() - 4, "length covers type and payload");
        (framed[4], framed[5..].to_vec())
    }

    fn all_to() -> Vec<ToHelper> {
        vec![
            ToHelper::Hello(Hello { w: 1920, h: 1080, scale_pct: 150, user_agent: "Chrome/130".into(), profile: "/var/lib/iyakki/web-profile/a".into(), locale: "en-US".into(), allow_hosts: vec!["*".into(), "ünï.example".into()], ..Hello::default() }),
            ToHelper::Navigate("https://example.org/?q=ü&r=2".into()),
            ToHelper::Reload { hard: true },
            ToHelper::Stop,
            ToHelper::Back,
            ToHelper::Forward,
            ToHelper::Resize { w: 801, h: 599 },
            ToHelper::Mouse { kind: MouseKind::Down, x: -5, y: 17, button: BUTTON_LEFT, buttons: HELD_LEFT, mods: MOD_SHIFT | MOD_CTRL, clicks: 2 },
            ToHelper::Mouse { kind: MouseKind::Move, x: 3, y: 4, button: 0, buttons: 0, mods: 0, clicks: 0 },
            ToHelper::Wheel { x: 10, y: 20, dx: 0, dy: -240, mods: 0 },
            ToHelper::Key { down: true, keysym: 0xff0d, mods: 0, text: "\r".into() },
            ToHelper::Key { down: false, keysym: 0x1000041, mods: MOD_ALT, text: String::new() },
            ToHelper::Text("héllo \u{1f600}".into()),
            ToHelper::Zoom(125),
            ToHelper::Focus(true),
            ToHelper::SetVisible(false),
            ToHelper::Ack(1),
            ToHelper::Quit,
        ]
    }

    fn all_from() -> Vec<FromHelper> {
        vec![
            FromHelper::Ready { version: 1, engine: "WebKitGTK".into(), engine_version: "2.52.0".into() },
            FromHelper::Frame { slot: 1, w: 1280, h: 720, rects: vec![Rect::new(0, 0, 1280, 720), Rect::new(5, 6, 7, 8)] },
            FromHelper::Frame { slot: 0, w: 1, h: 1, rects: vec![] },
            FromHelper::Title("Ünïcode title".into()),
            FromHelper::Url("https://www.youtube.com/".into()),
            FromHelper::Loading { loading: true, progress: 40 },
            FromHelper::History { can_back: true, can_forward: false },
            FromHelper::Cursor(cursor::POINTER),
            FromHelper::Download { url: "https://a.org/f.zip".into(), name: "f.zip".into() },
            FromHelper::Crashed("web process crashed".into()),
            FromHelper::Log("hello".into()),
            FromHelper::Clipboard("copied".into()),
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for m in all_to() {
            let (ty, p) = split(&m.encode());
            assert_eq!(ToHelper::decode(ty, &p).unwrap(), m);
        }
        for m in all_from() {
            let (ty, p) = split(&m.encode());
            assert_eq!(FromHelper::decode(ty, &p).unwrap(), m);
        }
    }

    #[test]
    fn exact_bytes_of_a_few_messages_are_fixed() {
        assert_eq!(ToHelper::Navigate("ab".into()).encode(), vec![7, 0, 0, 0, 0x02, 2, 0, 0, 0, b'a', b'b']);
        assert_eq!(ToHelper::Resize { w: 0x0102, h: 3 }.encode(), vec![9, 0, 0, 0, 0x07, 2, 1, 0, 0, 3, 0, 0, 0]);
        assert_eq!(ToHelper::Quit.encode(), vec![1, 0, 0, 0, 0x10]);
        assert_eq!(FromHelper::Frame { slot: 1, w: 2, h: 3, rects: vec![Rect::new(0, 0, 2, 3)] }.encode(), vec![30, 0, 0, 0, 0x82, 1, 2, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        for m in all_to().iter().map(|m| m.encode()).chain(all_from().iter().map(|m| m.encode())) {
            let (ty, p) = split(&m);
            for cut in 0..p.len() {
                let ok = if ty < 0x80 { ToHelper::decode(ty, &p[..cut]).is_ok() } else { FromHelper::decode(ty, &p[..cut]).is_ok() };
                assert!(!ok, "type {ty:#x} cut at {cut} of {}", p.len());
            }
        }
    }

    #[test]
    fn extra_bytes_and_unknown_types_are_tolerated_or_reported() {
        let (ty, mut p) = split(&ToHelper::Zoom(150).encode());
        p.extend_from_slice(&[9, 9, 9]);
        assert_eq!(ToHelper::decode(ty, &p).unwrap(), ToHelper::Zoom(150));
        assert_eq!(ToHelper::decode(0x7f, &[]), Err(DecodeError::Unknown(0x7f)));
        assert_eq!(FromHelper::decode(0xff, &[]), Err(DecodeError::Unknown(0xff)));
    }

    #[test]
    fn hostile_lengths_are_refused() {
        // a string that claims 2 GB
        assert_eq!(ToHelper::decode(ToHelper::NAVIGATE, &[0xff, 0xff, 0xff, 0x7f]), Err(DecodeError::TooLong));
        // invalid UTF-8
        assert_eq!(ToHelper::decode(ToHelper::NAVIGATE, &[2, 0, 0, 0, 0xff, 0xfe]), Err(DecodeError::BadUtf8));
        // too many rectangles
        let mut p = vec![0u8, 1, 0, 0, 0, 1, 0, 0, 0];
        p.extend_from_slice(&1000u32.to_le_bytes());
        assert_eq!(FromHelper::decode(FromHelper::FRAME, &p), Err(DecodeError::TooLong));
        // a stream announcing a huge message
        let mut s: &[u8] = &[0xff, 0xff, 0xff, 0xff];
        assert!(read_frame(&mut s).is_err());
        let mut z: &[u8] = &[0, 0, 0, 0];
        assert!(read_frame(&mut z).is_err());
    }

    #[test]
    fn encoding_clamps_long_strings_on_a_character_boundary() {
        let long = "é".repeat(MAX_STRING); // 2 MB
        let (ty, p) = split(&ToHelper::Text(long).encode());
        match ToHelper::decode(ty, &p).unwrap() {
            ToHelper::Text(t) => assert!(t.len() <= MAX_STRING && t.chars().all(|c| c == 'é')),
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_stream_reader_and_framer_agree_and_survive_pieces() {
        let mut stream = Vec::new();
        for m in all_to() {
            stream.extend(m.encode());
        }
        let mut r: &[u8] = &stream;
        let mut got = Vec::new();
        while let Some((t, p)) = read_frame(&mut r).unwrap() {
            got.push(ToHelper::decode(t, &p).unwrap());
        }
        assert_eq!(got, all_to());
        // the same bytes fed 3 at a time
        let mut f = Framer::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(3) {
            f.push(chunk);
            while let Some((t, p)) = f.next().unwrap() {
                got.push(ToHelper::decode(t, &p).unwrap());
            }
        }
        assert_eq!(got, all_to());
        // a stream cut inside a message is an error, between messages a clean end
        let cut = &stream[..stream.len() - 1];
        let mut r: &[u8] = cut;
        let mut err = false;
        loop {
            match read_frame(&mut r) {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    err = true;
                    break;
                }
            }
        }
        assert!(err);
    }

    #[test]
    fn garbage_never_panics() {
        // a simple deterministic generator
        let mut x: u32 = 0x1234_5678;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for _ in 0..5000 {
            let ty = (next() & 0xff) as u8;
            let n = (next() % 40) as usize;
            let p: Vec<u8> = (0..n).map(|_| next() as u8).collect();
            let _ = ToHelper::decode(ty, &p);
            let _ = FromHelper::decode(ty, &p);
        }
    }

    #[test]
    fn rect_helpers() {
        assert_eq!(Rect::new(0, 0, 2, 2).union(Rect::new(5, 5, 1, 1)), Rect::new(0, 0, 6, 6));
        assert_eq!(Rect::new(0, 0, 0, 0).union(Rect::new(1, 1, 1, 1)), Rect::new(1, 1, 1, 1));
        assert_eq!(Rect::new(90, 90, 50, 50).clip(100, 100), Rect::new(90, 90, 10, 10));
        assert!(Rect::new(200, 0, 5, 5).clip(100, 100).is_empty());
        assert!(Rect::new(u32::MAX, 0, u32::MAX, 5).clip(100, 100).is_empty());
    }
}

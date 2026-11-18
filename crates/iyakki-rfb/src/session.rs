//! The RFB protocol as a state machine without I/O. See the crate documentation.

use crate::inflate::Inflater;
use crate::{MAX_CUT, MAX_DIM, MAX_PIXELS, MAX_ZRLE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The server sent something that is not RFB, or something outside the bounds.
    Protocol(String),
    /// The server refused, or wants something this client does not do (a password).
    Refused(String),
    /// The session already failed or ended.
    Closed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Protocol(s) => write!(f, "protocol error: {s}"),
            Error::Refused(s) => write!(f, "{s}"),
            Error::Closed => write!(f, "the connection is closed"),
        }
    }
}

impl std::error::Error for Error {}

fn bad(s: &str) -> Error {
    Error::Protocol(s.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn union(self, o: Rect) -> Rect {
        if self.w == 0 || self.h == 0 {
            return o;
        }
        if o.w == 0 || o.h == 0 {
            return self;
        }
        let (x0, y0) = (self.x.min(o.x), self.y.min(o.y));
        let (x1, y1) = ((self.x + self.w).max(o.x + o.w), (self.y + self.h).max(o.y + o.h));
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The handshake is done; the picture has its first size.
    Ready { w: u32, h: u32 },
    /// Part of the picture changed (consecutive changes are merged).
    Updated(Rect),
    /// The picture has a new size (it is black until the server sends it).
    Resized { w: u32, h: u32 },
    /// The server refused a resize request (the RFB status code).
    ResizeRefused(u32),
    Bell,
    /// Text the server's clipboard got (ISO 8859-1 turned into text).
    CutText(String),
}

/// The remote screen as pixels `0x00RRGGBB`.
#[derive(Clone, Debug, Default)]
pub struct Framebuffer {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

impl Framebuffer {
    pub fn new(w: u32, h: u32) -> Option<Framebuffer> {
        if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM || (w as u64) * (h as u64) > MAX_PIXELS {
            return None;
        }
        Some(Framebuffer { w, h, px: vec![0; (w * h) as usize] })
    }

    fn inside(&self, x: u32, y: u32, w: u32, h: u32) -> bool {
        x.checked_add(w).is_some_and(|e| e <= self.w) && y.checked_add(h).is_some_and(|e| e <= self.h)
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: u32) {
        for yy in y..y + h {
            let a = (yy * self.w + x) as usize;
            if let Some(row) = self.px.get_mut(a..a + w as usize) {
                row.fill(c);
            }
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Option<u32> {
        if x < self.w && y < self.h { self.px.get((y * self.w + x) as usize).copied() } else { None }
    }
}

enum Stop {
    Need,
    Bad(Error),
}

impl From<Error> for Stop {
    fn from(e: Error) -> Stop {
        Stop::Bad(e)
    }
}

type R<T> = Result<T, Stop>;

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let e = self.p.checked_add(n).ok_or(Stop::Need)?;
        let s = self.d.get(self.p..e).ok_or(Stop::Need)?;
        self.p = e;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> R<u32> {
        let b = self.take(2)?;
        Ok(u32::from(b[0]) << 8 | u32::from(b[1]))
    }
    fn u32(&mut self) -> R<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// A 32-bit pixel as the server sends it in our format (blue, green, red, unused).
    fn px4(&mut self) -> R<u32> {
        let b = self.take(4)?;
        Ok(u32::from(b[2]) << 16 | u32::from(b[1]) << 8 | u32::from(b[0]))
    }
    /// A ZRLE compressed pixel: three bytes (blue, green, red).
    fn px3(&mut self) -> R<u32> {
        let b = self.take(3)?;
        Ok(u32::from(b[2]) << 16 | u32::from(b[1]) << 8 | u32::from(b[0]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Version,
    SecList,
    Sec33,
    SecResult,
    Init,
    Msg,
    Update(u32),
}

pub struct Session {
    state: State,
    minor: u32,
    inbuf: Vec<u8>,
    start: usize,
    out: Vec<u8>,
    events: Vec<Event>,
    fb: Framebuffer,
    name: String,
    infl: Inflater,
    ext: bool,
    screen_id: u32,
    screen_flags: u32,
    want_full: bool,
    failed: Option<Error>,
    ready: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

const ENCODINGS: [i32; 6] = [16, 5, 1, 0, -223, -308];
/// More than this waiting unparsed is a server that never finishes a message.
const MAX_BUFFER: usize = MAX_ZRLE + (2 << 20);

impl Session {
    pub fn new() -> Session {
        Session { state: State::Version, minor: 8, inbuf: Vec::new(), start: 0, out: Vec::new(), events: Vec::new(), fb: Framebuffer::default(), name: String::new(), infl: Inflater::new(), ext: false, screen_id: 0, screen_flags: 0, want_full: false, failed: None, ready: false }
    }

    pub fn ready(&self) -> bool {
        self.ready && self.failed.is_none()
    }
    pub fn failed(&self) -> Option<&Error> {
        self.failed.as_ref()
    }
    pub fn fb(&self) -> &Framebuffer {
        &self.fb
    }
    /// The desktop name the server gave (cut to 256 characters).
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The server can resize its screen on request (Extended Desktop Size).
    pub fn can_resize(&self) -> bool {
        self.ext
    }
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn push_event(&mut self, e: Event) {
        if let (Event::Updated(r), Some(Event::Updated(last))) = (&e, self.events.last_mut()) {
            *last = last.union(*r);
            return;
        }
        if self.events.len() >= 1024 {
            self.events.remove(0);
        }
        self.events.push(e);
    }

    /// Bytes that arrived from the server. After an error the session is closed.
    pub fn feed(&mut self, data: &[u8]) -> Result<(), Error> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.inbuf.extend_from_slice(data);
        if self.inbuf.len() - self.start > MAX_BUFFER {
            return self.fail(bad("a message never ends"));
        }
        loop {
            match self.step() {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => return self.fail(e),
            }
        }
        if self.start > 0 && (self.start >= self.inbuf.len() || self.start > (1 << 16)) {
            self.inbuf.drain(..self.start);
            self.start = 0;
        }
        Ok(())
    }

    fn fail(&mut self, e: Error) -> Result<(), Error> {
        self.failed = Some(e.clone());
        self.inbuf = Vec::new();
        self.start = 0;
        Err(e)
    }

    /// One step; Ok(true) when something was consumed.
    fn step(&mut self) -> Result<bool, Error> {
        let buf = std::mem::take(&mut self.inbuf);
        let mut rd = Rd { d: &buf[self.start..], p: 0 };
        let r = self.parse(&mut rd);
        let used = rd.p;
        self.inbuf = buf;
        match r {
            Ok(()) => {
                self.start += used;
                Ok(true)
            }
            Err(Stop::Need) => Ok(false),
            Err(Stop::Bad(e)) => Err(e),
        }
    }

    fn parse(&mut self, rd: &mut Rd) -> R<()> {
        match self.state {
            State::Version => {
                let v = rd.take(12)?;
                if &v[..4] != b"RFB " || v[7] != b'.' || v[11] != b'\n' || !v[4..7].iter().chain(&v[8..11]).all(u8::is_ascii_digit) {
                    return Err(bad("this is not an RFB server").into());
                }
                let minor = std::str::from_utf8(&v[8..11]).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                self.minor = if minor >= 8 { 8 } else if minor == 7 { 7 } else { 3 };
                self.out.extend_from_slice(format!("RFB 003.00{}\n", self.minor).as_bytes());
                self.state = if self.minor == 3 { State::Sec33 } else { State::SecList };
            }
            State::SecList => {
                let n = rd.u8()? as usize;
                if n == 0 {
                    return Err(self.reason(rd)?.into());
                }
                let types = rd.take(n)?;
                if !types.contains(&1) {
                    return Err(Error::Refused("the display server asks for a password (VNC authentication), which is not supported here".into()).into());
                }
                self.out.push(1);
                if self.minor == 8 {
                    self.state = State::SecResult;
                } else {
                    self.client_init();
                }
            }
            State::Sec33 => {
                match rd.u32()? {
                    1 => self.client_init(),
                    0 => return Err(self.reason(rd)?.into()),
                    _ => return Err(Error::Refused("the display server asks for a password (VNC authentication), which is not supported here".into()).into()),
                };
            }
            State::SecResult => {
                if rd.u32()? != 0 {
                    return Err(self.reason(rd)?.into());
                }
                self.client_init();
            }
            State::Init => {
                let h = rd.take(24)?;
                let (w, hh) = (u32::from(h[0]) << 8 | u32::from(h[1]), u32::from(h[2]) << 8 | u32::from(h[3]));
                let n = u32::from_be_bytes([h[20], h[21], h[22], h[23]]) as usize;
                if n > 4096 {
                    return Err(bad("the desktop name is too long").into());
                }
                let name = rd.take(n)?;
                self.name = String::from_utf8_lossy(name).chars().filter(|c| !c.is_control()).take(256).collect();
                self.fb = Framebuffer::new(w, hh).ok_or_else(|| bad("the screen size is outside the bounds"))?;
                self.send_setup();
                self.request_update(false);
                self.ready = true;
                self.state = State::Msg;
                self.push_event(Event::Ready { w, h: hh });
            }
            State::Msg => match rd.u8()? {
                0 => {
                    rd.take(1)?;
                    let n = rd.u16()?;
                    self.state = State::Update(n);
                }
                1 => {
                    rd.take(1)?;
                    rd.u16()?;
                    let n = rd.u16()? as usize;
                    rd.take(n * 6)?;
                }
                2 => self.push_event(Event::Bell),
                3 => {
                    rd.take(3)?;
                    let len = rd.u32()?;
                    if len as i32 <= 0 && len != 0 || len as usize > MAX_CUT {
                        return Err(bad("the clipboard text is outside the bounds").into());
                    }
                    let t = rd.take(len as usize)?;
                    let text: String = t.iter().map(|&b| b as char).filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
                    self.push_event(Event::CutText(text));
                }
                t => return Err(bad(&format!("unknown message type {t}")).into()),
            },
            State::Update(0) => {
                let full = std::mem::take(&mut self.want_full);
                self.request_update(!full);
                self.state = State::Msg;
            }
            State::Update(left) => {
                let (x, y, w, h) = (rd.u16()?, rd.u16()?, rd.u16()?, rd.u16()?);
                let enc = rd.u32()? as i32;
                self.rect(rd, x, y, w, h, enc)?;
                self.state = State::Update(left - 1);
            }
        }
        Ok(())
    }

    fn reason(&self, rd: &mut Rd) -> R<Error> {
        let n = rd.u32()? as usize;
        if n > 4096 {
            return Ok(Error::Refused("the display server refused the connection".into()));
        }
        let t = rd.take(n)?;
        let t: String = String::from_utf8_lossy(t).chars().filter(|c| !c.is_control()).collect();
        Ok(Error::Refused(format!("the display server refused the connection: {t}")))
    }

    fn client_init(&mut self) {
        self.out.push(1);
        self.state = State::Init;
    }

    fn send_setup(&mut self) {
        // 32 bits per pixel, depth 24, little endian, true colour, red << 16, green << 8, blue
        self.out.extend_from_slice(&[0, 0, 0, 0, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        self.out.extend_from_slice(&[2, 0, 0, ENCODINGS.len() as u8]);
        for e in ENCODINGS {
            self.out.extend_from_slice(&e.to_be_bytes());
        }
    }

    /// Ask for the next picture (all of it, or only what changed).
    pub fn request_update(&mut self, incremental: bool) {
        let (w, h) = (self.fb.w.min(65535) as u16, self.fb.h.min(65535) as u16);
        self.out.extend_from_slice(&[3, incremental as u8, 0, 0, 0, 0]);
        self.out.extend_from_slice(&w.to_be_bytes());
        self.out.extend_from_slice(&h.to_be_bytes());
    }

    // ------------------------------------------------------------------------------------------------ rectangles

    fn rect(&mut self, rd: &mut Rd, x: u32, y: u32, w: u32, h: u32, enc: i32) -> R<()> {
        match enc {
            -223 => {
                self.resize(w, h)?;
                self.want_full = true;
                return Ok(());
            }
            -308 => {
                let n = {
                    let hd = rd.take(4)?;
                    hd[0] as usize
                };
                let screens = rd.take(n * 16)?;
                let (reason, status) = (x, y);
                if reason == 1 && status != 0 {
                    self.push_event(Event::ResizeRefused(status));
                    return Ok(());
                }
                if status != 0 {
                    return Ok(());
                }
                self.ext = true;
                if n > 0 {
                    self.screen_id = u32::from_be_bytes([screens[0], screens[1], screens[2], screens[3]]);
                    self.screen_flags = u32::from_be_bytes([screens[12], screens[13], screens[14], screens[15]]);
                }
                if (w, h) != (self.fb.w, self.fb.h) {
                    self.resize(w, h)?;
                    self.want_full = true;
                }
                return Ok(());
            }
            _ => {}
        }
        if !self.fb.inside(x, y, w, h) {
            return Err(bad("a rectangle lies outside the screen").into());
        }
        if w == 0 || h == 0 {
            // nothing to draw; the encodings below carry no data for an empty rectangle except ZRLE's length
            if enc != 16 {
                return Ok(());
            }
        }
        let dirty = Rect { x, y, w, h };
        match enc {
            0 => {
                let n = (w as usize) * (h as usize);
                let d = rd.take(n * 4)?;
                for (i, c) in d.chunks_exact(4).enumerate() {
                    let (xx, yy) = (x + (i as u32 % w), y + (i as u32 / w));
                    self.fb.px[(yy * self.fb.w + xx) as usize] = u32::from(c[2]) << 16 | u32::from(c[1]) << 8 | u32::from(c[0]);
                }
            }
            1 => {
                let (sx, sy) = (rd.u16()?, rd.u16()?);
                if !self.fb.inside(sx, sy, w, h) {
                    return Err(bad("a CopyRect source lies outside the screen").into());
                }
                let fw = self.fb.w;
                let rows: Vec<Vec<u32>> = (0..h).map(|r| self.fb.px[((sy + r) * fw + sx) as usize..((sy + r) * fw + sx + w) as usize].to_vec()).collect();
                for (r, row) in rows.iter().enumerate() {
                    let a = ((y + r as u32) * fw + x) as usize;
                    self.fb.px[a..a + w as usize].copy_from_slice(row);
                }
            }
            5 => self.hextile(rd, x, y, w, h)?,
            16 => {
                let len = rd.u32()? as usize;
                if len > MAX_ZRLE {
                    return Err(bad("a ZRLE rectangle is too large").into());
                }
                let data = rd.take(len)?;
                if w == 0 || h == 0 {
                    return Ok(());
                }
                let tiles = (w.div_ceil(64) * h.div_ceil(64)) as usize;
                let cap = 4 * (w as usize) * (h as usize) + 2 * tiles + 1024;
                let raw = self.infl.inflate(data, cap).map_err(|e| bad(&format!("bad ZRLE data: {e:?}")))?;
                self.zrle(&raw, x, y, w, h)?;
            }
            e => return Err(bad(&format!("unsupported encoding {e}")).into()),
        }
        self.push_event(Event::Updated(dirty));
        Ok(())
    }

    fn resize(&mut self, w: u32, h: u32) -> Result<(), Error> {
        self.fb = Framebuffer::new(w, h).ok_or_else(|| bad("the new screen size is outside the bounds"))?;
        self.push_event(Event::Resized { w, h });
        Ok(())
    }

    fn hextile(&mut self, rd: &mut Rd, x: u32, y: u32, w: u32, h: u32) -> R<()> {
        let (mut bg, mut fg) = (0u32, 0u32);
        let mut ty = 0;
        while ty < h {
            let th = (h - ty).min(16);
            let mut tx = 0;
            while tx < w {
                let tw = (w - tx).min(16);
                let sub = rd.u8()?;
                if sub & 1 != 0 {
                    for r in 0..th {
                        for c in 0..tw {
                            let p = rd.px4()?;
                            let i = ((y + ty + r) * self.fb.w + x + tx + c) as usize;
                            self.fb.px[i] = p;
                        }
                    }
                } else {
                    if sub & 2 != 0 {
                        bg = rd.px4()?;
                    }
                    if sub & 4 != 0 {
                        fg = rd.px4()?;
                    }
                    self.fb.fill(x + tx, y + ty, tw, th, bg);
                    if sub & 8 != 0 {
                        let n = rd.u8()?;
                        for _ in 0..n {
                            let c = if sub & 16 != 0 { rd.px4()? } else { fg };
                            let (xy, wh) = (rd.u8()?, rd.u8()?);
                            let (sx, sy, sw, sh) = (u32::from(xy >> 4), u32::from(xy & 15), u32::from(wh >> 4) + 1, u32::from(wh & 15) + 1);
                            if sx + sw > tw || sy + sh > th {
                                return Err(bad("a Hextile subrectangle lies outside its tile").into());
                            }
                            self.fb.fill(x + tx + sx, y + ty + sy, sw, sh, c);
                        }
                    }
                }
                tx += 16;
            }
            ty += 16;
        }
        Ok(())
    }

    fn zrle(&mut self, raw: &[u8], x: u32, y: u32, w: u32, h: u32) -> Result<(), Error> {
        let mut rd = Rd { d: raw, p: 0 };
        let short = |_: Stop| bad("ZRLE data ends inside a tile");
        let mut ty = 0;
        while ty < h {
            let th = (h - ty).min(64);
            let mut tx = 0;
            while tx < w {
                let tw = (w - tx).min(64);
                self.zrle_tile(&mut rd, x + tx, y + ty, tw, th).map_err(|e| match e {
                    Stop::Bad(b) => b,
                    s => short(s),
                })?;
                tx += 64;
            }
            ty += 64;
        }
        Ok(())
    }

    fn zrle_tile(&mut self, rd: &mut Rd, x: u32, y: u32, tw: u32, th: u32) -> R<()> {
        let sub = rd.u8()?;
        let fw = self.fb.w;
        let put = |fb: &mut Framebuffer, i: u32, c: u32| {
            let (xx, yy) = (x + i % tw, y + i / tw);
            fb.px[(yy * fw + xx) as usize] = c;
        };
        let total = tw * th;
        match sub {
            0 => {
                for i in 0..total {
                    let c = rd.px3()?;
                    put(&mut self.fb, i, c);
                }
            }
            1 => {
                let c = rd.px3()?;
                self.fb.fill(x, y, tw, th, c);
            }
            2..=16 => {
                let n = sub as usize;
                let mut pal = [0u32; 16];
                for p in pal.iter_mut().take(n) {
                    *p = rd.px3()?;
                }
                let bits = if n == 2 { 1 } else if n <= 4 { 2 } else { 4 };
                for r in 0..th {
                    let row = rd.take(((tw * bits + 7) / 8) as usize)?;
                    for c in 0..tw {
                        let bitpos = c * bits;
                        let byte = row[(bitpos / 8) as usize];
                        let idx = ((byte >> (8 - bits - (bitpos % 8))) & ((1 << bits) - 1)) as usize;
                        put(&mut self.fb, r * tw + c, pal[idx.min(n - 1)]);
                    }
                }
            }
            128 => {
                let mut i = 0;
                while i < total {
                    let c = rd.px3()?;
                    let run = run_len(rd)?;
                    if run > total - i {
                        return Err(bad("a ZRLE run leaves its tile").into());
                    }
                    for k in 0..run {
                        put(&mut self.fb, i + k, c);
                    }
                    i += run;
                }
            }
            130..=255 => {
                let n = (sub - 128) as usize;
                let mut pal = [0u32; 127];
                for p in pal.iter_mut().take(n) {
                    *p = rd.px3()?;
                }
                let mut i = 0;
                while i < total {
                    let b = rd.u8()?;
                    let idx = (b & 127) as usize;
                    if idx >= n {
                        return Err(bad("a ZRLE palette index is out of range").into());
                    }
                    let run = if b & 128 != 0 { run_len(rd)? } else { 1 };
                    if run > total - i {
                        return Err(bad("a ZRLE run leaves its tile").into());
                    }
                    for k in 0..run {
                        put(&mut self.fb, i + k, pal[idx]);
                    }
                    i += run;
                }
            }
            _ => return Err(bad("an invalid ZRLE tile type").into()),
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------ what the client sends

    pub fn key_event(&mut self, down: bool, keysym: u32) {
        self.out.extend_from_slice(&[4, down as u8, 0, 0]);
        self.out.extend_from_slice(&keysym.to_be_bytes());
    }

    /// `buttons`: bit 0 left, 1 middle, 2 right, 3 wheel up, 4 wheel down, 5 wheel left, 6 wheel right. The position is kept on the screen.
    pub fn pointer_event(&mut self, x: i32, y: i32, buttons: u8) {
        let px = x.clamp(0, (self.fb.w.max(1) - 1) as i32) as u16;
        let py = y.clamp(0, (self.fb.h.max(1) - 1) as i32) as u16;
        self.out.extend_from_slice(&[5, buttons]);
        self.out.extend_from_slice(&px.to_be_bytes());
        self.out.extend_from_slice(&py.to_be_bytes());
    }

    /// Put text on the server's clipboard (ISO 8859-1: other characters become `?`; control characters except newline and tab are dropped).
    pub fn cut_text(&mut self, text: &str) {
        let b: Vec<u8> = text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).take(64 * 1024).collect();
        self.out.extend_from_slice(&[6, 0, 0, 0]);
        self.out.extend_from_slice(&(b.len() as u32).to_be_bytes());
        self.out.extend_from_slice(&b);
    }

    /// Ask the server to make its screen `w` x `h` (bounded to what is allowed). False when the server cannot resize.
    pub fn request_resize(&mut self, w: u32, h: u32) -> bool {
        if !self.ext || self.failed.is_some() {
            return false;
        }
        let (mut w, mut h) = (w.clamp(64, MAX_DIM), h.clamp(64, MAX_DIM));
        while (w as u64) * (h as u64) > MAX_PIXELS {
            w = (w * 7 / 8).max(64);
            h = (h * 7 / 8).max(64);
        }
        self.out.extend_from_slice(&[251, 0]);
        self.out.extend_from_slice(&(w as u16).to_be_bytes());
        self.out.extend_from_slice(&(h as u16).to_be_bytes());
        self.out.extend_from_slice(&[1, 0]);
        self.out.extend_from_slice(&self.screen_id.to_be_bytes());
        self.out.extend_from_slice(&[0, 0, 0, 0]);
        self.out.extend_from_slice(&(w as u16).to_be_bytes());
        self.out.extend_from_slice(&(h as u16).to_be_bytes());
        self.out.extend_from_slice(&self.screen_flags.to_be_bytes());
        true
    }
}

/// A run length: 1 plus the sum of bytes, each 255 meaning another follows; bounded by one tile.
fn run_len(rd: &mut Rd) -> R<u32> {
    let mut n = 1u32;
    loop {
        let b = rd.u8()?;
        n += u32::from(b);
        if b != 255 {
            return Ok(n);
        }
        if n > 4096 {
            return Err(bad("a ZRLE run is too long").into());
        }
    }
}


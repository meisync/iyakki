//! The Touch Bar's display ("DFR") from user space: the wire format, the USB descriptor reading, the session with the device
//! and the safe way to bring a stuck device up. Pure logic behind two small seams ([`Usb`] for the interface the display is
//! drawn through, [`Port`] for the device as a whole), so everything here is tested with a fake device that can time out.
//! The real seams (usbfs ioctls, sysfs) are in `usbfs.rs`.
//!
//! **Clean-room.** The protocol below was written down from the *description* of the wire format in the mainline Linux driver
//! `drivers/gpu/drm/tiny/appletbdrm.c` (v6.15, GPL-2.0, by Kerem Karabay), read as a reference only: field offsets, message tags
//! and the order of the start-up messages. No code was copied; the structures, the encoder, the decoder and the tests here are
//! this project's own. Unknown header fields are sent as the values the kernel driver is documented to send; they are marked
//! `opaque` below because nothing public says what they mean. `docs/TOUCH-BAR.md` lists what is verified and what is not.
//!
//! **What the device is.** USB `05ac:8302` ("Touch Bar Display") behind the T2's virtual USB host (apple-bce). Besides the HID
//! interfaces that give Esc, F-keys or media keys (`hid-appletb-kbd`), it has a vendor interface (class 0x10) with one bulk OUT
//! and one bulk IN endpoint. Messages are little-endian. A request goes out on the OUT endpoint, the answer comes back on the
//! IN endpoint. The panel is 2170 x 60 pixels, 24 bits per pixel, blue-green-red order.
//!
//! **Orientation.** The device is built portrait: the kernel driver reports the panel as `width` x `height` = 2170 x 60 and swaps
//! the axes for the frame header. In the coordinates used here (x along the long side, 0 at the left; y across, 0 at the top) a
//! rectangle's header holds (x, y, width, height) as is, and its pixel data runs *column by column*: for each x from left to
//! right, the pixels from the bottom row up to the top row. That reading of the kernel's description is **not yet checked on
//! the device**: [`Orient`] has the two flips that would fix the picture if it came out mirrored, and the settings carry them.

use std::path::{Path, PathBuf};

pub const VENDOR: u16 = 0x05ac;
/// The display and its HID keys.
pub const DISPLAY_PRODUCT: u16 = 0x8302;
/// The backlight (`hid-appletb-bl`); never touched by the display code.
pub const BACKLIGHT_PRODUCT: u16 = 0x8102;

/// Message tags (the 32-bit value is sent little-endian): CLRD, GINF, UDCL, REDY.
pub const MSG_CLEAR: u32 = 0x434c_5244;
pub const MSG_INFORMATION: u32 = 0x4749_4e46;
pub const MSG_UPDATE_COMPLETE: u32 = 0x5544_434c;
pub const MSG_READY: u32 = 0x5245_4459;
/// "RGBA" as the device names the pixel format (the pixels really are 24-bit BGR).
pub const PIXEL_FORMAT: u32 = 0x5247_4241;
pub const BITS_PER_PIXEL: u8 = 24;
/// The time the kernel allows for one bulk transfer.
pub const TIMEOUT_MS: u32 = 1000;

/// Bigger than any real panel by far; a response claiming more is garbage.
pub const MAX_SIDE: u32 = 8192;
/// Sizes of the fixed parts.
pub const REQUEST_HEADER: usize = 16;
pub const SIMPLE_REQUEST: usize = 32;
pub const INFO_RESPONSE: usize = 65;
pub const UPDATE_RESPONSE: usize = 40;
const FB_PREFIX: usize = 48;
const FRAME_HEADER: usize = 12;
const FOOTER: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoError {
    Short { need: usize, got: usize },
    Unexpected(u32),
    Format(String),
    Timestamp,
    TooBig,
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Short { need, got } => write!(f, "answer too short ({got} of {need} bytes)"),
            ProtoError::Unexpected(m) => write!(f, "unexpected answer {}", tag_text(*m)),
            ProtoError::Format(s) => write!(f, "{s}"),
            ProtoError::Timestamp => write!(f, "the answer is for another frame"),
            ProtoError::TooBig => write!(f, "frame too big for one message"),
        }
    }
}

/// A tag as the four characters the device sees (little-endian bytes).
pub fn tag_text(t: u32) -> String {
    t.to_le_bytes().iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '?' }).collect()
}

fn put16(v: &mut [u8], at: usize, x: u16) {
    if let Some(s) = v.get_mut(at..at + 2) {
        s.copy_from_slice(&x.to_le_bytes());
    }
}
fn put32(v: &mut [u8], at: usize, x: u32) {
    if let Some(s) = v.get_mut(at..at + 4) {
        s.copy_from_slice(&x.to_le_bytes());
    }
}
fn put64(v: &mut [u8], at: usize, x: u64) {
    if let Some(s) = v.get_mut(at..at + 8) {
        s.copy_from_slice(&x.to_le_bytes());
    }
}
fn get32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn get64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The 32-byte request that carries only a tag (information, ready, clear).
///
/// Layout: u16 2, u16 0x1512 (opaque), u32 0, u32 0, u32 16 (bytes after this header), u32 tag, 8 zero bytes, u32 16.
pub fn simple_request(tag: u32) -> [u8; SIMPLE_REQUEST] {
    let mut v = [0u8; SIMPLE_REQUEST];
    put16(&mut v, 0, 2);
    put16(&mut v, 2, 0x1512);
    put32(&mut v, 12, (SIMPLE_REQUEST - REQUEST_HEADER) as u32);
    put32(&mut v, 16, tag);
    put32(&mut v, 28, (SIMPLE_REQUEST - REQUEST_HEADER) as u32);
    v
}

/// The tag of an answer (bytes 16..20 of every answer).
pub fn response_tag(buf: &[u8]) -> Option<u32> {
    get32(buf, 16)
}

/// What the display says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// As reported: the long side (2170 on the 2016-2020 Touch Bars).
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: u32,
    pub orientation: u32,
}

/// Decode the answer to the information request. Bytes: tag at 16; width at 32, height at 36, bits per pixel at 40, bytes per row
/// at 41, orientation at 45, bitmap info at 49, pixel format at 53 (all u32 little-endian, unaligned).
pub fn parse_information(buf: &[u8]) -> Result<Info, ProtoError> {
    if buf.len() < INFO_RESPONSE {
        return Err(ProtoError::Short { need: INFO_RESPONSE, got: buf.len() });
    }
    let tag = response_tag(buf).ok_or(ProtoError::Short { need: 20, got: buf.len() })?;
    if tag != MSG_INFORMATION {
        return Err(ProtoError::Unexpected(tag));
    }
    let (w, h) = (get32(buf, 32).unwrap_or(0), get32(buf, 36).unwrap_or(0));
    let bpp = buf.get(40).copied().unwrap_or(0);
    if bpp != BITS_PER_PIXEL {
        return Err(ProtoError::Format(format!("{bpp} bits per pixel, 24 expected")));
    }
    let fmt = get32(buf, 53).unwrap_or(0);
    if fmt != PIXEL_FORMAT {
        return Err(ProtoError::Format(format!("pixel format {}, RGBA expected", tag_text(fmt))));
    }
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(ProtoError::Format(format!("a {w} x {h} panel")));
    }
    Ok(Info { width: w, height: h, bytes_per_row: get32(buf, 41).unwrap_or(0), orientation: get32(buf, 45).unwrap_or(0) })
}

impl Info {
    /// The picture size to draw: the long side first (the device reports 2170 x 60; a device that reported it the other way
    /// round is turned).
    pub fn landscape(&self) -> (u32, u32) {
        (self.width.max(self.height), self.width.min(self.height))
    }
}

/// A rectangle in picture coordinates (x along the long side).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn right(&self) -> u32 {
        self.x.saturating_add(self.w)
    }
    pub fn bottom(&self) -> u32 {
        self.y.saturating_add(self.h)
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    /// The part inside a `w` x `h` picture.
    pub fn clip(&self, w: u32, h: u32) -> Rect {
        let (x, y) = (self.x.min(w), self.y.min(h));
        Rect { x, y, w: self.right().min(w) - x, h: self.bottom().min(h) - y }
    }
}

/// How the pixels of a rectangle are put on the wire. See the module note: the default is the reading of the kernel's description.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Orient {
    /// Send the columns right to left.
    pub flip_x: bool,
    /// Send each column top to bottom instead of bottom to top.
    pub flip_y: bool,
}

impl Orient {
    pub fn from_bits(b: u8) -> Orient {
        Orient { flip_x: b & 1 != 0, flip_y: b & 2 != 0 }
    }
    pub fn bits(self) -> u8 {
        self.flip_x as u8 | (self.flip_y as u8) << 1
    }
}

/// The bytes of one rectangle of a picture (`rgba`: `pic_w` x `pic_h`, 4 bytes a pixel, row by row) in wire order, 3 bytes a
/// pixel (blue, green, red). Pixels outside the picture are black. Never panics.
pub fn pack_rect(rgba: &[u8], pic_w: u32, pic_h: u32, r: Rect, o: Orient) -> Vec<u8> {
    let r = r.clip(pic_w, pic_h);
    let mut out = Vec::with_capacity(r.w as usize * r.h as usize * 3);
    let cols: Vec<u32> = if o.flip_x { (r.x..r.right()).rev().collect() } else { (r.x..r.right()).collect() };
    for x in cols {
        let rows: Vec<u32> = if o.flip_y { (r.y..r.bottom()).collect() } else { (r.y..r.bottom()).rev().collect() };
        for y in rows {
            let at = (y as usize * pic_w as usize + x as usize) * 4;
            match rgba.get(at..at + 3) {
                Some(p) => out.extend_from_slice(&[p[2], p[1], p[0]]),
                None => out.extend_from_slice(&[0, 0, 0]),
            }
        }
    }
    out
}

/// A frame update: one or more rectangles with their wire-order data, a frame id and a timestamp the answer repeats.
///
/// Layout: the 16-byte header (u16 2, u16 0x12, u32 9 (opaque), u32 0, u32 bytes after the header), u16 1 (opaque), the low byte of
/// the timestamp as frame id, 29 zero bytes (48 so far); per rectangle u16 x, u16 y, u16 width, u16 height, u32 data size, data;
/// an 80-byte footer (u32 0xfffe at 12, u32 0x80001 at 28, u64 timestamp at 32, u32 0x80002 at 52, u32 0xffff at 76, the rest
/// zero); the whole padded with zeros to a multiple of 16 bytes.
pub fn update_request(rects: &[(Rect, Vec<u8>)], timestamp: u64) -> Result<Vec<u8>, ProtoError> {
    let mut body = 0usize;
    for (r, data) in rects {
        if data.len() != r.w as usize * r.h as usize * 3 || r.w > u16::MAX as u32 || r.h > u16::MAX as u32 || r.x > u16::MAX as u32 || r.y > u16::MAX as u32 {
            return Err(ProtoError::TooBig);
        }
        body += FRAME_HEADER + data.len();
    }
    let total = (FB_PREFIX + body + FOOTER).div_ceil(16) * 16;
    let mut v = vec![0u8; total];
    put16(&mut v, 0, 2);
    put16(&mut v, 2, 0x12);
    put32(&mut v, 4, 9);
    put32(&mut v, 12, (total - REQUEST_HEADER) as u32);
    put16(&mut v, 16, 1);
    if let Some(b) = v.get_mut(18) {
        *b = timestamp as u8;
    }
    let mut at = FB_PREFIX;
    for (r, data) in rects {
        put16(&mut v, at, r.x as u16);
        put16(&mut v, at + 2, r.y as u16);
        put16(&mut v, at + 4, r.w as u16);
        put16(&mut v, at + 6, r.h as u16);
        put32(&mut v, at + 8, data.len() as u32);
        if let Some(s) = v.get_mut(at + FRAME_HEADER..at + FRAME_HEADER + data.len()) {
            s.copy_from_slice(data);
        }
        at += FRAME_HEADER + data.len();
    }
    put32(&mut v, at + 12, 0xfffe);
    put32(&mut v, at + 28, 0x8_0001);
    put64(&mut v, at + 32, timestamp);
    put32(&mut v, at + 52, 0x8_0002);
    put32(&mut v, at + 76, 0xffff);
    Ok(v)
}

/// Check the answer to a frame update: tag UDCL and the timestamp of the request at 32.
pub fn parse_update_response(buf: &[u8], timestamp: u64) -> Result<(), ProtoError> {
    if buf.len() < UPDATE_RESPONSE {
        return Err(ProtoError::Short { need: UPDATE_RESPONSE, got: buf.len() });
    }
    let tag = response_tag(buf).ok_or(ProtoError::Short { need: 20, got: buf.len() })?;
    if tag != MSG_UPDATE_COMPLETE {
        return Err(ProtoError::Unexpected(tag));
    }
    if get64(buf, 32) != Some(timestamp) {
        return Err(ProtoError::Timestamp);
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ USB descriptors

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub address: u8,
    pub attributes: u8,
    pub max_packet: u16,
}

impl Endpoint {
    pub fn is_bulk(&self) -> bool {
        self.attributes & 3 == 2
    }
    pub fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub alt: u8,
    pub class: u8,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub value: u8,
    pub interfaces: Vec<Interface>,
}

/// Parse the raw descriptors a device gives (sysfs `descriptors`, or what usbfs returns when read): a device descriptor, then per
/// configuration its descriptor, interfaces and endpoints. A truncated or odd list gives what could be read; never panics.
pub fn parse_descriptors(raw: &[u8]) -> Vec<Config> {
    let mut cfgs: Vec<Config> = Vec::new();
    let mut at = 0usize;
    while at + 2 <= raw.len() {
        let len = raw[at] as usize;
        if len < 2 || at + len > raw.len() {
            break;
        }
        let d = &raw[at..at + len];
        match (d[1], len) {
            (2, 9..) => cfgs.push(Config { value: d[5], interfaces: Vec::new() }),
            (4, 9..) => {
                if let Some(c) = cfgs.last_mut() {
                    c.interfaces.push(Interface { number: d[2], alt: d[3], class: d[5], endpoints: Vec::new() });
                }
            }
            (5, 7..) => {
                if let Some(i) = cfgs.last_mut().and_then(|c| c.interfaces.last_mut()) {
                    i.endpoints.push(Endpoint { address: d[2], attributes: d[3], max_packet: u16::from_le_bytes([d[4], d[5]]) });
                }
            }
            _ => {}
        }
        at += len;
    }
    cfgs
}

/// Where the display is drawn through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayIface {
    pub config: u8,
    pub interface: u8,
    pub alt: u8,
    pub ep_out: u8,
    pub ep_in: u8,
}

/// The vendor interface with one bulk IN and one bulk OUT endpoint (class 0x10, the one the kernel driver binds), and the
/// configuration it belongs to. The first alternate setting that fits wins.
pub fn find_display(cfgs: &[Config]) -> Option<DisplayIface> {
    for c in cfgs {
        for i in &c.interfaces {
            if i.class != 0x10 {
                continue;
            }
            let out = i.endpoints.iter().find(|e| e.is_bulk() && !e.is_in());
            let inn = i.endpoints.iter().find(|e| e.is_bulk() && e.is_in());
            if let (Some(o), Some(n)) = (out, inn) {
                return Some(DisplayIface { config: c.value, interface: i.number, alt: i.alt, ep_out: o.address, ep_in: n.address });
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------ the seam to the device

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbError {
    Timeout,
    NoDevice,
    Busy,
    Stall,
    Other(i32),
}

impl UsbError {
    pub fn from_errno(e: i32) -> UsbError {
        match e {
            110 => UsbError::Timeout,
            19 | 108 | 71 => UsbError::NoDevice,
            16 => UsbError::Busy,
            32 => UsbError::Stall,
            n => UsbError::Other(n),
        }
    }
}

impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsbError::Timeout => write!(f, "timed out (-110)"),
            UsbError::NoDevice => write!(f, "the device is gone"),
            UsbError::Busy => write!(f, "another driver holds the interface"),
            UsbError::Stall => write!(f, "endpoint stalled"),
            UsbError::Other(n) => write!(f, "error {n}"),
        }
    }
}

/// One claimed device as the display code sees it (usbfs on the real machine).
pub trait Usb {
    fn claim(&mut self, interface: u8) -> Result<(), UsbError>;
    fn release(&mut self, interface: u8);
    fn bulk_out(&mut self, ep: u8, data: &[u8], timeout_ms: u32) -> Result<usize, UsbError>;
    fn bulk_in(&mut self, ep: u8, buf: &mut [u8], timeout_ms: u32) -> Result<usize, UsbError>;
    fn clear_halt(&mut self, ep: u8) -> Result<(), UsbError>;
}

impl<T: Usb + ?Sized> Usb for Box<T> {
    fn claim(&mut self, interface: u8) -> Result<(), UsbError> {
        (**self).claim(interface)
    }
    fn release(&mut self, interface: u8) {
        (**self).release(interface)
    }
    fn bulk_out(&mut self, ep: u8, data: &[u8], timeout_ms: u32) -> Result<usize, UsbError> {
        (**self).bulk_out(ep, data, timeout_ms)
    }
    fn bulk_in(&mut self, ep: u8, buf: &mut [u8], timeout_ms: u32) -> Result<usize, UsbError> {
        (**self).bulk_in(ep, buf, timeout_ms)
    }
    fn clear_halt(&mut self, ep: u8) -> Result<(), UsbError> {
        (**self).clear_halt(ep)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DfrError {
    Usb(UsbError),
    Proto(ProtoError),
    /// Too many failures in a row: stop and leave the keys to the firmware.
    Dead,
}

impl std::fmt::Display for DfrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DfrError::Usb(e) => write!(f, "{e}"),
            DfrError::Proto(e) => write!(f, "{e}"),
            DfrError::Dead => write!(f, "too many failures in a row"),
        }
    }
}

impl From<UsbError> for DfrError {
    fn from(e: UsbError) -> DfrError {
        DfrError::Usb(e)
    }
}
impl From<ProtoError> for DfrError {
    fn from(e: ProtoError) -> DfrError {
        DfrError::Proto(e)
    }
}

/// Failures in a row after which a session is [`DfrError::Dead`].
pub const MAX_FAILS: u32 = 3;

/// A running conversation with the display. Opening claims the interface and does the start-up the kernel driver does: ask
/// for the information, tell the device we are ready, clear the screen. Any error counts; [`MAX_FAILS`] in a row end it.
pub struct Dfr<U: Usb> {
    usb: U,
    pub iface: DisplayIface,
    pub info: Info,
    fails: u32,
    frames: u64,
    claimed: bool,
}

impl<U: Usb> Dfr<U> {
    /// Claim the interface and start. On any error the interface is released again.
    pub fn open(mut usb: U, iface: DisplayIface) -> Result<Dfr<U>, DfrError> {
        usb.claim(iface.interface)?;
        let mut d = Dfr { usb, iface, info: Info { width: 0, height: 0, bytes_per_row: 0, orientation: 0 }, fails: 0, frames: 0, claimed: true };
        match d.start() {
            Ok(info) => {
                d.info = info;
                Ok(d)
            }
            Err(e) => {
                d.close_quiet();
                Err(e)
            }
        }
    }

    fn start(&mut self) -> Result<Info, DfrError> {
        self.send(&simple_request(MSG_INFORMATION))?;
        let buf = self.read(INFO_RESPONSE, MSG_INFORMATION)?;
        let info = parse_information(&buf)?;
        self.send(&simple_request(MSG_READY))?;
        self.send(&simple_request(MSG_CLEAR))?;
        Ok(info)
    }

    fn send(&mut self, data: &[u8]) -> Result<(), DfrError> {
        match self.usb.bulk_out(self.iface.ep_out, data, TIMEOUT_MS) {
            Ok(n) if n == data.len() => Ok(()),
            Ok(n) => Err(DfrError::Proto(ProtoError::Short { need: data.len(), got: n })),
            Err(e) => Err(e.into()),
        }
    }

    /// Read one answer; the device answers the first request after a configuration with a readiness signal, which is read past once.
    fn read(&mut self, need: usize, want: u32) -> Result<Vec<u8>, DfrError> {
        let mut seen_ready = false;
        loop {
            let mut buf = vec![0u8; need.max(512)];
            let n = self.usb.bulk_in(self.iface.ep_in, &mut buf, TIMEOUT_MS)?;
            buf.truncate(n);
            match response_tag(&buf) {
                Some(MSG_READY) if !seen_ready => {
                    seen_ready = true;
                    continue;
                }
                Some(t) if t == want => return Ok(buf),
                Some(t) => return Err(ProtoError::Unexpected(t).into()),
                None => return Err(ProtoError::Short { need: 20, got: buf.len() }.into()),
            }
        }
    }

    fn note(&mut self, r: Result<(), DfrError>) -> Result<(), DfrError> {
        match r {
            Ok(()) => {
                self.fails = 0;
                Ok(())
            }
            Err(e) => {
                self.fails += 1;
                if matches!(e, DfrError::Usb(UsbError::Stall)) {
                    let _ = self.usb.clear_halt(self.iface.ep_out);
                    let _ = self.usb.clear_halt(self.iface.ep_in);
                }
                if matches!(e, DfrError::Usb(UsbError::NoDevice)) || self.fails >= MAX_FAILS {
                    Err(DfrError::Dead)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Draw rectangles (picture coordinates; the data in wire order, see [`pack_rect`]). `now_ns` is any value that changes from
    /// frame to frame (the kernel uses its clock in nanoseconds).
    pub fn update(&mut self, rects: &[(Rect, Vec<u8>)], now_ns: u64) -> Result<(), DfrError> {
        if rects.is_empty() {
            return Ok(());
        }
        self.frames += 1;
        let ts = now_ns.max(self.frames);
        let r = (|| {
            let req = update_request(rects, ts)?;
            self.send(&req)?;
            let buf = self.read(UPDATE_RESPONSE, MSG_UPDATE_COMPLETE)?;
            Ok(parse_update_response(&buf, ts)?)
        })();
        self.note(r)
    }

    /// Blank the display.
    pub fn clear(&mut self) -> Result<(), DfrError> {
        let r = self.send(&simple_request(MSG_CLEAR));
        self.note(r)
    }

    pub fn frames_sent(&self) -> u64 {
        self.frames
    }

    fn close_quiet(&mut self) {
        if self.claimed {
            self.claimed = false;
            self.usb.release(self.iface.interface);
        }
    }

    /// Clear the display (best effort) and give the interface back.
    pub fn close(mut self) {
        let _ = self.send(&simple_request(MSG_CLEAR));
        self.close_quiet();
    }
}

impl<U: Usb> Drop for Dfr<U> {
    fn drop(&mut self) {
        self.close_quiet();
    }
}

// ------------------------------------------------------------------------------------------------ finding the device

/// A Touch Bar display in sysfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub sysfs: PathBuf,
    pub bus: u32,
    pub dev: u32,
    /// `bConfigurationValue`; `None` while the device has no configuration (the boot-time -110).
    pub configured: Option<u8>,
}

impl Found {
    /// The usbfs node (`/dev/bus/usb/BBB/DDD`) under `dev_root` (`/dev`).
    pub fn usbfs_path(&self, dev_root: &Path) -> PathBuf {
        dev_root.join(format!("bus/usb/{:03}/{:03}", self.bus, self.dev))
    }
}

fn read_trim(d: &Path, f: &str) -> String {
    std::fs::read_to_string(d.join(f)).unwrap_or_default().trim().to_string()
}

/// The Touch Bar displays under `usb_devices` (`/sys/bus/usb/devices`), by vendor and product id; nothing else is listed.
pub fn find_devices(usb_devices: &Path) -> Vec<Found> {
    let mut v: Vec<Found> = std::fs::read_dir(usb_devices)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|d| read_trim(d, "idVendor") == "05ac" && read_trim(d, "idProduct") == "8302")
        .filter_map(|d| {
            Some(Found { bus: read_trim(&d, "busnum").parse().ok()?, dev: read_trim(&d, "devnum").parse().ok()?, configured: read_trim(&d, "bConfigurationValue").parse().ok(), sysfs: d })
        })
        .collect();
    v.sort_by(|a, b| a.sysfs.cmp(&b.sysfs));
    v
}

/// The display interface of a found device, from the descriptors sysfs keeps. `None` when there are none or no such interface.
pub fn display_of(f: &Found) -> Option<DisplayIface> {
    find_display(&parse_descriptors(&std::fs::read(f.sysfs.join("descriptors")).ok()?))
}

// ------------------------------------------------------------------------------------------------ the safe bring-up

/// The device as a whole: configuration and reset. The real one is in `usbfs.rs`; the tests use a fake that times out.
pub trait Port {
    /// The device is still there (a replug or a disconnect ends the bring-up).
    fn present(&mut self) -> bool;
    /// The active configuration, `None` when there is none.
    fn configured(&mut self) -> Option<u8>;
    /// Select a configuration (a sysfs `bConfigurationValue` write); can take seconds to time out.
    fn set_configuration(&mut self, value: u8) -> Result<(), UsbError>;
    /// Reset this one device (`USBDEVFS_RESET` on its own node).
    fn reset(&mut self) -> Result<(), UsbError>;
    fn sleep_ms(&mut self, ms: u64);
    /// Select a configuration to *use* the display (`USBDEVFS_SETCONFIGURATION` on the device's own node on the real port). The drivers
    /// bound to the old configuration's interfaces are unbound and the new one's probe again. Defaults to [`Port::set_configuration`].
    fn switch_configuration(&mut self, value: u8) -> Result<(), UsbError> {
        self.set_configuration(value)
    }
}

/// How a configuration switch for the display ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// The device was already in the wanted configuration: nothing written.
    Already,
    /// Now in the wanted configuration.
    Switched,
    /// The switch failed or timed out and the device was put back in the configuration it had.
    FellBack,
    /// The switch failed and the way back failed too: the device stays as it is.
    Failed,
    Absent,
}

/// Move a configured Touch Bar to configuration `to`, which holds the display interface, **once**: no retry, no reset. Order:
/// the device must still be the same one; if it is already in `to` nothing is written; one `switch_configuration(to)`, a wait for the
/// drivers to rebind, a look at the configuration; if that did not work (error or timeout, or the configuration did not change), one
/// try to go back to `back` (the configuration it had), and that is the end whatever the answer.
pub fn switch_to_display(port: &mut dyn Port, to: u8, back: u8) -> SwitchOutcome {
    if !port.present() {
        return SwitchOutcome::Absent;
    }
    if port.configured() == Some(to) {
        return SwitchOutcome::Already;
    }
    let r = port.switch_configuration(to);
    port.sleep_ms(if r.is_ok() { 1_500 } else { 1_000 });
    if port.configured() == Some(to) {
        return SwitchOutcome::Switched;
    }
    if !port.present() {
        return SwitchOutcome::Absent;
    }
    if port.configured() == Some(back) && r.is_err() {
        // the write was refused and nothing moved: the device is as it was
        return SwitchOutcome::FellBack;
    }
    match port.switch_configuration(back) {
        Ok(()) => {
            port.sleep_ms(1_500);
            if port.configured() == Some(back) {
                SwitchOutcome::FellBack
            } else {
                SwitchOutcome::Failed
            }
        }
        Err(_) => SwitchOutcome::Failed,
    }
}

/// How hard to try. The defaults are what the one measured recovery suggests: the first try is the sequence that worked (2 then 1),
/// later ones wait longer because repeated writes to a device that timed out were seen to leave it stuck until a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Total rounds, never more.
    pub attempts: u32,
    /// Waits before round 2, 3, ... (the last value repeats).
    pub backoff_ms: Vec<u64>,
    /// Resets of the device allowed in all (0 = never). A reset ends in a re-enumeration, so it is done at most this often.
    pub max_resets: u32,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { attempts: 3, backoff_ms: vec![20_000, 60_000], max_resets: 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to do: the device already has a configuration.
    AlreadyConfigured,
    /// Configured after this many rounds.
    Configured { rounds: u32 },
    /// Gave up after this many rounds; the device stays as it is until a restart or a replug.
    GaveUp { rounds: u32 },
    /// The device went away.
    Absent,
}

/// Bring an unconfigured Touch Bar to configuration 1 without hammering it:
///
/// * a device that already has a configuration is left alone (no write at all);
/// * round 1 writes 2 and then 1, the sequence that was seen to work;
/// * later rounds come after a backoff and look first whether the kernel configured it meanwhile; round 2 resets this device (once at most,
///   [`Policy::max_resets`]) and writes 1; round 3 writes 1;
/// * never more than [`Policy::attempts`] rounds, never more resets than allowed, and nothing is done for a device that is gone.
pub fn bring_up(port: &mut dyn Port, p: &Policy) -> Outcome {
    if !port.present() {
        return Outcome::Absent;
    }
    if port.configured().is_some() {
        return Outcome::AlreadyConfigured;
    }
    let mut resets = 0;
    for round in 1..=p.attempts {
        if round > 1 {
            let wait = p.backoff_ms.get((round - 2) as usize).or(p.backoff_ms.last()).copied().unwrap_or(0);
            port.sleep_ms(wait);
            if !port.present() {
                return Outcome::Absent;
            }
            if port.configured().is_some() {
                return Outcome::Configured { rounds: round - 1 };
            }
        }
        match round {
            1 => {
                let _ = port.set_configuration(2);
                let _ = port.set_configuration(1);
            }
            2 if resets < p.max_resets => {
                resets += 1;
                if port.reset().is_ok() {
                    port.sleep_ms(2_000);
                }
                let _ = port.set_configuration(1);
            }
            _ => {
                let _ = port.set_configuration(1);
            }
        }
        if port.configured().is_some() {
            return Outcome::Configured { rounds: round };
        }
    }
    Outcome::GaveUp { rounds: p.attempts }
}

// ------------------------------------------------------------------------------------------------ a fake device for tests

/// A scripted Touch Bar: answers like the real one and fails on demand. Used by the tests here and in the shell.
pub mod fake {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    pub struct FakeUsb {
        pub claimed: Vec<u8>,
        pub released: Vec<u8>,
        /// Every request sent, in order.
        pub sent: Vec<Vec<u8>>,
        /// Answers waiting to be read.
        pub queue: VecDeque<Result<Vec<u8>, UsbError>>,
        /// Make the next N reads time out.
        pub timeouts: u32,
        /// Make the next N writes fail with this error.
        pub write_fail: Option<(u32, UsbError)>,
        /// The first answer to the information request is preceded by a readiness signal.
        pub ready_first: bool,
        /// The panel as reported: (width, height); `None` = 2170 x 60.
        pub panel: Option<(u32, u32)>,
        pub claim_error: Option<UsbError>,
        pub halts: u32,
        /// Reply to the next information request with this instead.
        pub info_override: Option<Vec<u8>>,
        /// Answer updates with a wrong timestamp.
        pub wrong_timestamp: bool,
    }

    pub fn info_answer(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; INFO_RESPONSE];
        put32(&mut v, 16, MSG_INFORMATION);
        put32(&mut v, 32, w);
        put32(&mut v, 36, h);
        v[40] = 24;
        put32(&mut v, 41, w.wrapping_mul(3));
        put32(&mut v, 53, PIXEL_FORMAT);
        v
    }

    pub fn ready_answer() -> Vec<u8> {
        let mut v = vec![0u8; 20];
        put32(&mut v, 16, MSG_READY);
        v
    }

    pub fn update_answer(ts: u64) -> Vec<u8> {
        let mut v = vec![0u8; UPDATE_RESPONSE];
        put32(&mut v, 16, MSG_UPDATE_COMPLETE);
        put64(&mut v, 32, ts);
        v
    }

    impl Usb for FakeUsb {
        fn claim(&mut self, interface: u8) -> Result<(), UsbError> {
            if let Some(e) = self.claim_error {
                return Err(e);
            }
            self.claimed.push(interface);
            Ok(())
        }
        fn release(&mut self, interface: u8) {
            self.released.push(interface);
        }
        fn bulk_out(&mut self, _ep: u8, data: &[u8], _t: u32) -> Result<usize, UsbError> {
            if let Some((n, e)) = self.write_fail.as_mut() {
                if *n > 0 {
                    *n -= 1;
                    return Err(*e);
                }
            }
            self.sent.push(data.to_vec());
            match (get32(data, 16), data.len()) {
                (Some(MSG_INFORMATION), SIMPLE_REQUEST) => {
                    if self.ready_first {
                        self.queue.push_back(Ok(ready_answer()));
                        self.ready_first = false;
                    }
                    let (w, h) = self.panel.unwrap_or((2170, 60));
                    self.queue.push_back(Ok(self.info_override.take().unwrap_or_else(|| info_answer(w, h))));
                }
                _ if data.len() > SIMPLE_REQUEST && get16_at(data, 2) == Some(0x12) => {
                    // the footer holds the timestamp 48 bytes from the end of the (padded) message
                    let ts = footer_timestamp(data).unwrap_or(0);
                    self.queue.push_back(Ok(update_answer(if self.wrong_timestamp { ts ^ 1 } else { ts })));
                }
                _ => {}
            }
            Ok(data.len())
        }
        fn bulk_in(&mut self, _ep: u8, buf: &mut [u8], _t: u32) -> Result<usize, UsbError> {
            if self.timeouts > 0 {
                // the device never answered: nothing was queued for this read
                self.timeouts -= 1;
                self.queue.pop_front();
                return Err(UsbError::Timeout);
            }
            match self.queue.pop_front() {
                Some(Ok(v)) => {
                    let n = v.len().min(buf.len());
                    buf[..n].copy_from_slice(&v[..n]);
                    Ok(n)
                }
                Some(Err(e)) => Err(e),
                None => Err(UsbError::Timeout),
            }
        }
        fn clear_halt(&mut self, _ep: u8) -> Result<(), UsbError> {
            self.halts += 1;
            Ok(())
        }
    }

    fn get16_at(b: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
    }

    /// The timestamp of an update request, found by walking its frames (the test double reads the format back independently of the
    /// encoder's own arithmetic).
    pub fn footer_timestamp(req: &[u8]) -> Option<u64> {
        let mut at = FB_PREFIX;
        let end = REQUEST_HEADER + get32(req, 12)? as usize;
        while at + FOOTER <= end {
            // a frame header has a plausible data size; the footer starts with 12 zero bytes and then 0xfffe
            if get32(req, at + 12) == Some(0xfffe) && get32(req, at + 28) == Some(0x8_0001) {
                return get64(req, at + 32);
            }
            at += FRAME_HEADER + get32(req, at + 8)? as usize;
        }
        None
    }

    /// A port for the configuration switch: `fail_to` makes a write of that configuration time out (the device stays as it is);
    /// `stuck` makes every write time out after the first failure (a device that stops answering).
    pub struct FakeSwitchPort {
        pub present: bool,
        pub config: Option<u8>,
        pub fail_to: Option<u8>,
        pub stuck: bool,
        pub log: Vec<u8>,
        pub sleeps: Vec<u64>,
    }

    impl Default for FakeSwitchPort {
        fn default() -> FakeSwitchPort {
            FakeSwitchPort::new(1)
        }
    }

    impl FakeSwitchPort {
        pub fn new(config: u8) -> FakeSwitchPort {
            FakeSwitchPort { present: true, config: Some(config), fail_to: None, stuck: false, log: Vec::new(), sleeps: Vec::new() }
        }
    }

    impl Port for FakeSwitchPort {
        fn present(&mut self) -> bool {
            self.present
        }
        fn configured(&mut self) -> Option<u8> {
            self.config
        }
        fn set_configuration(&mut self, value: u8) -> Result<(), UsbError> {
            self.switch_configuration(value)
        }
        fn reset(&mut self) -> Result<(), UsbError> {
            self.log.push(255);
            Ok(())
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.sleeps.push(ms);
        }
        fn switch_configuration(&mut self, value: u8) -> Result<(), UsbError> {
            self.log.push(value);
            if self.stuck || self.fail_to == Some(value) {
                self.stuck = self.stuck || self.fail_to == Some(value);
                return Err(UsbError::Timeout);
            }
            self.config = Some(value);
            Ok(())
        }
    }

    /// A scripted port.
    pub struct FakePort {
        pub present: bool,
        pub config: Option<u8>,
        /// Writes seen: configuration values in order; resets as 255.
        pub log: Vec<u8>,
        /// Sleeps requested.
        pub sleeps: Vec<u64>,
        /// Configuration writes that must be given before one is accepted (a device that times out until then); `None` = never.
        pub accept_after: Option<u32>,
        pub writes: u32,
        pub resets: u32,
        /// A reset makes the next write succeed.
        pub reset_heals: bool,
        healed: bool,
    }

    impl FakePort {
        pub fn new(config: Option<u8>) -> FakePort {
            FakePort { present: true, config, log: Vec::new(), sleeps: Vec::new(), accept_after: None, writes: 0, resets: 0, reset_heals: false, healed: false }
        }
    }

    impl Port for FakePort {
        fn present(&mut self) -> bool {
            self.present
        }
        fn configured(&mut self) -> Option<u8> {
            self.config
        }
        fn set_configuration(&mut self, value: u8) -> Result<(), UsbError> {
            self.log.push(value);
            self.writes += 1;
            if self.healed || self.accept_after.is_some_and(|n| self.writes >= n) {
                if value == 1 {
                    self.config = Some(1);
                }
                return Ok(());
            }
            Err(UsbError::Timeout)
        }
        fn reset(&mut self) -> Result<(), UsbError> {
            self.log.push(255);
            self.resets += 1;
            self.healed = self.reset_heals;
            Ok(())
        }
        fn sleep_ms(&mut self, ms: u64) {
            self.sleeps.push(ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    const ICE: DisplayIface = DisplayIface { config: 1, interface: 2, alt: 0, ep_out: 0x02, ep_in: 0x82 };

    #[test]
    fn the_information_request_is_the_documented_32_bytes() {
        let want: [u8; 32] = [2, 0, 0x12, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, b'F', b'N', b'I', b'G', 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0];
        assert_eq!(simple_request(MSG_INFORMATION), want);
        assert_eq!(&simple_request(MSG_CLEAR)[16..20], b"DRLC");
        assert_eq!(&simple_request(MSG_READY)[16..20], b"YDER");
        assert_eq!(tag_text(MSG_INFORMATION), "FNIG");
    }

    #[test]
    fn the_information_answer_gives_the_panel_and_rejects_everything_else() {
        let a = info_answer(2170, 60);
        let i = parse_information(&a).unwrap();
        assert_eq!((i.width, i.height, i.landscape()), (2170, 60, (2170, 60)));
        assert_eq!(parse_information(&info_answer(60, 2170)).unwrap().landscape(), (2170, 60), "a turned report is turned back");
        assert!(matches!(parse_information(&a[..64]), Err(ProtoError::Short { need: 65, got: 64 })));
        assert!(matches!(parse_information(&[]), Err(ProtoError::Short { .. })));
        let mut b = a.clone();
        b[40] = 32;
        assert!(matches!(parse_information(&b), Err(ProtoError::Format(_))));
        let mut b = a.clone();
        b[53] = 0;
        assert!(matches!(parse_information(&b), Err(ProtoError::Format(_))));
        let mut b = a.clone();
        b[16] = 0;
        assert!(matches!(parse_information(&b), Err(ProtoError::Unexpected(_))));
        for (w, h) in [(0, 60), (2170, 0), (u32::MAX, 60), (2170, 9000)] {
            assert!(parse_information(&info_answer(w, h)).is_err(), "{w}x{h}");
        }
    }

    #[test]
    fn a_one_pixel_update_has_the_documented_bytes() {
        let ts = 0x1122_3344_5566_7788u64;
        let v = update_request(&[(Rect { x: 3, y: 4, w: 1, h: 1 }, vec![9, 8, 7])], ts).unwrap();
        assert_eq!(v.len(), 144, "48 + 12 + 3 + 80 = 143, padded to 144");
        assert_eq!(&v[..16], &[2, 0, 0x12, 0, 9, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0]);
        assert_eq!(&v[16..19], &[1, 0, 0x88], "u16 1, then the low byte of the timestamp");
        assert!(v[19..48].iter().all(|b| *b == 0));
        assert_eq!(&v[48..63], &[3, 0, 4, 0, 1, 0, 1, 0, 3, 0, 0, 0, 9, 8, 7]);
        assert_eq!(&v[75..79], &[0xfe, 0xff, 0, 0]);
        assert_eq!(&v[91..95], &[1, 0, 8, 0]);
        assert_eq!(&v[95..103], &ts.to_le_bytes());
        assert_eq!(&v[115..119], &[2, 0, 8, 0]);
        assert_eq!(&v[139..143], &[0xff, 0xff, 0, 0]);
        assert_eq!(v[143], 0);
        assert_eq!(footer_timestamp(&v), Some(ts), "the test double reads it back");
        assert_eq!(parse_update_response(&update_answer(ts), ts), Ok(()));
        assert_eq!(parse_update_response(&update_answer(ts + 1), ts), Err(ProtoError::Timestamp));
        assert!(matches!(parse_update_response(&update_answer(ts)[..39], ts), Err(ProtoError::Short { .. })));
    }

    #[test]
    fn several_rectangles_follow_each_other_and_the_total_is_a_multiple_of_16() {
        let r1 = Rect { x: 0, y: 0, w: 2, h: 2 };
        let r2 = Rect { x: 10, y: 5, w: 1, h: 3 };
        let v = update_request(&[(r1, vec![1; 12]), (r2, vec![2; 9])], 5).unwrap();
        assert_eq!(v.len() % 16, 0);
        assert_eq!(v.len(), (48 + 12 + 12 + 12 + 9 + 80usize).div_ceil(16) * 16);
        assert_eq!(&v[48 + 24..48 + 24 + 4], &[10, 0, 5, 0]);
        assert_eq!(u32::from_le_bytes(v[12..16].try_into().unwrap()) as usize, v.len() - 16);
        assert_eq!(footer_timestamp(&v), Some(5));
        // data that does not fit its rectangle, or a rectangle beyond 16 bits
        assert_eq!(update_request(&[(r1, vec![1; 11])], 1), Err(ProtoError::TooBig));
        assert_eq!(update_request(&[(Rect { x: 70_000, y: 0, w: 0, h: 0 }, vec![])], 1), Err(ProtoError::TooBig));
    }

    #[test]
    fn pixels_go_column_by_column_bottom_up_in_blue_green_red_and_the_flips_change_only_the_order() {
        // 3 wide x 2 high picture; pixel value = (x, y, 0, 255): red = x, green = y
        let mut px = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                px.extend_from_slice(&[x * 10, y * 10 + 1, 0, 255]);
            }
        }
        let all = Rect { x: 0, y: 0, w: 3, h: 2 };
        // x = 0: bottom (y=1) first: B=0 G=11 R=0, then top: B=0 G=1 R=0; then x = 1 ...
        let v = pack_rect(&px, 3, 2, all, Orient::default());
        assert_eq!(&v[..6], &[0, 11, 0, 0, 1, 0]);
        assert_eq!(&v[6..12], &[0, 11, 10, 0, 1, 10]);
        assert_eq!(v.len(), 18);
        let f = pack_rect(&px, 3, 2, all, Orient { flip_x: false, flip_y: true });
        assert_eq!(&f[..6], &[0, 1, 0, 0, 11, 0]);
        let f = pack_rect(&px, 3, 2, all, Orient { flip_x: true, flip_y: false });
        assert_eq!(&f[..6], &[0, 11, 20, 0, 1, 20]);
        assert_eq!(Orient::from_bits(3), Orient { flip_x: true, flip_y: true });
        assert_eq!(Orient::from_bits(Orient { flip_x: true, flip_y: false }.bits()), Orient { flip_x: true, flip_y: false });
        // a sub-rectangle, one outside the picture and a short buffer never panic
        assert_eq!(pack_rect(&px, 3, 2, Rect { x: 2, y: 1, w: 1, h: 1 }, Orient::default()), vec![0, 11, 20]);
        assert_eq!(pack_rect(&px, 3, 2, Rect { x: 5, y: 5, w: 4, h: 4 }, Orient::default()), Vec::<u8>::new());
        assert_eq!(pack_rect(&px[..8], 3, 2, all, Orient::default()).len(), 18);
        assert_eq!(pack_rect(&px, 3, 2, Rect { x: u32::MAX, y: 0, w: u32::MAX, h: 1 }, Orient::default()).len(), 0);
    }

    fn raw_config(value: u8, ifaces: &[(u8, u8, u8, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (num, alt, class, eps) in ifaces {
            body.extend_from_slice(&[9, 4, *num, *alt, eps.len() as u8, *class, 0, 0, 0]);
            for e in *eps {
                body.extend_from_slice(&[7, 5, *e, 2, 0, 2, 0]);
            }
        }
        let mut v = vec![9, 2, (9 + body.len()) as u8, 0, ifaces.len() as u8, value, 0, 0x80, 50];
        v.extend(body);
        v
    }

    #[test]
    fn the_display_interface_is_found_in_the_descriptors_with_its_configuration() {
        let mut raw = vec![18, 1, 0, 2, 0, 0, 0, 64, 0xac, 5, 2, 0x83, 0, 0, 0, 0, 0, 2];
        raw.extend(raw_config(1, &[(0, 0, 3, &[])]));
        raw.extend(raw_config(2, &[(0, 0, 3, &[]), (1, 0, 0x10, &[0x02, 0x82])]));
        let cfgs = parse_descriptors(&raw);
        assert_eq!(cfgs.len(), 2);
        assert_eq!(find_display(&cfgs), Some(DisplayIface { config: 2, interface: 1, alt: 0, ep_out: 0x02, ep_in: 0x82 }));
        assert_eq!(find_display(&cfgs[..1]), None, "configuration 1 alone has no display");
        // endpoints given in the other order, and an interface of another class with bulk endpoints
        let raw = raw_config(1, &[(0, 0, 8, &[0x01, 0x81]), (3, 0, 0x10, &[0x85, 0x05])]);
        assert_eq!(find_display(&parse_descriptors(&raw)), Some(DisplayIface { config: 1, interface: 3, alt: 0, ep_out: 0x05, ep_in: 0x85 }));
        // truncated, empty and garbage input
        for n in 0..raw.len() {
            let _ = parse_descriptors(&raw[..n]);
        }
        assert!(parse_descriptors(&[]).is_empty() && parse_descriptors(&[0, 0, 0]).is_empty() && parse_descriptors(&[255, 4, 1]).is_empty());
    }

    #[test]
    fn start_up_asks_tells_and_clears_in_the_kernels_order() {
        let d = Dfr::open(FakeUsb::default(), ICE).unwrap();
        assert_eq!(d.info.landscape(), (2170, 60));
        let tags: Vec<u32> = d.usb.sent.iter().map(|m| get32(m, 16).unwrap()).collect();
        assert_eq!(tags, [MSG_INFORMATION, MSG_READY, MSG_CLEAR]);
        assert_eq!(d.usb.claimed, [2]);
        d.close();
    }

    #[test]
    fn a_readiness_signal_before_the_answer_is_read_past_once() {
        let u = FakeUsb { ready_first: true, ..Default::default() };
        assert!(Dfr::open(u, ICE).is_ok());
        // two in a row is an error, not a loop
        let mut u = FakeUsb::default();
        u.queue.push_back(Ok(ready_answer()));
        u.queue.push_back(Ok(ready_answer()));
        u.queue.push_back(Ok(info_answer(2170, 60)));
        let mut d = Dfr { usb: u, iface: ICE, info: Info { width: 0, height: 0, bytes_per_row: 0, orientation: 0 }, fails: 0, frames: 0, claimed: false };
        assert!(matches!(d.read(INFO_RESPONSE, MSG_INFORMATION), Err(DfrError::Proto(ProtoError::Unexpected(MSG_READY)))));
    }

    #[test]
    fn start_up_failures_release_the_interface_and_report_why() {
        let u = FakeUsb { timeouts: 1, ..Default::default() };
        assert!(matches!(Dfr::open(u, ICE), Err(DfrError::Usb(UsbError::Timeout))));
        let u = FakeUsb { claim_error: Some(UsbError::Busy), ..Default::default() };
        assert!(matches!(Dfr::open(u, ICE), Err(DfrError::Usb(UsbError::Busy))));
        let u = FakeUsb { info_override: Some(vec![0; 10]), ..Default::default() };
        assert!(matches!(Dfr::open(u, ICE), Err(DfrError::Proto(ProtoError::Short { .. }))));
        let u = FakeUsb { info_override: Some(info_answer(0, 0)), ..Default::default() };
        assert!(matches!(Dfr::open(u, ICE), Err(DfrError::Proto(ProtoError::Format(_)))));
        // the fake records the release even when start-up failed
        let u = FakeUsb { timeouts: 1, ..Default::default() };
        let mut d2 = Dfr { usb: u, iface: ICE, info: Info { width: 0, height: 0, bytes_per_row: 0, orientation: 0 }, fails: 0, frames: 0, claimed: true };
        assert!(d2.start().is_err());
        d2.close_quiet();
        assert_eq!(d2.usb.released, [2]);
    }

    #[test]
    fn updates_are_answered_and_three_failures_in_a_row_end_the_session() {
        let mut d = Dfr::open(FakeUsb::default(), ICE).unwrap();
        let r = Rect { x: 0, y: 0, w: 2, h: 1 };
        assert_eq!(d.update(&[(r, vec![0; 6])], 100), Ok(()));
        assert_eq!(d.update(&[], 101), Ok(()), "nothing to draw sends nothing");
        let n = d.usb.sent.len();
        d.usb.timeouts = 2;
        assert_eq!(d.update(&[(r, vec![0; 6])], 102), Err(DfrError::Usb(UsbError::Timeout)));
        assert_eq!(d.update(&[(r, vec![0; 6])], 103), Err(DfrError::Usb(UsbError::Timeout)));
        assert_eq!(d.update(&[(r, vec![0; 6])], 104), Ok(()), "a success resets the count");
        d.usb.timeouts = 10;
        for _ in 0..2 {
            assert!(matches!(d.update(&[(r, vec![0; 6])], 105), Err(DfrError::Usb(_))));
        }
        assert_eq!(d.update(&[(r, vec![0; 6])], 106), Err(DfrError::Dead));
        assert!(d.usb.sent.len() > n);
        // a vanished device is dead at once; a wrong timestamp is an error but not fatal
        let mut d = Dfr::open(FakeUsb::default(), ICE).unwrap();
        d.usb.write_fail = Some((1, UsbError::NoDevice));
        assert_eq!(d.update(&[(r, vec![0; 6])], 1), Err(DfrError::Dead));
        let mut d = Dfr::open(FakeUsb { wrong_timestamp: true, ..Default::default() }, ICE).unwrap();
        assert_eq!(d.update(&[(r, vec![0; 6])], 1), Err(DfrError::Proto(ProtoError::Timestamp)));
        // a stall clears the halt on both endpoints
        let mut d = Dfr::open(FakeUsb::default(), ICE).unwrap();
        d.usb.write_fail = Some((1, UsbError::Stall));
        assert!(d.update(&[(r, vec![0; 6])], 1).is_err());
        assert_eq!(d.usb.halts, 2);
    }

    #[test]
    fn a_garbage_answer_never_panics() {
        let mut state = 0x1234_5678u32;
        for len in [0usize, 1, 19, 20, 39, 40, 64, 65, 66, 200] {
            for _ in 0..40 {
                let b: Vec<u8> = (0..len)
                    .map(|_| {
                        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                        (state >> 24) as u8
                    })
                    .collect();
                let _ = parse_information(&b);
                let _ = parse_update_response(&b, 7);
                let _ = response_tag(&b);
                let _ = parse_descriptors(&b);
            }
        }
        let mut u = FakeUsb::default();
        for len in [0usize, 3, 25] {
            u.queue.push_back(Ok(vec![0xAB; len]));
        }
        assert!(Dfr::open(u, ICE).is_err());
    }

    #[test]
    fn a_device_with_a_configuration_is_left_alone() {
        let mut p = FakePort::new(Some(1));
        assert_eq!(bring_up(&mut p, &Policy::default()), Outcome::AlreadyConfigured);
        assert!(p.log.is_empty() && p.sleeps.is_empty());
        let mut p = FakePort::new(None);
        p.present = false;
        assert_eq!(bring_up(&mut p, &Policy::default()), Outcome::Absent);
        assert!(p.log.is_empty());
    }

    #[test]
    fn the_first_round_is_the_measured_sequence_and_stops_when_it_works() {
        let mut p = FakePort::new(None);
        p.accept_after = Some(1);
        assert_eq!(bring_up(&mut p, &Policy::default()), Outcome::Configured { rounds: 1 });
        assert_eq!(p.log, [2, 1]);
        assert!(p.sleeps.is_empty());
    }

    #[test]
    fn a_stuck_device_gets_three_rounds_one_reset_and_growing_waits_then_is_left_alone() {
        let mut p = FakePort::new(None);
        assert_eq!(bring_up(&mut p, &Policy::default()), Outcome::GaveUp { rounds: 3 });
        assert_eq!(p.log, [2, 1, 255, 1, 1], "round 1: 2 then 1; round 2: reset then 1; round 3: 1");
        assert_eq!(p.resets, 1, "one reset, never a storm");
        assert_eq!(p.sleeps, [20_000, 2_000, 60_000], "backoff before each later round, a pause after the reset");
        // the configuration appearing by itself during a wait ends it without another write
        struct Late(FakePort, u32);
        impl Port for Late {
            fn present(&mut self) -> bool {
                true
            }
            fn configured(&mut self) -> Option<u8> {
                self.0.config
            }
            fn set_configuration(&mut self, v: u8) -> Result<(), UsbError> {
                self.0.set_configuration(v)
            }
            fn reset(&mut self) -> Result<(), UsbError> {
                self.0.reset()
            }
            fn sleep_ms(&mut self, ms: u64) {
                self.1 += 1;
                self.0.sleep_ms(ms);
                self.0.config = Some(1);
            }
        }
        let mut l = Late(FakePort::new(None), 0);
        assert_eq!(bring_up(&mut l, &Policy::default()), Outcome::Configured { rounds: 1 });
        assert_eq!(l.0.log, [2, 1], "no write after the kernel configured it");
    }

    #[test]
    fn a_reset_that_heals_the_device_lets_round_two_succeed_and_no_reset_is_allowed_when_the_policy_says_so() {
        let mut p = FakePort::new(None);
        p.reset_heals = true;
        assert_eq!(bring_up(&mut p, &Policy::default()), Outcome::Configured { rounds: 2 });
        assert_eq!(p.log, [2, 1, 255, 1]);
        let mut p = FakePort::new(None);
        let pol = Policy { attempts: 4, backoff_ms: vec![1], max_resets: 0 };
        assert_eq!(bring_up(&mut p, &pol), Outcome::GaveUp { rounds: 4 });
        assert_eq!(p.resets, 0);
        assert_eq!(p.writes, 2 + 3, "no more writes than rounds plus the first pair");
        let mut p = FakePort::new(None);
        assert_eq!(bring_up(&mut p, &Policy { attempts: 1, backoff_ms: vec![], max_resets: 1 }), Outcome::GaveUp { rounds: 1 });
        assert_eq!(p.log, [2, 1]);
        let mut p = FakePort::new(None);
        assert_eq!(bring_up(&mut p, &Policy { attempts: 0, backoff_ms: vec![], max_resets: 1 }), Outcome::GaveUp { rounds: 0 });
        assert!(p.log.is_empty());
    }

    #[test]
    fn a_device_in_configuration_1_is_switched_to_the_one_with_the_display_once() {
        let mut p = FakeSwitchPort::new(1);
        assert_eq!(switch_to_display(&mut p, 2, 1), SwitchOutcome::Switched);
        assert_eq!(p.log, [2], "one write, no reset");
        assert_eq!(p.config, Some(2));
        assert_eq!(p.sleeps, [1_500], "the drivers get time to rebind");
    }

    #[test]
    fn a_device_already_in_the_display_configuration_is_not_written() {
        let mut p = FakeSwitchPort::new(2);
        assert_eq!(switch_to_display(&mut p, 2, 1), SwitchOutcome::Already);
        assert!(p.log.is_empty() && p.sleeps.is_empty());
        let mut gone = FakeSwitchPort::new(1);
        gone.present = false;
        assert_eq!(switch_to_display(&mut gone, 2, 1), SwitchOutcome::Absent);
        assert!(gone.log.is_empty());
    }

    #[test]
    fn a_switch_that_times_out_goes_back_once_and_never_tries_again() {
        // the write is refused and nothing moved: the device is as it was, no second write
        let mut p = FakeSwitchPort::new(1);
        p.fail_to = Some(2);
        assert_eq!(switch_to_display(&mut p, 2, 1), SwitchOutcome::FellBack);
        assert_eq!(p.log, [2]);
        assert_eq!(p.config, Some(1));
        // the switch half happened (the device lost its configuration) and the way back works
        struct Half(FakeSwitchPort);
        impl Port for Half {
            fn present(&mut self) -> bool {
                true
            }
            fn configured(&mut self) -> Option<u8> {
                self.0.config
            }
            fn set_configuration(&mut self, v: u8) -> Result<(), UsbError> {
                self.switch_configuration(v)
            }
            fn reset(&mut self) -> Result<(), UsbError> {
                Ok(())
            }
            fn sleep_ms(&mut self, ms: u64) {
                self.0.sleeps.push(ms);
            }
            fn switch_configuration(&mut self, v: u8) -> Result<(), UsbError> {
                self.0.log.push(v);
                if v == 2 {
                    self.0.config = None;
                    return Err(UsbError::Timeout);
                }
                self.0.config = Some(v);
                Ok(())
            }
        }
        let mut h = Half(FakeSwitchPort::new(1));
        assert_eq!(switch_to_display(&mut h, 2, 1), SwitchOutcome::FellBack);
        assert_eq!(h.0.log, [2, 1], "one try to go forward, one to go back");
        // a device that stops answering altogether: both fail, and that is the end
        let mut s = FakeSwitchPort::new(1);
        s.stuck = true;
        s.config = None;
        assert_eq!(switch_to_display(&mut s, 2, 1), SwitchOutcome::Failed);
        assert_eq!(s.log, [2, 1], "never more than the one pair of writes");
    }

    #[test]
    fn devices_are_found_by_id_only_with_their_usbfs_node() {
        let d = std::env::temp_dir().join(format!("tbar-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (n, v, p, bus, dev, c) in [("5-6", "05ac", "8302", "5", "7", ""), ("5-7", "05ac", "8102", "5", "8", "1"), ("5-8", "05ac", "8302", "5", "9", "1\n"), ("3-1", "0bda", "8302", "3", "2", "1")] {
            std::fs::create_dir_all(d.join(n)).unwrap();
            for (f, x) in [("idVendor", v), ("idProduct", p), ("busnum", bus), ("devnum", dev), ("bConfigurationValue", c)] {
                std::fs::write(d.join(n).join(f), format!("{x}\n")).unwrap();
            }
        }
        std::fs::write(d.join("5-6/bConfigurationValue"), "").unwrap();
        let f = find_devices(&d);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].configured, f[1].configured), (None, Some(1)));
        assert_eq!(f[0].usbfs_path(Path::new("/dev")), PathBuf::from("/dev/bus/usb/005/007"));
        assert!(display_of(&f[0]).is_none(), "no descriptors file: nothing guessed");
        std::fs::write(d.join("5-8/descriptors"), raw_config(1, &[(2, 0, 0x10, &[0x02, 0x82])])).unwrap();
        assert_eq!(display_of(&f[1]).map(|x| x.interface), Some(2));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn errno_values_map_to_the_errors_the_code_acts_on() {
        assert_eq!(UsbError::from_errno(110), UsbError::Timeout);
        assert_eq!(UsbError::from_errno(19), UsbError::NoDevice);
        assert_eq!(UsbError::from_errno(16), UsbError::Busy);
        assert_eq!(UsbError::from_errno(32), UsbError::Stall);
        assert_eq!(UsbError::from_errno(5), UsbError::Other(5));
    }
}

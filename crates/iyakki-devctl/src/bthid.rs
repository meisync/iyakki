//! Using a paired Bluetooth keyboard, mouse or game pad without BlueZ (F-NET-14): the protocol parts, with no socket and no device
//! file in this module, so every byte is unit-tested with fakes. The real sockets, `/dev/uhid` and the background hub are in
//! [`crate::bthid_sys`].
//!
//! Classic (BR/EDR) HID, the way a Bluetooth keyboard talks:
//! 1. SDP (L2CAP PSM 0x0001): ask the device's service record for its HID report descriptor ([`fetch_hid_record`]).
//! 2. HID control (PSM 0x0011) and HID interrupt (PSM 0x0013) channels; reports arrive on the interrupt channel as `0xA1` + report.
//! 3. The reports are handed to the kernel through `/dev/uhid` (`UHID_CREATE2` with the descriptor, then `UHID_INPUT2` per report),
//!    which makes an ordinary input device: the shell's existing input code sees a keyboard or a mouse.
//! 4. LED changes (caps lock) come back as `UHID_OUTPUT` and go out as `0xA2` + report.
//!
//! Not here: HID over GATT for low-energy devices (HOGP, a separate step: needs an ATT/GATT client), and audio (A2DP).
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::btmgmt::{self, Error as MgmtError, Mgmt, Transport};

pub const PSM_SDP: u16 = 0x0001;
pub const PSM_HID_CONTROL: u16 = 0x0011;
pub const PSM_HID_INTERRUPT: u16 = 0x0013;
/// SDP: the HID service class.
pub const UUID_HID: u16 = 0x1124;

// ------------------------------------------------------------------------------------------------ channels and errors

/// What a channel gave back.
#[derive(Debug, PartialEq, Eq)]
pub enum Recv {
    Data(Vec<u8>),
    Timeout,
    /// The other side closed the channel.
    Closed,
}

/// A message-oriented (L2CAP seqpacket) channel. The real one is a socket; tests use a scripted one.
pub trait Channel {
    fn send(&mut self, data: &[u8]) -> io::Result<()>;
    fn recv(&mut self, wait: Duration) -> io::Result<Recv>;
}

/// Where UHID events are written (the real one is `/dev/uhid`).
pub trait Uhid {
    fn write(&mut self, event: &[u8]) -> io::Result<()>;
}

/// Why a HID connection did not work, in words for the person.
#[derive(Debug)]
pub enum HidError {
    /// connecting a channel failed with this errno
    Connect(i32),
    /// the device has no HID record (it is not a keyboard, mouse or game pad, or SDP is switched off)
    NoHidService,
    Sdp(String),
    Uhid(io::Error),
    Io(io::Error),
    Closed,
    Timeout,
}

impl std::fmt::Display for HidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HidError::Connect(e) => write!(f, "{}", connect_reason(*e)),
            HidError::NoHidService => write!(f, "the device does not offer a keyboard, mouse or game pad service (HID)"),
            HidError::Sdp(m) => write!(f, "the device's service list could not be read ({m})"),
            HidError::Uhid(e) => write!(f, "the computer could not create the input device ({e}); is the uhid driver loaded?"),
            HidError::Io(e) => write!(f, "the Bluetooth link failed ({e})"),
            HidError::Closed => write!(f, "the device closed the connection"),
            HidError::Timeout => write!(f, "the device did not answer in time"),
        }
    }
}

impl std::error::Error for HidError {}

impl From<io::Error> for HidError {
    fn from(e: io::Error) -> HidError {
        HidError::Io(e)
    }
}

/// Plain-language reason for a failed L2CAP connect (the errno the kernel gave).
pub fn connect_reason(errno: i32) -> String {
    match errno {
        e if e == libc::EHOSTUNREACH || e == libc::ETIMEDOUT || e == libc::EHOSTDOWN => {
            "the device did not answer: switch it on, bring it closer and press a key (or its connect button)".into()
        }
        e if e == libc::ECONNREFUSED || e == libc::EACCES => {
            "the device refused: it may have forgotten this computer. Choose Forget, then pair it again".into()
        }
        e if e == libc::ECONNRESET || e == libc::ECONNABORTED => "the device dropped the connection while connecting; try again".into(),
        e if e == libc::ENETDOWN || e == libc::ENODEV || e == crate::oscompat::EBADFD || e == libc::ENOPROTOOPT => {
            "Bluetooth is off or has no controller: power the controller on in Settings, Bluetooth".into()
        }
        e if e == libc::EPERM => "this needs administrator rights".into(),
        e if e == libc::EALREADY || e == libc::EISCONN || e == libc::EADDRINUSE => "the device is already connected".into(),
        e => format!("could not connect ({})", io::Error::from_raw_os_error(e)),
    }
}

// ------------------------------------------------------------------------------------------------ SDP data elements

/// An SDP data element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Elem {
    Nil,
    Uint(u64),
    Int(i64),
    Uuid(Vec<u8>),
    Str(Vec<u8>),
    Bool(bool),
    Seq(Vec<Elem>),
    Alt(Vec<Elem>),
}

impl Elem {
    pub fn uint(&self) -> Option<u64> {
        if let Elem::Uint(n) = self { Some(*n) } else { None }
    }
    pub fn seq(&self) -> Option<&[Elem]> {
        match self {
            Elem::Seq(v) | Elem::Alt(v) => Some(v),
            _ => None,
        }
    }
    /// The 16-bit value of a UUID element (a short UUID, or a 128-bit one on the Bluetooth base).
    pub fn uuid16(&self) -> Option<u16> {
        match self {
            Elem::Uuid(b) if b.len() == 2 => Some(u16::from_be_bytes([b[0], b[1]])),
            Elem::Uuid(b) if b.len() == 4 && b[0] == 0 && b[1] == 0 => Some(u16::from_be_bytes([b[2], b[3]])),
            Elem::Uuid(b) if b.len() == 16 && b[0..2] == [0, 0] && b[4..] == [0, 0, 0x10, 0, 0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb] => Some(u16::from_be_bytes([b[2], b[3]])),
            _ => None,
        }
    }
}

/// One data element and the number of bytes it used. Depth and length are bounded, so hostile data is an error, not a panic.
pub fn parse_elem(buf: &[u8]) -> Result<(Elem, usize), String> {
    parse_elem_at(buf, 0)
}

fn parse_elem_at(buf: &[u8], depth: u8) -> Result<(Elem, usize), String> {
    if depth > 8 {
        return Err("data elements nested too deep".into());
    }
    let h = *buf.first().ok_or("empty data element")?;
    let (ty, idx) = (h >> 3, h & 7);
    let (len, head) = match (ty, idx) {
        (0, 0) => (0usize, 1usize),
        (_, 0) => (1, 1),
        (_, 1) => (2, 1),
        (_, 2) => (4, 1),
        (_, 3) => (8, 1),
        (_, 4) => (16, 1),
        (_, 5) => (*buf.get(1).ok_or("short data element")? as usize, 2),
        (_, 6) => (u16::from_be_bytes([*buf.get(1).ok_or("short")?, *buf.get(2).ok_or("short")?]) as usize, 3),
        _ => (u32::from_be_bytes([*buf.get(1).ok_or("short")?, *buf.get(2).ok_or("short")?, *buf.get(3).ok_or("short")?, *buf.get(4).ok_or("short")?]) as usize, 5),
    };
    let end = head.checked_add(len).filter(|e| *e <= buf.len()).ok_or("data element longer than its data")?;
    let body = &buf[head..end];
    let be = |b: &[u8]| b.iter().fold(0u64, |a, x| (a << 8) | *x as u64);
    let e = match ty {
        0 => Elem::Nil,
        1 if body.len() <= 8 => Elem::Uint(be(body)),
        1 => Elem::Str(body.to_vec()),
        2 if !body.is_empty() && body.len() <= 8 => {
            let shift = 64 - 8 * body.len() as u32;
            Elem::Int(((be(body) << shift) as i64) >> shift)
        }
        2 => Elem::Str(body.to_vec()),
        3 => Elem::Uuid(body.to_vec()),
        4 | 8 => Elem::Str(body.to_vec()),
        5 => Elem::Bool(body.first().is_some_and(|b| *b != 0)),
        6 | 7 => {
            let mut v = Vec::new();
            let mut at = 0;
            while at < body.len() {
                let (el, n) = parse_elem_at(&body[at..], depth + 1)?;
                v.push(el);
                at += n;
            }
            if ty == 6 { Elem::Seq(v) } else { Elem::Alt(v) }
        }
        _ => return Err("unknown data element type".into()),
    };
    Ok((e, end))
}

fn elem_uuid16(u: u16) -> Vec<u8> {
    vec![0x19, (u >> 8) as u8, u as u8]
}

// ------------------------------------------------------------------------------------------------ SDP client

/// What the device's HID service record tells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HidRecord {
    /// The HID report descriptor (attribute 0x0206).
    pub descriptor: Vec<u8>,
    pub name: Option<String>,
    /// The device connects back by itself (attribute 0x0205 HIDReconnectInitiate).
    pub reconnect_initiate: Option<bool>,
}

const SDP_ERROR: u8 = 0x01;
const SDP_SSA_REQUEST: u8 = 0x06;
const SDP_SSA_RESPONSE: u8 = 0x07;
/// Largest report descriptor `UHID_CREATE2` takes.
pub const MAX_DESCRIPTOR: usize = 4096;

/// Service Search Attribute Request for every attribute of the HID service records.
pub fn sdp_request(tid: u16, continuation: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&[0x35, 0x03]);
    p.extend_from_slice(&elem_uuid16(UUID_HID));
    p.extend_from_slice(&0xffffu16.to_be_bytes()); // maximum attribute byte count
    p.extend_from_slice(&[0x35, 0x05, 0x0a, 0x00, 0x00, 0xff, 0xff]); // attribute id range 0x0000..0xffff
    p.push(continuation.len() as u8);
    p.extend_from_slice(continuation);
    let mut v = vec![SDP_SSA_REQUEST];
    v.extend_from_slice(&tid.to_be_bytes());
    v.extend_from_slice(&(p.len() as u16).to_be_bytes());
    v.extend_from_slice(&p);
    v
}

/// One response PDU: the attribute bytes of this fragment and the continuation state (empty: last fragment).
pub fn parse_sdp_response(tid: u16, pdu: &[u8]) -> Result<(Vec<u8>, Vec<u8>), HidError> {
    if pdu.len() < 5 {
        return Err(HidError::Sdp("short answer".into()));
    }
    if u16::from_be_bytes([pdu[1], pdu[2]]) != tid {
        return Err(HidError::Sdp("answer to another request".into()));
    }
    let plen = u16::from_be_bytes([pdu[3], pdu[4]]) as usize;
    let p = pdu.get(5..5 + plen).ok_or_else(|| HidError::Sdp("answer shorter than its length".into()))?;
    if pdu[0] == SDP_ERROR {
        return Err(HidError::Sdp(format!("the device refused (code {})", p.get(0..2).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0))));
    }
    if pdu[0] != SDP_SSA_RESPONSE || p.len() < 3 {
        return Err(HidError::Sdp("unexpected answer".into()));
    }
    let n = u16::from_be_bytes([p[0], p[1]]) as usize;
    let data = p.get(2..2 + n).ok_or_else(|| HidError::Sdp("attribute data shorter than announced".into()))?;
    let cl = *p.get(2 + n).ok_or_else(|| HidError::Sdp("no continuation state".into()))? as usize;
    let cont = p.get(3 + n..3 + n + cl).ok_or_else(|| HidError::Sdp("short continuation state".into()))?;
    Ok((data.to_vec(), cont.to_vec()))
}

/// The HID record inside the assembled attribute lists (a sequence of records, each a sequence of attribute id / value pairs).
pub fn parse_hid_record(lists: &[u8]) -> Result<HidRecord, HidError> {
    let (top, _) = parse_elem(lists).map_err(HidError::Sdp)?;
    for rec in top.seq().unwrap_or(&[]) {
        let attrs = rec.seq().unwrap_or(&[]);
        let get = |id: u64| attrs.chunks_exact(2).find(|c| c[0].uint() == Some(id)).map(|c| &c[1]);
        // 0x0206 HIDDescriptorList: sequence of sequences [class type 0x22, descriptor string]
        let desc = get(0x0206).and_then(|l| l.seq()).and_then(|l| {
            l.iter().find_map(|d| {
                let d = d.seq()?;
                match (d.first()?.uint(), d.get(1)) {
                    (Some(0x22), Some(Elem::Str(b))) => Some(b.clone()),
                    _ => None,
                }
            })
        });
        let Some(descriptor) = desc else { continue };
        if descriptor.is_empty() || descriptor.len() > MAX_DESCRIPTOR {
            return Err(HidError::Sdp(format!("the report descriptor has {} bytes, which is not usable", descriptor.len())));
        }
        let name = get(0x0100).and_then(|e| if let Elem::Str(b) = e { Some(String::from_utf8_lossy(b).trim_matches('\0').trim().to_string()) } else { None }).filter(|n| !n.is_empty());
        let reconnect_initiate = get(0x0205).and_then(|e| if let Elem::Bool(b) = e { Some(*b) } else { None });
        return Ok(HidRecord { descriptor, name, reconnect_initiate });
    }
    Err(HidError::NoHidService)
}

/// Reads the HID record over an SDP channel, following continuation fragments.
pub fn fetch_hid_record<C: Channel>(ch: &mut C, timeout: Duration) -> Result<HidRecord, HidError> {
    let deadline = Instant::now() + timeout;
    let mut all = Vec::new();
    let mut cont: Vec<u8> = Vec::new();
    for tid in 1u16..=64 {
        ch.send(&sdp_request(tid, &cont))?;
        let pdu = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(HidError::Timeout);
            }
            match ch.recv(left)? {
                Recv::Data(d) => break d,
                Recv::Timeout => return Err(HidError::Timeout),
                Recv::Closed => return Err(HidError::Closed),
            }
        };
        let (data, next) = parse_sdp_response(tid, &pdu)?;
        all.extend_from_slice(&data);
        if all.len() > 64 * 1024 {
            return Err(HidError::Sdp("the answer is far too long".into()));
        }
        if next.is_empty() {
            return parse_hid_record(&all);
        }
        cont = next;
    }
    Err(HidError::Sdp("the answer has too many parts".into()))
}

// ------------------------------------------------------------------------------------------------ HIDP framing

/// `0xA1`: a DATA message carrying an input report (report type 1) on the interrupt channel.
pub const DATA_INPUT: u8 = 0xa1;
/// `0xA2`: a DATA message carrying an output report (LEDs).
pub const DATA_OUTPUT: u8 = 0xa2;
/// Control: HANDSHAKE result 0 is success.
pub const HS_SUCCESSFUL: u8 = 0;
/// Control: HID_CONTROL operation VIRTUAL_CABLE_UNPLUG: the device asks to be forgotten.
pub const CTRL_VIRTUAL_CABLE_UNPLUG: u8 = 5;

/// `SET_PROTOCOL` to report protocol (a keyboard may start in boot protocol).
pub fn set_protocol_report() -> Vec<u8> {
    vec![0x71]
}

/// The report of an input DATA message (`0xA1` + report), or `None` for anything else.
pub fn input_report(msg: &[u8]) -> Option<&[u8]> {
    if msg.len() >= 2 && msg[0] == DATA_INPUT { Some(&msg[1..]) } else { None }
}

pub fn output_message(report: &[u8]) -> Vec<u8> {
    let mut v = vec![DATA_OUTPUT];
    v.extend_from_slice(report);
    v
}

// ------------------------------------------------------------------------------------------------ UHID

pub const UHID_DESTROY: u32 = 1;
pub const UHID_START: u32 = 2;
pub const UHID_STOP: u32 = 3;
pub const UHID_OPEN: u32 = 4;
pub const UHID_CLOSE: u32 = 5;
pub const UHID_OUTPUT: u32 = 6;
pub const UHID_GET_REPORT: u32 = 9;
pub const UHID_GET_REPORT_REPLY: u32 = 10;
pub const UHID_CREATE2: u32 = 11;
pub const UHID_INPUT2: u32 = 12;
pub const UHID_SET_REPORT: u32 = 13;
pub const UHID_SET_REPORT_REPLY: u32 = 14;
/// `sizeof(struct uhid_event)`: a 4-byte type and the largest request (`UHID_CREATE2`, 4372 bytes).
pub const UHID_EVENT_SIZE: usize = 4 + 128 + 64 + 64 + 2 + 2 + 4 * 4 + MAX_DESCRIPTOR;
const BUS_BLUETOOTH: u16 = 5;

fn event(ty: u32) -> Vec<u8> {
    let mut v = vec![0u8; UHID_EVENT_SIZE];
    v[0..4].copy_from_slice(&ty.to_le_bytes());
    v
}

fn put_str(dst: &mut [u8], s: &str) {
    // keep room for the terminating zero
    let b = s.as_bytes();
    let n = b.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&b[..n]);
}

/// `UHID_CREATE2`: makes the input device from a report descriptor.
pub fn uhid_create2(name: &str, uniq: &str, vendor: u32, product: u32, descriptor: &[u8]) -> Vec<u8> {
    let mut v = event(UHID_CREATE2);
    put_str(&mut v[4..4 + 128], name);
    put_str(&mut v[4 + 192..4 + 192 + 64], uniq);
    let o = 4 + 128 + 64 + 64;
    v[o..o + 2].copy_from_slice(&(descriptor.len().min(MAX_DESCRIPTOR) as u16).to_le_bytes());
    v[o + 2..o + 4].copy_from_slice(&BUS_BLUETOOTH.to_le_bytes());
    v[o + 4..o + 8].copy_from_slice(&vendor.to_le_bytes());
    v[o + 8..o + 12].copy_from_slice(&product.to_le_bytes());
    // version and country stay 0
    let d = o + 20;
    let n = descriptor.len().min(MAX_DESCRIPTOR);
    v[d..d + n].copy_from_slice(&descriptor[..n]);
    v
}

/// `UHID_INPUT2`: one input report (with its report id first when the descriptor uses report ids).
pub fn uhid_input2(report: &[u8]) -> Vec<u8> {
    let mut v = event(UHID_INPUT2);
    let n = report.len().min(MAX_DESCRIPTOR);
    v[4..6].copy_from_slice(&(n as u16).to_le_bytes());
    v[6..6 + n].copy_from_slice(&report[..n]);
    v
}

pub fn uhid_destroy() -> Vec<u8> {
    event(UHID_DESTROY)
}

pub fn uhid_get_report_reply(id: u32, err: u16) -> Vec<u8> {
    let mut v = event(UHID_GET_REPORT_REPLY);
    v[4..8].copy_from_slice(&id.to_le_bytes());
    v[8..10].copy_from_slice(&err.to_le_bytes());
    v
}

pub fn uhid_set_report_reply(id: u32, err: u16) -> Vec<u8> {
    let mut v = event(UHID_SET_REPORT_REPLY);
    v[4..8].copy_from_slice(&id.to_le_bytes());
    v[8..10].copy_from_slice(&err.to_le_bytes());
    v
}

/// What the kernel tells us about the input device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UhidEvent {
    Start,
    Stop,
    Open,
    Close,
    /// An output report (LED state) to send to the device.
    Output(Vec<u8>),
    GetReport { id: u32 },
    SetReport { id: u32 },
    Other(u32),
}

pub fn parse_uhid(buf: &[u8]) -> Option<UhidEvent> {
    let ty = u32::from_le_bytes(buf.get(0..4)?.try_into().ok()?);
    Some(match ty {
        UHID_START => UhidEvent::Start,
        UHID_STOP => UhidEvent::Stop,
        UHID_OPEN => UhidEvent::Open,
        UHID_CLOSE => UhidEvent::Close,
        UHID_OUTPUT => {
            // data[4096], size u16, rtype u8
            let size = u16::from_le_bytes(buf.get(4 + MAX_DESCRIPTOR..6 + MAX_DESCRIPTOR)?.try_into().ok()?) as usize;
            UhidEvent::Output(buf.get(4..4 + size.min(MAX_DESCRIPTOR))?.to_vec())
        }
        UHID_GET_REPORT => UhidEvent::GetReport { id: u32::from_le_bytes(buf.get(4..8)?.try_into().ok()?) },
        UHID_SET_REPORT => UhidEvent::SetReport { id: u32::from_le_bytes(buf.get(4..8)?.try_into().ok()?) },
        other => UhidEvent::Other(other),
    })
}

// ------------------------------------------------------------------------------------------------ the session

/// What a message did to the session.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// The device asked to be unplugged (forgotten) with a virtual cable unplug.
    Unplugged,
}

/// One connected HID device: its input device in the kernel and the translation of messages in both directions.
pub struct Session<U: Uhid> {
    uhid: U,
    created: bool,
    pub reports: u64,
}

impl<U: Uhid> Session<U> {
    /// Creates the input device. `control` gets `SET_PROTOCOL(report)` (best effort: a device that answers with an error keeps its protocol).
    pub fn open<C: Channel>(mut uhid: U, name: &str, uniq: &str, vendor: u32, product: u32, descriptor: &[u8], control: &mut C) -> Result<Session<U>, HidError> {
        if descriptor.is_empty() || descriptor.len() > MAX_DESCRIPTOR {
            return Err(HidError::Sdp("the report descriptor is not usable".into()));
        }
        uhid.write(&uhid_create2(name, uniq, vendor, product, descriptor)).map_err(HidError::Uhid)?;
        let _ = control.send(&set_protocol_report());
        Ok(Session { uhid, created: true, reports: 0 })
    }

    /// A message from the interrupt channel.
    pub fn on_interrupt(&mut self, msg: &[u8]) -> Result<Flow, HidError> {
        if let Some(r) = input_report(msg) {
            self.uhid.write(&uhid_input2(r)).map_err(HidError::Uhid)?;
            self.reports += 1;
        }
        Ok(Flow::Continue)
    }

    /// A message from the control channel (handshakes are ignored; a virtual cable unplug ends the bond).
    pub fn on_control(&mut self, msg: &[u8]) -> Flow {
        match msg.first() {
            Some(b) if b >> 4 == 1 && b & 0x0f == CTRL_VIRTUAL_CABLE_UNPLUG => Flow::Unplugged,
            _ => Flow::Continue,
        }
    }

    /// A request from the kernel's input device. LED reports go to the device; report requests are answered with an error at once
    /// (the kernel would otherwise wait for its time-out).
    pub fn on_uhid<C: Channel>(&mut self, ev: UhidEvent, interrupt: &mut C) -> Result<(), HidError> {
        match ev {
            UhidEvent::Output(r) if !r.is_empty() => interrupt.send(&output_message(&r))?,
            UhidEvent::GetReport { id } => self.uhid.write(&uhid_get_report_reply(id, libc::EIO as u16)).map_err(HidError::Uhid)?,
            UhidEvent::SetReport { id } => self.uhid.write(&uhid_set_report_reply(id, libc::EIO as u16)).map_err(HidError::Uhid)?,
            _ => {}
        }
        Ok(())
    }

    /// Removes the input device.
    pub fn close(&mut self) {
        if self.created {
            let _ = self.uhid.write(&uhid_destroy());
            self.created = false;
        }
    }
}

impl<U: Uhid> Drop for Session<U> {
    fn drop(&mut self) {
        self.close();
    }
}

/// One thing that happened while a session runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Ev {
    Control(Vec<u8>),
    Interrupt(Vec<u8>),
    Uhid(Vec<u8>),
    /// A channel closed (the device went away or switched off).
    ChannelClosed,
    Idle,
}

/// Waits for the next event on the three descriptors (the real one is `poll`; tests script it).
pub trait Source {
    fn wait(&mut self, t: Duration) -> io::Result<Ev>;
}

/// Why a running session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// We were told to stop (Disconnect, Forget, shutting down).
    Stopped,
    /// The link went away (the device switched off, moved out of range or disconnected itself): reconnect applies.
    LinkLost,
    /// The device asked to be unplugged: the bond is forgotten.
    Unplugged,
    Failed(String),
}

/// Runs a session until it ends.
pub fn run_session<U: Uhid, C: Channel, S: Source>(sess: &mut Session<U>, interrupt: &mut C, src: &mut S, stop: &AtomicBool) -> End {
    loop {
        if stop.load(Ordering::SeqCst) {
            return End::Stopped;
        }
        let ev = match src.wait(Duration::from_millis(500)) {
            Ok(e) => e,
            Err(e) => return End::Failed(e.to_string()),
        };
        let r = match ev {
            Ev::Idle => Ok(()),
            Ev::ChannelClosed => return End::LinkLost,
            Ev::Interrupt(m) => sess.on_interrupt(&m).map(|_| ()),
            Ev::Control(m) => {
                if sess.on_control(&m) == Flow::Unplugged {
                    return End::Unplugged;
                }
                Ok(())
            }
            Ev::Uhid(b) => match parse_uhid(&b) {
                Some(e) => sess.on_uhid(e, interrupt),
                None => Ok(()),
            },
        };
        if let Err(e) = r {
            return End::Failed(e.to_string());
        }
    }
}

/// Seconds to wait before the next try to reconnect a device that is bonded and was lost; `None`: stop trying (the device connects back
/// itself when it is switched on, which the listener accepts).
pub fn retry_delay(attempt: u32) -> Option<Duration> {
    match attempt {
        0 => Some(Duration::from_secs(3)),
        1 => Some(Duration::from_secs(10)),
        2 => Some(Duration::from_secs(30)),
        3 | 4 => Some(Duration::from_secs(60)),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ what is remembered

pub const OP_LOAD_LINK_KEYS: u16 = 0x0012;
pub const OP_LOAD_LTKS: u16 = 0x0013;
pub const EV_NEW_LINK_KEY: u16 = 0x0009;
pub const EV_NEW_LTK: u16 = 0x000a;

/// A device the person paired (kept across restarts).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub address: String,
    pub addr_type: u8,
    pub name: String,
    /// "keyboard", "mouse", "game pad" or "other"
    pub kind: String,
    /// The HID report descriptor, hex (cached after the first connection so a reconnect needs no SDP).
    #[serde(default)]
    pub descriptor: String,
    #[serde(default)]
    pub vendor: u32,
    #[serde(default)]
    pub product: u32,
    /// Connect again after a restart and after the link was lost.
    #[serde(default = "yes")]
    pub auto: bool,
}

fn yes() -> bool {
    true
}

/// A classic link key as the kernel's management interface gives it (25 bytes on the wire). Secret: the file is mode 0600.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkKey {
    pub address: String,
    pub addr_type: u8,
    pub key_type: u8,
    pub value: String,
    pub pin_len: u8,
}

/// A low-energy long term key (36 bytes on the wire). Kept so a later HOGP step can reuse the bond.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ltk {
    pub address: String,
    pub addr_type: u8,
    pub raw: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub devices: Vec<Known>,
    #[serde(default)]
    pub link_keys: Vec<LinkKey>,
    #[serde(default)]
    pub ltks: Vec<Ltk>,
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

impl Store {
    /// A missing or unreadable file is an empty store (nothing is paired yet).
    pub fn parse(text: &str) -> Store {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn device(&self, address: &str) -> Option<&Known> {
        self.devices.iter().find(|d| d.address.eq_ignore_ascii_case(address))
    }

    pub fn upsert(&mut self, k: Known) {
        match self.devices.iter_mut().find(|d| d.address.eq_ignore_ascii_case(&k.address)) {
            Some(d) => *d = k,
            None => self.devices.push(k),
        }
    }

    /// Forget a device and its keys. Returns whether anything was there.
    pub fn forget(&mut self, address: &str) -> bool {
        let n = self.devices.len() + self.link_keys.len() + self.ltks.len();
        self.devices.retain(|d| !d.address.eq_ignore_ascii_case(address));
        self.link_keys.retain(|k| !k.address.eq_ignore_ascii_case(address));
        self.ltks.retain(|k| !k.address.eq_ignore_ascii_case(address));
        n != self.devices.len() + self.link_keys.len() + self.ltks.len()
    }

    /// A New Link Key event (`store_hint`, then the 25-byte key). Returns whether the store changed (the hint says whether it is to be kept).
    pub fn note_link_key(&mut self, params: &[u8]) -> bool {
        if params.len() < 26 || params[0] == 0 {
            return false;
        }
        let k = LinkKey { address: btmgmt::addr_string(&params[1..7]), addr_type: params[7], key_type: params[8], value: hex(&params[9..25]), pin_len: params[25] };
        self.link_keys.retain(|x| !(x.address == k.address && x.addr_type == k.addr_type));
        self.link_keys.push(k);
        true
    }

    /// A New Long Term Key event (`store_hint`, then the 36-byte key).
    pub fn note_ltk(&mut self, params: &[u8]) -> bool {
        if params.len() < 37 || params[0] == 0 {
            return false;
        }
        let k = Ltk { address: btmgmt::addr_string(&params[1..7]), addr_type: params[7], raw: hex(&params[1..37]) };
        self.ltks.retain(|x| !(x.address == k.address && x.addr_type == k.addr_type));
        self.ltks.push(k);
        true
    }

    /// Load Link Keys parameters (`debug_keys` 0, count, keys), or `None` when there are none.
    pub fn link_keys_params(&self) -> Option<Vec<u8>> {
        let mut body = Vec::new();
        let mut n = 0u16;
        for k in &self.link_keys {
            let (Some(a), Some(v)) = (btmgmt::parse_addr(&k.address), unhex(&k.value)) else { continue };
            if v.len() != 16 {
                continue;
            }
            body.extend_from_slice(&a);
            body.extend_from_slice(&[k.addr_type, k.key_type]);
            body.extend_from_slice(&v);
            body.push(k.pin_len);
            n += 1;
        }
        if n == 0 {
            return None;
        }
        let mut p = vec![0u8];
        p.extend_from_slice(&n.to_le_bytes());
        p.extend_from_slice(&body);
        Some(p)
    }

    /// Load Long Term Keys parameters (count, keys), or `None`.
    pub fn ltk_params(&self) -> Option<Vec<u8>> {
        let keys: Vec<Vec<u8>> = self.ltks.iter().filter_map(|k| unhex(&k.raw)).filter(|b| b.len() == 36).collect();
        if keys.is_empty() {
            return None;
        }
        let mut p = (keys.len() as u16).to_le_bytes().to_vec();
        keys.iter().for_each(|k| p.extend_from_slice(k));
        Some(p)
    }
}

/// Gives the kernel the remembered keys again after a restart, so bonded devices reconnect without pairing. Returns how many keys went in.
pub fn load_keys<T: Transport>(m: &mut Mgmt<T>, index: u16, store: &Store) -> Result<usize, MgmtError> {
    let mut n = 0;
    if let Some(p) = store.link_keys_params() {
        m.command(OP_LOAD_LINK_KEYS, index, &p)?;
        n += store.link_keys.len();
    }
    if let Some(p) = store.ltk_params() {
        m.command(OP_LOAD_LTKS, index, &p)?;
        n += store.ltks.len();
    }
    Ok(n)
}

/// The label stored for a kind of device.
pub fn kind_label(k: btmgmt::Kind) -> &'static str {
    match k {
        btmgmt::Kind::Keyboard => "keyboard",
        btmgmt::Kind::Mouse => "mouse",
        btmgmt::Kind::Gamepad => "game pad",
        btmgmt::Kind::Headphones => "headphones",
        btmgmt::Kind::Speaker => "speaker",
        btmgmt::Kind::Phone => "phone",
        btmgmt::Kind::Computer => "computer",
        btmgmt::Kind::Other => "other",
    }
}

/// Whether this kind of device is used through a HID session in this slice.
pub fn is_hid_kind(k: btmgmt::Kind) -> bool {
    matches!(k, btmgmt::Kind::Keyboard | btmgmt::Kind::Mouse | btmgmt::Kind::Gamepad)
}

/// Devices this version can pair: keyboards, mice and game pads (to use them) and phones (for their Bluetooth tethering, [`crate::bt_pan`]).
pub fn is_pairable_kind(k: btmgmt::Kind) -> bool {
    is_hid_kind(k) || k == btmgmt::Kind::Phone
}

// ------------------------------------------------------------------------------------------------ the list the pages and tools use

/// Where a device is in its connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// One paired device as Settings and the assistant show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceView {
    pub address: String,
    pub addr_type: u8,
    pub name: String,
    pub kind: String,
    pub state: LinkState,
}

impl DeviceView {
    pub fn title(&self) -> String {
        if self.name.is_empty() { self.address.clone() } else { self.name.clone() }
    }

    /// The second line: state and, when something failed, the plain reason.
    pub fn line(&self) -> String {
        let s = match &self.state {
            LinkState::Connected => "connected".to_string(),
            LinkState::Connecting => "connecting ...".to_string(),
            LinkState::Disconnected => "paired, not connected".to_string(),
            LinkState::Failed(why) => format!("not connected: {why}"),
        };
        format!("{} · {} · {s}", self.kind, self.address)
    }
}

/// The paired devices and what can be done with them. The real one is [`crate::bthid_sys::hub`]; tests use a fake.
pub trait Devices: Send + Sync {
    fn list(&self) -> Vec<DeviceView>;
    /// Starts connecting (it takes seconds; the state shows the progress and, on failure, the reason).
    fn connect(&self, address: &str) -> Result<(), String>;
    fn disconnect(&self, address: &str) -> Result<(), String>;
    /// Forgets the pairing: disconnects and removes the keys.
    fn forget(&self, address: &str) -> Result<(), String>;
}

/// Finds a device by address or by name (case does not matter; a name must match exactly one).
pub fn find_device(list: &[DeviceView], who: &str) -> Result<DeviceView, String> {
    let w = who.trim();
    if w.is_empty() {
        return Err("name the device (its address or its name; bluetooth_list shows them)".into());
    }
    if let Some(d) = list.iter().find(|d| d.address.eq_ignore_ascii_case(w)) {
        return Ok(d.clone());
    }
    let hits: Vec<&DeviceView> = list.iter().filter(|d| d.name.eq_ignore_ascii_case(w)).collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err("there is no such paired Bluetooth device (bluetooth_list shows them)".into()),
        _ => Err("more than one paired device has that name: use its address".into()),
    }
}

/// What pairing is doing, for the page to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PairState {
    #[default]
    Idle,
    Running(String),
    /// Compare this number with the one on the device and confirm.
    Confirm(String, u32),
    /// A phone that shows no number ("just works"): the person still has to say that they want this phone paired.
    ConfirmNoNumber(String),
    /// Type this passkey on the keyboard, then Enter.
    ShowPasskey(String, u32),
    /// Type this PIN on the keyboard, then Enter.
    ShowPin(String, String),
    /// The other device shows a 6-digit passkey: the person types it here (`bt_pair_type`). Holds only the device name, never a number.
    TypePasskey(String),
    /// The other device shows a PIN (1-16 characters) to type here. The flag says the device is one that often has none printed and uses
    /// 0000 or 1234 (a mouse, a game pad): the page may offer those two as a hint.
    TypePin(String, bool),
    Done(Result<String, String>),
}

/// The passkey and the PIN a person types for a pairing: the checks that both the page and the backend make, as a plain message.
pub fn check_typed_passkey(text: &str) -> Result<u32, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("Type the 6 digits shown on the other device.".into());
    }
    if !t.chars().all(|c| c.is_ascii_digit()) {
        return Err("A passkey has digits only.".into());
    }
    if t.len() != 6 {
        return Err(format!("A passkey has exactly 6 digits; {} typed so far.", t.len()));
    }
    t.parse::<u32>().map_err(|_| "A passkey has exactly 6 digits.".to_string())
}

pub fn check_typed_pin(text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Err("Type the PIN shown on the other device.".into());
    }
    if text.chars().count() > 16 || text.len() > 16 {
        return Err("A PIN has at most 16 characters.".into());
    }
    if text.chars().any(|c| c.is_control()) {
        return Err("A PIN has printable characters only.".into());
    }
    Ok(text.to_string())
}

#[cfg(test)]
#[path = "bthid_tests.rs"]
mod tests;

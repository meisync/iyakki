//! Bluetooth low-energy keyboards, mice and game pads (HID over GATT, "HOGP") without BlueZ: the protocol parts, with no socket and no
//! device file in this module, so every byte is unit-tested with a scripted GATT server and a fake uhid. The real socket (L2CAP fixed
//! channel 4, the LE attribute protocol) and the hub are in [`crate::bthid_sys`].
//!
//! * [`Pdu`]: the attribute protocol (ATT) messages, parsed defensively: a short, odd or oversized packet is an error, never a panic.
//! * [`Gatt`]: a client over any [`Channel`]: MTU exchange, service / characteristic / descriptor discovery, read (long too), write,
//!   write command, notifications and indications (confirmed, queued while a request is waiting).
//! * [`discover_hid`]: finds the HID service 0x1812 (report map, reports with their Report Reference, HID information, protocol mode)
//!   and the PnP id of the device information service for the vendor and product numbers.
//! * [`HogpSession`]: creates the kernel input device from the report map, turns notifications into `UHID_INPUT2` exactly like the
//!   classic session does, and sends LED reports back as ATT write commands.
//!
//! Pairing and the long term key come from the kernel's management interface ([`crate::btmgmt`], `Pair Device` with an LE address
//! type); the link is encrypted by the kernel when the socket asks for it (`BT_SECURITY`).
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::bthid::{parse_uhid, uhid_create2, uhid_destroy, uhid_get_report_reply, uhid_input2, uhid_set_report_reply, Channel, Ev, HidError, Recv, Source, Uhid, UhidEvent, MAX_DESCRIPTOR};

pub const OP_ERROR: u8 = 0x01;
pub const OP_MTU_REQ: u8 = 0x02;
pub const OP_MTU_RSP: u8 = 0x03;
pub const OP_FIND_INFO_REQ: u8 = 0x04;
pub const OP_FIND_INFO_RSP: u8 = 0x05;
pub const OP_READ_BY_TYPE_REQ: u8 = 0x08;
pub const OP_READ_BY_TYPE_RSP: u8 = 0x09;
pub const OP_READ_REQ: u8 = 0x0a;
pub const OP_READ_RSP: u8 = 0x0b;
pub const OP_READ_BLOB_REQ: u8 = 0x0c;
pub const OP_READ_BLOB_RSP: u8 = 0x0d;
pub const OP_READ_BY_GROUP_REQ: u8 = 0x10;
pub const OP_READ_BY_GROUP_RSP: u8 = 0x11;
pub const OP_WRITE_REQ: u8 = 0x12;
pub const OP_WRITE_RSP: u8 = 0x13;
pub const OP_WRITE_CMD: u8 = 0x52;
pub const OP_NOTIFY: u8 = 0x1b;
pub const OP_INDICATE: u8 = 0x1d;
pub const OP_CONFIRM: u8 = 0x1e;

/// The default ATT message size on LE before an exchange.
pub const DEFAULT_MTU: u16 = 23;
/// The largest message this client asks for.
pub const WANT_MTU: u16 = 247;
/// Never read more than this many bytes of one attribute (a hostile server could go on for ever).
pub const MAX_ATTR: usize = MAX_DESCRIPTOR;

pub const UUID_PRIMARY: u16 = 0x2800;
pub const UUID_CHARACTERISTIC: u16 = 0x2803;
pub const UUID_CCC: u16 = 0x2902;
pub const UUID_REPORT_REF: u16 = 0x2908;
pub const SVC_HID: u16 = 0x1812;
pub const SVC_BATTERY: u16 = 0x180f;
pub const CH_HID_INFO: u16 = 0x2a4a;
pub const CH_REPORT_MAP: u16 = 0x2a4b;
pub const CH_HID_CONTROL: u16 = 0x2a4c;
pub const CH_REPORT: u16 = 0x2a4d;
pub const CH_PROTOCOL_MODE: u16 = 0x2a4e;
pub const CH_PNP_ID: u16 = 0x2a50;
pub const CH_BATTERY_LEVEL: u16 = 0x2a19;

/// ATT error codes the client reacts to.
pub const ERR_INSUFFICIENT_AUTHENTICATION: u8 = 0x05;
pub const ERR_INSUFFICIENT_ENCRYPTION: u8 = 0x0f;
pub const ERR_ATTRIBUTE_NOT_FOUND: u8 = 0x0a;

/// An attribute UUID: the 16-bit short form, or the full 128 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uuid {
    Short(u16),
    Long([u8; 16]),
}

impl Uuid {
    pub fn from_bytes(b: &[u8]) -> Option<Uuid> {
        match b.len() {
            2 => Some(Uuid::Short(u16::from_le_bytes([b[0], b[1]]))),
            16 => Some(Uuid::Long(b.try_into().ok()?)),
            _ => None,
        }
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Uuid::Short(s) => s.to_le_bytes().to_vec(),
            Uuid::Long(l) => l.to_vec(),
        }
    }
    pub fn short(&self) -> Option<u16> {
        match self {
            Uuid::Short(s) => Some(*s),
            Uuid::Long(_) => None,
        }
    }
}

/// One attribute protocol message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pdu {
    Error { op: u8, handle: u16, code: u8 },
    MtuReq(u16),
    MtuRsp(u16),
    FindInfoReq { start: u16, end: u16 },
    FindInfoRsp(Vec<(u16, Uuid)>),
    ReadByTypeReq { start: u16, end: u16, uuid: Uuid },
    ReadByTypeRsp(Vec<(u16, Vec<u8>)>),
    ReadReq(u16),
    ReadRsp(Vec<u8>),
    ReadBlobReq { handle: u16, offset: u16 },
    ReadBlobRsp(Vec<u8>),
    ReadByGroupReq { start: u16, end: u16, uuid: Uuid },
    ReadByGroupRsp(Vec<(u16, u16, Vec<u8>)>),
    WriteReq { handle: u16, value: Vec<u8> },
    WriteRsp,
    WriteCmd { handle: u16, value: Vec<u8> },
    Notify { handle: u16, value: Vec<u8> },
    Indicate { handle: u16, value: Vec<u8> },
    Confirm,
    /// A message this client does not use (a server request, a signed write ...).
    Other(u8),
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, String> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "message too short".to_string())
}

impl Pdu {
    pub fn opcode(&self) -> u8 {
        match self {
            Pdu::Error { .. } => OP_ERROR,
            Pdu::MtuReq(_) => OP_MTU_REQ,
            Pdu::MtuRsp(_) => OP_MTU_RSP,
            Pdu::FindInfoReq { .. } => OP_FIND_INFO_REQ,
            Pdu::FindInfoRsp(_) => OP_FIND_INFO_RSP,
            Pdu::ReadByTypeReq { .. } => OP_READ_BY_TYPE_REQ,
            Pdu::ReadByTypeRsp(_) => OP_READ_BY_TYPE_RSP,
            Pdu::ReadReq(_) => OP_READ_REQ,
            Pdu::ReadRsp(_) => OP_READ_RSP,
            Pdu::ReadBlobReq { .. } => OP_READ_BLOB_REQ,
            Pdu::ReadBlobRsp(_) => OP_READ_BLOB_RSP,
            Pdu::ReadByGroupReq { .. } => OP_READ_BY_GROUP_REQ,
            Pdu::ReadByGroupRsp(_) => OP_READ_BY_GROUP_RSP,
            Pdu::WriteReq { .. } => OP_WRITE_REQ,
            Pdu::WriteRsp => OP_WRITE_RSP,
            Pdu::WriteCmd { .. } => OP_WRITE_CMD,
            Pdu::Notify { .. } => OP_NOTIFY,
            Pdu::Indicate { .. } => OP_INDICATE,
            Pdu::Confirm => OP_CONFIRM,
            Pdu::Other(o) => *o,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![self.opcode()];
        let p16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        match self {
            Pdu::Error { op, handle, code } => {
                v.push(*op);
                p16(&mut v, *handle);
                v.push(*code);
            }
            Pdu::MtuReq(m) | Pdu::MtuRsp(m) => p16(&mut v, *m),
            Pdu::FindInfoReq { start, end } => {
                p16(&mut v, *start);
                p16(&mut v, *end);
            }
            Pdu::FindInfoRsp(list) => {
                let long = list.first().is_some_and(|(_, u)| matches!(u, Uuid::Long(_)));
                v.push(if long { 2 } else { 1 });
                for (h, u) in list.iter().filter(|(_, u)| matches!(u, Uuid::Long(_)) == long) {
                    p16(&mut v, *h);
                    v.extend_from_slice(&u.to_bytes());
                }
            }
            Pdu::ReadByTypeReq { start, end, uuid } | Pdu::ReadByGroupReq { start, end, uuid } => {
                p16(&mut v, *start);
                p16(&mut v, *end);
                v.extend_from_slice(&uuid.to_bytes());
            }
            Pdu::ReadByTypeRsp(list) => {
                let len = list.first().map_or(0, |(_, d)| d.len() + 2);
                v.push(len as u8);
                for (h, d) in list.iter().filter(|(_, d)| d.len() + 2 == len) {
                    p16(&mut v, *h);
                    v.extend_from_slice(d);
                }
            }
            Pdu::ReadByGroupRsp(list) => {
                let len = list.first().map_or(0, |(_, _, d)| d.len() + 4);
                v.push(len as u8);
                for (s, e, d) in list.iter().filter(|(_, _, d)| d.len() + 4 == len) {
                    p16(&mut v, *s);
                    p16(&mut v, *e);
                    v.extend_from_slice(d);
                }
            }
            Pdu::ReadReq(h) => p16(&mut v, *h),
            Pdu::ReadRsp(d) | Pdu::ReadBlobRsp(d) => v.extend_from_slice(d),
            Pdu::ReadBlobReq { handle, offset } => {
                p16(&mut v, *handle);
                p16(&mut v, *offset);
            }
            Pdu::WriteReq { handle, value } | Pdu::WriteCmd { handle, value } | Pdu::Notify { handle, value } | Pdu::Indicate { handle, value } => {
                p16(&mut v, *handle);
                v.extend_from_slice(value);
            }
            Pdu::WriteRsp | Pdu::Confirm | Pdu::Other(_) => {}
        }
        v
    }

    /// Reads one message. Anything malformed is an error with a short reason.
    pub fn parse(b: &[u8]) -> Result<Pdu, String> {
        let op = *b.first().ok_or("empty message")?;
        let rest = &b[1..];
        Ok(match op {
            OP_ERROR => {
                if rest.len() != 4 {
                    return Err("error response has the wrong size".into());
                }
                Pdu::Error { op: rest[0], handle: u16_at(rest, 1)?, code: rest[3] }
            }
            OP_MTU_REQ | OP_MTU_RSP => {
                if rest.len() != 2 {
                    return Err("MTU message has the wrong size".into());
                }
                let m = u16_at(rest, 0)?;
                if op == OP_MTU_REQ { Pdu::MtuReq(m) } else { Pdu::MtuRsp(m) }
            }
            OP_FIND_INFO_REQ if rest.len() == 4 => Pdu::FindInfoReq { start: u16_at(rest, 0)?, end: u16_at(rest, 2)? },
            OP_FIND_INFO_RSP => {
                let size = match rest.first() {
                    Some(1) => 2,
                    Some(2) => 16,
                    _ => return Err("find information response has an unknown format".into()),
                };
                let body = &rest[1..];
                if body.is_empty() || body.len() % (2 + size) != 0 {
                    return Err("find information response is cut".into());
                }
                let mut list = Vec::new();
                for c in body.chunks_exact(2 + size) {
                    list.push((u16_at(c, 0)?, Uuid::from_bytes(&c[2..]).ok_or("bad uuid")?));
                }
                Pdu::FindInfoRsp(list)
            }
            OP_READ_BY_TYPE_REQ | OP_READ_BY_GROUP_REQ if rest.len() == 6 || rest.len() == 20 => {
                let (start, end, uuid) = (u16_at(rest, 0)?, u16_at(rest, 2)?, Uuid::from_bytes(&rest[4..]).ok_or("bad uuid")?);
                if op == OP_READ_BY_TYPE_REQ { Pdu::ReadByTypeReq { start, end, uuid } } else { Pdu::ReadByGroupReq { start, end, uuid } }
            }
            OP_READ_BY_TYPE_RSP => {
                let len = *rest.first().ok_or("read by type response is empty")? as usize;
                let body = &rest[1..];
                if len < 2 || body.is_empty() || body.len() % len != 0 {
                    return Err("read by type response is cut".into());
                }
                Pdu::ReadByTypeRsp(body.chunks_exact(len).map(|c| (u16::from_le_bytes([c[0], c[1]]), c[2..].to_vec())).collect())
            }
            OP_READ_BY_GROUP_RSP => {
                let len = *rest.first().ok_or("read by group type response is empty")? as usize;
                let body = &rest[1..];
                if len < 4 || body.is_empty() || body.len() % len != 0 {
                    return Err("read by group type response is cut".into());
                }
                Pdu::ReadByGroupRsp(body.chunks_exact(len).map(|c| (u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]), c[4..].to_vec())).collect())
            }
            OP_READ_REQ if rest.len() == 2 => Pdu::ReadReq(u16_at(rest, 0)?),
            OP_READ_RSP => Pdu::ReadRsp(rest.to_vec()),
            OP_READ_BLOB_REQ if rest.len() == 4 => Pdu::ReadBlobReq { handle: u16_at(rest, 0)?, offset: u16_at(rest, 2)? },
            OP_READ_BLOB_RSP => Pdu::ReadBlobRsp(rest.to_vec()),
            OP_WRITE_REQ | OP_WRITE_CMD | OP_NOTIFY | OP_INDICATE => {
                let handle = u16_at(rest, 0)?;
                let value = rest[2..].to_vec();
                match op {
                    OP_WRITE_REQ => Pdu::WriteReq { handle, value },
                    OP_WRITE_CMD => Pdu::WriteCmd { handle, value },
                    OP_NOTIFY => Pdu::Notify { handle, value },
                    _ => Pdu::Indicate { handle, value },
                }
            }
            OP_WRITE_RSP if rest.is_empty() => Pdu::WriteRsp,
            OP_CONFIRM if rest.is_empty() => Pdu::Confirm,
            // known opcodes with a wrong size are malformed, unknown ones are simply not used here
            OP_FIND_INFO_REQ | OP_READ_BY_TYPE_REQ | OP_READ_BY_GROUP_REQ | OP_READ_REQ | OP_READ_BLOB_REQ | OP_WRITE_RSP | OP_CONFIRM => return Err("message has the wrong size".into()),
            other => Pdu::Other(other),
        })
    }
}

// ------------------------------------------------------------------------------------------------ the client

/// Why a GATT step failed.
#[derive(Debug)]
pub enum GattError {
    /// The server answered with an error response (operation, handle, code).
    Att { op: u8, handle: u16, code: u8 },
    Timeout,
    Closed,
    /// The server sent something that makes no sense.
    Protocol(String),
    Io(std::io::Error),
}

impl std::fmt::Display for GattError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GattError::Att { code, .. } => write!(f, "{}", att_reason(*code)),
            GattError::Timeout => write!(f, "the device did not answer in time"),
            GattError::Closed => write!(f, "the device closed the connection"),
            GattError::Protocol(m) => write!(f, "the device sent something unexpected ({m})"),
            GattError::Io(e) => write!(f, "the Bluetooth link failed ({e})"),
        }
    }
}

impl std::error::Error for GattError {}

impl From<std::io::Error> for GattError {
    fn from(e: std::io::Error) -> GattError {
        GattError::Io(e)
    }
}

impl From<GattError> for HidError {
    fn from(e: GattError) -> HidError {
        match e {
            GattError::Timeout => HidError::Timeout,
            GattError::Closed => HidError::Closed,
            GattError::Io(e) => HidError::Io(e),
            other => HidError::Sdp(other.to_string()),
        }
    }
}

/// Plain-language reason for an ATT error code.
pub fn att_reason(code: u8) -> String {
    match code {
        0x01 => "the device does not know that item (invalid handle)".into(),
        0x02 => "the device does not allow reading it".into(),
        0x03 => "the device does not allow writing it".into(),
        0x05 => "the device wants the pairing first (authentication needed): choose Pair for it".into(),
        0x08 => "the device wants an authorised pairing".into(),
        0x06 => "the device does not support that request".into(),
        0x0a => "the device does not have that item".into(),
        0x0f => "the device wants an encrypted link: pair it again".into(),
        0x0c => "the device needs a longer encryption key".into(),
        c => format!("the device refused (code 0x{c:02x})"),
    }
}

/// A GATT client over one ATT channel.
pub struct Gatt<C: Channel> {
    ch: C,
    pub mtu: u16,
    /// How long one request waits for its answer.
    pub timeout: Duration,
    notes: VecDeque<(u16, Vec<u8>)>,
}

/// Longest discovery loop: a server that never ends its answer cannot keep the client busy for ever.
const MAX_ROUNDS: usize = 256;

impl<C: Channel> Gatt<C> {
    pub fn new(ch: C) -> Gatt<C> {
        Gatt { ch, mtu: DEFAULT_MTU, timeout: Duration::from_secs(5), notes: VecDeque::new() }
    }

    pub fn channel(&mut self) -> &mut C {
        &mut self.ch
    }

    pub fn send(&mut self, p: &Pdu) -> Result<(), GattError> {
        Ok(self.ch.send(&p.encode())?)
    }

    fn keep_notification(&mut self, p: &Pdu) -> Result<bool, GattError> {
        match p {
            Pdu::Notify { handle, value } => {
                self.notes.push_back((*handle, value.clone()));
                Ok(true)
            }
            Pdu::Indicate { handle, value } => {
                self.notes.push_back((*handle, value.clone()));
                self.send(&Pdu::Confirm)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Sends a request and returns its response (or the server's error). Notifications that arrive meanwhile are kept; messages that do
    /// not belong are ignored.
    pub fn request(&mut self, req: &Pdu, answer: u8) -> Result<Pdu, GattError> {
        self.send(req)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(GattError::Timeout);
            }
            match self.ch.recv(left)? {
                Recv::Timeout => return Err(GattError::Timeout),
                Recv::Closed => return Err(GattError::Closed),
                Recv::Data(d) => {
                    let Ok(p) = Pdu::parse(&d) else { continue };
                    if self.keep_notification(&p)? {
                        continue;
                    }
                    match p {
                        Pdu::Error { op, handle, code } if op == req.opcode() => return Err(GattError::Att { op, handle, code }),
                        p if p.opcode() == answer => return Ok(p),
                        _ => {}
                    }
                }
            }
        }
    }

    /// Asks for a larger message size and takes the smaller of the two numbers.
    pub fn exchange_mtu(&mut self) -> Result<u16, GattError> {
        match self.request(&Pdu::MtuReq(WANT_MTU), OP_MTU_RSP) {
            Ok(Pdu::MtuRsp(m)) => {
                self.mtu = m.clamp(DEFAULT_MTU, WANT_MTU);
                Ok(self.mtu)
            }
            // a server without the exchange stays at 23
            Err(GattError::Att { .. }) => Ok(self.mtu),
            Err(e) => Err(e),
            Ok(_) => Ok(self.mtu),
        }
    }

    /// Primary services as (first handle, last handle, uuid).
    pub fn primary_services(&mut self) -> Result<Vec<(u16, u16, Uuid)>, GattError> {
        let mut out = Vec::new();
        let mut start = 1u16;
        for _ in 0..MAX_ROUNDS {
            match self.request(&Pdu::ReadByGroupReq { start, end: 0xffff, uuid: Uuid::Short(UUID_PRIMARY) }, OP_READ_BY_GROUP_RSP) {
                Ok(Pdu::ReadByGroupRsp(list)) => {
                    let mut last = start;
                    for (s, e, v) in list {
                        if s < start || e < s {
                            return Err(GattError::Protocol("service handles go backwards".into()));
                        }
                        last = e;
                        if let Some(u) = Uuid::from_bytes(&v) {
                            out.push((s, e, u));
                        }
                    }
                    if last == 0xffff {
                        return Ok(out);
                    }
                    start = last + 1;
                }
                Err(GattError::Att { code: ERR_ATTRIBUTE_NOT_FOUND, .. }) => return Ok(out),
                Err(e) => return Err(e),
                Ok(_) => return Err(GattError::Protocol("wrong answer to the service list".into())),
            }
        }
        Err(GattError::Protocol("the service list does not end".into()))
    }

    /// Characteristics of one service as (declaration handle, properties, value handle, uuid).
    pub fn characteristics(&mut self, first: u16, last: u16) -> Result<Vec<(u16, u8, u16, Uuid)>, GattError> {
        let mut out = Vec::new();
        let mut start = first;
        for _ in 0..MAX_ROUNDS {
            if start > last {
                return Ok(out);
            }
            match self.request(&Pdu::ReadByTypeReq { start, end: last, uuid: Uuid::Short(UUID_CHARACTERISTIC) }, OP_READ_BY_TYPE_RSP) {
                Ok(Pdu::ReadByTypeRsp(list)) => {
                    let mut next = start;
                    for (h, v) in list {
                        if h < start {
                            return Err(GattError::Protocol("characteristic handles go backwards".into()));
                        }
                        next = h;
                        if v.len() >= 5 {
                            if let Some(u) = Uuid::from_bytes(&v[3..]) {
                                out.push((h, v[0], u16::from_le_bytes([v[1], v[2]]), u));
                            }
                        }
                    }
                    if next == 0xffff {
                        return Ok(out);
                    }
                    start = next + 1;
                }
                Err(GattError::Att { code: ERR_ATTRIBUTE_NOT_FOUND, .. }) => return Ok(out),
                Err(e) => return Err(e),
                Ok(_) => return Err(GattError::Protocol("wrong answer to the characteristic list".into())),
            }
        }
        Err(GattError::Protocol("the characteristic list does not end".into()))
    }

    /// Descriptors (handle, uuid) in a range.
    pub fn descriptors(&mut self, first: u16, last: u16) -> Result<Vec<(u16, Uuid)>, GattError> {
        let mut out = Vec::new();
        let mut start = first;
        for _ in 0..MAX_ROUNDS {
            if start > last {
                return Ok(out);
            }
            match self.request(&Pdu::FindInfoReq { start, end: last }, OP_FIND_INFO_RSP) {
                Ok(Pdu::FindInfoRsp(list)) => {
                    let mut next = start;
                    for (h, u) in list {
                        if h < start {
                            return Err(GattError::Protocol("descriptor handles go backwards".into()));
                        }
                        next = h;
                        out.push((h, u));
                    }
                    if next == 0xffff {
                        return Ok(out);
                    }
                    start = next + 1;
                }
                Err(GattError::Att { code: ERR_ATTRIBUTE_NOT_FOUND, .. }) => return Ok(out),
                Err(e) => return Err(e),
                Ok(_) => return Err(GattError::Protocol("wrong answer to the descriptor list".into())),
            }
        }
        Err(GattError::Protocol("the descriptor list does not end".into()))
    }

    pub fn read(&mut self, handle: u16) -> Result<Vec<u8>, GattError> {
        match self.request(&Pdu::ReadReq(handle), OP_READ_RSP)? {
            Pdu::ReadRsp(d) => Ok(d),
            _ => Err(GattError::Protocol("wrong answer to a read".into())),
        }
    }

    /// Reads an attribute longer than one message with read blob requests (at most [`MAX_ATTR`] bytes).
    pub fn read_long(&mut self, handle: u16) -> Result<Vec<u8>, GattError> {
        let mut data = self.read(handle)?;
        let full = (self.mtu as usize).saturating_sub(1);
        let mut rounds = 0;
        while data.len() >= full && data.len() < MAX_ATTR && rounds < MAX_ROUNDS {
            rounds += 1;
            match self.request(&Pdu::ReadBlobReq { handle, offset: data.len() as u16 }, OP_READ_BLOB_RSP) {
                Ok(Pdu::ReadBlobRsp(d)) => {
                    if d.is_empty() {
                        break;
                    }
                    data.extend_from_slice(&d);
                    if d.len() < full {
                        break;
                    }
                }
                // an attribute of exactly one message length ends with this error
                Err(GattError::Att { code: 0x07, .. }) | Err(GattError::Att { code: 0x0b, .. }) => break,
                Err(e) => return Err(e),
                Ok(_) => return Err(GattError::Protocol("wrong answer to a long read".into())),
            }
        }
        data.truncate(MAX_ATTR);
        Ok(data)
    }

    pub fn write(&mut self, handle: u16, value: &[u8]) -> Result<(), GattError> {
        self.request(&Pdu::WriteReq { handle, value: value.to_vec() }, OP_WRITE_RSP).map(|_| ())
    }

    /// A write that needs no answer (LED reports, protocol mode).
    pub fn write_command(&mut self, handle: u16, value: &[u8]) -> Result<(), GattError> {
        let max = (self.mtu as usize).saturating_sub(3);
        self.send(&Pdu::WriteCmd { handle, value: value[..value.len().min(max)].to_vec() })
    }

    /// The oldest notification or indication (kept while a request waited, or read now within `wait`).
    pub fn next_notification(&mut self, wait: Duration) -> Result<Option<(u16, Vec<u8>)>, GattError> {
        if let Some(n) = self.notes.pop_front() {
            return Ok(Some(n));
        }
        match self.ch.recv(wait)? {
            Recv::Timeout => Ok(None),
            Recv::Closed => Err(GattError::Closed),
            Recv::Data(d) => {
                if let Ok(p) = Pdu::parse(&d) {
                    self.keep_notification(&p)?;
                }
                Ok(self.notes.pop_front())
            }
        }
    }

    /// Takes the notifications that were queued while a request waited.
    pub fn take_notifications(&mut self) -> Vec<(u16, Vec<u8>)> {
        self.notes.drain(..).collect()
    }

    /// Turns notifications on (or off) with the client characteristic configuration descriptor. A server that wants encryption first
    /// answers with an error; the kernel raises the link security meanwhile, so one more try after a moment usually works.
    pub fn enable_notifications(&mut self, ccc: u16, on: bool) -> Result<(), GattError> {
        let v = [if on { 1 } else { 0 }, 0];
        match self.write(ccc, &v) {
            Err(GattError::Att { code: ERR_INSUFFICIENT_AUTHENTICATION | ERR_INSUFFICIENT_ENCRYPTION, .. }) => {
                std::thread::sleep(Duration::from_millis(if cfg!(test) { 1 } else { 1500 }));
                self.write(ccc, &v)
            }
            r => r,
        }
    }
}

// ------------------------------------------------------------------------------------------------ HID service discovery

/// What an input, output or feature report is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Input,
    Output,
    Feature,
}

/// One report characteristic of the HID service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportChar {
    pub value_handle: u16,
    /// Handle of the client characteristic configuration, when the report can notify.
    pub ccc_handle: Option<u16>,
    pub report_id: u8,
    pub kind: ReportKind,
}

/// A low-energy HID device as discovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HogpDevice {
    pub report_map: Vec<u8>,
    pub reports: Vec<ReportChar>,
    pub protocol_mode_handle: Option<u16>,
    pub hid_info: Option<[u8; 4]>,
    pub battery_level_handle: Option<u16>,
    pub vendor: u32,
    pub product: u32,
}

/// Finds the HID service and everything a session needs. A device without the HID service, a report map or any input report is an error.
pub fn discover_hid<C: Channel>(g: &mut Gatt<C>) -> Result<HogpDevice, HidError> {
    let services = g.primary_services()?;
    let (first, last) = services.iter().find(|(_, _, u)| *u == Uuid::Short(SVC_HID)).map(|(s, e, _)| (*s, *e)).ok_or(HidError::NoHidService)?;
    let chars = g.characteristics(first, last)?;
    let mut dev = HogpDevice { report_map: Vec::new(), reports: Vec::new(), protocol_mode_handle: None, hid_info: None, battery_level_handle: None, vendor: 0, product: 0 };
    let mut map_handle = None;
    for (i, (_, props, vh, uuid)) in chars.iter().enumerate() {
        let Some(short) = uuid.short() else { continue };
        let end = chars.get(i + 1).map_or(last, |n| n.0.saturating_sub(1)).max(*vh);
        match short {
            CH_REPORT_MAP => map_handle = Some(*vh),
            CH_PROTOCOL_MODE => dev.protocol_mode_handle = Some(*vh),
            CH_HID_INFO => {
                if let Ok(d) = g.read(*vh) {
                    if d.len() >= 4 {
                        dev.hid_info = Some([d[0], d[1], d[2], d[3]]);
                    }
                }
            }
            CH_REPORT => {
                let descs = if *vh < end { g.descriptors(vh + 1, end)? } else { Vec::new() };
                let ccc = descs.iter().find(|(_, u)| *u == Uuid::Short(UUID_CCC)).map(|(h, _)| *h);
                let Some(rr) = descs.iter().find(|(_, u)| *u == Uuid::Short(UUID_REPORT_REF)).map(|(h, _)| *h) else { continue };
                let r = g.read(rr)?;
                if r.len() < 2 {
                    continue;
                }
                let kind = match r[1] {
                    1 => ReportKind::Input,
                    2 => ReportKind::Output,
                    3 => ReportKind::Feature,
                    _ => continue,
                };
                let _ = props;
                dev.reports.push(ReportChar { value_handle: *vh, ccc_handle: ccc, report_id: r[0], kind });
            }
            _ => {}
        }
    }
    let mh = map_handle.ok_or_else(|| HidError::Sdp("the device has no report map".into()))?;
    dev.report_map = g.read_long(mh)?;
    if dev.report_map.is_empty() || dev.report_map.len() > MAX_DESCRIPTOR {
        return Err(HidError::Sdp("the report descriptor is not usable".into()));
    }
    if !dev.reports.iter().any(|r| r.kind == ReportKind::Input && r.ccc_handle.is_some()) {
        return Err(HidError::Sdp("the device offers no input report that can notify".into()));
    }
    // the battery level and the vendor and product number are nice to have, never needed
    if let Some((s, e, _)) = services.iter().find(|(_, _, u)| *u == Uuid::Short(SVC_BATTERY)) {
        if let Ok(c) = g.characteristics(*s, *e) {
            dev.battery_level_handle = c.iter().find(|x| x.3 == Uuid::Short(CH_BATTERY_LEVEL)).map(|x| x.2);
        }
    }
    if let Ok(Pdu::ReadByTypeRsp(list)) = g.request(&Pdu::ReadByTypeReq { start: 1, end: 0xffff, uuid: Uuid::Short(CH_PNP_ID) }, OP_READ_BY_TYPE_RSP) {
        if let Some((_, v)) = list.first().filter(|(_, v)| v.len() >= 7) {
            dev.vendor = u16::from_le_bytes([v[1], v[2]]) as u32;
            dev.product = u16::from_le_bytes([v[3], v[4]]) as u32;
        }
    }
    Ok(dev)
}

// ------------------------------------------------------------------------------------------------ the session

/// Whether the report map uses report ids: a Report ID item (0x85) outside the data bytes of other items.
pub fn map_uses_report_ids(map: &[u8]) -> bool {
    let mut i = 0;
    while i < map.len() {
        let b = map[i];
        if b == 0xfe {
            // long item: size in the next byte
            i += 3 + map.get(i + 1).copied().unwrap_or(0) as usize;
            continue;
        }
        if b & 0xfc == 0x84 {
            return true;
        }
        i += 1 + match b & 3 {
            3 => 4,
            n => n as usize,
        };
    }
    false
}

/// What a notification did.
#[derive(Debug, PartialEq, Eq)]
pub enum Note {
    /// Reached the kernel as an input report.
    Delivered,
    /// Not an input report we know (battery level, an output report echo ...).
    Ignored,
}

/// One connected low-energy HID device: the kernel input device and the translation in both directions.
pub struct HogpSession<U: Uhid> {
    uhid: U,
    dev: HogpDevice,
    ids: bool,
    created: bool,
    pub reports: u64,
}

impl<U: Uhid> HogpSession<U> {
    /// Switches the device to report protocol, turns notifications on for every input report, then creates the input device.
    pub fn open<C: Channel>(mut uhid: U, g: &mut Gatt<C>, dev: HogpDevice, name: &str, uniq: &str) -> Result<HogpSession<U>, HidError> {
        if let Some(h) = dev.protocol_mode_handle {
            // 1 = report protocol; best effort, most devices start in it
            let _ = g.write_command(h, &[1]);
        }
        let mut on = 0;
        for r in dev.reports.iter().filter(|r| r.kind == ReportKind::Input) {
            if let Some(c) = r.ccc_handle {
                g.enable_notifications(c, true)?;
                on += 1;
            }
        }
        if on == 0 {
            return Err(HidError::Sdp("the device offers no input report that can notify".into()));
        }
        uhid.write(&uhid_create2(name, uniq, dev.vendor, dev.product, &dev.report_map)).map_err(HidError::Uhid)?;
        let ids = map_uses_report_ids(&dev.report_map);
        Ok(HogpSession { uhid, dev, ids, created: true, reports: 0 })
    }

    /// A notification or indication from the device.
    pub fn on_notification(&mut self, handle: u16, value: &[u8]) -> Result<Note, HidError> {
        let Some(r) = self.dev.reports.iter().find(|r| r.value_handle == handle && r.kind == ReportKind::Input) else { return Ok(Note::Ignored) };
        if value.is_empty() {
            return Ok(Note::Ignored);
        }
        let mut report = Vec::with_capacity(value.len() + 1);
        // the kernel wants the report id first when the map uses ids; the attribute value never carries it
        if self.ids {
            report.push(r.report_id);
        }
        report.extend_from_slice(value);
        self.uhid.write(&uhid_input2(&report)).map_err(HidError::Uhid)?;
        self.reports += 1;
        Ok(Note::Delivered)
    }

    /// A request from the kernel's input device. LED reports become write commands; report requests get an error at once.
    pub fn on_uhid<C: Channel>(&mut self, ev: UhidEvent, g: &mut Gatt<C>) -> Result<(), HidError> {
        match ev {
            UhidEvent::Output(r) if !r.is_empty() => {
                let (id, body) = if self.ids { (r[0], &r[1..]) } else { (0, &r[..]) };
                if let Some(c) = self.dev.reports.iter().find(|c| c.kind == ReportKind::Output && c.report_id == id) {
                    g.write_command(c.value_handle, body)?;
                }
            }
            UhidEvent::GetReport { id } => self.uhid.write(&uhid_get_report_reply(id, libc::EIO as u16)).map_err(HidError::Uhid)?,
            UhidEvent::SetReport { id } => self.uhid.write(&uhid_set_report_reply(id, libc::EIO as u16)).map_err(HidError::Uhid)?,
            _ => {}
        }
        Ok(())
    }

    pub fn close(&mut self) {
        if self.created {
            let _ = self.uhid.write(&uhid_destroy());
            self.created = false;
        }
    }
}

impl<U: Uhid> Drop for HogpSession<U> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Runs a low-energy session until it ends. The source's interrupt events are ATT messages from the device; its uhid events are the
/// kernel's. Ends like [`crate::bthid::run_session`].
pub fn run_hogp<U: Uhid, C: Channel, S: Source>(sess: &mut HogpSession<U>, g: &mut Gatt<C>, src: &mut S, stop: &std::sync::atomic::AtomicBool) -> crate::bthid::End {
    use crate::bthid::End;
    use std::sync::atomic::Ordering;
    // notifications that arrived while the session was being set up
    for (h, v) in g.take_notifications() {
        if let Err(e) = sess.on_notification(h, &v) {
            return End::Failed(e.to_string());
        }
    }
    loop {
        if stop.load(Ordering::SeqCst) {
            return End::Stopped;
        }
        let ev = match src.wait(Duration::from_millis(500)) {
            Ok(e) => e,
            Err(e) => return End::Failed(e.to_string()),
        };
        let r: Result<(), HidError> = match ev {
            Ev::Idle | Ev::Control(_) => Ok(()),
            Ev::ChannelClosed => return End::LinkLost,
            Ev::Interrupt(m) => match Pdu::parse(&m) {
                Ok(Pdu::Notify { handle, value }) => sess.on_notification(handle, &value).map(|_| ()),
                Ok(Pdu::Indicate { handle, value }) => g.send(&Pdu::Confirm).map_err(HidError::from).and_then(|_| sess.on_notification(handle, &value).map(|_| ())),
                // answers to our own writes and anything hostile or unknown are dropped
                _ => Ok(()),
            },
            Ev::Uhid(b) => match parse_uhid(&b) {
                Some(e) => sess.on_uhid(e, g),
                None => Ok(()),
            },
        };
        if let Err(e) = r {
            return End::Failed(e.to_string());
        }
    }
}

#[cfg(test)]
#[path = "att_tests.rs"]
mod tests;

//! A small client for the kernel's Bluetooth management interface (the "mgmt" control channel of an HCI socket), so
//! IYAKKI can switch a controller on, scan, pair, disconnect and forget devices without BlueZ.
//!
//! Scope, honestly:
//! * done here: controller list and info, power, discoverable/connectable/bondable, Simple Secure Pairing and LE,
//!   discovery with names and device classes, pairing (confirm / passkey / legacy PIN prompts handed to the caller),
//!   connected devices, disconnect, unpair.
//! * not done here: using the paired device. A keyboard, mouse or game pad needs a HID session on top of the bond
//!   (classic: SDP + an L2CAP HIDP connection and the kernel `hidp` module; low energy: a GATT client for HID over GATT
//!   feeding `/dev/uhid`). Audio needs A2DP/AVRCP (AVDTP over L2CAP, SBC codec, an ALSA or PipeWire sink). See
//!   docs/MAC-HARDWARE.md, "Bluetooth".
//!
//! The packet format is the kernel's `Documentation/bluetooth`-style mgmt API (`net/bluetooth/mgmt.c`, `mgmt.h`): a
//! 6-byte little-endian header (opcode or event, controller index, parameter length) and the parameters. The transport
//! is a trait so tests use a scripted fake instead of a socket.
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

pub const INDEX_NONE: u16 = 0xffff;

// Commands
pub const OP_READ_INDEX_LIST: u16 = 0x0003;
pub const OP_READ_INFO: u16 = 0x0004;
pub const OP_SET_POWERED: u16 = 0x0005;
pub const OP_READ_CONFIG_INFO: u16 = 0x0037;
pub const OP_READ_EXT_INDEX_LIST: u16 = 0x003c;
pub const OP_SET_DISCOVERABLE: u16 = 0x0006;
pub const OP_SET_CONNECTABLE: u16 = 0x0007;
pub const OP_SET_BONDABLE: u16 = 0x0009;
pub const OP_SET_SSP: u16 = 0x000b;
pub const OP_SET_LE: u16 = 0x000d;
pub const OP_DISCONNECT: u16 = 0x0014;
pub const OP_GET_CONNECTIONS: u16 = 0x0015;
pub const OP_PIN_CODE_REPLY: u16 = 0x0016;
pub const OP_PIN_CODE_NEG_REPLY: u16 = 0x0017;
pub const OP_SET_IO_CAPABILITY: u16 = 0x0018;
pub const OP_PAIR_DEVICE: u16 = 0x0019;
pub const OP_CANCEL_PAIR_DEVICE: u16 = 0x001a;
pub const OP_UNPAIR_DEVICE: u16 = 0x001b;
pub const OP_USER_CONFIRM_REPLY: u16 = 0x001c;
pub const OP_USER_CONFIRM_NEG_REPLY: u16 = 0x001d;
pub const OP_USER_PASSKEY_REPLY: u16 = 0x001e;
pub const OP_USER_PASSKEY_NEG_REPLY: u16 = 0x001f;
pub const OP_ADD_DEVICE: u16 = 0x0033;
pub const OP_SET_DEV_CLASS: u16 = 0x000e;
pub const OP_ADD_UUID: u16 = 0x0010;
pub const OP_START_DISCOVERY: u16 = 0x0023;
pub const OP_STOP_DISCOVERY: u16 = 0x0024;
pub const OP_CONFIRM_NAME: u16 = 0x0025;
pub const OP_SET_BREDR: u16 = 0x002a;

// Events
pub const EV_CMD_COMPLETE: u16 = 0x0001;
pub const EV_CMD_STATUS: u16 = 0x0002;
pub const EV_INDEX_ADDED: u16 = 0x0004;
pub const EV_INDEX_REMOVED: u16 = 0x0005;
pub const EV_NEW_SETTINGS: u16 = 0x0006;
pub const EV_DEVICE_CONNECTED: u16 = 0x000b;
pub const EV_DEVICE_DISCONNECTED: u16 = 0x000c;
pub const EV_CONNECT_FAILED: u16 = 0x000d;
pub const EV_PIN_CODE_REQUEST: u16 = 0x000e;
pub const EV_USER_CONFIRM_REQUEST: u16 = 0x000f;
pub const EV_USER_PASSKEY_REQUEST: u16 = 0x0010;
pub const EV_AUTH_FAILED: u16 = 0x0011;
pub const EV_DEVICE_FOUND: u16 = 0x0012;
pub const EV_DISCOVERING: u16 = 0x0013;
pub const EV_PASSKEY_NOTIFY: u16 = 0x0017;

// Settings bits (Read Info, New Settings)
pub const SET_POWERED: u32 = 1 << 0;
pub const SET_CONNECTABLE: u32 = 1 << 1;
pub const SET_DISCOVERABLE: u32 = 1 << 3;
pub const SET_BONDABLE: u32 = 1 << 4;
pub const SET_SSP: u32 = 1 << 6;
pub const SET_BREDR: u32 = 1 << 7;
pub const SET_LE: u32 = 1 << 9;

/// Device Found flag: the kernel asks whether the name is known (the answer "no" makes it look the name up).
pub const DEV_FOUND_CONFIRM_NAME: u32 = 0x02;

/// Address types: BR/EDR (classic), LE public, LE random.
/// The 17 bytes of an Add UUID command: the 128-bit UUID (little endian) of a 16-bit Bluetooth UUID, then the service class hint.
pub fn uuid128_le(uuid16: u16, hint: u8) -> Vec<u8> {
    // 0000xxxx-0000-1000-8000-00805F9B34FB, least significant byte first
    let mut v = vec![0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00, uuid16 as u8, (uuid16 >> 8) as u8, 0x00, 0x00];
    v.push(hint);
    v
}

/// Add Device action: connect in the background whenever the device advertises.
pub const ADD_DEVICE_AUTO_CONNECT: u8 = 2;
pub const ADDR_BREDR: u8 = 0;
pub const ADDR_LE_PUBLIC: u8 = 1;
pub const ADDR_LE_RANDOM: u8 = 2;

/// IO capabilities for pairing (what this machine can show or ask).
pub const IO_DISPLAY_ONLY: u8 = 0;
pub const IO_DISPLAY_YES_NO: u8 = 1;
pub const IO_KEYBOARD_ONLY: u8 = 2;
pub const IO_NO_INPUT_NO_OUTPUT: u8 = 3;
pub const IO_KEYBOARD_DISPLAY: u8 = 4;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The kernel refused the command: status code and its plain-language meaning.
    Status(u8),
    Timeout,
    /// A reply that cannot be understood (too short, wrong shape).
    Protocol(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Status(s) => write!(f, "{}", status_text(*s)),
            Error::Timeout => write!(f, "the Bluetooth controller did not answer in time"),
            Error::Protocol(m) => write!(f, "unexpected Bluetooth reply: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

/// Plain-language text for a mgmt status code.
pub fn status_text(s: u8) -> &'static str {
    match s {
        0 => "success",
        1 => "the kernel does not know this command",
        2 => "not connected",
        3 => "failed",
        4 => "could not connect",
        5 => "authentication failed (wrong passkey, or the device refused)",
        6 => "not paired",
        7 => "out of resources",
        8 => "timed out",
        9 => "already connected",
        10 => "the controller is busy",
        11 => "rejected",
        12 => "not supported by this controller",
        13 => "invalid parameters",
        14 => "disconnected",
        15 => "Bluetooth is off (power the controller on first)",
        16 => "cancelled",
        17 => "no such controller",
        18 => "Bluetooth is switched off by rfkill (key or software switch)",
        19 => "already paired",
        20 => "permission denied (needs root)",
        _ => "unknown error",
    }
}

/// One mgmt packet from the kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub code: u16,
    pub index: u16,
    pub params: Vec<u8>,
}

pub fn encode(opcode: u16, index: u16, params: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(6 + params.len());
    v.extend_from_slice(&opcode.to_le_bytes());
    v.extend_from_slice(&index.to_le_bytes());
    v.extend_from_slice(&(params.len() as u16).to_le_bytes());
    v.extend_from_slice(params);
    v
}

pub fn decode(buf: &[u8]) -> Result<Packet, Error> {
    if buf.len() < 6 {
        return Err(Error::Protocol("packet shorter than its header"));
    }
    let len = u16::from_le_bytes([buf[4], buf[5]]) as usize;
    if buf.len() < 6 + len {
        return Err(Error::Protocol("packet shorter than its length field"));
    }
    Ok(Packet { code: u16::from_le_bytes([buf[0], buf[1]]), index: u16::from_le_bytes([buf[2], buf[3]]), params: buf[6..6 + len].to_vec() })
}

/// Where packets go. The real one is [`ControlSocket`]; tests script a fake.
pub trait Transport {
    fn send(&mut self, packet: &[u8]) -> io::Result<()>;
    /// The next packet, or `None` when nothing arrived within `wait`.
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>>;
}

/// The kernel's Bluetooth control channel (`AF_BLUETOOTH`, `BTPROTO_HCI`, `HCI_CHANNEL_CONTROL`). Needs root.
pub struct ControlSocket {
    fd: libc::c_int,
}

impl ControlSocket {
    pub fn open() -> io::Result<ControlSocket> {
        const AF_BLUETOOTH: libc::c_int = 31;
        const BTPROTO_HCI: libc::c_int = 1;
        #[repr(C)]
        struct SockaddrHci {
            family: libc::sa_family_t,
            dev: u16,
            channel: u16,
        }
        unsafe {
            let fd = libc::socket(AF_BLUETOOTH, libc::SOCK_RAW | libc::SOCK_CLOEXEC, BTPROTO_HCI);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let sa = SockaddrHci { family: AF_BLUETOOTH as libc::sa_family_t, dev: 0xffff, channel: 3 };
            if libc::bind(fd, &sa as *const SockaddrHci as *const libc::sockaddr, std::mem::size_of::<SockaddrHci>() as libc::socklen_t) < 0 {
                let e = io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            Ok(ControlSocket { fd })
        }
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

impl Transport for ControlSocket {
    fn send(&mut self, packet: &[u8]) -> io::Result<()> {
        let n = unsafe { libc::write(self.fd, packet.as_ptr() as *const libc::c_void, packet.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>> {
        let mut p = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut p, 1, wait.as_millis().min(i32::MAX as u128) as i32) };
        if r < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(None) } else { Err(e) };
        }
        if r == 0 {
            return Ok(None);
        }
        let mut buf = vec![0u8; 4096];
        let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        buf.truncate(n as usize);
        Ok(Some(buf))
    }
}

/// A Bluetooth address as the kernel stores it (little endian) printed the usual way.
pub fn addr_string(a: &[u8]) -> String {
    a.iter().rev().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// "AA:BB:CC:DD:EE:FF" to the kernel's byte order.
pub fn parse_addr(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != 2 {
            return None;
        }
        out[5 - i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerInfo {
    pub index: u16,
    pub address: String,
    pub name: String,
    pub supported: u32,
    pub current: u32,
    /// HCI version of the controller (6 = Bluetooth 4.0, 9 = 5.0 ...), as the kernel read it.
    pub version: u8,
    /// Company id from the controller (15 = Broadcom).
    pub manufacturer: u16,
    pub short_name: String,
}

/// The reply of Read Controller Information (`mgmt_rp_read_info`, 280 bytes): address 6, version 1, manufacturer 2, supported settings 4,
/// current settings 4, class 3, name 249, short name 11. Shorter replies are refused; longer ones are read as far as the layout goes.
pub fn parse_info(index: u16, r: &[u8]) -> Result<ControllerInfo, Error> {
    if r.len() < 280 {
        return Err(Error::Protocol("controller info"));
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).split('\0').next().unwrap_or("").to_string();
    Ok(ControllerInfo {
        index,
        address: addr_string(&r[0..6]),
        name: text(&r[20..20 + 249]),
        supported: u32::from_le_bytes([r[9], r[10], r[11], r[12]]),
        current: u32::from_le_bytes([r[13], r[14], r[15], r[16]]),
        version: r[6],
        manufacturer: u16::from_le_bytes([r[7], r[8]]),
        short_name: text(&r[269..280]),
    })
}

impl ControllerInfo {
    pub fn powered(&self) -> bool {
        self.current & SET_POWERED != 0
    }
}

/// What kind of device an advertisement or inquiry result looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyboard,
    Mouse,
    Gamepad,
    Headphones,
    Speaker,
    Phone,
    Computer,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub address: String,
    pub addr_type: u8,
    pub rssi: i8,
    pub name: String,
    pub class: Option<u32>,
    pub appearance: Option<u16>,
    /// 16-bit service UUIDs seen in the data (0x1812 HID, 0x110b A2DP sink, 0x111e Handsfree ...).
    pub services: Vec<u16>,
    pub kind: Kind,
}

/// Reads the advertising / extended inquiry data: name, class of device, appearance, 16-bit service UUIDs.
pub fn parse_eir(eir: &[u8]) -> (String, Option<u32>, Option<u16>, Vec<u16>) {
    let (mut name, mut short, mut class, mut appearance, mut services) = (String::new(), String::new(), None, None, Vec::new());
    let mut i = 0;
    while i < eir.len() {
        let l = eir[i] as usize;
        if l == 0 || i + 1 + l > eir.len() {
            break;
        }
        let t = eir[i + 1];
        let d = &eir[i + 2..i + 1 + l];
        match t {
            0x09 => name = String::from_utf8_lossy(d).trim_end_matches('\0').to_string(),
            0x08 => short = String::from_utf8_lossy(d).trim_end_matches('\0').to_string(),
            0x0d if d.len() >= 3 => class = Some(d[0] as u32 | (d[1] as u32) << 8 | (d[2] as u32) << 16),
            0x19 if d.len() >= 2 => appearance = Some(u16::from_le_bytes([d[0], d[1]])),
            0x02 | 0x03 => services.extend(d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]))),
            _ => {}
        }
        i += 1 + l;
    }
    (if name.is_empty() { short } else { name }, class, appearance, services)
}

pub fn classify(class: Option<u32>, appearance: Option<u16>, services: &[u16]) -> Kind {
    if let Some(c) = class {
        let (major, minor) = ((c >> 8) & 0x1f, (c >> 2) & 0x3f);
        match major {
            1 => return Kind::Computer,
            2 => return Kind::Phone,
            4 => return if matches!(minor, 1 | 2 | 6 | 7) { Kind::Headphones } else { Kind::Speaker },
            5 => {
                // peripheral: bits 6-7 of the minor-class byte say keyboard (0x10) / pointing (0x20); minor 1-2 game pad
                let bits = (c >> 6) & 0x3;
                return match bits {
                    1 => Kind::Keyboard,
                    2 => Kind::Mouse,
                    3 => Kind::Keyboard,
                    _ if matches!(minor & 0xf, 1 | 2) => Kind::Gamepad,
                    _ => Kind::Other,
                };
            }
            _ => {}
        }
    }
    if let Some(a) = appearance {
        match a >> 6 {
            0x0f => {
                return match a & 0x3f {
                    1 => Kind::Keyboard,
                    2 => Kind::Mouse,
                    3 | 4 => Kind::Gamepad,
                    _ => Kind::Other,
                }
            }
            0x01 => return Kind::Phone,
            0x02 => return Kind::Computer,
            _ => {}
        }
    }
    if services.contains(&0x110b) || services.contains(&0x110a) {
        return Kind::Speaker;
    }
    if services.contains(&0x1812) {
        return Kind::Keyboard;
    }
    Kind::Other
}

pub fn parse_found(p: &[u8]) -> Option<Found> {
    if p.len() < 14 {
        return None;
    }
    let eir_len = u16::from_le_bytes([p[12], p[13]]) as usize;
    let eir = p.get(14..14 + eir_len)?;
    let (name, class, appearance, services) = parse_eir(eir);
    let kind = classify(class, appearance, &services);
    Some(Found { address: addr_string(&p[0..6]), addr_type: p[6], rssi: p[7] as i8, name, class, appearance, services, kind })
}

/// What the person has to answer while pairing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Compare this 6-digit number with the device's, then confirm (or just confirm when `hint` says "just works").
    Confirm { value: u32, hint: bool },
    /// The device asks for the passkey shown on this machine's side to be typed here (rare), or typed on the device.
    PasskeyRequest,
    /// Type this passkey on the device (a keyboard) and press Enter there.
    PasskeyShow(u32),
    /// Legacy pairing: a PIN, usually "0000" for mice and headsets, or one the device shows.
    PinRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Passkey(u32),
    Pin(String),
}

/// The management client.
pub struct Mgmt<T: Transport> {
    t: T,
    /// Events that arrived while waiting for a command's reply.
    pub events: VecDeque<Packet>,
    pub timeout: Duration,
}

impl<T: Transport> Mgmt<T> {
    pub fn new(t: T) -> Mgmt<T> {
        Mgmt { t, events: VecDeque::new(), timeout: Duration::from_secs(5) }
    }

    /// The transport (tests look at what was sent).
    pub fn transport(&self) -> &T {
        &self.t
    }

    /// Sends a command and waits for its Command Complete; other events are kept in `events`.
    pub fn command(&mut self, opcode: u16, index: u16, params: &[u8]) -> Result<Vec<u8>, Error> {
        self.t.send(&encode(opcode, index, params))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            let Some(raw) = self.t.recv(left)? else { return Err(Error::Timeout) };
            let pkt = decode(&raw)?;
            let ours = |p: &Packet| p.params.len() >= 3 && u16::from_le_bytes([p.params[0], p.params[1]]) == opcode && p.index == index;
            match pkt.code {
                EV_CMD_COMPLETE if ours(&pkt) => {
                    let status = pkt.params[2];
                    return if status == 0 { Ok(pkt.params[3..].to_vec()) } else { Err(Error::Status(status)) };
                }
                EV_CMD_STATUS if ours(&pkt) => {
                    // A bare status is only the answer for a refusal; success arrives later as Command Complete.
                    if pkt.params[2] != 0 {
                        return Err(Error::Status(pkt.params[2]));
                    }
                }
                _ => self.events.push_back(pkt),
            }
        }
    }

    /// Waits up to `wait` for the next event (queued ones first).
    pub fn next_event(&mut self, wait: Duration) -> Result<Option<Packet>, Error> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        match self.t.recv(wait)? {
            Some(raw) => Ok(Some(decode(&raw)?)),
            None => Ok(None),
        }
    }

    pub fn index_list(&mut self) -> Result<Vec<u16>, Error> {
        let r = self.command(OP_READ_INDEX_LIST, INDEX_NONE, &[])?;
        if r.len() < 2 {
            return Err(Error::Protocol("index list"));
        }
        let n = u16::from_le_bytes([r[0], r[1]]) as usize;
        Ok(r[2..].chunks_exact(2).take(n).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
    }

    pub fn info(&mut self, index: u16) -> Result<ControllerInfo, Error> {
        let r = self.command(OP_READ_INFO, index, &[])?;
        parse_info(index, &r)
    }

    fn set(&mut self, op: u16, index: u16, params: &[u8]) -> Result<u32, Error> {
        let r = self.command(op, index, params)?;
        if r.len() < 4 {
            return Err(Error::Protocol("settings reply"));
        }
        Ok(u32::from_le_bytes([r[0], r[1], r[2], r[3]]))
    }

    /// Switches the controller on or off. Returns the new settings bits.
    pub fn set_powered(&mut self, index: u16, on: bool) -> Result<u32, Error> {
        self.set(OP_SET_POWERED, index, &[on as u8])
    }

    pub fn set_connectable(&mut self, index: u16, on: bool) -> Result<u32, Error> {
        self.set(OP_SET_CONNECTABLE, index, &[on as u8])
    }

    pub fn set_bondable(&mut self, index: u16, on: bool) -> Result<u32, Error> {
        self.set(OP_SET_BONDABLE, index, &[on as u8])
    }

    /// Visible to others for `secs` seconds (0 = until switched off). Needs the controller connectable.
    pub fn set_discoverable(&mut self, index: u16, on: bool, secs: u16) -> Result<u32, Error> {
        let mut p = vec![on as u8];
        p.extend_from_slice(&secs.to_le_bytes());
        self.set(OP_SET_DISCOVERABLE, index, &p)
    }

    /// The settings a controller needs to scan and pair: Simple Secure Pairing, LE, classic, bondable. Errors on
    /// unsupported bits (an LE-only controller) are ignored: what the controller can do, it does.
    pub fn prepare(&mut self, index: u16) -> Result<u32, Error> {
        let mut cur = 0;
        for (op, supported) in [(OP_SET_BREDR, SET_BREDR), (OP_SET_SSP, SET_SSP), (OP_SET_LE, SET_LE)] {
            let info = self.info(index)?;
            if info.supported & supported == 0 {
                continue;
            }
            cur = self.set(op, index, &[1])?;
        }
        cur = self.set_bondable(index, true).unwrap_or(cur);
        Ok(cur)
    }

    /// Scans for `dur` (classic and low energy) and returns the devices seen, strongest signal first.
    pub fn scan(&mut self, index: u16, dur: Duration) -> Result<Vec<Found>, Error> {
        self.scan_types(index, dur, 7)
    }

    /// The discovery address types (1 classic, 2 LE public, 4 LE random) this controller can search now: classic and/or low energy,
    /// turning on the radio types it supports and has off (a setting of the controller, not a pairing). Errors on unsupported bits are ignored.
    pub fn discovery_types(&mut self, index: u16) -> Result<u8, Error> {
        let info = self.info(index)?;
        for (op, bit) in [(OP_SET_BREDR, SET_BREDR), (OP_SET_LE, SET_LE)] {
            if info.supported & bit != 0 && info.current & bit == 0 {
                let _ = self.set(op, index, &[1]);
            }
        }
        let cur = self.info(index)?.current;
        Ok((if cur & SET_BREDR != 0 { 1 } else { 0 }) | (if cur & SET_LE != 0 { 6 } else { 0 }))
    }

    /// [`scan`](Self::scan) for the given address types.
    pub fn scan_types(&mut self, index: u16, dur: Duration, types: u8) -> Result<Vec<Found>, Error> {
        self.scan_types_until(index, dur, types, None)
    }

    /// [`scan_types`](Self::scan_types) that also ends early (with what it has) when `stop` is set: the Bluetooth page's Stop button.
    pub fn scan_types_until(&mut self, index: u16, dur: Duration, types: u8, stop: Option<&std::sync::atomic::AtomicBool>) -> Result<Vec<Found>, Error> {
        self.command(OP_START_DISCOVERY, index, &[types])?;
        let mut found: Vec<Found> = Vec::new();
        let deadline = Instant::now() + dur;
        // Classic devices usually answer an inquiry without their name. The kernel asks ("confirm name" flag) whether we know it; saying no
        // makes it look the name up, which takes a moment after the inquiry. Without this the first scan lists bare addresses. The scan
        // waits a little past its time for the names it asked for.
        let name_grace = deadline + Duration::from_secs(6);
        let mut pending: Vec<String> = Vec::new();
        loop {
            let end = if pending.is_empty() { deadline } else { name_grace };
            let mut left = end.saturating_duration_since(Instant::now());
            if left.is_zero() || stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
                break;
            }
            let watching = stop.is_some();
            if watching {
                left = left.min(Duration::from_millis(200));
            }
            let Some(ev) = self.next_event(left)? else {
                if watching && Instant::now() < end {
                    continue; // only the short look at the stop flag ended
                }
                break;
            };
            match ev.code {
                EV_DEVICE_FOUND if ev.index == index => {
                    if let Some(f) = parse_found(&ev.params) {
                        let flags = u32::from_le_bytes([ev.params[8], ev.params[9], ev.params[10], ev.params[11]]);
                        if f.name.is_empty() && flags & DEV_FOUND_CONFIRM_NAME != 0 && f.addr_type == ADDR_BREDR && !pending.contains(&f.address) && pending.len() < 16 {
                            let mut p = ev.params[..7].to_vec();
                            p.push(0); // name not known: look it up
                            if self.command(OP_CONFIRM_NAME, index, &p).is_ok() {
                                pending.push(f.address.clone());
                            }
                        }
                        if !f.name.is_empty() {
                            pending.retain(|a| a != &f.address);
                        }
                        match found.iter_mut().find(|x| x.address == f.address) {
                            Some(x) => {
                                x.rssi = f.rssi;
                                if x.name.is_empty() {
                                    x.name = f.name;
                                }
                                if x.class.is_none() {
                                    x.class = f.class;
                                }
                                if x.appearance.is_none() {
                                    x.appearance = f.appearance;
                                }
                                for s in f.services {
                                    if !x.services.contains(&s) {
                                        x.services.push(s);
                                    }
                                }
                                x.kind = classify(x.class, x.appearance, &x.services);
                            }
                            None => found.push(f),
                        }
                    }
                }
                EV_DISCOVERING if ev.index == index && ev.params.len() >= 2 && ev.params[1] == 0 => break,
                _ => {}
            }
        }
        // the kernel may already have stopped it (finished or error); that is fine
        let _ = self.command(OP_STOP_DISCOVERY, index, &[types]);
        found.sort_by(|a, b| b.rssi.cmp(&a.rssi));
        Ok(found)
    }

    pub fn set_io_capability(&mut self, index: u16, cap: u8) -> Result<(), Error> {
        self.command(OP_SET_IO_CAPABILITY, index, &[cap]).map(|_| ())
    }

    /// Pairs (bonds) with a device. `ask` is called when the device needs an answer from the person; keyboards that
    /// show a passkey arrive as [`Prompt::PasskeyShow`] (nothing to answer: type it on the keyboard).
    pub fn pair(&mut self, index: u16, addr: &str, addr_type: u8, io_cap: u8, ask: &mut dyn FnMut(&Prompt) -> Answer) -> Result<(), Error> {
        let a = parse_addr(addr).ok_or(Error::Protocol("bad address"))?;
        let mut ai = a.to_vec();
        ai.push(addr_type);
        let mut p = ai.clone();
        p.push(io_cap);
        self.t.send(&encode(OP_PAIR_DEVICE, index, &p))?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let _ = self.command(OP_CANCEL_PAIR_DEVICE, index, &ai);
                return Err(Error::Timeout);
            }
            let Some(raw) = self.t.recv(left)? else { continue };
            let pkt = decode(&raw)?;
            if pkt.index != index {
                continue;
            }
            let for_us = pkt.params.len() >= 7 && pkt.params[..6] == a;
            match pkt.code {
                EV_CMD_COMPLETE | EV_CMD_STATUS if pkt.params.len() >= 3 && u16::from_le_bytes([pkt.params[0], pkt.params[1]]) == OP_PAIR_DEVICE => {
                    let st = pkt.params[2];
                    if pkt.code == EV_CMD_STATUS && st == 0 {
                        continue;
                    }
                    return if st == 0 { Ok(()) } else { Err(Error::Status(st)) };
                }
                EV_USER_CONFIRM_REQUEST if for_us && pkt.params.len() >= 12 => {
                    let hint = pkt.params[7] != 0;
                    let value = u32::from_le_bytes([pkt.params[8], pkt.params[9], pkt.params[10], pkt.params[11]]);
                    let op = if ask(&Prompt::Confirm { value, hint }) == Answer::Yes { OP_USER_CONFIRM_REPLY } else { OP_USER_CONFIRM_NEG_REPLY };
                    self.t.send(&encode(op, index, &ai))?;
                }
                EV_USER_PASSKEY_REQUEST if for_us => match ask(&Prompt::PasskeyRequest) {
                    Answer::Passkey(k) => {
                        let mut r = ai.clone();
                        r.extend_from_slice(&k.to_le_bytes());
                        self.t.send(&encode(OP_USER_PASSKEY_REPLY, index, &r))?;
                    }
                    _ => self.t.send(&encode(OP_USER_PASSKEY_NEG_REPLY, index, &ai))?,
                },
                EV_PASSKEY_NOTIFY if for_us && pkt.params.len() >= 12 => {
                    let k = u32::from_le_bytes([pkt.params[7], pkt.params[8], pkt.params[9], pkt.params[10]]);
                    let _ = ask(&Prompt::PasskeyShow(k));
                }
                EV_PIN_CODE_REQUEST if for_us => match ask(&Prompt::PinRequest) {
                    Answer::Pin(pin) if !pin.is_empty() && pin.len() <= 16 => {
                        let mut r = ai.clone();
                        r.push(pin.len() as u8);
                        let mut b = pin.into_bytes();
                        b.resize(16, 0);
                        r.extend_from_slice(&b);
                        self.t.send(&encode(OP_PIN_CODE_REPLY, index, &r))?;
                    }
                    _ => self.t.send(&encode(OP_PIN_CODE_NEG_REPLY, index, &ai))?,
                },
                EV_AUTH_FAILED if for_us && pkt.params.len() >= 8 => return Err(Error::Status(if pkt.params[7] == 0 { 5 } else { pkt.params[7] })),
                EV_CONNECT_FAILED if for_us && pkt.params.len() >= 8 => return Err(Error::Status(if pkt.params[7] == 0 { 4 } else { pkt.params[7] })),
                _ => self.events.push_back(pkt),
            }
        }
    }

    /// Devices connected right now, as (address, address type).
    pub fn connections(&mut self, index: u16) -> Result<Vec<(String, u8)>, Error> {
        let r = self.command(OP_GET_CONNECTIONS, index, &[])?;
        if r.len() < 2 {
            return Err(Error::Protocol("connections"));
        }
        let n = u16::from_le_bytes([r[0], r[1]]) as usize;
        Ok(r[2..].chunks_exact(7).take(n).map(|c| (addr_string(&c[..6]), c[6])).collect())
    }

    pub fn disconnect(&mut self, index: u16, addr: &str, addr_type: u8) -> Result<(), Error> {
        let mut p = parse_addr(addr).ok_or(Error::Protocol("bad address"))?.to_vec();
        p.push(addr_type);
        self.command(OP_DISCONNECT, index, &p).map(|_| ())
    }

    /// Forgets a paired device (and disconnects it).
    /// Add Device: tells the controller what to do when this device is seen (action 2: connect in the background; used for bonded
    /// low-energy keyboards and mice so they reconnect when they advertise).
    pub fn add_device(&mut self, index: u16, addr: &str, addr_type: u8, action: u8) -> Result<(), Error> {
        let mut p = parse_addr(addr).ok_or(Error::Protocol("bad address"))?.to_vec();
        p.push(addr_type);
        p.push(action);
        self.command(OP_ADD_DEVICE, index, &p).map(|_| ())
    }

    /// Set Device Class: the major and minor class the controller advertises (audio / video, loudspeaker is 4 and 0x14 >> 2 = 5 ...).
    pub fn set_device_class(&mut self, index: u16, major: u8, minor: u8) -> Result<(), Error> {
        self.command(OP_SET_DEV_CLASS, index, &[major, minor]).map(|_| ())
    }

    /// Add UUID: tells the controller a service it offers (its 16-bit form on the Bluetooth base) and the service class bits (`hint`, the
    /// upper byte of the class of device: 0x04 rendering, 0x20 audio) that go with it.
    pub fn add_uuid16(&mut self, index: u16, uuid: u16, hint: u8) -> Result<(), Error> {
        self.command(OP_ADD_UUID, index, &uuid128_le(uuid, hint)).map(|_| ())
    }

    pub fn unpair(&mut self, index: u16, addr: &str, addr_type: u8) -> Result<(), Error> {
        let mut p = parse_addr(addr).ok_or(Error::Protocol("bad address"))?.to_vec();
        p.push(addr_type);
        p.push(1);
        self.command(OP_UNPAIR_DEVICE, index, &p).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays a script: when a command is sent, the listed packets become readable.
    struct Fake {
        sent: Vec<Vec<u8>>,
        inbox: VecDeque<Vec<u8>>,
        /// (opcode, replies) - replies are queued when that opcode is sent
        script: Vec<(u16, Vec<Vec<u8>>)>,
    }

    impl Transport for Fake {
        fn send(&mut self, p: &[u8]) -> io::Result<()> {
            self.sent.push(p.to_vec());
            let op = u16::from_le_bytes([p[0], p[1]]);
            if let Some(i) = self.script.iter().position(|(o, _)| *o == op) {
                let (_, replies) = self.script.remove(i);
                self.inbox.extend(replies);
            }
            Ok(())
        }
        fn recv(&mut self, _wait: Duration) -> io::Result<Option<Vec<u8>>> {
            Ok(self.inbox.pop_front())
        }
    }

    fn fake(script: Vec<(u16, Vec<Vec<u8>>)>) -> Mgmt<Fake> {
        Mgmt::new(Fake { sent: vec![], inbox: VecDeque::new(), script })
    }

    fn complete(op: u16, index: u16, status: u8, data: &[u8]) -> Vec<u8> {
        let mut p = op.to_le_bytes().to_vec();
        p.push(status);
        p.extend_from_slice(data);
        encode(EV_CMD_COMPLETE, index, &p)
    }

    fn found_event(index: u16, addr: [u8; 6], ty: u8, rssi: i8, eir: &[u8]) -> Vec<u8> {
        let mut p = addr.to_vec();
        p.push(ty);
        p.push(rssi as u8);
        p.extend_from_slice(&0u32.to_le_bytes());
        p.extend_from_slice(&(eir.len() as u16).to_le_bytes());
        p.extend_from_slice(eir);
        encode(EV_DEVICE_FOUND, index, &p)
    }

    fn info_reply() -> Vec<u8> {
        let mut r = vec![0u8; 280];
        r[..6].copy_from_slice(&[0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
        r[9..13].copy_from_slice(&(SET_POWERED | SET_SSP | SET_LE | SET_BREDR).to_le_bytes());
        r[13..17].copy_from_slice(&SET_BREDR.to_le_bytes());
        r[20..30].copy_from_slice(b"iyakki-mac");
        r
    }

    #[test]
    fn packets_round_trip_and_short_ones_are_refused() {
        let b = encode(OP_SET_POWERED, 0, &[1]);
        assert_eq!(b, vec![5, 0, 0, 0, 1, 0, 1]);
        let p = decode(&b).unwrap();
        assert_eq!((p.code, p.index, p.params.clone()), (5, 0, vec![1]));
        assert!(decode(&b[..4]).is_err());
        assert!(decode(&b[..6]).is_err(), "declared one byte, none present");
    }

    #[test]
    fn addresses_use_the_kernels_byte_order() {
        assert_eq!(addr_string(&[0x66, 0x55, 0x44, 0x33, 0x22, 0x11]), "11:22:33:44:55:66");
        assert_eq!(parse_addr("11:22:33:44:55:66"), Some([0x66, 0x55, 0x44, 0x33, 0x22, 0x11]));
        assert_eq!(parse_addr("11:22:33"), None);
        assert_eq!(parse_addr("zz:22:33:44:55:66"), None);
    }

    #[test]
    fn index_list_info_and_power() {
        let mut m = fake(vec![
            (OP_READ_INDEX_LIST, vec![complete(OP_READ_INDEX_LIST, INDEX_NONE, 0, &[1, 0, 0, 0])]),
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &info_reply())]),
            (OP_SET_POWERED, vec![encode(EV_NEW_SETTINGS, 0, &1u32.to_le_bytes()), complete(OP_SET_POWERED, 0, 0, &1u32.to_le_bytes())]),
        ]);
        assert_eq!(m.index_list().unwrap(), vec![0]);
        let i = m.info(0).unwrap();
        assert_eq!((i.address.as_str(), i.name.as_str(), i.powered()), ("11:22:33:44:55:66", "iyakki-mac", false));
        assert_eq!(m.set_powered(0, true).unwrap() & SET_POWERED, SET_POWERED);
        assert_eq!(m.events.len(), 1, "the New Settings event was kept, not lost");
        assert_eq!(m.events[0].code, EV_NEW_SETTINGS);
        assert_eq!(m.t.sent.last().unwrap(), &encode(OP_SET_POWERED, 0, &[1]));
    }

    #[test]
    fn refusals_become_plain_errors() {
        let mut m = fake(vec![(OP_SET_POWERED, vec![complete(OP_SET_POWERED, 0, 18, &[])])]);
        let e = m.set_powered(0, true).unwrap_err();
        assert!(matches!(e, Error::Status(18)));
        assert!(e.to_string().contains("rfkill"));
        let mut m = fake(vec![]);
        assert!(matches!(m.set_powered(0, true), Err(Error::Timeout)), "a silent controller times out");
        let mut m = fake(vec![(OP_SET_POWERED, vec![encode(EV_CMD_STATUS, 0, &[5, 0, 17])])]);
        assert!(matches!(m.set_powered(0, true), Err(Error::Status(17))));
    }

    #[test]
    fn eir_gives_name_class_appearance_and_services() {
        let mut eir = vec![];
        eir.extend_from_slice(&[9, 0x09]);
        eir.extend_from_slice(b"Keyboard");
        eir.extend_from_slice(&[4, 0x0d, 0x40, 0x25, 0x00]); // class 0x002540: peripheral, keyboard
        eir.extend_from_slice(&[3, 0x03, 0x12, 0x18]);
        let (n, c, a, s) = parse_eir(&eir);
        assert_eq!((n.as_str(), c, a, s.clone()), ("Keyboard", Some(0x2540), None, vec![0x1812]));
        assert_eq!(classify(c, a, &s), Kind::Keyboard);
        assert_eq!(classify(Some(0x240404), None, &[]), Kind::Headphones, "audio, wearable headset minor class");
        assert_eq!(classify(Some(0x240414), None, &[]), Kind::Speaker, "loudspeaker");
        assert_eq!(classify(Some(0x002580), None, &[]), Kind::Mouse);
        assert_eq!(classify(None, Some(0x03c1), &[]), Kind::Keyboard, "LE appearance: HID keyboard");
        assert_eq!(classify(None, None, &[0x110b]), Kind::Speaker);
        assert_eq!(classify(None, None, &[]), Kind::Other);
        assert_eq!(parse_eir(&[0, 1, 2]).0, "", "a zero length ends the data");
        assert_eq!(parse_eir(&[9, 0x09, b'x']).0, "", "a field longer than the data is ignored");
    }

    #[test]
    fn scan_merges_repeated_sightings_and_sorts_by_signal() {
        let kb = [0x01, 0x00, 0x00, 0x00, 0x00, 0xaa];
        let sp = [0x02, 0x00, 0x00, 0x00, 0x00, 0xbb];
        let mut name_kb = vec![9, 0x09];
        name_kb.extend_from_slice(b"Keyboard");
        let mut name_sp = vec![6, 0x09];
        name_sp.extend_from_slice(b"Boom!");
        let cls_sp = [4, 0x0d, 0x14, 0x04, 0x24];
        let mut sp_eir = name_sp.clone();
        sp_eir.extend_from_slice(&cls_sp);
        let mut m = fake(vec![(
            OP_START_DISCOVERY,
            vec![
                complete(OP_START_DISCOVERY, 0, 0, &[7]),
                found_event(0, kb, ADDR_LE_PUBLIC, -70, &[]),
                found_event(0, sp, ADDR_BREDR, -40, &sp_eir),
                found_event(0, kb, ADDR_LE_PUBLIC, -60, &name_kb),
                encode(EV_DISCOVERING, 0, &[7, 0]),
            ],
        )]);
        let f = m.scan(0, Duration::from_secs(3)).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].address.as_str(), f[0].name.as_str(), f[0].kind), ("BB:00:00:00:00:02", "Boom!", Kind::Speaker));
        assert_eq!((f[1].address.as_str(), f[1].name.as_str(), f[1].rssi), ("AA:00:00:00:00:01", "Keyboard", -60));
        assert_eq!(m.t.sent.last().unwrap()[0..2], OP_STOP_DISCOVERY.to_le_bytes(), "scan always stops discovery");
    }

    #[test]
    fn a_classic_device_without_a_name_gets_its_name_looked_up_so_the_first_scan_shows_it() {
        let ph = [0x05, 0x00, 0x00, 0x00, 0x00, 0xcc];
        let with_flags = |name: &[u8], flags: u32| {
            let mut p = ph.to_vec();
            p.extend_from_slice(&[ADDR_BREDR, (-50i8) as u8]);
            p.extend_from_slice(&flags.to_le_bytes());
            p.extend_from_slice(&(name.len() as u16).to_le_bytes());
            p.extend_from_slice(name);
            encode(EV_DEVICE_FOUND, 0, &p)
        };
        let mut name = vec![6, 0x09];
        name.extend_from_slice(b"Phone");
        let mut cmd = ph.to_vec();
        cmd.push(ADDR_BREDR);
        let mut m = fake(vec![
            (OP_START_DISCOVERY, vec![complete(OP_START_DISCOVERY, 0, 0, &[7]), with_flags(&[], DEV_FOUND_CONFIRM_NAME)]),
            (OP_CONFIRM_NAME, vec![complete(OP_CONFIRM_NAME, 0, 0, &cmd), with_flags(&name, 0), encode(EV_DISCOVERING, 0, &[7, 0])]),
        ]);
        let f = m.scan(0, Duration::from_secs(1)).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "Phone", "the name that arrived after the look-up is kept");
        let ask = m.t.sent.iter().find(|p| p[0..2] == OP_CONFIRM_NAME.to_le_bytes()).expect("the kernel was asked to look the name up");
        assert_eq!(&ask[6..], &[&ph[..], &[ADDR_BREDR, 0]].concat()[..], "address, type, name not known");
    }

    #[test]
    fn pairing_with_a_confirm_prompt_and_a_failure() {
        let addr = "AA:00:00:00:00:01";
        let a = parse_addr(addr).unwrap();
        let mut confirm = a.to_vec();
        confirm.extend_from_slice(&[0, 0]);
        confirm.extend_from_slice(&123456u32.to_le_bytes());
        confirm[6] = 0; // type
        confirm[7] = 0; // hint
        let mut done = a.to_vec();
        done.push(0);
        let mut m = fake(vec![
            (OP_PAIR_DEVICE, vec![encode(EV_USER_CONFIRM_REQUEST, 0, &confirm)]),
            (OP_USER_CONFIRM_REPLY, vec![complete(OP_PAIR_DEVICE, 0, 0, &done)]),
        ]);
        let mut seen = vec![];
        m.pair(0, addr, ADDR_BREDR, IO_DISPLAY_YES_NO, &mut |p| {
            seen.push(p.clone());
            Answer::Yes
        })
        .unwrap();
        assert_eq!(seen, vec![Prompt::Confirm { value: 123456, hint: false }]);
        let reply = m.t.sent.iter().find(|p| p[0..2] == OP_USER_CONFIRM_REPLY.to_le_bytes()).expect("confirmation sent");
        assert_eq!(&reply[6..12], &a, "the reply names the device");

        // a refused confirmation sends the negative reply; an authentication failure is reported as such
        let mut fail = a.to_vec();
        fail.extend_from_slice(&[0, 5]);
        let mut m = fake(vec![
            (OP_PAIR_DEVICE, vec![encode(EV_USER_CONFIRM_REQUEST, 0, &confirm)]),
            (OP_USER_CONFIRM_NEG_REPLY, vec![encode(EV_AUTH_FAILED, 0, &fail)]),
        ]);
        let e = m.pair(0, addr, ADDR_BREDR, IO_DISPLAY_YES_NO, &mut |_| Answer::No).unwrap_err();
        assert!(matches!(e, Error::Status(5)));
        assert!(m.t.sent.iter().any(|p| p[0..2] == OP_USER_CONFIRM_NEG_REPLY.to_le_bytes()));
    }

    #[test]
    fn pairing_a_keyboard_that_shows_a_passkey_and_a_legacy_pin() {
        let addr = "AA:00:00:00:00:01";
        let a = parse_addr(addr).unwrap();
        let mut notify = a.to_vec();
        notify.push(0);
        notify.extend_from_slice(&654321u32.to_le_bytes());
        notify.push(0);
        let mut pin = a.to_vec();
        pin.extend_from_slice(&[0, 0]);
        let mut done = a.to_vec();
        done.push(0);
        let mut m = fake(vec![(OP_PAIR_DEVICE, vec![encode(EV_PASSKEY_NOTIFY, 0, &notify), encode(EV_PIN_CODE_REQUEST, 0, &pin)]), (OP_PIN_CODE_REPLY, vec![complete(OP_PAIR_DEVICE, 0, 0, &done)])]);
        let mut seen = vec![];
        m.pair(0, addr, ADDR_BREDR, IO_KEYBOARD_DISPLAY, &mut |p| {
            seen.push(p.clone());
            Answer::Pin("0000".into())
        })
        .unwrap();
        assert_eq!(seen, vec![Prompt::PasskeyShow(654321), Prompt::PinRequest]);
        let r = m.t.sent.iter().find(|p| p[0..2] == OP_PIN_CODE_REPLY.to_le_bytes()).unwrap();
        assert_eq!(r[13], 4, "pin length");
        assert_eq!(&r[14..18], b"0000");
    }

    #[test]
    fn connections_disconnect_and_unpair() {
        let mut list = vec![1, 0];
        list.extend_from_slice(&[1, 0, 0, 0, 0, 0xaa, 0]);
        let a = parse_addr("AA:00:00:00:00:01").unwrap();
        let mut ai = a.to_vec();
        ai.push(0);
        let mut m = fake(vec![
            (OP_GET_CONNECTIONS, vec![complete(OP_GET_CONNECTIONS, 0, 0, &list)]),
            (OP_DISCONNECT, vec![complete(OP_DISCONNECT, 0, 0, &ai)]),
            (OP_UNPAIR_DEVICE, vec![complete(OP_UNPAIR_DEVICE, 0, 0, &ai)]),
        ]);
        assert_eq!(m.connections(0).unwrap(), vec![("AA:00:00:00:00:01".to_string(), 0)]);
        m.disconnect(0, "AA:00:00:00:00:01", ADDR_BREDR).unwrap();
        m.unpair(0, "AA:00:00:00:00:01", ADDR_BREDR).unwrap();
        assert_eq!(*m.t.sent.last().unwrap().last().unwrap(), 1, "unpair also disconnects");
        assert!(m.disconnect(0, "nonsense", 0).is_err());
    }

    #[test]
    fn prepare_turns_on_only_what_the_controller_supports() {
        let mut info = info_reply();
        info[9..13].copy_from_slice(&(SET_POWERED | SET_LE | SET_BONDABLE).to_le_bytes()); // LE-only controller
        let mut m = fake(vec![
            (OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &info)]),
            (OP_SET_LE, vec![complete(OP_SET_LE, 0, 0, &SET_LE.to_le_bytes())]),
            (OP_SET_BONDABLE, vec![complete(OP_SET_BONDABLE, 0, 0, &(SET_LE | SET_BONDABLE).to_le_bytes())]),
        ]);
        // each loop step reads info again, so give it enough copies
        for _ in 0..2 {
            m.t.script.push((OP_READ_INFO, vec![complete(OP_READ_INFO, 0, 0, &info)]));
        }
        let cur = m.prepare(0).unwrap();
        assert_eq!(cur & SET_BONDABLE, SET_BONDABLE);
        let ops: Vec<u16> = m.t.sent.iter().map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        assert!(!ops.contains(&OP_SET_SSP) && !ops.contains(&OP_SET_BREDR), "unsupported settings are not asked for");
        assert!(ops.contains(&OP_SET_LE));
    }
}

//! nl80211 (the Linux Wi-Fi control interface over generic netlink): message builders and parsers as pure functions over bytes,
//! and a small control layer on a transport trait. The constants are the values of the public kernel header `linux/nl80211.h`.
//!
//! The real transport (an AF_NETLINK socket) needs system calls and so `unsafe`, which this crate forbids; on this build
//! [`UnsupportedTransport`] answers "not supported". The hardware step will add a transport in a crate that may hold the
//! socket (the same way `iyakki-fw` has its own nf_tables writer) and implement [`NetlinkTransport`] for it; nothing else here
//! changes. Tests use a fake transport that records the exact bytes.

use crate::Mac;

pub const NLMSG_ERROR: u16 = 2;
pub const NLMSG_DONE: u16 = 3;
pub const NLM_F_REQUEST: u16 = 1;
pub const NLM_F_ACK: u16 = 4;
pub const NLM_F_DUMP: u16 = 0x300;
pub const GENL_ID_CTRL: u16 = 16;
pub const CTRL_CMD_GETFAMILY: u8 = 3;
pub const CTRL_ATTR_FAMILY_ID: u16 = 1;
pub const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const NLA_F_NESTED: u16 = 0x8000;
const NLA_TYPE_MASK: u16 = 0x3fff;

pub mod cmd {
    pub const GET_WIPHY: u8 = 1;
    pub const NEW_WIPHY: u8 = 3;
    pub const GET_INTERFACE: u8 = 5;
    pub const SET_INTERFACE: u8 = 6;
    pub const NEW_INTERFACE: u8 = 7;
    pub const DEL_INTERFACE: u8 = 8;
    pub const NEW_KEY: u8 = 11;
    pub const SET_KEY: u8 = 10;
    pub const START_AP: u8 = 15;
    pub const STOP_AP: u8 = 16;
    pub const SET_STATION: u8 = 18;
    pub const NEW_STATION: u8 = 19;
    pub const DEL_STATION: u8 = 20;
    pub const SET_BSS: u8 = 25;
    pub const REGISTER_FRAME: u8 = 58;
    pub const FRAME: u8 = 59;
    pub const CONTROL_PORT_FRAME: u8 = 129;
}

pub mod attr {
    pub const WIPHY: u16 = 1;
    pub const WIPHY_NAME: u16 = 2;
    pub const IFINDEX: u16 = 3;
    pub const IFNAME: u16 = 4;
    pub const IFTYPE: u16 = 5;
    pub const MAC: u16 = 6;
    pub const KEY_DATA: u16 = 7;
    pub const KEY_IDX: u16 = 8;
    pub const KEY_CIPHER: u16 = 9;
    pub const BEACON_INTERVAL: u16 = 12;
    pub const DTIM_PERIOD: u16 = 13;
    pub const BEACON_HEAD: u16 = 14;
    pub const BEACON_TAIL: u16 = 15;
    pub const STA_AID: u16 = 16;
    pub const STA_LISTEN_INTERVAL: u16 = 18;
    pub const STA_SUPPORTED_RATES: u16 = 19;
    pub const SUPPORTED_IFTYPES: u16 = 32;
    pub const WIPHY_FREQ: u16 = 38;
    pub const FRAME: u16 = 51;
    pub const SSID: u16 = 52;
    pub const AUTH_TYPE: u16 = 53;
    pub const REASON_CODE: u16 = 54;
    pub const MGMT_SUBTYPE: u16 = 41;
    pub const STA_FLAGS2: u16 = 67;
    pub const CONTROL_PORT: u16 = 68;
    pub const PRIVACY: u16 = 70;
    pub const CIPHER_SUITES_PAIRWISE: u16 = 73;
    pub const CIPHER_SUITE_GROUP: u16 = 74;
    pub const WPA_VERSIONS: u16 = 75;
    pub const AKM_SUITES: u16 = 76;
    pub const KEY: u16 = 80;
    pub const AP_ISOLATE: u16 = 96;
    pub const FRAME_MATCH: u16 = 91;
    pub const FRAME_TYPE: u16 = 101;
    pub const CONTROL_PORT_ETHERTYPE: u16 = 102;
    pub const KEY_DEFAULT_TYPES: u16 = 110;
    pub const HIDDEN_SSID: u16 = 126;
    pub const PROBE_RESP: u16 = 145;
    pub const INACTIVITY_TIMEOUT: u16 = 150;
    pub const CHANNEL_WIDTH: u16 = 159;
    pub const CENTER_FREQ1: u16 = 160;
    pub const STA_CAPABILITY: u16 = 171;
    /// The pairwise master key for a card that runs the 4-way handshake itself (`NL80211_EXT_FEATURE_4WAY_HANDSHAKE_AP_PSK`).
    pub const PMK: u16 = 254;
    /// Inside the nested `KEY` attribute.
    pub const KEY_NESTED_DATA: u16 = 1;
    pub const KEY_NESTED_IDX: u16 = 2;
    pub const KEY_NESTED_CIPHER: u16 = 3;
    pub const KEY_NESTED_DEFAULT: u16 = 5;
    pub const KEY_NESTED_DEFAULT_TYPES: u16 = 8;
}

pub const IFTYPE_STATION: u32 = 2;
pub const IFTYPE_AP: u32 = 3;
pub const CIPHER_CCMP: u32 = 0x000f_ac04;
pub const AKM_PSK: u32 = 0x000f_ac02;
pub const WPA_VERSION_2: u32 = 2;
pub const STA_FLAG_AUTHORIZED: u32 = 1;
pub const STA_FLAG_AUTHENTICATED: u32 = 5;
pub const STA_FLAG_ASSOCIATED: u32 = 7;
pub const ETH_P_PAE: u16 = 0x888e;

/// A netlink + generic netlink message under construction.
pub struct Msg {
    buf: Vec<u8>,
}

impl Msg {
    pub fn new(ty: u16, flags: u16, seq: u32, cmd: u8) -> Msg {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf.extend_from_slice(&ty.to_le_bytes());
        buf.extend_from_slice(&flags.to_le_bytes());
        buf.extend_from_slice(&seq.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[cmd, 0, 0, 0]);
        Msg { buf }
    }

    pub fn bytes(mut self, ty: u16, data: &[u8]) -> Msg {
        self.put(ty, data);
        self
    }

    fn put(&mut self, ty: u16, data: &[u8]) {
        self.buf.extend_from_slice(&((4 + data.len()) as u16).to_le_bytes());
        self.buf.extend_from_slice(&ty.to_le_bytes());
        self.buf.extend_from_slice(data);
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
    }

    pub fn u8(self, ty: u16, v: u8) -> Msg {
        self.bytes(ty, &[v])
    }
    pub fn u16(self, ty: u16, v: u16) -> Msg {
        self.bytes(ty, &v.to_le_bytes())
    }
    pub fn u32(self, ty: u16, v: u32) -> Msg {
        self.bytes(ty, &v.to_le_bytes())
    }
    pub fn flag(self, ty: u16) -> Msg {
        self.bytes(ty, &[])
    }
    /// A NUL-terminated string.
    pub fn strz(self, ty: u16, s: &str) -> Msg {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        self.bytes(ty, &v)
    }
    /// A nested attribute whose content is built by `f`.
    pub fn nest(mut self, ty: u16, f: impl FnOnce(Msg) -> Msg) -> Msg {
        let start = self.buf.len();
        self.put(ty | NLA_F_NESTED, &[]);
        let inner = f(Msg { buf: Vec::new() });
        self.buf.extend_from_slice(&inner.buf);
        let len = (self.buf.len() - start) as u16;
        self.buf[start..start + 2].copy_from_slice(&len.to_le_bytes());
        self
    }

    pub fn finish(mut self) -> Vec<u8> {
        let l = self.buf.len() as u32;
        self.buf[..4].copy_from_slice(&l.to_le_bytes());
        self.buf
    }
}

/// One netlink message cut out of a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NlMessage<'a> {
    pub ty: u16,
    pub flags: u16,
    pub seq: u32,
    pub payload: &'a [u8],
}

/// Cut a buffer into messages; `None` if a length is wrong.
pub fn split_messages(mut b: &[u8]) -> Option<Vec<NlMessage<'_>>> {
    let mut v = Vec::new();
    while !b.is_empty() {
        if b.len() < 16 {
            return None;
        }
        let len = u32::from_le_bytes(b[0..4].try_into().ok()?) as usize;
        if len < 16 || len > b.len() {
            return None;
        }
        v.push(NlMessage { ty: u16::from_le_bytes([b[4], b[5]]), flags: u16::from_le_bytes([b[6], b[7]]), seq: u32::from_le_bytes(b[8..12].try_into().ok()?), payload: &b[16..len] });
        let next = (len + 3) & !3;
        b = b.get(next..).unwrap_or(&[]);
    }
    Some(v)
}

/// The generic netlink header of a payload: command and the attribute bytes.
pub fn genl(payload: &[u8]) -> Option<(u8, &[u8])> {
    (payload.len() >= 4).then(|| (payload[0], &payload[4..]))
}

/// Attributes of a buffer (type without the nested/byte-order flags, value); `None` if one is damaged.
pub fn attrs(mut b: &[u8]) -> Option<Vec<(u16, &[u8])>> {
    let mut v = Vec::new();
    while !b.is_empty() {
        if b.len() < 4 {
            return None;
        }
        let len = u16::from_le_bytes([b[0], b[1]]) as usize;
        if len < 4 || len > b.len() {
            return None;
        }
        v.push((u16::from_le_bytes([b[2], b[3]]) & NLA_TYPE_MASK, &b[4..len]));
        b = b.get(((len + 3) & !3)..).unwrap_or(&[]);
    }
    Some(v)
}

fn find<'a>(a: &[(u16, &'a [u8])], ty: u16) -> Option<&'a [u8]> {
    a.iter().find(|(t, _)| *t == ty).map(|(_, v)| *v)
}

fn as_u32(b: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

// ---- builders ----

const REQ_ACK: u16 = NLM_F_REQUEST | NLM_F_ACK;

/// Ask the generic netlink controller for the id of the `nl80211` family.
pub fn get_family_request(seq: u32) -> Vec<u8> {
    Msg::new(GENL_ID_CTRL, NLM_F_REQUEST, seq, CTRL_CMD_GETFAMILY).strz(CTRL_ATTR_FAMILY_NAME, "nl80211").finish()
}

/// The family id out of the controller's answer.
pub fn parse_family_id(reply: &[u8]) -> Option<u16> {
    for m in split_messages(reply)? {
        if m.ty != GENL_ID_CTRL {
            continue;
        }
        let (_, a) = genl(m.payload)?;
        let id: [u8; 2] = find(&attrs(a)?, CTRL_ATTR_FAMILY_ID)?.try_into().ok()?;
        return Some(u16::from_le_bytes(id));
    }
    None
}

/// Ask for one interface (its type, address and the channel it is on).
pub fn get_interface(family: u16, seq: u32, ifindex: u32) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::GET_INTERFACE).u32(attr::IFINDEX, ifindex).finish()
}

/// The channel (MHz) an interface is on, from the answer to [`get_interface`]; `None` when it is on no channel (not connected).
pub fn parse_interface_freq(reply: &[u8]) -> Option<u32> {
    for m in split_messages(reply)? {
        if m.ty == NLMSG_DONE || m.ty == NLMSG_ERROR {
            continue;
        }
        let (c, a) = genl(m.payload)?;
        if c != cmd::NEW_INTERFACE {
            continue;
        }
        let a = attrs(a)?;
        return find(&a, attr::WIPHY_FREQ).and_then(as_u32).filter(|f| *f > 0);
    }
    None
}

pub fn get_wiphy_dump(family: u16, seq: u32) -> Vec<u8> {
    Msg::new(family, NLM_F_REQUEST | NLM_F_DUMP, seq, cmd::GET_WIPHY).finish()
}

/// Change the interface type (to AP before starting, back to station after stopping). The interface must be down.
pub fn set_interface(family: u16, seq: u32, ifindex: u32, iftype: u32) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::SET_INTERFACE).u32(attr::IFINDEX, ifindex).u32(attr::IFTYPE, iftype).finish()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApParams {
    pub ifindex: u32,
    pub ssid: Vec<u8>,
    pub freq_mhz: u32,
    pub beacon_interval_tu: u32,
    pub dtim_period: u32,
    pub beacon_head: Vec<u8>,
    pub beacon_tail: Vec<u8>,
    pub probe_resp: Vec<u8>,
    pub inactivity_secs: u16,
    /// Set for a card that runs the handshake itself: the key goes to the card and no frame handling is done here.
    pub pmk: Option<[u8; 32]>,
}

/// Start the access point: WPA2, CCMP, PSK, open-system authentication (the keys come from the handshake), SSID visible,
/// a 20 MHz channel, beacon pieces and probe response supplied by us.
pub fn start_ap(family: u16, seq: u32, p: &ApParams) -> Vec<u8> {
    let suites = |s: u32| s.to_le_bytes();
    let m = Msg::new(family, REQ_ACK, seq, cmd::START_AP)
        .u32(attr::IFINDEX, p.ifindex)
        .u32(attr::BEACON_INTERVAL, p.beacon_interval_tu)
        .u32(attr::DTIM_PERIOD, p.dtim_period)
        .bytes(attr::BEACON_HEAD, &p.beacon_head)
        .bytes(attr::BEACON_TAIL, &p.beacon_tail)
        .bytes(attr::SSID, &p.ssid)
        .u32(attr::HIDDEN_SSID, 0)
        .u32(attr::AUTH_TYPE, 0)
        .flag(attr::PRIVACY)
        .u32(attr::WPA_VERSIONS, WPA_VERSION_2)
        .bytes(attr::CIPHER_SUITES_PAIRWISE, &suites(CIPHER_CCMP))
        .u32(attr::CIPHER_SUITE_GROUP, CIPHER_CCMP)
        .bytes(attr::AKM_SUITES, &suites(AKM_PSK))
        .u32(attr::WIPHY_FREQ, p.freq_mhz)
        .u32(attr::CHANNEL_WIDTH, 0)
        .u32(attr::CENTER_FREQ1, p.freq_mhz)
        .bytes(attr::PROBE_RESP, &p.probe_resp);
    // found on virtual radios: a radio without NL80211_FEATURE_INACTIVITY_TIMER refuses START_AP (EOPNOTSUPP) if this attribute is present
    // at all, so 0 means "leave it out" (the caller sets it from the capability probe)
    let m = if p.inactivity_secs > 0 { m.u16(attr::INACTIVITY_TIMEOUT, p.inactivity_secs) } else { m };
    let m = match &p.pmk {
        Some(k) => m.bytes(attr::PMK, k),
        None => m,
    };
    m.finish()
}

/// Create a second interface (an access point next to the client) on radio `wiphy`.
pub fn new_interface(family: u16, seq: u32, wiphy: u32, name: &str, iftype: u32) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::NEW_INTERFACE).u32(attr::WIPHY, wiphy).strz(attr::IFNAME, name).u32(attr::IFTYPE, iftype).finish()
}

pub fn del_interface(family: u16, seq: u32, ifindex: u32) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::DEL_INTERFACE).u32(attr::IFINDEX, ifindex).finish()
}

pub fn stop_ap(family: u16, seq: u32, ifindex: u32) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::STOP_AP).u32(attr::IFINDEX, ifindex).finish()
}

/// Devices on the hotspot cannot reach each other at the Wi-Fi level (set through SET_BSS).
pub fn set_ap_isolate(family: u16, seq: u32, ifindex: u32, on: bool) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::SET_BSS).u32(attr::IFINDEX, ifindex).u8(attr::AP_ISOLATE, on as u8).finish()
}

/// Install the pairwise CCMP key of one client (key index 0).
pub fn new_pairwise_key(family: u16, seq: u32, ifindex: u32, sta: Mac, tk: &[u8; 16]) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::NEW_KEY)
        .u32(attr::IFINDEX, ifindex)
        .bytes(attr::MAC, &sta)
        .nest(attr::KEY, |m| m.bytes(attr::KEY_NESTED_DATA, tk).u32(attr::KEY_NESTED_CIPHER, CIPHER_CCMP).u8(attr::KEY_NESTED_IDX, 0))
        .finish()
}

/// Install the group key (no address).
pub fn new_group_key(family: u16, seq: u32, ifindex: u32, idx: u8, gtk: &[u8; 16]) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::NEW_KEY)
        .u32(attr::IFINDEX, ifindex)
        .nest(attr::KEY, |m| m.bytes(attr::KEY_NESTED_DATA, gtk).u32(attr::KEY_NESTED_CIPHER, CIPHER_CCMP).u8(attr::KEY_NESTED_IDX, idx))
        .finish()
}

/// Make a group key the one used to send multicast frames.
pub fn set_default_group_key(family: u16, seq: u32, ifindex: u32, idx: u8) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::SET_KEY)
        .u32(attr::IFINDEX, ifindex)
        .nest(attr::KEY, |m| m.u8(attr::KEY_NESTED_IDX, idx).flag(attr::KEY_NESTED_DEFAULT).nest(attr::KEY_NESTED_DEFAULT_TYPES, |t| t.flag(2)))
        .finish()
}

fn flags2(mask_bits: &[u32], set_bits: &[u32]) -> [u8; 8] {
    let m = mask_bits.iter().fold(0u32, |a, b| a | (1 << b));
    let s = set_bits.iter().fold(0u32, |a, b| a | (1 << b));
    let mut o = [0u8; 8];
    o[..4].copy_from_slice(&m.to_le_bytes());
    o[4..].copy_from_slice(&s.to_le_bytes());
    o
}

/// Tell the kernel about an associated client (not yet authorized: its data frames wait for the handshake).
pub fn new_station(family: u16, seq: u32, ifindex: u32, sta: Mac, aid: u16, listen_interval: u16, rates: &[u8], capability: u16) -> Vec<u8> {
    let r: Vec<u8> = rates.iter().map(|x| x & 0x7f).collect();
    Msg::new(family, REQ_ACK, seq, cmd::NEW_STATION)
        .u32(attr::IFINDEX, ifindex)
        .bytes(attr::MAC, &sta)
        .u16(attr::STA_AID, aid)
        .u16(attr::STA_LISTEN_INTERVAL, listen_interval)
        .bytes(attr::STA_SUPPORTED_RATES, &r)
        .u16(attr::STA_CAPABILITY, capability)
        .bytes(attr::STA_FLAGS2, &flags2(&[STA_FLAG_AUTHENTICATED, STA_FLAG_ASSOCIATED], &[STA_FLAG_AUTHENTICATED, STA_FLAG_ASSOCIATED]))
        .finish()
}

/// Open or close the controlled port of a client (after the handshake the client may send and receive data).
pub fn set_station_authorized(family: u16, seq: u32, ifindex: u32, sta: Mac, on: bool) -> Vec<u8> {
    let set: &[u32] = if on { &[STA_FLAG_AUTHORIZED] } else { &[] };
    Msg::new(family, REQ_ACK, seq, cmd::SET_STATION).u32(attr::IFINDEX, ifindex).bytes(attr::MAC, &sta).bytes(attr::STA_FLAGS2, &flags2(&[STA_FLAG_AUTHORIZED], set)).finish()
}

/// Remove a client and send it a de-authentication with `reason`.
pub fn del_station(family: u16, seq: u32, ifindex: u32, sta: Mac, reason: u16) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::DEL_STATION).u32(attr::IFINDEX, ifindex).bytes(attr::MAC, &sta).u8(attr::MGMT_SUBTYPE, 12).u16(attr::REASON_CODE, reason).finish()
}

/// Ask for management frames of one type (`frame_type` is the 16-bit frame control value with the type and subtype set, for
/// example `0x00b0` for authentication, `0x0000` for association requests).
pub fn register_frame(family: u16, seq: u32, ifindex: u32, frame_type: u16) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::REGISTER_FRAME).u32(attr::IFINDEX, ifindex).u16(attr::FRAME_TYPE, frame_type).bytes(attr::FRAME_MATCH, &[]).finish()
}

/// Send a management frame.
pub fn send_frame(family: u16, seq: u32, ifindex: u32, freq_mhz: u32, frame: &[u8]) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::FRAME).u32(attr::IFINDEX, ifindex).u32(attr::WIPHY_FREQ, freq_mhz).bytes(attr::FRAME, frame).finish()
}

/// Send an EAPOL frame to a client through the control port (kernel 5.x and later).
pub fn send_control_port_frame(family: u16, seq: u32, ifindex: u32, sta: Mac, eapol: &[u8]) -> Vec<u8> {
    Msg::new(family, REQ_ACK, seq, cmd::CONTROL_PORT_FRAME).u32(attr::IFINDEX, ifindex).bytes(attr::MAC, &sta).u16(attr::CONTROL_PORT_ETHERTYPE, ETH_P_PAE).bytes(attr::FRAME, eapol).finish()
}

// ---- parsers ----

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WiphyInfo {
    pub index: u32,
    pub name: String,
    pub iftypes: Vec<u32>,
}

impl WiphyInfo {
    pub fn supports_ap(&self) -> bool {
        self.iftypes.contains(&IFTYPE_AP)
    }
}

/// Why a card cannot be used as a hotspot, in words for the Settings page; `Ok` when it can.
pub fn ap_support(w: &WiphyInfo) -> Result<(), &'static str> {
    if w.supports_ap() {
        Ok(())
    } else {
        Err("this Wi-Fi card cannot work as an access point")
    }
}

/// The wireless devices in the answer to a GET_WIPHY dump.
pub fn parse_wiphy_dump(reply: &[u8]) -> Option<Vec<WiphyInfo>> {
    let mut out: Vec<WiphyInfo> = Vec::new();
    for m in split_messages(reply)? {
        if m.ty == NLMSG_DONE || m.ty == NLMSG_ERROR {
            continue;
        }
        let (c, a) = genl(m.payload)?;
        if c != cmd::NEW_WIPHY {
            continue;
        }
        let a = attrs(a)?;
        let mut w = WiphyInfo { index: find(&a, attr::WIPHY).and_then(as_u32)?, ..Default::default() };
        if let Some(n) = find(&a, attr::WIPHY_NAME) {
            w.name = String::from_utf8_lossy(n.split(|b| *b == 0).next().unwrap_or(&[])).into_owned();
        }
        if let Some(t) = find(&a, attr::SUPPORTED_IFTYPES) {
            w.iftypes = attrs(t)?.iter().map(|(ty, _)| *ty as u32).collect();
        }
        // a wiphy may arrive in several messages; merge by index
        if let Some(prev) = out.iter_mut().find(|p| p.index == w.index) {
            if prev.name.is_empty() {
                prev.name = w.name;
            }
            for t in w.iftypes {
                if !prev.iftypes.contains(&t) {
                    prev.iftypes.push(t);
                }
            }
        } else {
            out.push(w);
        }
    }
    Some(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A management frame arrived on an interface.
    Frame { ifindex: u32, freq_mhz: u32, frame: Vec<u8> },
    /// An EAPOL (or other control port) frame from a client.
    ControlPort { ifindex: u32, sta: Mac, ethertype: u16, frame: Vec<u8> },
    /// A client appeared or went away in the kernel's table.
    Station { ifindex: u32, sta: Mac, added: bool },
}

/// Parse one event message the kernel sent; unknown events give `None`.
pub fn parse_event(msg: &[u8]) -> Option<Event> {
    let m = split_messages(msg)?.into_iter().next()?;
    let (c, a) = genl(m.payload)?;
    let a = attrs(a)?;
    let ifindex = find(&a, attr::IFINDEX).and_then(as_u32)?;
    let mac = |b: &[u8]| -> Option<Mac> { b.try_into().ok() };
    match c {
        cmd::FRAME => Some(Event::Frame { ifindex, freq_mhz: find(&a, attr::WIPHY_FREQ).and_then(as_u32).unwrap_or(0), frame: find(&a, attr::FRAME)?.to_vec() }),
        cmd::CONTROL_PORT_FRAME => Some(Event::ControlPort { ifindex, sta: mac(find(&a, attr::MAC)?)?, ethertype: u16::from_le_bytes(find(&a, attr::CONTROL_PORT_ETHERTYPE)?.try_into().ok()?), frame: find(&a, attr::FRAME)?.to_vec() }),
        cmd::NEW_STATION | cmd::DEL_STATION => Some(Event::Station { ifindex, sta: mac(find(&a, attr::MAC)?)?, added: c == cmd::NEW_STATION }),
        _ => None,
    }
}

/// Look at an answer for the acknowledgement: `Ok(())` for an ack (or no error), the kernel's errno otherwise.
pub fn check_ack(reply: &[u8]) -> Result<(), NlError> {
    for m in split_messages(reply).ok_or(NlError::Malformed)? {
        if m.ty == NLMSG_ERROR {
            let e = i32::from_le_bytes(m.payload.get(..4).ok_or(NlError::Malformed)?.try_into().map_err(|_| NlError::Malformed)?);
            if e != 0 {
                return Err(NlError::Kernel(-e));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NlError {
    /// No netlink on this build.
    NotSupported,
    /// The kernel refused with this errno (EBUSY 16, EOPNOTSUPP 95, EINVAL 22 ...).
    Kernel(i32),
    Malformed,
    Io(String),
}

impl std::fmt::Display for NlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NlError::NotSupported => f.write_str("not supported on this build"),
            NlError::Kernel(16) => f.write_str("the Wi-Fi card is busy (it may be in use as a client)"),
            NlError::Kernel(95) => f.write_str("the Wi-Fi card does not support this"),
            NlError::Kernel(e) => write!(f, "the Wi-Fi driver refused (error {e})"),
            NlError::Malformed => f.write_str("a damaged answer from the Wi-Fi driver"),
            NlError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Sends one request and returns everything the kernel answered up to the acknowledgement or the end of a dump.
pub trait NetlinkTransport {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, NlError>;
}

/// The transport of this build: there is no netlink socket in safe Rust, so every call says "not supported".
pub struct UnsupportedTransport;

impl NetlinkTransport for UnsupportedTransport {
    fn exchange(&mut self, _request: &[u8]) -> Result<Vec<u8>, NlError> {
        Err(NlError::NotSupported)
    }
}

/// The access point control calls, on any transport.
pub struct ApControl<T: NetlinkTransport> {
    t: T,
    family: Option<u16>,
    seq: u32,
}

impl<T: NetlinkTransport> ApControl<T> {
    pub fn new(t: T) -> ApControl<T> {
        ApControl { t, family: None, seq: 0 }
    }

    pub fn transport(&self) -> &T {
        &self.t
    }

    fn next(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }

    fn family(&mut self) -> Result<u16, NlError> {
        if let Some(f) = self.family {
            return Ok(f);
        }
        let seq = self.next();
        let r = self.t.exchange(&get_family_request(seq))?;
        let f = parse_family_id(&r).ok_or(NlError::Malformed)?;
        self.family = Some(f);
        Ok(f)
    }

    fn run(&mut self, build: impl FnOnce(u16, u32) -> Vec<u8>) -> Result<(), NlError> {
        let fam = self.family()?;
        let seq = self.next();
        let r = self.t.exchange(&build(fam, seq))?;
        check_ack(&r)
    }

    pub fn wiphys(&mut self) -> Result<Vec<WiphyInfo>, NlError> {
        let fam = self.family()?;
        let seq = self.next();
        let r = self.t.exchange(&get_wiphy_dump(fam, seq))?;
        check_ack(&r)?;
        parse_wiphy_dump(&r).ok_or(NlError::Malformed)
    }

    /// The channel (MHz) the interface is on right now, if it is on one.
    pub fn interface_freq(&mut self, ifindex: u32) -> Result<Option<u32>, NlError> {
        let fam = self.family()?;
        let seq = self.next();
        let r = self.t.exchange(&get_interface(fam, seq, ifindex))?;
        check_ack(&r)?;
        Ok(parse_interface_freq(&r))
    }

    pub fn add_ap_interface(&mut self, wiphy: u32, name: &str) -> Result<(), NlError> {
        self.run(|f, s| new_interface(f, s, wiphy, name, IFTYPE_AP))
    }

    pub fn remove_interface(&mut self, ifindex: u32) -> Result<(), NlError> {
        self.run(|f, s| del_interface(f, s, ifindex))
    }

    pub fn set_ap_mode(&mut self, ifindex: u32) -> Result<(), NlError> {
        self.run(|f, s| set_interface(f, s, ifindex, IFTYPE_AP))
    }

    pub fn set_station_mode(&mut self, ifindex: u32) -> Result<(), NlError> {
        self.run(|f, s| set_interface(f, s, ifindex, IFTYPE_STATION))
    }

    pub fn start_ap(&mut self, p: &ApParams) -> Result<(), NlError> {
        self.run(|f, s| start_ap(f, s, p))
    }

    pub fn stop_ap(&mut self, ifindex: u32) -> Result<(), NlError> {
        self.run(|f, s| stop_ap(f, s, ifindex))
    }

    pub fn install_ptk(&mut self, ifindex: u32, sta: Mac, tk: &[u8; 16]) -> Result<(), NlError> {
        self.run(|f, s| new_pairwise_key(f, s, ifindex, sta, tk))
    }

    pub fn install_gtk(&mut self, ifindex: u32, idx: u8, gtk: &[u8; 16]) -> Result<(), NlError> {
        self.run(|f, s| new_group_key(f, s, ifindex, idx, gtk))?;
        self.run(|f, s| set_default_group_key(f, s, ifindex, idx))
    }

    pub fn add_station(&mut self, ifindex: u32, sta: Mac, aid: u16, listen: u16, rates: &[u8], cap: u16) -> Result<(), NlError> {
        self.run(|f, s| new_station(f, s, ifindex, sta, aid, listen, rates, cap))
    }

    pub fn authorize_station(&mut self, ifindex: u32, sta: Mac) -> Result<(), NlError> {
        self.run(|f, s| set_station_authorized(f, s, ifindex, sta, true))
    }

    pub fn remove_station(&mut self, ifindex: u32, sta: Mac, reason: u16) -> Result<(), NlError> {
        self.run(|f, s| del_station(f, s, ifindex, sta, reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    const FAM: u16 = 0x1c;
    const STA: Mac = [2, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];

    #[test]
    fn the_family_request_is_the_documented_32_bytes() {
        assert_eq!(hex(&get_family_request(1)), "20000000 1000 0100 01000000 00000000 03000000 0c00 0200 6e6c3830323131 00".replace(' ', ""));
    }

    #[test]
    fn set_interface_to_ap_golden_bytes() {
        let m = set_interface(FAM, 5, 2, IFTYPE_AP);
        assert_eq!(hex(&m), "24000000 1c00 0500 05000000 00000000 06000000 08000300 02000000 08000500 03000000".replace(' ', ""));
    }

    #[test]
    fn a_second_interface_and_its_removal_golden_bytes() {
        let m = new_interface(FAM, 4, 1, "ap0", IFTYPE_AP);
        assert_eq!(hex(&m), "2c000000 1c00 0500 04000000 00000000 07000000 08000100 01000000 08000400 61703000 08000500 03000000".replace(' ', ""));
        assert_eq!(hex(&del_interface(FAM, 5, 9)), "1c000000 1c00 0500 05000000 00000000 08000000 08000300 09000000".replace(' ', ""));
    }

    #[test]
    fn a_card_that_runs_the_handshake_gets_the_key_and_others_do_not() {
        let mut p = ApParams { ifindex: 7, ssid: b"x".to_vec(), freq_mhz: 2437, beacon_interval_tu: 100, dtim_period: 2, beacon_head: vec![1], beacon_tail: vec![2], probe_resp: vec![3], inactivity_secs: 0, pmk: None };
        let has = |m: &[u8]| {
            let msgs = split_messages(m).unwrap();
            let (_, a) = genl(msgs[0].payload).unwrap();
            find(&attrs(a).unwrap(), attr::PMK).map(|v| v.to_vec())
        };
        assert_eq!(has(&start_ap(FAM, 1, &p)), None);
        p.pmk = Some([0x5a; 32]);
        assert_eq!(has(&start_ap(FAM, 1, &p)), Some(vec![0x5a; 32]));
    }

    #[test]
    fn stop_ap_and_the_station_calls_golden_bytes() {
        assert_eq!(hex(&stop_ap(FAM, 6, 7)), "1c000000 1c00 0500 06000000 00000000 10000000 08000300 07000000".replace(' ', ""));
        let d = del_station(FAM, 9, 7, STA, 15);
        // ifindex | mac (6 bytes padded to 8) | subtype 12 (1 byte padded to 4) | reason 15 (2 bytes padded to 4)
        assert_eq!(hex(&d), "38000000 1c00 0500 09000000 00000000 14000000 08000300 07000000 0a000600 02aabbccddee0000 05002900 0c000000 06003600 0f000000".replace(' ', ""));
        let a = set_station_authorized(FAM, 10, 7, STA, true);
        assert!(hex(&a).ends_with("0c004300 02000000 02000000".replace(' ', "").as_str()), "mask and set are both bit 1 (authorized): {}", hex(&a));
        let off = set_station_authorized(FAM, 11, 7, STA, false);
        assert!(hex(&off).ends_with("0c004300 02000000 00000000".replace(' ', "").as_str()));
    }

    #[test]
    fn keys_are_nested_with_the_flag_and_the_ccmp_suite() {
        let m = new_pairwise_key(FAM, 3, 7, STA, &[0xab; 16]);
        let msgs = split_messages(&m).unwrap();
        assert_eq!(msgs[0].ty, FAM);
        let (c, a) = genl(msgs[0].payload).unwrap();
        assert_eq!(c, cmd::NEW_KEY);
        let a = attrs(a).unwrap();
        assert_eq!(find(&a, attr::MAC).unwrap(), &STA);
        // the raw type carries the nested flag
        assert!(m.windows(2).any(|w| w == [0x50, 0x80]));
        let key = attrs(find(&a, attr::KEY).unwrap()).unwrap();
        assert_eq!(find(&key, attr::KEY_NESTED_DATA).unwrap(), &[0xab; 16]);
        assert_eq!(find(&key, attr::KEY_NESTED_CIPHER).unwrap(), &CIPHER_CCMP.to_le_bytes());
        assert_eq!(find(&key, attr::KEY_NESTED_IDX).unwrap(), &[0]);
        let g = new_group_key(FAM, 4, 7, 2, &[1; 16]);
        let (_, a) = genl(split_messages(&g).unwrap()[0].payload).unwrap();
        let a = attrs(a).unwrap();
        assert!(find(&a, attr::MAC).is_none(), "a group key has no address");
        assert_eq!(find(&attrs(find(&a, attr::KEY).unwrap()).unwrap(), attr::KEY_NESTED_IDX).unwrap(), &[2]);
        let d = set_default_group_key(FAM, 5, 7, 2);
        let (_, a) = genl(split_messages(&d).unwrap()[0].payload).unwrap();
        let key = attrs(find(&attrs(a).unwrap(), attr::KEY).unwrap()).unwrap();
        assert!(find(&key, attr::KEY_NESTED_DEFAULT).is_some());
        let types = attrs(find(&key, attr::KEY_NESTED_DEFAULT_TYPES).unwrap()).unwrap();
        assert_eq!(types[0].0, 2, "multicast");
    }

    #[test]
    fn start_ap_carries_everything_the_kernel_needs() {
        let p = ApParams { ifindex: 7, ssid: b"IYAKKI-1A2B".to_vec(), freq_mhz: 2437, beacon_interval_tu: 100, dtim_period: 2, beacon_head: vec![1, 2, 3], beacon_tail: vec![4, 5], probe_resp: vec![6], inactivity_secs: 300, pmk: None };
        let m = start_ap(FAM, 8, &p);
        let msgs = split_messages(&m).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!((msgs[0].flags, msgs[0].seq), (NLM_F_REQUEST | NLM_F_ACK, 8));
        let (c, a) = genl(msgs[0].payload).unwrap();
        assert_eq!(c, cmd::START_AP);
        let a = attrs(a).unwrap();
        let u32of = |t| u32::from_le_bytes(find(&a, t).unwrap().try_into().unwrap());
        assert_eq!((u32of(attr::IFINDEX), u32of(attr::BEACON_INTERVAL), u32of(attr::DTIM_PERIOD), u32of(attr::WIPHY_FREQ), u32of(attr::CENTER_FREQ1)), (7, 100, 2, 2437, 2437));
        assert_eq!((u32of(attr::WPA_VERSIONS), u32of(attr::CIPHER_SUITE_GROUP), u32of(attr::AUTH_TYPE), u32of(attr::HIDDEN_SSID)), (2, 0x000fac04, 0, 0));
        assert_eq!(find(&a, attr::CIPHER_SUITES_PAIRWISE).unwrap(), &0x000fac04u32.to_le_bytes());
        assert_eq!(find(&a, attr::AKM_SUITES).unwrap(), &0x000fac02u32.to_le_bytes());
        assert_eq!(find(&a, attr::SSID).unwrap(), b"IYAKKI-1A2B");
        assert_eq!(find(&a, attr::BEACON_HEAD).unwrap(), &[1, 2, 3]);
        assert_eq!(find(&a, attr::BEACON_TAIL).unwrap(), &[4, 5]);
        assert_eq!(find(&a, attr::PROBE_RESP).unwrap(), &[6]);
        assert_eq!(find(&a, attr::PRIVACY).unwrap().len(), 0);
        assert_eq!(find(&a, attr::INACTIVITY_TIMEOUT).unwrap(), &300u16.to_le_bytes());
        assert_eq!(m.len() % 4, 0);
    }

    #[test]
    fn a_new_station_has_its_aid_rates_without_the_basic_bit_and_the_two_flags() {
        let m = new_station(FAM, 2, 7, STA, 1, 10, &[0x82, 0x84, 0x30], 0x0431);
        let (c, a) = genl(split_messages(&m).unwrap()[0].payload).unwrap();
        assert_eq!(c, cmd::NEW_STATION);
        let a = attrs(a).unwrap();
        assert_eq!(find(&a, attr::STA_AID).unwrap(), &1u16.to_le_bytes());
        assert_eq!(find(&a, attr::STA_SUPPORTED_RATES).unwrap(), &[2, 4, 0x30]);
        assert_eq!(find(&a, attr::STA_CAPABILITY).unwrap(), &0x0431u16.to_le_bytes());
        let both = (1u32 << 5) | (1 << 7);
        let mut want = both.to_le_bytes().to_vec();
        want.extend_from_slice(&both.to_le_bytes());
        assert_eq!(find(&a, attr::STA_FLAGS2).unwrap(), &want[..]);
    }

    #[test]
    fn frames_register_and_send_with_the_documented_attributes() {
        let r = register_frame(FAM, 1, 7, 0x00b0);
        let (c, a) = genl(split_messages(&r).unwrap()[0].payload).unwrap();
        assert_eq!(c, cmd::REGISTER_FRAME);
        assert_eq!(find(&attrs(a).unwrap(), attr::FRAME_TYPE).unwrap(), &0x00b0u16.to_le_bytes());
        let s = send_frame(FAM, 2, 7, 2437, &[0xb0, 0, 1]);
        let (c, a) = genl(split_messages(&s).unwrap()[0].payload).unwrap();
        assert_eq!(c, cmd::FRAME);
        assert_eq!(find(&attrs(a).unwrap(), attr::FRAME).unwrap(), &[0xb0, 0, 1]);
        let e = send_control_port_frame(FAM, 3, 7, STA, &[9, 9]);
        let (c, a) = genl(split_messages(&e).unwrap()[0].payload).unwrap();
        assert_eq!(c, cmd::CONTROL_PORT_FRAME);
        assert_eq!(find(&attrs(a).unwrap(), attr::CONTROL_PORT_ETHERTYPE).unwrap(), &ETH_P_PAE.to_le_bytes());
    }

    fn reply_msg(ty: u16, cmdn: u8, build: impl FnOnce(Msg) -> Msg) -> Vec<u8> {
        build(Msg::new(ty, 2, 1, cmdn)).finish()
    }

    #[test]
    fn the_family_id_and_a_wiphy_dump_are_read_back() {
        let fam = reply_msg(GENL_ID_CTRL, 1, |m| m.u16(CTRL_ATTR_FAMILY_ID, 0x1c).strz(CTRL_ATTR_FAMILY_NAME, "nl80211"));
        assert_eq!(parse_family_id(&fam), Some(0x1c));
        assert_eq!(parse_family_id(&[]), None);
        let mut dump = reply_msg(FAM, cmd::NEW_WIPHY, |m| m.u32(attr::WIPHY, 0).strz(attr::WIPHY_NAME, "phy0").nest(attr::SUPPORTED_IFTYPES, |n| n.flag(2).flag(3).flag(6)));
        dump.extend(reply_msg(FAM, cmd::NEW_WIPHY, |m| m.u32(attr::WIPHY, 1).strz(attr::WIPHY_NAME, "phy1").nest(attr::SUPPORTED_IFTYPES, |n| n.flag(2))));
        dump.extend(Msg::new(NLMSG_DONE, 2, 1, 0).finish());
        let w = parse_wiphy_dump(&dump).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].name.as_str(), w[0].supports_ap(), w[0].iftypes.clone()), ("phy0", true, vec![2, 3, 6]));
        assert!(ap_support(&w[0]).is_ok());
        assert!(!w[1].supports_ap());
        assert!(ap_support(&w[1]).unwrap_err().contains("access point"));
    }

    #[test]
    fn the_channel_of_a_connected_interface_is_read_and_an_unconnected_one_has_none() {
        let req = get_interface(0x1c, 4, 5);
        assert_eq!(req[6..8], REQ_ACK.to_le_bytes(), "acknowledged, so the exchange ends");
        let on = reply_msg(FAM, cmd::NEW_INTERFACE, |m| m.u32(attr::IFINDEX, 5).u32(attr::WIPHY_FREQ, 2417));
        assert_eq!(parse_interface_freq(&on), Some(2417));
        let off = reply_msg(FAM, cmd::NEW_INTERFACE, |m| m.u32(attr::IFINDEX, 5));
        assert_eq!(parse_interface_freq(&off), None);
        assert_eq!(parse_interface_freq(&[]), None);
    }

    #[test]
    fn events_are_parsed_and_unknown_ones_ignored() {
        let f = reply_msg(FAM, cmd::FRAME, |m| m.u32(attr::IFINDEX, 7).u32(attr::WIPHY_FREQ, 2437).bytes(attr::FRAME, &[0xb0, 0, 0]));
        assert_eq!(parse_event(&f), Some(Event::Frame { ifindex: 7, freq_mhz: 2437, frame: vec![0xb0, 0, 0] }));
        let e = reply_msg(FAM, cmd::CONTROL_PORT_FRAME, |m| m.u32(attr::IFINDEX, 7).bytes(attr::MAC, &STA).u16(attr::CONTROL_PORT_ETHERTYPE, ETH_P_PAE).bytes(attr::FRAME, &[2, 3]));
        assert_eq!(parse_event(&e), Some(Event::ControlPort { ifindex: 7, sta: STA, ethertype: 0x888e, frame: vec![2, 3] }));
        let s = reply_msg(FAM, cmd::DEL_STATION, |m| m.u32(attr::IFINDEX, 7).bytes(attr::MAC, &STA));
        assert_eq!(parse_event(&s), Some(Event::Station { ifindex: 7, sta: STA, added: false }));
        let other = reply_msg(FAM, 99, |m| m.u32(attr::IFINDEX, 7));
        assert_eq!(parse_event(&other), None);
        // a station event with a short address is not trusted
        let bad = reply_msg(FAM, cmd::NEW_STATION, |m| m.u32(attr::IFINDEX, 7).bytes(attr::MAC, &STA[..4]));
        assert_eq!(parse_event(&bad), None);
    }

    #[test]
    fn acks_and_kernel_errors_are_told_apart() {
        let ack = Msg::new(NLMSG_ERROR, 0, 1, 0).finish();
        // an ack is an error message with errno 0: the genl header position holds the 32-bit error
        assert_eq!(check_ack(&ack), Ok(()));
        let mut busy = Msg::new(NLMSG_ERROR, 0, 1, 0).finish();
        busy[16..20].copy_from_slice(&(-16i32).to_le_bytes());
        assert_eq!(check_ack(&busy), Err(NlError::Kernel(16)));
        assert!(NlError::Kernel(16).to_string().contains("busy"));
        assert_eq!(check_ack(&[1, 2, 3]), Err(NlError::Malformed));
    }

    #[test]
    fn the_parsers_never_panic_on_junk() {
        let mut s = 11u32;
        let good = start_ap(FAM, 1, &ApParams { ifindex: 1, ssid: b"x".to_vec(), freq_mhz: 2412, beacon_interval_tu: 100, dtim_period: 2, beacon_head: vec![0; 40], beacon_tail: vec![0; 30], probe_resp: vec![], inactivity_secs: 10, pmk: None });
        for n in 0..6000usize {
            let mut v = if n % 2 == 0 { good.clone() } else { (0..n % 200).map(|_| { s = s.wrapping_mul(1664525).wrapping_add(1013904223); (s >> 24) as u8 }).collect() };
            if n % 2 == 0 && !v.is_empty() {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                let at = (s >> 8) as usize % v.len();
                v[at] ^= (s >> 3) as u8 | 1;
                v.truncate(v.len() - (s as usize % 5).min(v.len()));
            }
            let _ = split_messages(&v);
            let _ = parse_wiphy_dump(&v);
            let _ = parse_event(&v);
            let _ = check_ack(&v);
            let _ = parse_family_id(&v);
            let _ = attrs(&v);
        }
    }

    /// A transport that records requests and answers from a script.
    struct Fake {
        sent: Vec<Vec<u8>>,
        fail_with: Option<i32>,
    }

    impl NetlinkTransport for Fake {
        fn exchange(&mut self, req: &[u8]) -> Result<Vec<u8>, NlError> {
            self.sent.push(req.to_vec());
            let m = split_messages(req).unwrap().remove(0);
            if m.ty == GENL_ID_CTRL {
                return Ok(reply_msg(GENL_ID_CTRL, 1, |m| m.u16(CTRL_ATTR_FAMILY_ID, FAM)));
            }
            if m.flags & NLM_F_DUMP != 0 {
                let mut d = reply_msg(FAM, cmd::NEW_WIPHY, |m| m.u32(attr::WIPHY, 0).strz(attr::WIPHY_NAME, "phy0").nest(attr::SUPPORTED_IFTYPES, |n| n.flag(3)));
                d.extend(Msg::new(NLMSG_DONE, 2, 1, 0).finish());
                return Ok(d);
            }
            let mut ack = Msg::new(NLMSG_ERROR, 0, m.seq, 0).finish();
            if let Some(e) = self.fail_with {
                ack[16..20].copy_from_slice(&(-e).to_le_bytes());
            }
            Ok(ack)
        }
    }

    #[test]
    fn the_control_layer_resolves_the_family_once_and_sends_each_call_with_a_fresh_sequence_number() {
        let mut c = ApControl::new(Fake { sent: vec![], fail_with: None });
        assert!(c.wiphys().unwrap()[0].supports_ap());
        c.set_ap_mode(7).unwrap();
        c.start_ap(&ApParams { ifindex: 7, ssid: b"n".to_vec(), freq_mhz: 2437, beacon_interval_tu: 100, dtim_period: 2, beacon_head: vec![0], beacon_tail: vec![0], probe_resp: vec![], inactivity_secs: 300, pmk: None }).unwrap();
        c.install_gtk(7, 1, &[5; 16]).unwrap();
        c.add_station(7, STA, 1, 10, &[0x82], 0x0431).unwrap();
        c.install_ptk(7, STA, &[6; 16]).unwrap();
        c.authorize_station(7, STA).unwrap();
        c.remove_station(7, STA, 3).unwrap();
        c.stop_ap(7).unwrap();
        let sent = &c.transport().sent;
        let cmds: Vec<(u16, u8)> = sent.iter().map(|b| { let m = &split_messages(b).unwrap()[0]; (m.ty, m.payload[0]) }).collect();
        assert_eq!(cmds.iter().filter(|(t, _)| *t == GENL_ID_CTRL).count(), 1, "the family is asked for once");
        let order: Vec<u8> = cmds.iter().filter(|(t, _)| *t == FAM).map(|(_, c)| *c).collect();
        assert_eq!(order, vec![cmd::GET_WIPHY, cmd::SET_INTERFACE, cmd::START_AP, cmd::NEW_KEY, cmd::SET_KEY, cmd::NEW_STATION, cmd::NEW_KEY, cmd::SET_STATION, cmd::DEL_STATION, cmd::STOP_AP]);
        let seqs: Vec<u32> = sent.iter().map(|b| split_messages(b).unwrap()[0].seq).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_kernel_refusal_comes_back_as_an_error_and_the_unsupported_transport_says_so() {
        let mut c = ApControl::new(Fake { sent: vec![], fail_with: Some(16) });
        assert_eq!(c.set_ap_mode(7), Err(NlError::Kernel(16)));
        let mut u = ApControl::new(UnsupportedTransport);
        assert_eq!(u.wiphys(), Err(NlError::NotSupported));
        assert_eq!(u.start_ap(&ApParams { ifindex: 1, ssid: vec![], freq_mhz: 0, beacon_interval_tu: 0, dtim_period: 0, beacon_head: vec![], beacon_tail: vec![], probe_resp: vec![], inactivity_secs: 0, pmk: None }), Err(NlError::NotSupported));
        assert_eq!(NlError::NotSupported.to_string(), "not supported on this build");
    }
}

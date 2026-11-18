//! Bluetooth remote control (AVRCP 1.x over AVCTP) without BlueZ: the byte formats and the two roles, with no socket in this module
//! (the AVCTP channel is L2CAP PSM 0x17; the layer that owns the socket feeds whole packets in and sends what comes back).
//!
//! * [`Controller`] is this computer controlling a phone or a player: it builds pass-through keys (play, pause, next ...), asks for the
//!   play status and for notifications, and reads the answers.
//! * [`Target`] is this computer as the sound sink: it answers the phone's questions (unit and subunit info, which events it knows),
//!   takes the volume the phone sets (`SetAbsoluteVolume`, 0 to 127) and tells the phone when the volume is changed here.
//!
//! Packets that are fragments (AVCTP start, continue, end) are refused with a plain error: nothing this module sends or accepts is
//! larger than one packet. Bytes from the air are never trusted: every length is checked and a bad packet is an `Err`, never a panic.
// SPDX-License-Identifier: Apache-2.0

pub const PSM_AVCTP: u16 = 0x0017;

/// The UUIDs of the AVRCP target SDP record (a search pattern matches when all its UUIDs are in here): the A/V Remote Control
/// Target class, the L2CAP and AVCTP protocols, the A/V Remote Control profile and the public browse root.
pub const SDP_UUIDS: [u16; 5] = [0x110c, 0x0100, 0x0017, 0x110e, 0x1002];

/// The SDP record of this computer as an AVRCP 1.4 target (category 1, player/recorder, and category 2, monitor/amplifier: the
/// volume the phone sets). `handle` is the record handle the SDP server gives it; attributes in id order.
pub fn sdp_record(handle: u32, name: &str) -> Vec<(u16, Vec<u8>)> {
    use crate::a2dp::{sdp_seq as seq, sdp_text, sdp_uint16 as u16e, sdp_uint32 as u32e, sdp_uuid16 as uuid};
    vec![
        (0x0000, u32e(handle)),
        (0x0001, seq(&[uuid(0x110c)])),
        (0x0004, seq(&[seq(&[uuid(0x0100), u16e(PSM_AVCTP)]), seq(&[uuid(0x0017), u16e(0x0103)])])),
        (0x0005, seq(&[uuid(0x1002)])),
        (0x0009, seq(&[seq(&[uuid(PID_AVRCP), u16e(0x0104)])])),
        (0x0100, sdp_text(name)),
        // supported categories: 1 (player or recorder) and 2 (monitor or amplifier), as a bit set
        (0x0311, u16e(0x0003)),
    ]
}
/// The profile identifier of AVRCP in AVCTP headers (the A/V Remote Control service class).
pub const PID_AVRCP: u16 = 0x110e;
/// The Bluetooth SIG company identifier used by the AVRCP vendor-dependent commands.
pub const COMPANY_BT_SIG: [u8; 3] = [0x00, 0x19, 0x58];

pub const CTYPE_CONTROL: u8 = 0x0;
pub const CTYPE_STATUS: u8 = 0x1;
pub const CTYPE_NOTIFY: u8 = 0x3;
pub const RESP_NOT_IMPLEMENTED: u8 = 0x8;
pub const RESP_ACCEPTED: u8 = 0x9;
pub const RESP_REJECTED: u8 = 0xa;
pub const RESP_STABLE: u8 = 0xc;
pub const RESP_CHANGED: u8 = 0xd;
pub const RESP_INTERIM: u8 = 0xf;

const SUBUNIT_PANEL: u8 = 0x48;
const OP_VENDOR: u8 = 0x00;
const OP_UNIT_INFO: u8 = 0x30;
const OP_SUBUNIT_INFO: u8 = 0x31;
const OP_PASS_THROUGH: u8 = 0x7c;

pub const PDU_GET_CAPABILITIES: u8 = 0x10;
pub const PDU_GET_PLAY_STATUS: u8 = 0x30;
pub const PDU_REGISTER_NOTIFICATION: u8 = 0x31;
pub const PDU_SET_ABSOLUTE_VOLUME: u8 = 0x50;

pub const EVENT_PLAYBACK_STATUS: u8 = 0x01;
pub const EVENT_TRACK_CHANGED: u8 = 0x02;
pub const EVENT_VOLUME_CHANGED: u8 = 0x0d;

/// AVRCP error codes in a rejected vendor-dependent answer.
pub const ERR_INVALID_COMMAND: u8 = 0x00;
pub const ERR_INVALID_PARAMETER: u8 = 0x01;

/// The keys a remote control can press (the AV/C panel operation ids).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Rewind,
    FastForward,
    VolumeUp,
    VolumeDown,
    Mute,
}

impl Key {
    pub fn id(self) -> u8 {
        match self {
            Key::Play => 0x44,
            Key::Stop => 0x45,
            Key::Pause => 0x46,
            Key::Next => 0x4b,
            Key::Previous => 0x4c,
            Key::Rewind => 0x48,
            Key::FastForward => 0x49,
            Key::VolumeUp => 0x41,
            Key::VolumeDown => 0x42,
            Key::Mute => 0x43,
        }
    }

    pub fn from_id(id: u8) -> Option<Key> {
        [Key::Play, Key::Pause, Key::Stop, Key::Next, Key::Previous, Key::Rewind, Key::FastForward, Key::VolumeUp, Key::VolumeDown, Key::Mute]
            .into_iter()
            .find(|k| k.id() == id)
    }

    /// The name the assistant and Settings use ("play", "next" ...).
    pub fn name(self) -> &'static str {
        match self {
            Key::Play => "play",
            Key::Pause => "pause",
            Key::Stop => "stop",
            Key::Next => "next",
            Key::Previous => "previous",
            Key::Rewind => "rewind",
            Key::FastForward => "fast forward",
            Key::VolumeUp => "volume up",
            Key::VolumeDown => "volume down",
            Key::Mute => "mute",
        }
    }

    pub fn from_name(n: &str) -> Option<Key> {
        let n = n.trim().to_ascii_lowercase();
        [Key::Play, Key::Pause, Key::Stop, Key::Next, Key::Previous, Key::Rewind, Key::FastForward, Key::VolumeUp, Key::VolumeDown, Key::Mute]
            .into_iter()
            .find(|k| k.name() == n)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayStatus {
    Stopped,
    Playing,
    Paused,
    FwdSeek,
    RevSeek,
    Error,
}

impl PlayStatus {
    pub fn from_byte(b: u8) -> PlayStatus {
        match b {
            0 => PlayStatus::Stopped,
            1 => PlayStatus::Playing,
            2 => PlayStatus::Paused,
            3 => PlayStatus::FwdSeek,
            4 => PlayStatus::RevSeek,
            _ => PlayStatus::Error,
        }
    }
}

/// Why a packet was not understood.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvrcpError {
    Short,
    /// A fragment of a larger AVCTP message: not supported.
    Fragmented,
    /// The other side says it does not know our profile identifier (the IPID bit).
    WrongProfile,
    /// A different profile than AVRCP.
    OtherProfile(u16),
}

impl AvrcpError {
    pub fn text(self) -> &'static str {
        match self {
            AvrcpError::Short => "the remote-control packet was cut short",
            AvrcpError::Fragmented => "the remote-control message came in pieces, which this version does not take",
            AvrcpError::WrongProfile => "the other device does not know the remote-control profile",
            AvrcpError::OtherProfile(_) => "the packet was for another profile",
        }
    }
}

/// One AVCTP message that carries an AV/C frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub label: u8,
    pub response: bool,
    /// The command type (0 control, 1 status, 3 notify) or the response code (8 .. 0xf).
    pub code: u8,
    pub subunit: u8,
    pub opcode: u8,
    pub operands: Vec<u8>,
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![(self.label & 0xf) << 4 | (self.response as u8) << 1, (PID_AVRCP >> 8) as u8, PID_AVRCP as u8, self.code & 0xf, self.subunit, self.opcode];
        v.extend_from_slice(&self.operands);
        v
    }

    pub fn decode(d: &[u8]) -> Result<Frame, AvrcpError> {
        if d.len() < 3 {
            return Err(AvrcpError::Short);
        }
        let (label, ptype, response, ipid) = (d[0] >> 4, (d[0] >> 2) & 3, d[0] & 2 != 0, d[0] & 1 != 0);
        if ptype != 0 {
            return Err(AvrcpError::Fragmented);
        }
        if ipid {
            return Err(AvrcpError::WrongProfile);
        }
        let pid = u16::from_be_bytes([d[1], d[2]]);
        if pid != PID_AVRCP {
            return Err(AvrcpError::OtherProfile(pid));
        }
        if d.len() < 6 {
            return Err(AvrcpError::Short);
        }
        Ok(Frame { label, response, code: d[3] & 0xf, subunit: d[4], opcode: d[5], operands: d[6..].to_vec() })
    }
}

/// The answer to a packet for a profile we do not have: same label, response, "invalid profile identifier" bit set.
pub fn invalid_profile_reply(label: u8, pid: u16) -> Vec<u8> {
    vec![(label & 0xf) << 4 | 0b11, (pid >> 8) as u8, pid as u8]
}

fn vendor(label: u8, code: u8, response: bool, pdu: u8, params: &[u8]) -> Frame {
    let mut o = COMPANY_BT_SIG.to_vec();
    o.extend_from_slice(&[pdu, 0, (params.len() >> 8) as u8, params.len() as u8]);
    o.extend_from_slice(params);
    Frame { label, response, code, subunit: SUBUNIT_PANEL, opcode: OP_VENDOR, operands: o }
}

/// A vendor-dependent frame taken apart: `(pdu, parameters)`. Only single packets of the Bluetooth SIG company id.
fn vendor_parts(f: &Frame) -> Option<(u8, &[u8])> {
    let o = &f.operands;
    if f.opcode != OP_VENDOR || o.len() < 7 || o[..3] != COMPANY_BT_SIG || o[4] & 3 != 0 {
        return None;
    }
    let len = u16::from_be_bytes([o[5], o[6]]) as usize;
    if o.len() != 7 + len {
        return None;
    }
    Some((o[3], &o[7..]))
}

// ------------------------------------------------------------------------------------------------ the controller

/// What an answer from the remote device means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The key was taken.
    KeyAccepted(Key, bool),
    KeyRefused(Key),
    PlayStatus { status: PlayStatus, length_ms: u32, position_ms: u32 },
    /// The events the device can tell about.
    Events(Vec<u8>),
    /// The first answer to a notification request: the current value (the device will say when it changes).
    Interim { event: u8, value: Vec<u8> },
    /// The event happened.
    Changed { event: u8, value: Vec<u8> },
    Volume(u8),
    /// The device refused a vendor command with this AVRCP error code.
    Rejected(u8),
    NotImplemented,
}

/// This computer controlling the other device. Labels count 0 to 15; the caller sends the bytes.
#[derive(Default)]
pub struct Controller {
    label: u8,
    /// What was asked with each label: pass-through key and press, or the pdu.
    pending: [Option<Asked>; 16],
}

#[derive(Clone, Copy, Debug)]
enum Asked {
    Key(Key, bool),
    Pdu(u8),
}

impl Controller {
    pub fn new() -> Controller {
        Controller::default()
    }

    fn next(&mut self, asked: Asked) -> u8 {
        let l = self.label;
        self.label = (self.label + 1) & 0xf;
        self.pending[l as usize] = Some(asked);
        l
    }

    /// A key press or release (a full key stroke is a press, then a release).
    pub fn key(&mut self, key: Key, pressed: bool) -> Vec<u8> {
        let label = self.next(Asked::Key(key, pressed));
        Frame { label, response: false, code: CTYPE_CONTROL, subunit: SUBUNIT_PANEL, opcode: OP_PASS_THROUGH, operands: vec![key.id() | if pressed { 0 } else { 0x80 }, 0] }.encode()
    }

    /// Press and release, the two packets in order.
    pub fn stroke(&mut self, key: Key) -> [Vec<u8>; 2] {
        [self.key(key, true), self.key(key, false)]
    }

    /// Which events can the device tell about (capability 3).
    pub fn ask_events(&mut self) -> Vec<u8> {
        let label = self.next(Asked::Pdu(PDU_GET_CAPABILITIES));
        vendor(label, CTYPE_STATUS, false, PDU_GET_CAPABILITIES, &[0x03]).encode()
    }

    pub fn ask_play_status(&mut self) -> Vec<u8> {
        let label = self.next(Asked::Pdu(PDU_GET_PLAY_STATUS));
        vendor(label, CTYPE_STATUS, false, PDU_GET_PLAY_STATUS, &[]).encode()
    }

    /// Asks to be told when `event` changes (the playback interval is only used by the playback-position event).
    pub fn watch(&mut self, event: u8) -> Vec<u8> {
        let label = self.next(Asked::Pdu(PDU_REGISTER_NOTIFICATION));
        vendor(label, CTYPE_NOTIFY, false, PDU_REGISTER_NOTIFICATION, &[event, 0, 0, 0, 0]).encode()
    }

    /// Sets the volume of the device (0 to 127; more is cut to 127).
    pub fn set_volume(&mut self, volume: u8) -> Vec<u8> {
        let label = self.next(Asked::Pdu(PDU_SET_ABSOLUTE_VOLUME));
        vendor(label, CTYPE_CONTROL, false, PDU_SET_ABSOLUTE_VOLUME, &[volume.min(127)]).encode()
    }

    /// Reads one packet from the device. A notification that changes arrives with a label we used before; an answer to something we
    /// did not ask is an error.
    pub fn read(&mut self, d: &[u8]) -> Result<Answer, AvrcpError> {
        let f = Frame::decode(d)?;
        if !f.response {
            return Err(AvrcpError::Short);
        }
        let asked = self.pending[f.label as usize];
        match (asked, f.opcode) {
            (Some(Asked::Key(k, pressed)), OP_PASS_THROUGH) => {
                if f.code == RESP_ACCEPTED {
                    self.pending[f.label as usize] = None;
                    Ok(Answer::KeyAccepted(k, pressed))
                } else {
                    self.pending[f.label as usize] = None;
                    Ok(Answer::KeyRefused(k))
                }
            }
            (Some(Asked::Pdu(want)), OP_VENDOR) => {
                if f.code == RESP_NOT_IMPLEMENTED {
                    self.pending[f.label as usize] = None;
                    return Ok(Answer::NotImplemented);
                }
                let Some((pdu, p)) = vendor_parts(&f) else { return Err(AvrcpError::Short) };
                if pdu != want {
                    return Err(AvrcpError::Short);
                }
                if f.code == RESP_REJECTED {
                    self.pending[f.label as usize] = None;
                    return Ok(Answer::Rejected(p.first().copied().unwrap_or(ERR_INVALID_COMMAND)));
                }
                // an interim answer keeps the label: the "changed" answer comes with it later
                if f.code != RESP_INTERIM {
                    self.pending[f.label as usize] = if want == PDU_REGISTER_NOTIFICATION { Some(Asked::Pdu(want)) } else { None };
                }
                match pdu {
                    PDU_GET_CAPABILITIES => match p {
                        [0x03, n, rest @ ..] if rest.len() == *n as usize => Ok(Answer::Events(rest.to_vec())),
                        _ => Err(AvrcpError::Short),
                    },
                    PDU_GET_PLAY_STATUS if p.len() == 9 => Ok(Answer::PlayStatus {
                        length_ms: u32::from_be_bytes([p[0], p[1], p[2], p[3]]),
                        position_ms: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
                        status: PlayStatus::from_byte(p[8]),
                    }),
                    PDU_REGISTER_NOTIFICATION if !p.is_empty() => {
                        let (event, value) = (p[0], p[1..].to_vec());
                        Ok(if f.code == RESP_CHANGED { Answer::Changed { event, value } } else { Answer::Interim { event, value } })
                    }
                    PDU_SET_ABSOLUTE_VOLUME if p.len() == 1 => Ok(Answer::Volume(p[0] & 0x7f)),
                    _ => Err(AvrcpError::Short),
                }
            }
            _ => Err(AvrcpError::Short),
        }
    }
}

// ------------------------------------------------------------------------------------------------ the target

/// What the phone did that the rest of the system has to act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The phone set the volume (0 to 127).
    Volume(u8),
    /// A key came from the phone's side (the volume keys of a headset, for example), pressed or released.
    Key(Key, bool),
}

/// This computer as the sound sink answering the phone's remote-control packets.
pub struct Target {
    volume: u8,
    /// The label of the phone's open request for volume changes, if any.
    watch_volume: Option<u8>,
}

impl Default for Target {
    fn default() -> Self {
        Target::new(64)
    }
}

impl Target {
    pub fn new(volume: u8) -> Target {
        Target { volume: volume.min(127), watch_volume: None }
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// The events this target can tell about.
    pub fn events() -> &'static [u8] {
        &[EVENT_VOLUME_CHANGED]
    }

    /// The volume was changed here (the volume keys, Settings). Returns the "changed" notification when the phone asked for it. The
    /// phone has to ask again after each one, as the protocol says.
    pub fn local_volume(&mut self, volume: u8) -> Option<Vec<u8>> {
        self.volume = volume.min(127);
        let label = self.watch_volume.take()?;
        Some(vendor(label, RESP_CHANGED, true, PDU_REGISTER_NOTIFICATION, &[EVENT_VOLUME_CHANGED, self.volume]).encode())
    }

    /// Answers one packet. Returns the answer to send (none for answers from the phone) and what the system should do.
    pub fn handle(&mut self, d: &[u8]) -> Result<(Option<Vec<u8>>, Vec<Event>), AvrcpError> {
        let f = match Frame::decode(d) {
            Ok(f) => f,
            Err(AvrcpError::OtherProfile(pid)) => return Ok((Some(invalid_profile_reply(d[0] >> 4, pid)), vec![])),
            Err(e) => return Err(e),
        };
        if f.response {
            return Ok((None, vec![]));
        }
        let reply = |code: u8, operands: Vec<u8>| Frame { label: f.label, response: true, code, subunit: f.subunit, opcode: f.opcode, operands }.encode();
        let mut events = Vec::new();
        let out = match f.opcode {
            OP_UNIT_INFO => reply(RESP_STABLE, [&[0x07, SUBUNIT_PANEL][..], &COMPANY_BT_SIG].concat()),
            OP_SUBUNIT_INFO => reply(RESP_STABLE, vec![0x07, SUBUNIT_PANEL, 0xff, 0xff, 0xff]),
            OP_PASS_THROUGH => match f.operands.first().and_then(|b| Key::from_id(b & 0x7f).map(|k| (k, b & 0x80 == 0))) {
                Some((k @ (Key::VolumeUp | Key::VolumeDown | Key::Mute | Key::Play | Key::Pause | Key::Stop | Key::Next | Key::Previous), pressed)) => {
                    events.push(Event::Key(k, pressed));
                    reply(RESP_ACCEPTED, f.operands.clone())
                }
                _ => reply(RESP_NOT_IMPLEMENTED, f.operands.clone()),
            },
            OP_VENDOR => match vendor_parts(&f) {
                None => reply(RESP_REJECTED, f.operands.clone()),
                Some((pdu, p)) => self.vendor(&f, pdu, p, &mut events),
            },
            _ => reply(RESP_NOT_IMPLEMENTED, f.operands.clone()),
        };
        Ok((Some(out), events))
    }

    fn vendor(&mut self, f: &Frame, pdu: u8, p: &[u8], events: &mut Vec<Event>) -> Vec<u8> {
        let reject = |err: u8| vendor(f.label, RESP_REJECTED, true, pdu, &[err]).encode();
        match pdu {
            PDU_GET_CAPABILITIES => match p {
                [0x03] => {
                    let ev = Target::events();
                    let mut v = vec![0x03, ev.len() as u8];
                    v.extend_from_slice(ev);
                    vendor(f.label, RESP_STABLE, true, pdu, &v).encode()
                }
                [0x02] => vendor(f.label, RESP_STABLE, true, pdu, &[&[0x02, 1][..], &COMPANY_BT_SIG].concat()).encode(),
                _ => reject(ERR_INVALID_PARAMETER),
            },
            PDU_REGISTER_NOTIFICATION => match p {
                [EVENT_VOLUME_CHANGED, _, _, _, _] => {
                    self.watch_volume = Some(f.label);
                    vendor(f.label, RESP_INTERIM, true, pdu, &[EVENT_VOLUME_CHANGED, self.volume]).encode()
                }
                [_, _, _, _, _] => reject(ERR_INVALID_PARAMETER),
                _ => reject(ERR_INVALID_COMMAND),
            },
            PDU_SET_ABSOLUTE_VOLUME => match p {
                [v] => {
                    self.volume = v & 0x7f;
                    events.push(Event::Volume(self.volume));
                    vendor(f.label, RESP_ACCEPTED, true, pdu, &[self.volume]).encode()
                }
                _ => reject(ERR_INVALID_COMMAND),
            },
            _ => vendor(f.label, RESP_NOT_IMPLEMENTED, true, pdu, p).encode(),
        }
    }
}

/// The volume as a percentage (0 to 100) for Settings and the assistant.
/// What the system does for an [`Event`] from the phone. Only the volume keys and play, pause and stop count; every other key was already refused by the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Set the output volume to this percent (0 to 100).
    SetPercent(u8),
    /// Raise (positive) or lower (negative) the volume by this many percent.
    Step(i8),
    ToggleMute,
    /// A play, pause, stop, next or previous key of the headset or phone: IYAKKI's own player (the radio and the music it plays) does it. Next and
    /// previous step the shared play queue when one is active.
    Media(MediaKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKey {
    Play,
    Pause,
    Stop,
    /// Step the play queue (the music the MEDIA player has queued); nothing happens when no queue is active.
    Next,
    Previous,
}

/// The step of one volume key press, in percent.
pub const KEY_STEP_PERCENT: i8 = 5;

/// Turns what the phone did into what the system does. A key counts when it is pressed (the release does nothing), so one tap is one step.
pub fn act_for(e: &Event) -> Option<Act> {
    match e {
        Event::Volume(v) => Some(Act::SetPercent(volume_percent(*v))),
        Event::Key(Key::VolumeUp, true) => Some(Act::Step(KEY_STEP_PERCENT)),
        Event::Key(Key::VolumeDown, true) => Some(Act::Step(-KEY_STEP_PERCENT)),
        Event::Key(Key::Mute, true) => Some(Act::ToggleMute),
        Event::Key(Key::Play, true) => Some(Act::Media(MediaKey::Play)),
        Event::Key(Key::Pause, true) => Some(Act::Media(MediaKey::Pause)),
        Event::Key(Key::Stop, true) => Some(Act::Media(MediaKey::Stop)),
        Event::Key(Key::Next, true) => Some(Act::Media(MediaKey::Next)),
        Event::Key(Key::Previous, true) => Some(Act::Media(MediaKey::Previous)),
        Event::Key(..) => None,
    }
}

pub fn volume_percent(v: u8) -> u8 {
    ((v.min(127) as u32 * 100 + 63) / 127) as u8
}

/// A percentage as the protocol's 0 to 127.
pub fn percent_volume(p: u8) -> u8 {
    ((p.min(100) as u32 * 127 + 50) / 100) as u8
}

#[cfg(test)]
#[path = "avrcp_tests.rs"]
mod tests;

//! Bluetooth audio, the receiving side (A2DP sink: a phone or laptop plays through this computer's speakers) without BlueZ: the
//! AVDTP signalling, as the acceptor of one stream endpoint, with no socket in this module (the sockets are in `a2dp_sys`).
//!
//! The phone connects to L2CAP PSM 0x19 and sends commands; this module answers them and keeps the stream endpoint's state:
//! `Discover`, `Get(All)Capabilities`, `SetConfiguration`, `Open`, `Start`, `Suspend`, `Close`, `Abort`. Anything else is answered
//! with a general reject. A second connection on the same PSM then carries the media (see [`crate::a2dp`]).
//!
//! The one endpoint is an SBC sink. The configuration the phone picks must be one value of each field; anything else is refused with
//! the specification's error code. Messages that arrive in fragments are put together (up to a limit); we answer in single packets.
// SPDX-License-Identifier: Apache-2.0

use crate::sbc::{ChannelMode, Params};

pub const PSM_AVDTP: u16 = 0x0019;
/// The stream endpoint identifier of our one sink.
pub const OUR_SEID: u8 = 1;

pub const SIG_DISCOVER: u8 = 1;
pub const SIG_GET_CAPABILITIES: u8 = 2;
pub const SIG_SET_CONFIGURATION: u8 = 3;
pub const SIG_GET_CONFIGURATION: u8 = 4;
pub const SIG_RECONFIGURE: u8 = 5;
pub const SIG_OPEN: u8 = 6;
pub const SIG_START: u8 = 7;
pub const SIG_CLOSE: u8 = 8;
pub const SIG_SUSPEND: u8 = 9;
pub const SIG_ABORT: u8 = 10;
pub const SIG_GET_ALL_CAPABILITIES: u8 = 12;

pub const ERR_BAD_ACP_SEID: u8 = 0x12;
pub const ERR_SEP_IN_USE: u8 = 0x13;
pub const ERR_SEP_NOT_IN_USE: u8 = 0x14;
pub const ERR_BAD_SERV_CATEGORY: u8 = 0x17;
pub const ERR_BAD_PAYLOAD_FORMAT: u8 = 0x18;
pub const ERR_NOT_SUPPORTED_COMMAND: u8 = 0x19;
pub const ERR_INVALID_CAPABILITIES: u8 = 0x1a;
pub const ERR_BAD_STATE: u8 = 0x31;
pub const ERR_INVALID_CODEC_TYPE: u8 = 0xc1;
pub const ERR_INVALID_SAMPLING_FREQUENCY: u8 = 0xc3;
pub const ERR_INVALID_CHANNEL_MODE: u8 = 0xc5;
pub const ERR_INVALID_SUBBANDS: u8 = 0xc7;
pub const ERR_INVALID_ALLOCATION_METHOD: u8 = 0xc9;
pub const ERR_INVALID_MINIMUM_BITPOOL: u8 = 0xcb;
pub const ERR_INVALID_MAXIMUM_BITPOOL: u8 = 0xcd;
pub const ERR_INVALID_BLOCK_LENGTH: u8 = 0xcf;

const CAT_MEDIA_TRANSPORT: u8 = 1;
const CAT_MEDIA_CODEC: u8 = 7;
const CODEC_SBC: u8 = 0;
/// The longest signalling message put together from fragments.
const MAX_MESSAGE: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgType {
    Command,
    GeneralReject,
    Accept,
    Reject,
}

/// One whole signalling message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Msg {
    pub label: u8,
    pub kind: MsgType,
    pub signal: u8,
    pub params: Vec<u8>,
}

impl Msg {
    /// A single-packet encoding.
    pub fn encode(&self) -> Vec<u8> {
        let t = match self.kind {
            MsgType::Command => 0,
            MsgType::GeneralReject => 1,
            MsgType::Accept => 2,
            MsgType::Reject => 3,
        };
        let mut v = vec![(self.label << 4) | t, self.signal & 0x3f];
        v.extend_from_slice(&self.params);
        v
    }
}

/// Puts packets (single, or start / continue / end) together into messages.
#[derive(Default)]
pub struct Reassembler {
    partial: Option<(u8, MsgType, u8, usize, Vec<u8>)>,
}

impl Reassembler {
    /// Feeds one L2CAP packet; returns a message when one is complete. Malformed packets are an error and drop what was partial.
    pub fn push(&mut self, d: &[u8]) -> Result<Option<Msg>, &'static str> {
        let first = *d.first().ok_or("empty packet")?;
        let (label, pkt, mt) = (first >> 4, (first >> 2) & 3, first & 3);
        let kind = match mt {
            0 => MsgType::Command,
            1 => MsgType::GeneralReject,
            2 => MsgType::Accept,
            _ => MsgType::Reject,
        };
        match pkt {
            0 => {
                self.partial = None;
                if d.len() < 2 {
                    return Err("single packet is cut");
                }
                Ok(Some(Msg { label, kind, signal: d[1] & 0x3f, params: d[2..].to_vec() }))
            }
            1 => {
                self.partial = None;
                if d.len() < 3 || d[1] < 2 {
                    return Err("start packet is cut");
                }
                self.partial = Some((label, kind, d[2] & 0x3f, d[1] as usize - 1, d[3..].to_vec()));
                Ok(None)
            }
            2 | 3 => {
                let Some((l, k, sig, left, mut data)) = self.partial.take() else { return Err("fragment without a start") };
                if l != label {
                    return Err("fragment of another transaction");
                }
                data.extend_from_slice(&d[1..]);
                if data.len() > MAX_MESSAGE {
                    return Err("message too long");
                }
                if pkt == 3 {
                    if left != 1 {
                        return Err("fragment count does not match");
                    }
                    return Ok(Some(Msg { label: l, kind: k, signal: sig, params: data }));
                }
                if left <= 1 {
                    return Err("too many fragments");
                }
                self.partial = Some((l, k, sig, left - 1, data));
                Ok(None)
            }
            _ => unreachable!(),
        }
    }
}

// ------------------------------------------------------------------------------------------------ SBC capabilities

/// The SBC codec information element (four bytes). In capabilities several bits may be set; in a configuration exactly one per field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SbcInfo {
    /// 0x80 16 kHz, 0x40 32 kHz, 0x20 44.1 kHz, 0x10 48 kHz
    pub freq: u8,
    /// 0x08 mono, 0x04 dual, 0x02 stereo, 0x01 joint stereo
    pub mode: u8,
    /// 0x80 16 blocks, 0x40 12, 0x20 8, 0x10 4
    pub blocks: u8,
    /// 0x08 four subbands, 0x04 eight
    pub subbands: u8,
    /// 0x02 SNR, 0x01 loudness
    pub alloc: u8,
    pub min_bitpool: u8,
    pub max_bitpool: u8,
}

impl SbcInfo {
    /// What this computer can decode: everything the codec has, bitpool 2 to 250.
    pub fn ours() -> SbcInfo {
        SbcInfo { freq: 0xf0, mode: 0x0f, blocks: 0xf0, subbands: 0x0c, alloc: 0x03, min_bitpool: 2, max_bitpool: 250 }
    }

    pub fn to_bytes(&self) -> [u8; 4] {
        [self.freq | self.mode, self.blocks | self.subbands | self.alloc, self.min_bitpool, self.max_bitpool]
    }

    pub fn from_bytes(b: &[u8]) -> Option<SbcInfo> {
        if b.len() != 4 {
            return None;
        }
        Some(SbcInfo { freq: b[0] & 0xf0, mode: b[0] & 0x0f, blocks: b[1] & 0xf0, subbands: b[1] & 0x0c, alloc: b[1] & 0x03, min_bitpool: b[2], max_bitpool: b[3] })
    }

    /// The codec settings of a configuration. The error is the code to refuse it with.
    pub fn to_params(&self) -> Result<Params, u8> {
        let one = |v: u8| v.count_ones() == 1;
        if !one(self.freq) {
            return Err(ERR_INVALID_SAMPLING_FREQUENCY);
        }
        if !one(self.mode) {
            return Err(ERR_INVALID_CHANNEL_MODE);
        }
        if !one(self.blocks) {
            return Err(ERR_INVALID_BLOCK_LENGTH);
        }
        if !one(self.subbands) {
            return Err(ERR_INVALID_SUBBANDS);
        }
        if !one(self.alloc) {
            return Err(ERR_INVALID_ALLOCATION_METHOD);
        }
        if self.min_bitpool < 2 {
            return Err(ERR_INVALID_MINIMUM_BITPOOL);
        }
        if self.max_bitpool < self.min_bitpool {
            return Err(ERR_INVALID_MAXIMUM_BITPOOL);
        }
        let ours = SbcInfo::ours();
        if self.freq & ours.freq == 0 {
            return Err(ERR_INVALID_SAMPLING_FREQUENCY);
        }
        Ok(Params {
            freq: match self.freq {
                0x80 => 0,
                0x40 => 1,
                0x20 => 2,
                _ => 3,
            },
            blocks: match self.blocks {
                0x10 => 4,
                0x20 => 8,
                0x40 => 12,
                _ => 16,
            },
            mode: match self.mode {
                0x08 => ChannelMode::Mono,
                0x04 => ChannelMode::Dual,
                0x02 => ChannelMode::Stereo,
                _ => ChannelMode::Joint,
            },
            loudness: self.alloc == 0x01,
            subbands: if self.subbands == 0x08 { 4 } else { 8 },
            // the frame header carries the bitpool actually used; this is only the upper bound
            bitpool: self.max_bitpool.min(250),
        })
    }
}

/// Our capabilities as the service capability elements of a `Get(All)Capabilities` response.
pub fn our_capabilities() -> Vec<u8> {
    let mut v = vec![CAT_MEDIA_TRANSPORT, 0, CAT_MEDIA_CODEC, 6, 0x00, CODEC_SBC];
    v.extend_from_slice(&SbcInfo::ours().to_bytes());
    v
}

/// The codec information of a list of service capability elements (a configuration or capabilities). `Err`: the category and error to
/// refuse with.
pub fn parse_capabilities(mut d: &[u8]) -> Result<(bool, Option<SbcInfo>), (u8, u8)> {
    let (mut transport, mut sbc) = (false, None);
    while !d.is_empty() {
        if d.len() < 2 || d.len() < 2 + d[1] as usize {
            return Err((0, ERR_BAD_PAYLOAD_FORMAT));
        }
        let (cat, len) = (d[0], d[1] as usize);
        let body = &d[2..2 + len];
        match cat {
            CAT_MEDIA_TRANSPORT => transport = true,
            CAT_MEDIA_CODEC => {
                if len < 2 {
                    return Err((cat, ERR_BAD_PAYLOAD_FORMAT));
                }
                if body[0] >> 4 != 0 || body[1] != CODEC_SBC {
                    return Err((cat, ERR_INVALID_CODEC_TYPE));
                }
                sbc = Some(SbcInfo::from_bytes(&body[2..]).ok_or((cat, ERR_BAD_PAYLOAD_FORMAT))?);
            }
            // reporting, recovery, header compression ...: not offered, so not accepted in a configuration
            _ => return Err((cat, ERR_INVALID_CAPABILITIES)),
        }
        d = &d[2 + len..];
    }
    Ok((transport, sbc))
}

// ------------------------------------------------------------------------------------------------ the endpoint

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Configured,
    Open,
    Streaming,
}

/// What the endpoint did, for the layer that owns the sockets and the audio.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// The phone chose these settings (the sample rate and channels the audio path will get).
    Configured(Params),
    /// The phone is opening the media channel (a second connection on PSM 0x19 is to be expected).
    Opened,
    Started,
    Suspended,
    /// Closed or aborted: the media channel and the audio output go away.
    Ended,
}

/// The acceptor side of one sink endpoint.
pub struct Sink {
    state: StreamState,
    config: Option<Params>,
}

impl Default for Sink {
    fn default() -> Self {
        Sink::new()
    }
}

fn reject(m: &Msg, params: Vec<u8>) -> Msg {
    Msg { label: m.label, kind: MsgType::Reject, signal: m.signal, params }
}

fn accept(m: &Msg, params: Vec<u8>) -> Msg {
    Msg { label: m.label, kind: MsgType::Accept, signal: m.signal, params }
}

impl Sink {
    pub fn new() -> Sink {
        Sink { state: StreamState::Idle, config: None }
    }

    pub fn state(&self) -> StreamState {
        self.state
    }

    pub fn config(&self) -> Option<&Params> {
        self.config.as_ref()
    }

    fn seid_ok(b: Option<&u8>) -> bool {
        b.is_some_and(|b| b >> 2 == OUR_SEID)
    }

    /// Answers one message. Responses and rejects that come from the phone (we send no commands) are ignored.
    pub fn handle(&mut self, m: &Msg) -> (Option<Msg>, Vec<Action>) {
        if m.kind != MsgType::Command {
            return (None, vec![]);
        }
        let mut acts = Vec::new();
        let reply = match m.signal {
            SIG_DISCOVER => {
                // one endpoint: SEID, in use flag, audio, sink
                let in_use = self.state != StreamState::Idle;
                accept(m, vec![OUR_SEID << 2 | (in_use as u8) << 1, 0x08])
            }
            SIG_GET_CAPABILITIES | SIG_GET_ALL_CAPABILITIES => {
                if !Sink::seid_ok(m.params.first()) || m.params.len() != 1 {
                    reject(m, vec![ERR_BAD_ACP_SEID])
                } else {
                    accept(m, our_capabilities())
                }
            }
            SIG_SET_CONFIGURATION => self.set_configuration(m, &mut acts),
            SIG_GET_CONFIGURATION => {
                if !Sink::seid_ok(m.params.first()) {
                    reject(m, vec![ERR_BAD_ACP_SEID])
                } else if self.state == StreamState::Idle {
                    reject(m, vec![ERR_BAD_STATE])
                } else {
                    accept(m, our_capabilities())
                }
            }
            SIG_OPEN => {
                if !Sink::seid_ok(m.params.first()) {
                    reject(m, vec![ERR_BAD_ACP_SEID])
                } else if self.state != StreamState::Configured {
                    reject(m, vec![ERR_BAD_STATE])
                } else {
                    self.state = StreamState::Open;
                    acts.push(Action::Opened);
                    accept(m, vec![])
                }
            }
            SIG_START => self.start_or_suspend(m, true, &mut acts),
            SIG_SUSPEND => self.start_or_suspend(m, false, &mut acts),
            SIG_CLOSE => {
                if !Sink::seid_ok(m.params.first()) {
                    reject(m, vec![ERR_BAD_ACP_SEID])
                } else if !matches!(self.state, StreamState::Open | StreamState::Streaming) {
                    reject(m, vec![ERR_BAD_STATE])
                } else {
                    self.end(&mut acts);
                    accept(m, vec![])
                }
            }
            SIG_ABORT => {
                // always accepted, from any state
                self.end(&mut acts);
                accept(m, vec![])
            }
            s => Msg { label: m.label, kind: MsgType::GeneralReject, signal: s, params: vec![] },
        };
        (Some(reply), acts)
    }

    fn end(&mut self, acts: &mut Vec<Action>) {
        if self.state != StreamState::Idle {
            acts.push(Action::Ended);
        }
        self.state = StreamState::Idle;
        self.config = None;
    }

    /// The link went away: same as an abort.
    pub fn link_lost(&mut self) -> Vec<Action> {
        let mut a = Vec::new();
        self.end(&mut a);
        a
    }

    fn set_configuration(&mut self, m: &Msg, acts: &mut Vec<Action>) -> Msg {
        if m.params.len() < 2 {
            return reject(m, vec![0, ERR_BAD_PAYLOAD_FORMAT]);
        }
        if !Sink::seid_ok(m.params.first()) {
            return reject(m, vec![0, ERR_BAD_ACP_SEID]);
        }
        if self.state != StreamState::Idle {
            return reject(m, vec![0, ERR_SEP_IN_USE]);
        }
        let (transport, sbc) = match parse_capabilities(&m.params[2..]) {
            Ok(x) => x,
            Err((cat, e)) => return reject(m, vec![cat, e]),
        };
        let (Some(info), true) = (sbc, transport) else { return reject(m, vec![CAT_MEDIA_CODEC, ERR_INVALID_CAPABILITIES]) };
        match info.to_params() {
            Ok(p) => {
                self.config = Some(p);
                self.state = StreamState::Configured;
                acts.push(Action::Configured(p));
                accept(m, vec![])
            }
            Err(e) => reject(m, vec![CAT_MEDIA_CODEC, e]),
        }
    }

    fn start_or_suspend(&mut self, m: &Msg, start: bool, acts: &mut Vec<Action>) -> Msg {
        if m.params.is_empty() {
            return reject(m, vec![0, ERR_BAD_PAYLOAD_FORMAT]);
        }
        if let Some(bad) = m.params.iter().find(|b| *b >> 2 != OUR_SEID) {
            return reject(m, vec![*bad, ERR_BAD_ACP_SEID]);
        }
        let ok = if start { self.state == StreamState::Open } else { self.state == StreamState::Streaming };
        if !ok {
            return reject(m, vec![m.params[0], ERR_BAD_STATE]);
        }
        if start {
            self.state = StreamState::Streaming;
            acts.push(Action::Started);
        } else {
            self.state = StreamState::Open;
            acts.push(Action::Suspended);
        }
        accept(m, vec![])
    }
}

/// The text for a reason the phone gave when it refused or ended something.
pub fn error_text(code: u8) -> &'static str {
    match code {
        ERR_BAD_STATE => "the phone and this computer disagree about the stream state",
        ERR_SEP_IN_USE => "the audio endpoint is already in use",
        ERR_BAD_ACP_SEID => "no such audio endpoint",
        _ => "the audio setup was refused",
    }
}

#[cfg(test)]
#[path = "avdtp_tests.rs"]
mod tests;

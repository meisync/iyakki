//! Bluetooth audio, the sending side (A2DP source: this computer plays through headphones or a speaker) and the codec names of the
//! other side's endpoints: the AVDTP signalling as the initiator, with no socket in this module (the sockets are in `a2dp_sys`).
//!
//! The steps are the specification's: Discover, Get All Capabilities for each audio sink endpoint until one offers SBC, Set
//! Configuration with settings chosen from what that endpoint offers, Open, then (the caller opens the media channel) Start. AAC and
//! aptX endpoints are recognised and named but not used: this computer encodes SBC only. The packetiser puts encoded SBC frames into
//! RTP packets the way the sink side of [`crate::a2dp`] reads them.
// SPDX-License-Identifier: Apache-2.0

use crate::avdtp::*;
use crate::sbc::Params;

/// A codec as the capabilities name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Sbc,
    Mpeg12Audio,
    Aac,
    AptX,
    AptXHd,
    Ldac,
    Vendor { vendor: u32, id: u16 },
    Other(u8),
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::Sbc => "SBC",
            Codec::Mpeg12Audio => "MP3",
            Codec::Aac => "AAC",
            Codec::AptX => "aptX",
            Codec::AptXHd => "aptX HD",
            Codec::Ldac => "LDAC",
            Codec::Vendor { .. } | Codec::Other(_) => "a codec of its own",
        }
    }

    /// Whether this computer can encode it (SBC only).
    pub fn we_can_encode(self) -> bool {
        self == Codec::Sbc
    }
}

/// The codec of a media codec capability element's body (media type and codec type, then the codec's own bytes). None when it is too
/// short or not audio.
pub fn codec_of(body: &[u8]) -> Option<Codec> {
    if body.len() < 2 || body[0] >> 4 != 0 {
        return None;
    }
    Some(match body[1] {
        0x00 => Codec::Sbc,
        0x01 => Codec::Mpeg12Audio,
        0x02 => Codec::Aac,
        0xff => {
            let v = body.get(2..8)?;
            let (vendor, id) = (u32::from_le_bytes([v[0], v[1], v[2], v[3]]), u16::from_le_bytes([v[4], v[5]]));
            match (vendor, id) {
                (0x004f, 0x0001) => Codec::AptX,
                (0x00d7, 0x0024) => Codec::AptXHd,
                (0x012d, 0x00aa) => Codec::Ldac,
                _ => Codec::Vendor { vendor, id },
            }
        }
        c => Codec::Other(c),
    })
}

/// What an AAC endpoint offers (the six information bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AacInfo {
    /// 0x80 MPEG-2 AAC LC, 0x40 MPEG-4 AAC LC, 0x20 MPEG-4 AAC LTP, 0x10 MPEG-4 AAC scalable
    pub object_types: u8,
    pub sample_rates: Vec<u32>,
    pub max_channels: u8,
    pub variable_bit_rate: bool,
    /// Bits per second, 0 when the endpoint does not say.
    pub bit_rate: u32,
}

pub fn parse_aac(b: &[u8]) -> Option<AacInfo> {
    if b.len() != 6 {
        return None;
    }
    let hi = [(0x80u8, 8000u32), (0x40, 11025), (0x20, 12000), (0x10, 16000), (0x08, 22050), (0x04, 24000), (0x02, 32000), (0x01, 44100)];
    let lo = [(0x80u8, 48000u32), (0x40, 64000), (0x20, 88200), (0x10, 96000)];
    let mut rates: Vec<u32> = hi.iter().filter(|(m, _)| b[1] & m != 0).map(|(_, r)| *r).collect();
    rates.extend(lo.iter().filter(|(m, _)| b[2] & m != 0).map(|(_, r)| *r));
    let channels = if b[2] & 0x04 != 0 { 2 } else if b[2] & 0x08 != 0 { 1 } else { 0 };
    Some(AacInfo { object_types: b[0], sample_rates: rates, max_channels: channels, variable_bit_rate: b[3] & 0x80 != 0, bit_rate: ((b[3] & 0x7f) as u32) << 16 | (b[4] as u32) << 8 | b[5] as u32 })
}

/// The categories of a capability list, each with its body. A list that does not add up is an error.
pub fn scan_capabilities(mut d: &[u8]) -> Result<Vec<(u8, &[u8])>, ()> {
    let mut out = Vec::new();
    while !d.is_empty() {
        if d.len() < 2 || d.len() < 2 + d[1] as usize {
            return Err(());
        }
        out.push((d[0], &d[2..2 + d[1] as usize]));
        d = &d[2 + d[1] as usize..];
    }
    Ok(out)
}

/// The settings to ask an SBC sink for: the best of what it offers within this computer's limits (44.1 kHz or 48 kHz, joint stereo,
/// 16 blocks, 8 subbands, loudness, bitpool up to 53 as the "high quality" setting of the specification). A single value per field.
pub fn choose_sbc(c: &SbcInfo) -> Result<SbcInfo, String> {
    let pick = |offered: u8, order: &[u8], what: &str| order.iter().copied().find(|b| offered & b != 0).ok_or_else(|| format!("the speaker offers no {what} this computer can send"));
    if c.min_bitpool > c.max_bitpool || c.max_bitpool < 2 {
        return Err("the speaker's SBC bitpool range makes no sense".into());
    }
    let min = c.min_bitpool.max(2);
    Ok(SbcInfo {
        freq: pick(c.freq, &[0x20, 0x10, 0x40, 0x80], "sampling frequency")?,
        mode: pick(c.mode, &[0x01, 0x02, 0x04, 0x08], "channel mode")?,
        blocks: pick(c.blocks, &[0x80, 0x40, 0x20, 0x10], "block length")?,
        subbands: pick(c.subbands, &[0x04, 0x08], "subband count")?,
        alloc: pick(c.alloc, &[0x01, 0x02], "allocation method")?,
        min_bitpool: min,
        max_bitpool: c.max_bitpool.min(53).max(min),
    })
}

/// What the initiator did, for the layer that owns the sockets and the audio.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SrcAction {
    /// The settings agreed on; the encoder is made from them.
    Configured(Params),
    /// The settings agreed on are AAC ones (only when the source was built [`Source::with_aac`]); the encoder comes from the add-on.
    ConfiguredAac(crate::a2dp_aac::AacConfig),
    /// The sink accepted Open: connect the media channel now, then call [`Source::start`].
    Opened,
    Started,
    Suspended,
    Ended,
    /// The sink told its delay, in tenths of a millisecond.
    Delay(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SrcError {
    /// The other device refused a command: its signal and the specification's code.
    Refused { signal: u8, code: u8 },
    /// No audio sink endpoint was found.
    NoSink,
    /// There are sink endpoints but none takes SBC; the names of the codecs they use.
    NoSbc(Vec<&'static str>),
    /// The answer made no sense.
    Garbled,
    /// The settings the speaker offered cannot be used.
    Unusable(String),
}

impl SrcError {
    /// A plain sentence for Settings and the assistant.
    pub fn text(&self) -> String {
        match self {
            SrcError::Refused { signal, code } => format!("the speaker refused ({}): {}", signal_name(*signal), error_text(*code)),
            SrcError::NoSink => "the device has no audio endpoint that takes sound".into(),
            SrcError::NoSbc(names) => format!("the device only takes {}, which this version cannot send; it needs SBC", if names.is_empty() { "other codecs".to_string() } else { names.join(" or ") }),
            SrcError::Garbled => "the device's answer could not be understood".into(),
            SrcError::Unusable(s) => s.clone(),
        }
    }
}

pub fn signal_name(s: u8) -> &'static str {
    match s {
        SIG_DISCOVER => "looking for endpoints",
        SIG_GET_CAPABILITIES | SIG_GET_ALL_CAPABILITIES => "asking what it can do",
        SIG_SET_CONFIGURATION => "choosing the audio settings",
        SIG_OPEN => "opening the stream",
        SIG_START => "starting the stream",
        SIG_SUSPEND => "pausing the stream",
        SIG_CLOSE => "closing the stream",
        _ => "an audio command",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrcState {
    Idle,
    Discovering,
    Capabilities,
    Configuring,
    Opening,
    Open,
    Starting,
    Streaming,
    Suspending,
    Closing,
}

/// The initiator of one stream to a sink.
pub struct Source {
    state: SrcState,
    label: u8,
    /// Our own endpoint identifier (we have one source endpoint).
    our_seid: u8,
    /// The sink endpoints still to ask, the one being asked first.
    candidates: Vec<u8>,
    skipped: Vec<&'static str>,
    pending: Option<(u8, u8)>,
    remote_seid: u8,
    chosen: Option<SbcInfo>,
    delay_agreed: bool,
    /// AAC may be used (the add-on that encodes it is installed): the capabilities of all endpoints are collected first and AAC wins.
    aac_on: bool,
    aac_pick: Option<AacPick>,
    sbc_pick: Option<(u8, SbcInfo, bool)>,
    chosen_aac: Option<crate::a2dp_aac::AacConfig>,
}

/// An AAC endpoint of the sink that can be used: its identifier, the settings to ask for, MPEG-2 or MPEG-4 object type, delay reporting.
#[derive(Clone, Copy)]
struct AacPick {
    seid: u8,
    cfg: crate::a2dp_aac::AacConfig,
    mpeg2: bool,
    delay: bool,
}

impl Default for Source {
    fn default() -> Self {
        Source::new()
    }
}

impl Source {
    pub fn new() -> Source {
        Source { state: SrcState::Idle, label: 0, our_seid: 1, candidates: vec![], skipped: vec![], pending: None, remote_seid: 0, chosen: None, delay_agreed: false, aac_on: false, aac_pick: None, sbc_pick: None, chosen_aac: None }
    }

    /// Lets the source choose AAC when the sink offers it (the caller does this only while AAC can be encoded, see
    /// [`crate::a2dp_aac::aac_ready`]); otherwise, and when no endpoint offers a usable AAC, SBC is chosen as before.
    pub fn with_aac(mut self) -> Source {
        self.aac_on = true;
        self
    }

    /// The AAC settings agreed on, once the stream is configured as an AAC one.
    pub fn chosen_aac(&self) -> Option<&crate::a2dp_aac::AacConfig> {
        self.chosen_aac.as_ref()
    }

    pub fn state(&self) -> SrcState {
        self.state
    }

    pub fn chosen(&self) -> Option<&SbcInfo> {
        self.chosen.as_ref()
    }

    fn send(&mut self, signal: u8, params: Vec<u8>) -> Msg {
        let label = self.label;
        self.label = (self.label + 1) & 0xf;
        self.pending = Some((label, signal));
        Msg { label, kind: MsgType::Command, signal, params }
    }

    /// The first command: Discover. Starts over from any state.
    pub fn begin(&mut self) -> Msg {
        *self = Source { label: self.label, aac_on: self.aac_on, ..Source::new() };
        self.state = SrcState::Discovering;
        self.send(SIG_DISCOVER, vec![])
    }

    /// Start, once the stream is open and the media channel is connected.
    pub fn start(&mut self) -> Option<Msg> {
        if self.state != SrcState::Open {
            return None;
        }
        self.state = SrcState::Starting;
        Some(self.send(SIG_START, vec![self.remote_seid << 2]))
    }

    pub fn suspend(&mut self) -> Option<Msg> {
        if self.state != SrcState::Streaming {
            return None;
        }
        self.state = SrcState::Suspending;
        Some(self.send(SIG_SUSPEND, vec![self.remote_seid << 2]))
    }

    pub fn close(&mut self) -> Option<Msg> {
        if !matches!(self.state, SrcState::Open | SrcState::Streaming) {
            return None;
        }
        self.state = SrcState::Closing;
        Some(self.send(SIG_CLOSE, vec![self.remote_seid << 2]))
    }

    /// The link was lost.
    pub fn link_lost(&mut self) -> Vec<SrcAction> {
        let was = self.state;
        self.state = SrcState::Idle;
        self.pending = None;
        if matches!(was, SrcState::Open | SrcState::Streaming | SrcState::Starting | SrcState::Suspending | SrcState::Closing | SrcState::Opening) { vec![SrcAction::Ended] } else { vec![] }
    }

    fn fail(&mut self, e: SrcError) -> Result<(Option<Msg>, Vec<SrcAction>), SrcError> {
        self.state = SrcState::Idle;
        self.pending = None;
        Err(e)
    }

    /// Takes one message from the sink; returns the next command to send, if any, and what the system should do.
    pub fn on_msg(&mut self, m: &Msg) -> Result<(Option<Msg>, Vec<SrcAction>), SrcError> {
        match m.kind {
            MsgType::Command => return Ok(self.on_command(m)),
            MsgType::GeneralReject => {
                let s = m.signal;
                return self.fail(SrcError::Refused { signal: s, code: ERR_NOT_SUPPORTED_COMMAND });
            }
            _ => {}
        }
        let Some((label, signal)) = self.pending else { return Ok((None, vec![])) };
        if m.label != label || m.signal != signal {
            return Ok((None, vec![]));
        }
        self.pending = None;
        if m.kind == MsgType::Reject {
            // Set Configuration rejects carry the category first, Start and Suspend the endpoint; the others the code alone
            let code = if matches!(signal, SIG_SET_CONFIGURATION | SIG_START | SIG_SUSPEND) { m.params.get(1) } else { m.params.first() }.copied().unwrap_or(0);
            // a reject of Start or Suspend leaves the stream as it was
            self.state = match (self.state, signal) {
                (SrcState::Starting, _) => SrcState::Open,
                (SrcState::Suspending, _) => SrcState::Streaming,
                (SrcState::Closing, _) => SrcState::Open,
                _ => SrcState::Idle,
            };
            return Err(SrcError::Refused { signal, code });
        }
        match (self.state, signal) {
            (SrcState::Discovering, SIG_DISCOVER) => {
                if m.params.len() % 2 != 0 {
                    return self.fail(SrcError::Garbled);
                }
                // seid, in-use flag; media type (audio 0) and endpoint type (sink 1) in the second byte
                self.candidates = m.params.chunks(2).filter(|e| e[0] & 0x02 == 0 && e[1] == 0x08).map(|e| e[0] >> 2).collect();
                self.next_candidate()
            }
            (SrcState::Capabilities, SIG_GET_ALL_CAPABILITIES) => self.on_capabilities(m),
            (SrcState::Configuring, SIG_SET_CONFIGURATION) if self.chosen_aac.is_some() => {
                self.state = SrcState::Opening;
                let c = self.chosen_aac.expect("checked");
                let open = self.send(SIG_OPEN, vec![self.remote_seid << 2]);
                Ok((Some(open), vec![SrcAction::ConfiguredAac(c)]))
            }
            (SrcState::Configuring, SIG_SET_CONFIGURATION) => {
                self.state = SrcState::Opening;
                let p = self.chosen.as_ref().and_then(|c| c.to_params().ok());
                let Some(p) = p else { return self.fail(SrcError::Garbled) };
                let open = self.send(SIG_OPEN, vec![self.remote_seid << 2]);
                Ok((Some(open), vec![SrcAction::Configured(p)]))
            }
            (SrcState::Opening, SIG_OPEN) => {
                self.state = SrcState::Open;
                Ok((None, vec![SrcAction::Opened]))
            }
            (SrcState::Starting, SIG_START) => {
                self.state = SrcState::Streaming;
                Ok((None, vec![SrcAction::Started]))
            }
            (SrcState::Suspending, SIG_SUSPEND) => {
                self.state = SrcState::Open;
                Ok((None, vec![SrcAction::Suspended]))
            }
            (SrcState::Closing, SIG_CLOSE) => {
                self.state = SrcState::Idle;
                Ok((None, vec![SrcAction::Ended]))
            }
            _ => Ok((None, vec![])),
        }
    }

    /// Asks the next endpoint what it can do, or ends the search.
    fn next_candidate(&mut self) -> Result<(Option<Msg>, Vec<SrcAction>), SrcError> {
        if self.candidates.is_empty() && (self.aac_pick.is_some() || self.sbc_pick.is_some()) {
            return self.configure_pick();
        }
        if self.candidates.is_empty() {
            let e = if self.skipped.is_empty() { SrcError::NoSink } else { SrcError::NoSbc(std::mem::take(&mut self.skipped)) };
            return self.fail(e);
        }
        self.remote_seid = self.candidates.remove(0);
        self.state = SrcState::Capabilities;
        let m = self.send(SIG_GET_ALL_CAPABILITIES, vec![self.remote_seid << 2]);
        Ok((Some(m), vec![]))
    }

    fn on_capabilities(&mut self, m: &Msg) -> Result<(Option<Msg>, Vec<SrcAction>), SrcError> {
        let Ok(cats) = scan_capabilities(&m.params) else { return self.fail(SrcError::Garbled) };
        let codec = cats.iter().find(|(c, _)| *c == 7).and_then(|(_, b)| codec_of(b));
        let transport = cats.iter().any(|(c, _)| *c == 1);
        let sbc = cats.iter().find(|(c, b)| *c == 7 && codec_of(b) == Some(Codec::Sbc)).and_then(|(_, b)| SbcInfo::from_bytes(&b[2..]));
        let delay = cats.iter().any(|(c, _)| *c == CAT_DELAY_REPORTING);
        if self.aac_on {
            // collect: the best AAC endpoint and the first SBC one; the choice is made when every endpoint has been asked
            if self.aac_pick.is_none() && transport {
                let aac = cats.iter().find(|(c, b)| *c == 7 && codec_of(b) == Some(Codec::Aac)).and_then(|(_, b)| b.get(2..)).and_then(parse_aac);
                if let Some(Ok((cfg, mpeg2))) = aac.as_ref().map(crate::a2dp_aac::choose_aac) {
                    self.aac_pick = Some(AacPick { seid: self.remote_seid, cfg, mpeg2, delay });
                }
            }
            if self.sbc_pick.is_none() && transport {
                if let Some(caps) = sbc {
                    self.sbc_pick = Some((self.remote_seid, caps, delay));
                }
            }
            if self.aac_pick.is_none() && self.sbc_pick.is_none() {
                if let Some(c) = codec {
                    self.skipped.push(c.name());
                }
            }
            return self.next_candidate();
        }
        let (Some(caps), true) = (sbc, transport) else {
            if let Some(c) = codec {
                self.skipped.push(c.name());
            }
            return self.next_candidate();
        };
        let cfg = match choose_sbc(&caps) {
            Ok(c) => c,
            Err(e) => return self.fail(SrcError::Unusable(e)),
        };
        self.chosen = Some(cfg);
        self.delay_agreed = cats.iter().any(|(c, _)| *c == CAT_DELAY_REPORTING);
        let mut p = vec![self.remote_seid << 2, self.our_seid << 2, 1, 0, 7, 6, 0, 0];
        p.extend_from_slice(&cfg.to_bytes());
        if self.delay_agreed {
            p.extend_from_slice(&[CAT_DELAY_REPORTING, 0]);
        }
        self.state = SrcState::Configuring;
        Ok((Some(self.send(SIG_SET_CONFIGURATION, p)), vec![]))
    }

    /// Every endpoint has been asked: AAC if one was usable, else the first SBC one.
    fn configure_pick(&mut self) -> Result<(Option<Msg>, Vec<SrcAction>), SrcError> {
        if let Some(a) = self.aac_pick.take() {
            self.remote_seid = a.seid;
            self.chosen_aac = Some(a.cfg);
            self.delay_agreed = a.delay;
            let mut p = vec![a.seid << 2, self.our_seid << 2, 1, 0, 7, 8, 0, crate::a2dp_aac::CODEC_AAC];
            p.extend_from_slice(&a.cfg.to_config_bytes(a.mpeg2));
            if a.delay {
                p.extend_from_slice(&[CAT_DELAY_REPORTING, 0]);
            }
            self.state = SrcState::Configuring;
            return Ok((Some(self.send(SIG_SET_CONFIGURATION, p)), vec![]));
        }
        let (seid, caps, delay) = self.sbc_pick.take().expect("one pick exists");
        self.remote_seid = seid;
        let cfg = match choose_sbc(&caps) {
            Ok(c) => c,
            Err(e) => return self.fail(SrcError::Unusable(e)),
        };
        self.chosen = Some(cfg);
        self.delay_agreed = delay;
        let mut p = vec![seid << 2, self.our_seid << 2, 1, 0, 7, 6, 0, 0];
        p.extend_from_slice(&cfg.to_bytes());
        if delay {
            p.extend_from_slice(&[CAT_DELAY_REPORTING, 0]);
        }
        self.state = SrcState::Configuring;
        Ok((Some(self.send(SIG_SET_CONFIGURATION, p)), vec![]))
    }

    /// Commands from the sink: a delay report, and the sink ending the stream.
    fn on_command(&mut self, m: &Msg) -> (Option<Msg>, Vec<SrcAction>) {
        let acc = |p: Vec<u8>| Msg { label: m.label, kind: MsgType::Accept, signal: m.signal, params: p };
        match m.signal {
            SIG_DELAY_REPORT if m.params.len() == 3 => {
                if self.delay_agreed && matches!(self.state, SrcState::Opening | SrcState::Open | SrcState::Starting | SrcState::Streaming | SrcState::Suspending) {
                    (Some(acc(vec![])), vec![SrcAction::Delay(u16::from_be_bytes([m.params[1], m.params[2]]))])
                } else {
                    (Some(Msg { label: m.label, kind: MsgType::Reject, signal: m.signal, params: vec![ERR_BAD_STATE] }), vec![])
                }
            }
            SIG_ABORT => {
                let ended = self.state != SrcState::Idle;
                self.state = SrcState::Idle;
                self.pending = None;
                (Some(acc(vec![])), if ended { vec![SrcAction::Ended] } else { vec![] })
            }
            SIG_CLOSE => {
                let open = matches!(self.state, SrcState::Open | SrcState::Streaming);
                if open {
                    self.state = SrcState::Idle;
                    (Some(acc(vec![])), vec![SrcAction::Ended])
                } else {
                    (Some(Msg { label: m.label, kind: MsgType::Reject, signal: m.signal, params: vec![ERR_BAD_STATE] }), vec![])
                }
            }
            s => (Some(Msg { label: m.label, kind: MsgType::GeneralReject, signal: s, params: vec![] }), vec![]),
        }
    }
}

// ------------------------------------------------------------------------------------------------ RTP

/// How many SBC frames fit in one media packet of at most `mtu` bytes (12 for the RTP header, 1 for the payload header); 1 to 15.
pub fn frames_per_packet(mtu: usize, frame_len: usize) -> usize {
    if frame_len == 0 {
        return 1;
    }
    (mtu.saturating_sub(13) / frame_len).clamp(1, 15)
}

/// Puts encoded SBC frames into RTP packets: payload type 96, a running sequence number, a timestamp that advances by the samples
/// of the frames sent.
pub struct Packetizer {
    seq: u16,
    ts: u32,
    ssrc: u32,
    samples_per_frame: u32,
}

impl Packetizer {
    pub fn new(ssrc: u32, p: &Params) -> Packetizer {
        Packetizer { seq: 0, ts: 0, ssrc, samples_per_frame: p.frame_samples() as u32 }
    }

    /// One packet with 1 to 15 whole frames.
    pub fn packet(&mut self, frames: &[Vec<u8>]) -> Vec<u8> {
        let n = frames.len().clamp(1, 15);
        let mut v = vec![0x80, 96];
        v.extend_from_slice(&self.seq.to_be_bytes());
        v.extend_from_slice(&self.ts.to_be_bytes());
        v.extend_from_slice(&self.ssrc.to_be_bytes());
        v.push(n as u8);
        for f in frames.iter().take(n) {
            v.extend_from_slice(f);
        }
        self.seq = self.seq.wrapping_add(1);
        self.ts = self.ts.wrapping_add(self.samples_per_frame * n as u32);
        v
    }
}

#[cfg(test)]
#[path = "avdtp_src_tests.rs"]
mod tests;

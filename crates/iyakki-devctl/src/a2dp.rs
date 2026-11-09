//! Bluetooth audio, the receiving side, continued (see [`crate::avdtp`] for the signalling): the pieces between the media channel and
//! the speakers, and the minimal SDP server that lets a phone find the audio sink. No socket in here.
//!
//! * [`sdp_respond`]: a minimal in-house SDP server for ONE record, the A2DP sink (service class 0x110B, AVDTP on PSM 0x19, profile
//!   0x110D). It answers `ServiceSearch`, `ServiceAttribute` and `ServiceSearchAttribute` requests, with continuation when the
//!   answer does not fit the size the phone asked for, and the SDP error response for anything it cannot answer.
//! * [`parse_rtp`], [`Reorder`], [`StreamDecoder`]: RTP and the SBC media payload (one or more whole frames, or one frame in
//!   fragments), put back into order, lost packets noticed, frames decoded with [`crate::sbc`].
//! * [`AudioPath`]: PCM into an [`Output`] (the real one is the agent's mixer source named after the device, so the person's volume,
//!   mute and ducking apply), opened when the stream starts and closed when it is suspended or ends.
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;

use crate::bthid::{parse_elem, Elem};
use crate::sbc::{parse_header, Decoder, Params};

// ------------------------------------------------------------------------------------------------ SDP server

/// The one record's handle.
pub const SDP_RECORD_HANDLE: u32 = 0x0001_0001;
const SDP_ERR_INVALID_HANDLE: u16 = 0x0002;
const SDP_ERR_INVALID_SYNTAX: u16 = 0x0003;
const SDP_ERR_INVALID_PDU_SIZE: u16 = 0x0004;
const SDP_ERR_INVALID_CONTINUATION: u16 = 0x0005;

fn e_uint16(v: u16) -> Vec<u8> {
    vec![0x09, (v >> 8) as u8, v as u8]
}
fn e_uint32(v: u32) -> Vec<u8> {
    let mut o = vec![0x0a];
    o.extend_from_slice(&v.to_be_bytes());
    o
}
fn e_uuid16(v: u16) -> Vec<u8> {
    vec![0x19, (v >> 8) as u8, v as u8]
}
fn e_seq(items: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = items.concat();
    let mut o = if body.len() < 256 { vec![0x35, body.len() as u8] } else { vec![0x36, (body.len() >> 8) as u8, body.len() as u8] };
    o.extend_from_slice(&body);
    o
}
fn e_text(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut o = vec![0x25, b.len().min(255) as u8];
    o.extend_from_slice(&b[..b.len().min(255)]);
    o
}

/// The attributes of the record: (id, encoded value), in id order.
pub fn sdp_record(name: &str) -> Vec<(u16, Vec<u8>)> {
    vec![
        (0x0000, e_uint32(SDP_RECORD_HANDLE)),
        (0x0001, e_seq(&[e_uuid16(0x110b)])),
        (0x0004, e_seq(&[e_seq(&[e_uuid16(0x0100), e_uint16(0x0019)]), e_seq(&[e_uuid16(0x0019), e_uint16(0x0103)])])),
        (0x0005, e_seq(&[e_uuid16(0x1002)])),
        (0x0009, e_seq(&[e_seq(&[e_uuid16(0x110d), e_uint16(0x0103)])])),
        (0x0100, e_text(name)),
        // supported features: speaker
        (0x0311, e_uint16(0x0002)),
    ]
}

/// The UUIDs the record contains (a search pattern matches when all its UUIDs are in here).
const RECORD_UUIDS: [u16; 5] = [0x110b, 0x110d, 0x0100, 0x0019, 0x1002];

fn sdp_error(tid: u16, code: u16) -> Vec<u8> {
    let mut v = vec![0x01];
    v.extend_from_slice(&tid.to_be_bytes());
    v.extend_from_slice(&2u16.to_be_bytes());
    v.extend_from_slice(&code.to_be_bytes());
    v
}

fn sdp_pdu(id: u8, tid: u16, params: &[u8]) -> Vec<u8> {
    let mut v = vec![id];
    v.extend_from_slice(&tid.to_be_bytes());
    v.extend_from_slice(&(params.len() as u16).to_be_bytes());
    v.extend_from_slice(params);
    v
}

fn pattern_matches(e: &Elem) -> Option<bool> {
    let list = e.seq()?;
    if list.is_empty() || list.len() > 12 {
        return None;
    }
    let mut all = true;
    for u in list {
        match u.uuid16() {
            Some(u) => all &= RECORD_UUIDS.contains(&u),
            // a UUID that is not on the Bluetooth base cannot be in this record
            None if matches!(u, Elem::Uuid(_)) => all = false,
            None => return None,
        }
    }
    Some(all)
}

/// The attribute id ranges of an `AttributeIDList` (raw bytes of the sequence): a 16-bit item is one attribute, a 32-bit item is a
/// range (high half to low half).
fn id_ranges(raw: &[u8]) -> Option<Vec<(u16, u16)>> {
    let (head, len) = match *raw.first()? {
        0x35 => (2, *raw.get(1)? as usize),
        0x36 => (3, u16::from_be_bytes([*raw.get(1)?, *raw.get(2)?]) as usize),
        _ => return None,
    };
    let mut body = raw.get(head..head + len)?;
    let mut out = Vec::new();
    while !body.is_empty() {
        if out.len() >= 32 {
            return None;
        }
        match body[0] {
            0x09 if body.len() >= 3 => {
                let v = u16::from_be_bytes([body[1], body[2]]);
                out.push((v, v));
                body = &body[3..];
            }
            0x0a if body.len() >= 5 => {
                out.push((u16::from_be_bytes([body[1], body[2]]), u16::from_be_bytes([body[3], body[4]])));
                body = &body[5..];
            }
            _ => return None,
        }
    }
    Some(out)
}

/// `AttributeList` for the requested ids: a sequence of id / value pairs in id order.
fn attribute_list(name: &str, ranges: &[(u16, u16)]) -> Vec<u8> {
    let mut items = Vec::new();
    for (id, val) in sdp_record(name) {
        if ranges.iter().any(|(a, b)| id >= *a && id <= *b) {
            items.push(e_uint16(id));
            items.push(val);
        }
    }
    e_seq(&items)
}

/// The continuation state of a request: where in the full answer to go on from.
fn continuation(rest: &[u8]) -> Result<usize, u16> {
    match rest {
        [0] => Ok(0),
        [2, a, b] => Ok(u16::from_be_bytes([*a, *b]) as usize),
        _ => Err(SDP_ERR_INVALID_CONTINUATION),
    }
}

/// One response chunk of `full` for a request that allows `max` bytes of data, starting at `offset`, with its continuation state.
fn chunk(full: &[u8], offset: usize, max: usize) -> Result<(Vec<u8>, Vec<u8>), u16> {
    if offset > full.len() || (offset > 0 && offset == full.len()) {
        return Err(SDP_ERR_INVALID_CONTINUATION);
    }
    let end = full.len().min(offset + max.max(7));
    let more = end < full.len();
    let cont = if more { vec![2, (end >> 8) as u8, end as u8] } else { vec![0] };
    Ok((full[offset..end].to_vec(), cont))
}

/// Answers one SDP request PDU for the A2DP sink record named `name`. Always returns a PDU (an error response for a bad request).
pub fn sdp_respond(req: &[u8], name: &str) -> Vec<u8> {
    if req.len() < 5 {
        return sdp_error(0, SDP_ERR_INVALID_PDU_SIZE);
    }
    let (id, tid, plen) = (req[0], u16::from_be_bytes([req[1], req[2]]), u16::from_be_bytes([req[3], req[4]]) as usize);
    if req.len() != 5 + plen {
        return sdp_error(tid, SDP_ERR_INVALID_PDU_SIZE);
    }
    let p = &req[5..];
    match sdp_answer(id, tid, p, name) {
        Ok(v) => v,
        Err(code) => sdp_error(tid, code),
    }
}

fn sdp_answer(id: u8, tid: u16, p: &[u8], name: &str) -> Result<Vec<u8>, u16> {
    let bad = |_| SDP_ERR_INVALID_SYNTAX;
    match id {
        // ServiceSearchRequest: pattern, maximum record count, continuation
        0x02 => {
            let (pat, n) = parse_elem(p).map_err(bad)?;
            let rest = p.get(n..).filter(|r| r.len() >= 3).ok_or(SDP_ERR_INVALID_SYNTAX)?;
            let max = u16::from_be_bytes([rest[0], rest[1]]);
            continuation(&rest[2..])?;
            let hit = pattern_matches(&pat).ok_or(SDP_ERR_INVALID_SYNTAX)? && max > 0;
            let mut v = Vec::new();
            let c = hit as u16;
            v.extend_from_slice(&c.to_be_bytes());
            v.extend_from_slice(&c.to_be_bytes());
            if hit {
                v.extend_from_slice(&SDP_RECORD_HANDLE.to_be_bytes());
            }
            v.push(0);
            Ok(sdp_pdu(0x03, tid, &v))
        }
        // ServiceAttributeRequest: handle, maximum byte count, attribute id list, continuation
        0x04 => {
            if p.len() < 6 {
                return Err(SDP_ERR_INVALID_SYNTAX);
            }
            let handle = u32::from_be_bytes([p[0], p[1], p[2], p[3]]);
            let max = u16::from_be_bytes([p[4], p[5]]) as usize;
            let (_ids, n) = parse_elem(&p[6..]).map_err(bad)?;
            let ids_raw = &p[6..6 + n];
            let off = continuation(&p[6 + n..])?;
            if handle != SDP_RECORD_HANDLE {
                return Err(SDP_ERR_INVALID_HANDLE);
            }
            let full = attribute_list(name, &id_ranges(ids_raw).ok_or(SDP_ERR_INVALID_SYNTAX)?);
            let (data, cont) = chunk(&full, off, max)?;
            let mut v = (data.len() as u16).to_be_bytes().to_vec();
            v.extend_from_slice(&data);
            v.extend_from_slice(&cont);
            Ok(sdp_pdu(0x05, tid, &v))
        }
        // ServiceSearchAttributeRequest: pattern, maximum byte count, attribute id list, continuation
        0x06 => {
            let (pat, n) = parse_elem(p).map_err(bad)?;
            let rest = p.get(n..).filter(|r| r.len() >= 2).ok_or(SDP_ERR_INVALID_SYNTAX)?;
            let max = u16::from_be_bytes([rest[0], rest[1]]) as usize;
            let (_ids, m) = parse_elem(&rest[2..]).map_err(bad)?;
            let ids_raw = &rest[2..2 + m];
            let off = continuation(&rest[2 + m..])?;
            let hit = pattern_matches(&pat).ok_or(SDP_ERR_INVALID_SYNTAX)?;
            let ranges = id_ranges(ids_raw).ok_or(SDP_ERR_INVALID_SYNTAX)?;
            let full = if hit { e_seq(&[attribute_list(name, &ranges)]) } else { e_seq(&[]) };
            let (data, cont) = chunk(&full, off, max)?;
            let mut v = (data.len() as u16).to_be_bytes().to_vec();
            v.extend_from_slice(&data);
            v.extend_from_slice(&cont);
            Ok(sdp_pdu(0x07, tid, &v))
        }
        _ => Err(SDP_ERR_INVALID_SYNTAX),
    }
}

// ------------------------------------------------------------------------------------------------ RTP

/// A parsed RTP packet.
#[derive(Debug, PartialEq, Eq)]
pub struct Rtp<'a> {
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: &'a [u8],
}

/// Reads the RTP header (version 2, CSRC list, extension and padding handled).
pub fn parse_rtp(d: &[u8]) -> Result<Rtp<'_>, &'static str> {
    if d.len() < 12 {
        return Err("RTP packet is cut");
    }
    if d[0] >> 6 != 2 {
        return Err("not RTP version 2");
    }
    let (pad, ext, cc) = (d[0] & 0x20 != 0, d[0] & 0x10 != 0, (d[0] & 0x0f) as usize);
    let mut at = 12 + 4 * cc;
    if ext {
        let words = u16::from_be_bytes([*d.get(at + 2).ok_or("RTP extension is cut")?, *d.get(at + 3).ok_or("RTP extension is cut")?]) as usize;
        at += 4 + 4 * words;
    }
    let mut end = d.len();
    if pad {
        let n = *d.last().ok_or("RTP padding is cut")? as usize;
        end = end.checked_sub(n).ok_or("RTP padding is longer than the packet")?;
    }
    if at >= end {
        return Err("RTP packet has no payload");
    }
    Ok(Rtp { seq: u16::from_be_bytes([d[2], d[3]]), timestamp: u32::from_be_bytes([d[4], d[5], d[6], d[7]]), ssrc: u32::from_be_bytes([d[8], d[9], d[10], d[11]]), payload: &d[at..end] })
}

/// The SBC media payload header: fragmentation flags and the frame count.
#[derive(Debug, PartialEq, Eq)]
pub struct SbcPayload<'a> {
    pub fragmented: bool,
    pub start: bool,
    pub last: bool,
    pub frames: u8,
    pub data: &'a [u8],
}

pub fn parse_sbc_payload(p: &[u8]) -> Result<SbcPayload<'_>, &'static str> {
    let h = *p.first().ok_or("empty media payload")?;
    Ok(SbcPayload { fragmented: h & 0x80 != 0, start: h & 0x40 != 0, last: h & 0x20 != 0, frames: h & 0x0f, data: &p[1..] })
}

/// Cuts `count` whole frames out of `data` (each frame's length comes from its own header). A cut or garbled payload is an error.
pub fn split_frames(data: &[u8], count: usize) -> Result<Vec<&[u8]>, &'static str> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() && (count == 0 || out.len() < count) {
        let p = parse_header(&data[at..]).map_err(|_| "media payload does not hold an audio frame")?;
        let n = p.frame_len();
        if at + n > data.len() {
            return Err("audio frame is cut");
        }
        out.push(&data[at..at + n]);
        at += n;
    }
    if out.is_empty() {
        return Err("no audio frame in the media payload");
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ order and loss

/// What comes out of the reorder buffer.
#[derive(Debug, PartialEq, Eq)]
pub enum Ordered {
    Packet(Vec<u8>),
    /// This many packets never arrived (or arrived too late) and are given up on.
    Lost(u32),
}

/// Puts packets back in sequence order. A packet that is behind a gap waits for the missing ones until [`Reorder::MAX_HELD`] later packets
/// are held; then the gap is declared lost. Packets older than the next expected one (late or doubled) are dropped.
#[derive(Default)]
pub struct Reorder {
    next: Option<u16>,
    held: Vec<(u16, Vec<u8>)>,
    pub late: u64,
}

impl Reorder {
    pub const MAX_HELD: usize = 4;
    /// A jump bigger than this is a restarted stream, not a gap.
    const RESYNC: i16 = 512;

    pub fn push(&mut self, seq: u16, payload: Vec<u8>) -> Vec<Ordered> {
        let mut out = Vec::new();
        let Some(next) = self.next else {
            self.next = Some(seq.wrapping_add(1));
            out.push(Ordered::Packet(payload));
            return out;
        };
        let diff = seq.wrapping_sub(next) as i16;
        if diff < 0 {
            if diff < -Self::RESYNC {
                // the sender started again
                self.held.clear();
                self.next = Some(seq.wrapping_add(1));
                out.push(Ordered::Packet(payload));
            } else {
                self.late += 1;
            }
            return out;
        }
        if diff > Self::RESYNC {
            self.held.clear();
            self.next = Some(seq.wrapping_add(1));
            out.push(Ordered::Packet(payload));
            return out;
        }
        if diff > 0 {
            if self.held.iter().any(|(s, _)| *s == seq) {
                self.late += 1;
                return out;
            }
            self.held.push((seq, payload));
            if self.held.len() > Self::MAX_HELD {
                // give up on the gap: jump to the oldest held packet
                let first = self.held.iter().map(|(s, _)| s.wrapping_sub(next)).min().unwrap_or(0);
                out.push(Ordered::Lost(first as u32));
                self.next = Some(next.wrapping_add(first));
                self.drain(&mut out);
            }
            return out;
        }
        out.push(Ordered::Packet(payload));
        self.next = Some(next.wrapping_add(1));
        self.drain(&mut out);
        out
    }

    fn drain(&mut self, out: &mut Vec<Ordered>) {
        while let Some(next) = self.next {
            match self.held.iter().position(|(s, _)| *s == next) {
                Some(i) => {
                    out.push(Ordered::Packet(self.held.remove(i).1));
                    self.next = Some(next.wrapping_add(1));
                }
                None => break,
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ the stream

/// Decoded audio.
#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub rate: u32,
    pub channels: usize,
    /// interleaved
    pub pcm: Vec<i16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    pub packets: u64,
    pub frames: u64,
    pub lost_packets: u64,
    pub late_packets: u64,
    pub bad_packets: u64,
    pub bad_frames: u64,
}

/// RTP packets in, PCM out.
pub struct StreamDecoder {
    dec: Decoder,
    reorder: Reorder,
    frag: Option<Vec<u8>>,
    last: Option<(Params, usize)>,
    pub stats: StreamStats,
}

impl Default for StreamDecoder {
    fn default() -> Self {
        StreamDecoder::new()
    }
}

/// A fragmented frame is never longer than this.
const MAX_FRAGMENTED: usize = 4096;

impl StreamDecoder {
    pub fn new() -> StreamDecoder {
        StreamDecoder { dec: Decoder::new(), reorder: Reorder::default(), frag: None, last: None, stats: StreamStats::default() }
    }

    /// The codec settings seen in the last decoded frame.
    pub fn params(&self) -> Option<Params> {
        self.last.map(|l| l.0)
    }

    /// One media packet (as the media channel gives it). Returns the audio that became ready (several blocks after a gap is filled).
    pub fn on_media(&mut self, pkt: &[u8]) -> Vec<Block> {
        let mut blocks = Vec::new();
        let Ok(rtp) = parse_rtp(pkt) else {
            self.stats.bad_packets += 1;
            return blocks;
        };
        self.stats.packets += 1;
        for o in self.reorder.push(rtp.seq, rtp.payload.to_vec()) {
            match o {
                Ordered::Lost(n) => {
                    self.stats.lost_packets += n as u64;
                    // silence of the same length keeps the sound in time with the video / the phone's clock
                    if let Some((p, per)) = self.last {
                        let samples = p.frame_samples() * per * n as usize;
                        blocks.push(Block { rate: p.sample_rate(), channels: p.channels(), pcm: vec![0; samples * p.channels()] });
                    }
                }
                Ordered::Packet(payload) => self.payload(&payload, &mut blocks),
            }
        }
        self.stats.late_packets = self.reorder.late;
        blocks
    }

    fn payload(&mut self, payload: &[u8], blocks: &mut Vec<Block>) {
        let Ok(h) = parse_sbc_payload(payload) else {
            self.stats.bad_packets += 1;
            return;
        };
        let frames: Vec<Vec<u8>> = if h.fragmented {
            if h.start {
                self.frag = Some(h.data.to_vec());
            } else if let Some(f) = self.frag.as_mut() {
                f.extend_from_slice(h.data);
            }
            if self.frag.as_ref().is_some_and(|f| f.len() > MAX_FRAGMENTED) {
                self.frag = None;
                self.stats.bad_packets += 1;
            }
            if h.last { self.frag.take().into_iter().collect() } else { vec![] }
        } else {
            self.frag = None;
            match split_frames(h.data, h.frames as usize) {
                Ok(v) => v.into_iter().map(|f| f.to_vec()).collect(),
                Err(_) => {
                    self.stats.bad_packets += 1;
                    vec![]
                }
            }
        };
        let per = frames.len().max(1);
        for f in frames {
            match self.dec.decode(&f) {
                Ok(d) => {
                    self.stats.frames += 1;
                    self.last = Some((d.params, per));
                    blocks.push(Block { rate: d.params.sample_rate(), channels: d.params.channels(), pcm: d.pcm });
                }
                Err(_) => {
                    // a damaged frame is one frame of silence, not the end of the stream
                    self.stats.bad_frames += 1;
                    if let Some((p, _)) = self.last {
                        blocks.push(Block { rate: p.sample_rate(), channels: p.channels(), pcm: vec![0; p.frame_samples() * p.channels()] });
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ the audio path

/// A place for decoded sound (the real one is a source of the agent's mixer).
pub trait Output: Send {
    fn write(&mut self, pcm: &[i16]) -> Result<(), String>;
}

/// Opens an [`Output`] for a rate and channel count; `label` is the name the person sees ("playing from <device>").
pub trait OutputFactory: Send {
    fn open(&mut self, rate: u32, channels: usize, label: &str) -> Result<Box<dyn Output>, String>;
}

/// Decodes the media of one connected device into its output. The output is opened when the stream starts and closed when it is
/// suspended or ends; if the rate or channels change mid-stream it is opened again.
pub struct AudioPath {
    factory: Box<dyn OutputFactory>,
    label: String,
    out: Option<(u32, usize, Box<dyn Output>)>,
    decoder: StreamDecoder,
    playing: bool,
    pub error: Option<String>,
    queue: VecDeque<Block>,
}

impl AudioPath {
    pub fn new(factory: Box<dyn OutputFactory>, label: &str) -> AudioPath {
        AudioPath { factory, label: label.to_string(), out: None, decoder: StreamDecoder::new(), playing: false, error: None, queue: VecDeque::new() }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn playing(&self) -> bool {
        self.playing && self.out.is_some()
    }

    pub fn stats(&self) -> &StreamStats {
        &self.decoder.stats
    }

    pub fn params(&self) -> Option<Params> {
        self.decoder.params()
    }

    /// The stream started (AVDTP `Start`): media packets are played from now on.
    pub fn start(&mut self) {
        self.playing = true;
        self.error = None;
        self.decoder = StreamDecoder::new();
    }

    /// Suspended, closed or the link went away: the output ends at once so the speaker is free for other sounds.
    pub fn stop(&mut self) {
        self.playing = false;
        self.out = None;
        self.queue.clear();
    }

    /// One packet from the media channel. Dropped while not started (a phone sometimes sends a packet before `Start` is answered).
    pub fn on_media(&mut self, pkt: &[u8]) {
        if !self.playing {
            return;
        }
        for b in self.decoder.on_media(pkt) {
            self.queue.push_back(b);
        }
        while let Some(b) = self.queue.pop_front() {
            if b.pcm.is_empty() {
                continue;
            }
            let reopen = !matches!(&self.out, Some((r, c, _)) if *r == b.rate && *c == b.channels);
            if reopen {
                self.out = None;
                match self.factory.open(b.rate, b.channels, &self.label) {
                    Ok(o) => self.out = Some((b.rate, b.channels, o)),
                    Err(e) => {
                        self.error = Some(e);
                        self.queue.clear();
                        return;
                    }
                }
            }
            if let Some((_, _, o)) = self.out.as_mut() {
                if let Err(e) = o.write(&b.pcm) {
                    self.error = Some(e);
                    self.out = None;
                    self.queue.clear();
                    return;
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ what the pages and tools see

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioState {
    /// Connected, no sound at the moment (paused or not started).
    Connected,
    Playing,
}

/// The phone or laptop that is connected to the audio sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceView {
    pub address: String,
    pub name: String,
    pub state: AudioState,
    /// "SBC 44.1 kHz joint stereo" once the phone has chosen
    pub codec: String,
    /// why the sound stopped (the speaker could not be opened ...), when it did
    pub error: Option<String>,
    pub packets: u64,
    pub lost: u64,
}

impl AudioDeviceView {
    pub fn title(&self) -> String {
        if self.name.is_empty() { self.address.clone() } else { self.name.clone() }
    }
}

/// Bluetooth audio as one view: whether the sink is listening, whether pairing is open, who is connected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioView {
    /// the sink answers phones (the sockets are open)
    pub ready: bool,
    /// the plain reason it is not ready
    pub why_not: String,
    /// a phone may pair with this computer now
    pub pairing_open: bool,
    pub device: Option<AudioDeviceView>,
}

impl AudioView {
    /// One sentence for a page or the assistant.
    pub fn line(&self) -> String {
        if let Some(d) = &self.device {
            let base = match d.state {
                AudioState::Playing => format!("playing from {}", d.title()),
                AudioState::Connected => format!("{} is connected, nothing is playing", d.title()),
            };
            let codec = if d.codec.is_empty() { String::new() } else { format!(" ({})", d.codec) };
            return match &d.error {
                Some(e) => format!("{base}{codec}; problem: {e}"),
                None => format!("{base}{codec}"),
            };
        }
        if !self.ready {
            return format!("not available: {}", self.why_not);
        }
        if self.pairing_open {
            "waiting for a phone: it is discoverable now, choose this computer in the phone's Bluetooth list".into()
        } else {
            "ready: a paired phone can play through this computer; to pair a new one, let a phone pair".into()
        }
    }
}

/// The text for the codec of a stream.
pub fn codec_text(p: &Params) -> String {
    use crate::sbc::ChannelMode::*;
    let mode = match p.mode {
        Mono => "mono",
        Dual => "dual channel",
        Stereo => "stereo",
        Joint => "joint stereo",
    };
    format!("SBC {} kHz {mode}", p.sample_rate() as f64 / 1000.0)
}

/// What Settings and the assistant use to look at and steer Bluetooth audio. The real one is in `a2dp_sys`; tests use a fake.
pub trait AudioControl: Send + Sync {
    fn view(&self) -> AudioView;
    /// Connects to a paired phone or laptop (it then starts the audio set-up by itself). Returns at once; `view` shows the result.
    fn connect(&self, address: &str) -> Result<(), String>;
    /// Ends the audio connection.
    fn disconnect(&self) -> Result<(), String>;
    /// Opens or closes pairing for phones that start it themselves.
    fn pairing_open(&self, on: bool) -> Result<(), String>;
}

#[cfg(test)]
#[path = "a2dp_tests.rs"]
mod tests;

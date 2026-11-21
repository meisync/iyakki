//! Bluetooth audio with AAC (MPEG-2/4 AAC-LC), through the person's ffmpeg add-on. Nothing here decodes or encodes AAC: this module
//! holds what needs no codebook tables (the A2DP capability bytes, the choice of a setting, the LATM framing the phone sends and
//! expects, ADTS headers and the cutting of an ADTS stream into frames, RTP for AAC, reordering and loss as silence) and the three
//! small traits a real coder plugs into ([`AacBackend`], [`AacDecode`], [`AacEncode`]). The agent registers the real backend (ffmpeg,
//! started with fixed arguments, no shell, only when the person has installed the audio and video add-on); without one no AAC
//! endpoint is offered and no AAC is chosen, so SBC is used as before.
//!
//! Wire facts (A2DP 1.3, MPEG-2,4 AAC): a media payload is one `AudioMuxElement` of LATM with `muxConfigPresent = 0`: the payload
//! length as a run of 0xFF bytes ending in a byte below 0xFF, then the raw AAC frame. One element per RTP packet, 1024 samples each.
// SPDX-License-Identifier: Apache-2.0

use std::sync::{Arc, Mutex};

use crate::a2dp::{parse_rtp, Block, Ordered, Reorder, StreamStats};
use crate::avdtp_src::{parse_aac, AacInfo};

/// The A2DP codec type of MPEG-2,4 AAC.
pub const CODEC_AAC: u8 = 0x02;
/// Samples in one AAC frame.
pub const FRAME_SAMPLES: usize = 1024;
/// The service endpoint identifier of our AAC sink endpoint (the SBC one is [`crate::avdtp::OUR_SEID`]).
pub const AAC_SEID: u8 = 2;

/// What the person is told when the add-on that does AAC is not installed.
pub const NEEDS_ADDON: &str = "Bluetooth AAC needs the audio and video add-on (ffmpeg), which is not installed: SBC is used. Install the add-on in Store to use AAC.";

/// The settings of one AAC stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AacConfig {
    pub rate: u32,
    pub channels: u8,
    /// Bits per second the encoder is asked for (the source side).
    pub bit_rate: u32,
}

const RATES: [u32; 12] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000];

/// The index of a sampling rate in the table of the AAC header, None for a rate the table does not have.
pub fn rate_index(rate: u32) -> Option<u8> {
    RATES.iter().position(|r| *r == rate).map(|i| i as u8)
}

impl AacConfig {
    /// The sample rates of one frame's worth of time.
    pub fn frame_samples(&self) -> usize {
        FRAME_SAMPLES
    }

    /// The six information bytes of a configuration (one object type, one rate, one channel count).
    pub fn to_config_bytes(&self, mpeg2: bool) -> [u8; 6] {
        let obj = if mpeg2 { 0x80 } else { 0x40 };
        let (b1, b2r) = match self.rate {
            44100 => (0x01, 0x00),
            48000 => (0x00, 0x80),
            _ => (0, 0),
        };
        let ch = if self.channels == 1 { 0x08 } else { 0x04 };
        [obj, b1, b2r | ch, (self.bit_rate >> 16) as u8 & 0x7f, (self.bit_rate >> 8) as u8, self.bit_rate as u8]
    }

    /// Reads a configuration the other side sent. The error is the specification's code to refuse it with.
    pub fn from_config_bytes(b: &[u8]) -> Result<AacConfig, u8> {
        use crate::avdtp::*;
        if b.len() != 6 {
            return Err(ERR_BAD_PAYLOAD_FORMAT);
        }
        if !matches!(b[0], 0x80 | 0x40) {
            return Err(ERR_INVALID_OBJECT_TYPE);
        }
        let rate = match (b[1], b[2] & 0xf0) {
            (0x01, 0x00) => 44100,
            (0x00, 0x80) => 48000,
            _ => return Err(ERR_INVALID_SAMPLING_FREQUENCY),
        };
        let channels = match b[2] & 0x0f {
            0x08 => 1,
            0x04 => 2,
            _ => return Err(ERR_INVALID_CHANNELS),
        };
        if b[3] & 0x80 != 0 {
            return Err(ERR_INVALID_CAPABILITIES);
        }
        let bit_rate = ((b[3] & 0x7f) as u32) << 16 | (b[4] as u32) << 8 | b[5] as u32;
        Ok(AacConfig { rate, channels, bit_rate })
    }
}

/// The text for the codec of an AAC stream.
pub fn codec_text(c: &AacConfig) -> String {
    format!("AAC {} kHz {}", c.rate as f64 / 1000.0, if c.channels == 1 { "mono" } else { "stereo" })
}

/// The capabilities our AAC sink endpoint offers: AAC-LC (MPEG-2 and MPEG-4), 44.1 and 48 kHz, mono and stereo, variable bit rate, any
/// bit rate (the six information bytes).
pub fn sink_capability() -> [u8; 6] {
    [0xc0, 0x01, 0x8c, 0x80, 0, 0]
}

/// The settings to ask an AAC sink for: AAC-LC, 48 kHz if offered (else 44.1), stereo if offered, a bit rate of at most 128 kbit/s.
pub fn choose_aac(c: &AacInfo) -> Result<(AacConfig, bool), String> {
    let mpeg2 = c.object_types & 0x80 != 0;
    if !mpeg2 && c.object_types & 0x40 == 0 {
        return Err("the headphones offer AAC but not the AAC-LC kind this computer sends".into());
    }
    let rate = [48000, 44100].into_iter().find(|r| c.sample_rates.contains(r)).ok_or("the headphones offer no sampling rate of AAC this computer can send")?;
    if c.max_channels == 0 {
        return Err("the headphones offer AAC with no channel count this computer can send".into());
    }
    let channels = if c.max_channels >= 2 { 2 } else { 1 };
    let bit_rate = if c.bit_rate == 0 { 128_000 } else { c.bit_rate.min(128_000) };
    Ok((AacConfig { rate, channels, bit_rate }, mpeg2))
}

/// The capability element of the endpoint (category 7: media codec, audio, AAC, six bytes).
pub fn sink_codec_element() -> Vec<u8> {
    let mut v = vec![7, 8, 0, CODEC_AAC];
    v.extend_from_slice(&sink_capability());
    v
}

/// Whether capability bytes (as [`parse_aac`] reads them) are a valid offer of AAC.
pub fn offer_of(b: &[u8]) -> Option<AacInfo> {
    parse_aac(b)
}

// ------------------------------------------------------------------------------------------------ framing

/// One raw AAC frame as the A2DP payload (LATM, no in-band configuration).
pub fn latm_wrap(frame: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(frame.len() + frame.len() / 255 + 1);
    let mut n = frame.len();
    while n >= 255 {
        v.push(0xff);
        n -= 255;
    }
    v.push(n as u8);
    v.extend_from_slice(frame);
    v
}

/// The raw AAC frame inside an A2DP payload. Refuses a payload that is shorter than it says.
pub fn latm_unwrap(p: &[u8]) -> Result<&[u8], &'static str> {
    let mut len = 0usize;
    let mut at = 0usize;
    loop {
        let b = *p.get(at).ok_or("the AAC payload ends inside its length")?;
        at += 1;
        len += b as usize;
        if b != 0xff {
            break;
        }
        if at > 8 {
            return Err("the AAC payload length is not believable");
        }
    }
    if len == 0 {
        return Err("the AAC payload holds no frame");
    }
    p.get(at..at + len).ok_or("the AAC payload is shorter than it says")
}

/// The seven-byte ADTS header for a frame of `len` payload bytes (AAC-LC, no CRC).
pub fn adts_header(cfg: &AacConfig, len: usize) -> Option<[u8; 7]> {
    let sf = rate_index(cfg.rate)?;
    if !(1..=2).contains(&cfg.channels) {
        return None;
    }
    let total = len + 7;
    if total > 0x1fff {
        return None;
    }
    let ch = cfg.channels;
    Some([
        0xff,
        0xf1,
        0x40 | sf << 2 | (ch >> 2) & 1,
        (ch & 3) << 6 | (total >> 11) as u8 & 3,
        (total >> 3) as u8,
        ((total & 7) as u8) << 5 | 0x1f,
        0xfc,
    ])
}

/// A raw frame with its ADTS header.
pub fn adts_wrap(cfg: &AacConfig, frame: &[u8]) -> Option<Vec<u8>> {
    let mut v = adts_header(cfg, frame.len())?.to_vec();
    v.extend_from_slice(frame);
    Some(v)
}

/// Cuts whole raw frames (headers removed) off the front of an ADTS byte stream: junk before a header is skipped, an unfinished frame
/// is left in `buf`.
pub fn adts_take_frames(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let Some(s) = buf.windows(2).position(|w| w[0] == 0xff && w[1] & 0xf6 == 0xf0) else {
            // keep a trailing 0xff: it may be the first byte of the next header
            let keep = buf.last() == Some(&0xff);
            let n = buf.len() - keep as usize;
            buf.drain(..n);
            return out;
        };
        buf.drain(..s);
        if buf.len() < 7 {
            return out;
        }
        let hdr = if buf[1] & 1 == 0 { 9 } else { 7 };
        let len = ((buf[3] as usize & 3) << 11) | (buf[4] as usize) << 3 | (buf[5] as usize) >> 5;
        if len < hdr + 1 {
            buf.drain(..2);
            continue;
        }
        if buf.len() < len {
            return out;
        }
        out.push(buf[hdr..len].to_vec());
        buf.drain(..len);
    }
}

// ------------------------------------------------------------------------------------------------ the coder, behind traits

/// Turns ADTS frames into sound. The real one writes to a child process; it never blocks the caller.
pub trait AacDecode: Send {
    /// One whole ADTS frame in.
    fn feed(&mut self, adts: &[u8]) -> Result<(), String>;
    /// The sound that is ready (interleaved, at the configured rate and channels), without waiting.
    fn drain(&mut self) -> Result<Vec<i16>, String>;
}

/// Turns sound into raw AAC frames. The real one writes to a child process; it never blocks the caller.
pub trait AacEncode: Send {
    fn feed(&mut self, pcm: &[i16]) -> Result<(), String>;
    /// The raw frames (no header) that are ready, without waiting.
    fn drain(&mut self) -> Result<Vec<Vec<u8>>, String>;
}

/// Makes coders. `available()` is true only when the add-on that does the coding is installed.
pub trait AacBackend: Send + Sync {
    fn available(&self) -> bool;
    fn decoder(&self, cfg: &AacConfig) -> Result<Box<dyn AacDecode>, String>;
    fn encoder(&self, cfg: &AacConfig) -> Result<Box<dyn AacEncode>, String>;
}

static BACKEND: Mutex<Option<Arc<dyn AacBackend>>> = Mutex::new(None);

/// The agent registers the backend once (ffmpeg of the add-on).
pub fn set_backend(b: Arc<dyn AacBackend>) {
    *BACKEND.lock().unwrap_or_else(|e| e.into_inner()) = Some(b);
}

pub fn clear_backend() {
    *BACKEND.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The backend when the add-on is installed (checked now, so installing it takes effect at the next connection).
pub fn ready_backend() -> Option<Arc<dyn AacBackend>> {
    BACKEND.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|b| b.available())
}

/// Whether AAC can be used now.
pub fn aac_ready() -> bool {
    ready_backend().is_some()
}

/// The short Settings line about AAC (one sentence that fits the audio card).
pub fn note_line() -> String {
    if aac_ready() { "AAC works (audio and video add-on).".into() } else { "AAC needs the audio and video add-on (Store); SBC is used.".into() }
}

/// The sentence about AAC for the assistant: the plain message when the add-on is missing.
pub fn status_line() -> String {
    if aac_ready() { "AAC is available (through the audio and video add-on).".into() } else { NEEDS_ADDON.into() }
}

// ------------------------------------------------------------------------------------------------ receiving

/// RTP packets of AAC in, sound out.
pub struct AacStream {
    cfg: AacConfig,
    dec: Box<dyn AacDecode>,
    reorder: Reorder,
    pub stats: StreamStats,
}

impl AacStream {
    pub fn new(cfg: AacConfig, dec: Box<dyn AacDecode>) -> AacStream {
        AacStream { cfg, dec, reorder: Reorder::default(), stats: StreamStats::default() }
    }

    pub fn config(&self) -> AacConfig {
        self.cfg
    }

    /// One media packet. Lost packets become silence; a damaged one is counted and skipped.
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
                    blocks.push(Block { rate: self.cfg.rate, channels: self.cfg.channels as usize, pcm: vec![0; FRAME_SAMPLES * self.cfg.channels as usize * n.min(50) as usize] });
                }
                Ordered::Packet(p) => match latm_unwrap(&p).ok().and_then(|f| adts_wrap(&self.cfg, f)) {
                    Some(adts) => {
                        self.stats.frames += 1;
                        if self.dec.feed(&adts).is_err() {
                            self.stats.bad_frames += 1;
                        }
                    }
                    None => {
                        self.stats.bad_frames += 1;
                        blocks.push(Block { rate: self.cfg.rate, channels: self.cfg.channels as usize, pcm: vec![0; FRAME_SAMPLES * self.cfg.channels as usize] });
                    }
                },
            }
        }
        self.stats.late_packets = self.reorder.late;
        blocks
    }

    /// The sound the decoder has ready (call after every packet and on a timer). An error is the decoder's plain reason.
    pub fn poll(&mut self) -> Result<Option<Block>, String> {
        let pcm = self.dec.drain()?;
        Ok((!pcm.is_empty()).then(|| Block { rate: self.cfg.rate, channels: self.cfg.channels as usize, pcm }))
    }
}

// ------------------------------------------------------------------------------------------------ sending

/// Puts raw AAC frames into RTP packets: payload type 96, one LATM element per packet, 1024 samples per packet on the clock.
pub struct AacPacketizer {
    seq: u16,
    ts: u32,
    ssrc: u32,
}

impl AacPacketizer {
    pub fn new(ssrc: u32) -> AacPacketizer {
        AacPacketizer { seq: 0, ts: 0, ssrc }
    }

    pub fn packet(&mut self, frame: &[u8]) -> Vec<u8> {
        let mut v = vec![0x80, 96];
        v.extend_from_slice(&self.seq.to_be_bytes());
        v.extend_from_slice(&self.ts.to_be_bytes());
        v.extend_from_slice(&self.ssrc.to_be_bytes());
        v.extend_from_slice(&latm_wrap(frame));
        self.seq = self.seq.wrapping_add(1);
        self.ts = self.ts.wrapping_add(FRAME_SAMPLES as u32);
        v
    }
}

#[cfg(test)]
#[path = "a2dp_aac_tests.rs"]
mod tests;

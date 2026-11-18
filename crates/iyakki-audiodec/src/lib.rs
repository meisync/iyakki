//! Ogg and Vorbis, written in-house from the public format descriptions (no library, no table copied from one): the Ogg page layer
//! ([`ogg`]), the Vorbis decoder ([`vorbis`]) and the transform under it ([`mdct`]).
//!
//! Hostile streams are expected: every length, count and size read from the stream is checked against a cap before anything is
//! allocated, a damaged page is skipped (the next page is found by its capture pattern), a damaged packet gives silence for that
//! packet, and nothing panics. Opus streams are decoded by [`opus_dec`] (CELT, SILK and hybrid, written in-house).

pub mod flac;
pub mod mdct;
pub mod ogg;
pub mod opus;
pub mod opus_celt;
pub mod opus_dec;
pub mod opus_silk;
mod opus_silk_tables;
mod opus_tables;
pub mod vorbis;

/// The program's output header: `IYPC`, the sample rate as u32 little-endian, the channel count as u16 little-endian.
pub fn header(rate: u32, channels: u16) -> [u8; 10] {
    let mut h = [0u8; 10];
    h[..4].copy_from_slice(b"IYPC");
    h[4..8].copy_from_slice(&rate.to_le_bytes());
    h[8..10].copy_from_slice(&channels.to_le_bytes());
    h
}

/// What a whole-stream decode says.
#[derive(Debug, PartialEq)]
pub enum StreamError {
    /// The stream is Opus (or another Ogg codec): the plain name.
    Unsupported(&'static str),
    NotOgg,
    Bad(String),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Unsupported(n) => write!(f, "this stream is {n}, which this version cannot play (Ogg Vorbis and MP3 can be)"),
            StreamError::NotOgg => write!(f, "this is not an Ogg stream"),
            StreamError::Bad(s) => write!(f, "{s}"),
        }
    }
}

/// Decodes a stream as it arrives: bytes in, interleaved 16-bit samples out.
pub struct StreamDecoder {
    ogg: ogg::Demuxer,
    vorbis: Option<vorbis::Decoder>,
    opus: Option<OpusStream>,
    /// A FLAC stream is told by its first four bytes: it has no Ogg pages.
    flac: Option<flac::Decoder>,
    head: Vec<u8>,
    sniffed: bool,
    packets_seen: u32,
    headers: Vec<Vec<u8>>,
    serial: Option<u32>,
    pub rate: u32,
    pub channels: usize,
    started: bool,
    pending_format: Option<(u32, usize)>,
}

impl Default for StreamDecoder {
    fn default() -> Self {
        StreamDecoder::new()
    }
}

impl StreamDecoder {
    pub fn new() -> StreamDecoder {
        StreamDecoder { ogg: ogg::Demuxer::new(), vorbis: None, opus: None, flac: None, head: Vec::new(), sniffed: false, packets_seen: 0, headers: vec![], serial: None, rate: 0, channels: 0, started: false, pending_format: None }
    }

    /// Whether the format (rate, channels) is known yet.
    pub fn ready(&self) -> bool {
        self.started
    }

    /// Feeds bytes; returns the samples decoded from them.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<i16>, StreamError> {
        if !self.sniffed {
            self.head.extend_from_slice(bytes);
            if self.head.len() < 4 {
                return Ok(Vec::new());
            }
            self.sniffed = true;
            let all = std::mem::take(&mut self.head);
            if all.starts_with(b"fLaC") {
                self.flac = Some(flac::Decoder::new());
            }
            return self.push_sniffed(&all);
        }
        self.push_sniffed(bytes)
    }

    fn push_sniffed(&mut self, bytes: &[u8]) -> Result<Vec<i16>, StreamError> {
        if let Some(f) = self.flac.as_mut() {
            let pcm = f.push(bytes).map_err(StreamError::Bad)?;
            if let Some(i) = f.info() {
                self.rate = i.rate;
                self.channels = i.channels;
                self.started = true;
            }
            return Ok(pcm);
        }
        let mut out = Vec::new();
        for page in self.ogg.push(bytes) {
            if page.bos && self.serial.is_some() && self.serial != Some(page.serial) {
                // a chained stream: the next track starts over
                self.serial = Some(page.serial);
                self.headers.clear();
                let (r, c) = (self.rate, self.channels);
                self.vorbis = None;
                self.opus = None;
                self.packets_seen = 0;
                self.pending_format = Some((r, c));
            }
            if self.serial.is_none() {
                self.serial = Some(page.serial);
            }
            if Some(page.serial) != self.serial {
                continue;
            }
            let packets = page.packets;
            let n = packets.len();
            for (i, p) in packets.into_iter().enumerate() {
                if let Some(o) = self.opus.as_mut() {
                    if !o.tags_seen {
                        o.tags_seen = true;
                        continue;
                    }
                    let last = i + 1 == n && page.eos;
                    out.extend(o.decode(&p, last.then_some(page.granule)));
                    continue;
                }
                if self.vorbis.is_none() {
                    if self.packets_seen == 0 {
                        if p.starts_with(b"OpusHead") {
                            let head = opus::parse_head(&p).map_err(|e| StreamError::Bad(e.to_string()))?;
                            let (r, c) = (48000, head.channels as usize);
                            if let Some((r0, c0)) = self.pending_format.take() {
                                if (r0, c0) != (r, c) {
                                    return Err(StreamError::Bad("the stream changed its sample rate or channels".into()));
                                }
                            }
                            self.rate = r;
                            self.channels = c;
                            self.started = true;
                            self.packets_seen = 1;
                            self.opus = Some(OpusStream::new(head));
                            continue;
                        }
                        if p.starts_with(b"\x7fFLAC") {
                            return Err(StreamError::Unsupported("Ogg FLAC"));
                        }
                        if !(p.len() > 7 && p[0] == 1 && &p[1..7] == b"vorbis") {
                            return Err(StreamError::Unsupported("not Vorbis"));
                        }
                    }
                    self.packets_seen += 1;
                    self.headers.push(p);
                    if self.headers.len() == 3 {
                        let d = vorbis::Decoder::new(&self.headers[0], &self.headers[1], &self.headers[2]).map_err(StreamError::Bad)?;
                        let (r, c) = (d.rate(), d.channels());
                        if let Some((r0, c0)) = self.pending_format.take() {
                            if (r0, c0) != (r, c) {
                                return Err(StreamError::Bad("the stream changed its sample rate or channels".into()));
                            }
                        }
                        self.rate = r;
                        self.channels = c;
                        self.started = true;
                        self.vorbis = Some(d);
                    }
                    continue;
                }
                let Some(d) = self.vorbis.as_mut() else { continue };
                let last = i + 1 == n && page.eos;
                let mut pcm = d.decode(&p);
                if last && page.granule >= 0 {
                    // the last page says how many samples the stream really has
                    let total = page.granule as u64;
                    let produced = d.produced();
                    if produced > total {
                        let cut = ((produced - total) as usize).min(pcm.len() / d.channels().max(1));
                        pcm.truncate(pcm.len() - cut * d.channels());
                    }
                }
                out.extend(pcm.iter().map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16));
            }
        }
        Ok(out)
    }
}

/// The state of an Opus stream inside [`StreamDecoder`]: the decoder, the pre-skip still to drop and the count that the last page's
/// granule position cuts the end to.
struct OpusStream {
    dec: opus_dec::OpusDecoder,
    skip_left: usize,
    head_skip: u64,
    produced: u64,
    tags_seen: bool,
}

impl OpusStream {
    fn new(head: opus::Head) -> OpusStream {
        OpusStream { skip_left: head.pre_skip as usize, head_skip: head.pre_skip as u64, dec: opus_dec::OpusDecoder::new(&head), produced: 0, tags_seen: false }
    }

    /// Decodes a packet; `end_granule` is the last page's granule position when this is the stream's last packet.
    fn decode(&mut self, packet: &[u8], end_granule: Option<i64>) -> Vec<i16> {
        let ch = self.dec.channels();
        let mut pcm = match self.dec.decode_packet(packet) {
            Ok(p) => p,
            // a packet that breaks the format is a lost packet: the sound goes on with a concealed stretch of the same length
            Err(_) => {
                let n = opus::parse_packet(packet).map(|p| p.samples()).unwrap_or(960);
                self.dec.conceal(n)
            }
        };
        let frames = pcm.len() / ch;
        let drop = self.skip_left.min(frames);
        self.skip_left -= drop;
        pcm.drain(..drop * ch);
        let mut frames = pcm.len() / ch;
        if let Some(g) = end_granule.filter(|g| *g >= 0) {
            let total = (g as u64).saturating_sub(self.head_skip);
            let room = total.saturating_sub(self.produced) as usize;
            if frames > room {
                pcm.truncate(room * ch);
                frames = room;
            }
        }
        self.produced += frames as u64;
        pcm.iter().map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16).collect()
    }
}

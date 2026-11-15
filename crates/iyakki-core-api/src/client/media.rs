//! The media half of the client protocol (docs/development/CLIENT-SPLIT-DESIGN.md sections 6 and 7b): sound to the client, the text
//! clipboard, camera frames, and the client's own camera and microphone as virtual sources. EXPERIMENTAL.
//!
//! Pure rules and framing, no OS code and no clock, so the same file builds for the instance and for the browser engine (wasm32).
//!
//! Binary streams (first byte is the stream tag of [`super::binary`]; the directions use separate tags):
//! * [`super::STREAM_AUDIO`] instance to client: `[seq u32 LE][16-bit LE interleaved PCM of one 20 ms chunk]`, 48 kHz, the channels
//!   named by `audio_ok`. Sequence numbers rise by one per chunk, so a client sees a gap and conceals it.
//! * [`super::STREAM_CAMERA`] instance to client: `[unit length u8][unit][seq u32 LE][JPEG]`.
//! * [`super::STREAM_MIC_IN`] client to instance: 16-bit LE mono PCM at [`MIC_RATE`], up to [`MAX_MIC_CHUNK`] bytes.
//! * [`super::STREAM_CAMERA_IN`] client to instance: one JPEG, up to [`MAX_CAMERA_FRAME`] bytes.

use std::collections::VecDeque;

/// Sound is sent as this rate, in chunks of [`CHUNK_MS`].
pub const AUDIO_RATE: u32 = 48_000;
pub const CHUNK_MS: u32 = 20;
/// Frames (samples per channel) of one chunk: 960.
pub const CHUNK_FRAMES: usize = (AUDIO_RATE as usize * CHUNK_MS as usize) / 1000;
/// The most sound that waits for a client. Older chunks are dropped first: late sound is worse than a gap.
pub const MAX_QUEUE_MS: u32 = 100;
/// Chunks kept by a [`JitterQueue`] of the default size.
pub const MAX_QUEUE_CHUNKS: usize = (MAX_QUEUE_MS / CHUNK_MS) as usize;
/// The microphone format a client sends.
pub const MIC_RATE: u32 = 16_000;
pub const MAX_MIC_CHUNK: usize = 64 * 1024;
/// The most bytes of one JPEG camera frame, either way.
pub const MAX_CAMERA_FRAME: usize = 256 * 1024;
/// Frames per second the instance sends to a client viewing a camera.
pub const CAMERA_FPS: u32 = 5;
/// The most bytes of text the clipboard carries, either way.
pub const MAX_CLIPBOARD: usize = 64 * 1024;
/// Longest name of a camera unit in a frame header.
pub const MAX_UNIT: usize = 120;

/// Where the person's sound plays for one client session. There is no third state that opens both by accident: `Both` is an explicit
/// choice. The machine alone needs no route at all, so it is not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The client only; the machine's speakers stay quiet. The default for a remote client.
    Client,
    /// The client and the machine's speakers.
    Both,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Client => "client",
            Side::Both => "both",
        }
    }

    /// `None` (nothing named) is the client, the default for a remote client. Anything else that is not a known name is refused.
    pub fn parse(s: Option<&str>) -> Option<Side> {
        match s {
            None | Some("") | Some("client") => Some(Side::Client),
            Some("both") => Some(Side::Both),
            _ => None,
        }
    }
}

/// The payload of one audio chunk.
pub fn encode_audio(seq: u32, pcm: &[i16]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + pcm.len() * 2);
    v.extend_from_slice(&seq.to_le_bytes());
    for s in pcm {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// The sequence number and samples of an audio payload; `None` if it is not whole samples of `channels` channels or is longer than a second.
pub fn decode_audio(payload: &[u8], channels: usize) -> Option<(u32, Vec<i16>)> {
    if payload.len() < 4 || channels == 0 {
        return None;
    }
    let body = &payload[4..];
    if body.len() % (2 * channels) != 0 || body.len() > AUDIO_RATE as usize * 2 * channels {
        return None;
    }
    let seq = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    Some((seq, body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()))
}

/// The payload of one camera frame for `unit`.
pub fn encode_camera(unit: &str, seq: u32, jpeg: &[u8]) -> Option<Vec<u8>> {
    let u = unit.as_bytes();
    if u.len() > MAX_UNIT || jpeg.len() > MAX_CAMERA_FRAME {
        return None;
    }
    let mut v = Vec::with_capacity(1 + u.len() + 4 + jpeg.len());
    v.push(u.len() as u8);
    v.extend_from_slice(u);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(jpeg);
    Some(v)
}

/// The unit, sequence number and JPEG of a camera payload.
pub fn decode_camera(payload: &[u8]) -> Option<(String, u32, &[u8])> {
    let n = *payload.first()? as usize;
    if n > MAX_UNIT || payload.len() < 1 + n + 4 {
        return None;
    }
    let unit = std::str::from_utf8(&payload[1..1 + n]).ok()?.to_string();
    let seq = u32::from_le_bytes(payload[1 + n..5 + n].try_into().ok()?);
    let jpeg = &payload[5 + n..];
    (jpeg.len() <= MAX_CAMERA_FRAME).then_some((unit, seq, jpeg))
}

/// A JPEG starts with FF D8 and ends with FF D9.
pub fn looks_like_jpeg(b: &[u8]) -> bool {
    b.len() >= 4 && b[0] == 0xFF && b[1] == 0xD8 && b[b.len() - 2] == 0xFF && b[b.len() - 1] == 0xD9
}

/// Why a piece of clipboard text was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipError {
    TooBig,
    Empty,
}

/// Text the clipboard may carry: at most [`MAX_CLIPBOARD`] bytes, not empty, NUL characters removed (they end strings in other programs).
pub fn clean_clipboard(text: &str) -> Result<String, ClipError> {
    if text.len() > MAX_CLIPBOARD {
        return Err(ClipError::TooBig);
    }
    let t: String = text.chars().filter(|c| *c != '\0').collect();
    if t.is_empty() {
        Err(ClipError::Empty)
    } else {
        Ok(t)
    }
}

/// What a client's own camera or microphone is called on the instance: the device name, then the kind ("Maya's laptop camera").
pub fn source_name(device_name: &str, kind: SourceKind) -> String {
    let d: String = device_name.chars().filter(|c| !c.is_control()).take(60).collect();
    let d = d.trim();
    format!("{} {}", if d.is_empty() { "client" } else { d }, kind.word())
}

/// What a client can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Camera,
    Microphone,
}

impl SourceKind {
    pub fn word(self) -> &'static str {
        match self {
            SourceKind::Camera => "camera",
            SourceKind::Microphone => "microphone",
        }
    }
    pub fn parse(s: &str) -> Option<SourceKind> {
        match s {
            "camera" => Some(SourceKind::Camera),
            "microphone" => Some(SourceKind::Microphone),
            _ => None,
        }
    }
    /// The catalogue capability that a client needs to offer this.
    pub fn capability(self) -> &'static str {
        match self {
            SourceKind::Camera => "client.camera.offer",
            SourceKind::Microphone => "client.microphone.offer",
        }
    }
}

/// Collects interleaved samples of any length into whole chunks of [`CHUNK_FRAMES`] frames.
pub struct Chunker {
    channels: usize,
    carry: Vec<i16>,
}

impl Chunker {
    pub fn new(channels: usize) -> Chunker {
        Chunker { channels: channels.clamp(1, 2), carry: Vec::new() }
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn push(&mut self, pcm: &[i16]) -> Vec<Vec<i16>> {
        self.carry.extend_from_slice(pcm);
        let n = CHUNK_FRAMES * self.channels;
        let mut out = Vec::new();
        while self.carry.len() >= n {
            out.push(self.carry.drain(..n).collect());
        }
        out
    }
    /// Throw away the part chunk (a stop or a pause).
    pub fn clear(&mut self) {
        self.carry.clear();
    }
}

/// A queue of at most `cap` items; a new item pushes the oldest out. This is the jitter bound: sound that waited longer than the bound is
/// dropped, never played late.
pub struct JitterQueue<T> {
    cap: usize,
    items: VecDeque<T>,
    dropped: u64,
}

impl<T> JitterQueue<T> {
    pub fn new(cap: usize) -> JitterQueue<T> {
        JitterQueue { cap: cap.max(1), items: VecDeque::new(), dropped: 0 }
    }
    /// The default bound, [`MAX_QUEUE_MS`].
    pub fn bounded() -> JitterQueue<T> {
        JitterQueue::new(MAX_QUEUE_CHUNKS)
    }
    pub fn push(&mut self, t: T) {
        self.items.push_back(t);
        while self.items.len() > self.cap {
            self.items.pop_front();
            self.dropped += 1;
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }
    /// Drop everything waiting; returns how many items were thrown away.
    pub fn clear(&mut self) -> usize {
        let n = self.items.len();
        self.items.clear();
        n
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests;

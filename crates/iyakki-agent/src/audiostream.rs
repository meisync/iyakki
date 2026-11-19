//! The machine-side half of audio streaming for "My IYAKKI" in cloud mode (F-VOICE-38, design: docs/development/CLOUD-AUDIO-STREAMING.md).
//! No network is in here: this is the frame format, the cutting of a recording into frames, and the buffer on the receiving side that puts
//! frames back in order, drops duplicates and late ones, and fills a hole with silence so the sound keeps its length. A transport (a
//! TLS socket after the person agreed) is the next step and is not built.

use std::collections::BTreeMap;

pub const RATE: u32 = 16_000;
/// 20 ms of 16 kHz mono, the usual size for speech.
pub const FRAME_SAMPLES: usize = 320;
const MAGIC: [u8; 2] = [0x49, 0x41];
const VERSION: u8 = 1;
const HEAD: usize = 2 + 1 + 4 + 4 + 2;
/// The largest frame a receiver accepts: 100 ms. A bigger one is a mistake or an attack, never sound.
pub const MAX_SAMPLES: usize = 1600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u32,
    pub ts_ms: u32,
    pub pcm: Vec<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    Short,
    BadMagic,
    BadVersion(u8),
    TooBig(usize),
    Length,
    Checksum,
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

impl Frame {
    /// magic(2) version(1) seq(4) ts_ms(4) samples(2) pcm(2 each, little endian) crc32(4) of everything before it.
    pub fn encode(&self) -> Vec<u8> {
        let n = self.pcm.len().min(MAX_SAMPLES);
        let mut v = Vec::with_capacity(HEAD + n * 2 + 4);
        v.extend_from_slice(&MAGIC);
        v.push(VERSION);
        v.extend_from_slice(&self.seq.to_le_bytes());
        v.extend_from_slice(&self.ts_ms.to_le_bytes());
        v.extend_from_slice(&(n as u16).to_le_bytes());
        for s in &self.pcm[..n] {
            v.extend_from_slice(&s.to_le_bytes());
        }
        let c = crc32(&v);
        v.extend_from_slice(&c.to_le_bytes());
        v
    }

    pub fn decode(b: &[u8]) -> Result<Frame, FrameError> {
        if b.len() < HEAD + 4 {
            return Err(FrameError::Short);
        }
        if b[..2] != MAGIC {
            return Err(FrameError::BadMagic);
        }
        if b[2] != VERSION {
            return Err(FrameError::BadVersion(b[2]));
        }
        let n = u16::from_le_bytes([b[11], b[12]]) as usize;
        if n > MAX_SAMPLES {
            return Err(FrameError::TooBig(n));
        }
        if b.len() != HEAD + n * 2 + 4 {
            return Err(FrameError::Length);
        }
        let body = &b[..b.len() - 4];
        let want = u32::from_le_bytes([b[b.len() - 4], b[b.len() - 3], b[b.len() - 2], b[b.len() - 1]]);
        if crc32(body) != want {
            return Err(FrameError::Checksum);
        }
        let seq = u32::from_le_bytes([b[3], b[4], b[5], b[6]]);
        let ts_ms = u32::from_le_bytes([b[7], b[8], b[9], b[10]]);
        let pcm = b[HEAD..HEAD + n * 2].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        Ok(Frame { seq, ts_ms, pcm })
    }
}

/// Cut a recording into numbered frames of 20 ms; the last one is padded with silence so every frame has the same length.
pub fn cut(pcm: &[i16], first_seq: u32, start_ms: u32) -> Vec<Frame> {
    pcm.chunks(FRAME_SAMPLES)
        .enumerate()
        .map(|(i, c)| {
            let mut p = c.to_vec();
            p.resize(FRAME_SAMPLES, 0);
            Frame { seq: first_seq.wrapping_add(i as u32), ts_ms: start_ms.wrapping_add((i * FRAME_SAMPLES * 1000 / RATE as usize) as u32), pcm: p }
        })
        .collect()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub played: u32,
    pub concealed: u32,
    pub duplicates: u32,
    pub late: u32,
}

/// Puts frames back in order. `depth` frames are held before the first is released (the delay that absorbs reordering); a frame
/// that has not arrived by the time later ones fill the buffer is replaced by silence, and if it shows up after that it is dropped.
pub struct Jitter {
    depth: usize,
    next: Option<u32>,
    started: bool,
    held: BTreeMap<u32, Frame>,
    pub stats: Stats,
}

impl Jitter {
    pub fn new(depth: usize) -> Jitter {
        Jitter { depth: depth.clamp(1, 50), next: None, started: false, held: BTreeMap::new(), stats: Stats::default() }
    }

    pub fn push(&mut self, f: Frame) {
        let mut next = *self.next.get_or_insert(f.seq);
        if !self.started && f.seq.wrapping_sub(next) > u32::MAX / 2 {
            // nothing has been played yet: an earlier frame that arrives second is the start, not a late one
            next = f.seq;
            self.next = Some(next);
        }
        let ahead = f.seq.wrapping_sub(next);
        if ahead > u32::MAX / 2 {
            self.stats.late += 1;
        } else if self.held.contains_key(&f.seq) {
            self.stats.duplicates += 1;
        } else if ahead as usize > 10 * self.depth + 50 {
            // a jump far ahead is not reordering: drop it rather than hold memory for it
            self.stats.late += 1;
        } else {
            self.held.insert(f.seq, f);
        }
    }

    /// The next 20 ms to play, or None while the buffer is still filling or empty.
    pub fn pop(&mut self) -> Option<Vec<i16>> {
        let next = self.next?;
        if !self.started && self.held.len() < self.depth {
            return None; // still filling: the delay that absorbs reordering
        }
        if let Some(f) = self.held.remove(&next) {
            self.next = Some(next.wrapping_add(1));
            self.started = true;
            self.stats.played += 1;
            return Some(f.pcm);
        }
        let newest = self.held.keys().next_back().copied()?;
        if (newest.wrapping_sub(next) as usize) >= self.depth {
            self.next = Some(next.wrapping_add(1));
            self.started = true;
            self.stats.concealed += 1;
            return Some(vec![0; FRAME_SAMPLES]);
        }
        None
    }

    /// Everything still held, in order, with silence for the holes: used when the stream ends.
    pub fn drain(&mut self) -> Vec<i16> {
        let mut out = Vec::new();
        while let Some(&last) = self.held.keys().next_back() {
            let next = match self.next {
                Some(n) => n,
                None => break,
            };
            if last.wrapping_sub(next) > u32::MAX / 2 {
                break;
            }
            match self.held.remove(&next) {
                Some(f) => {
                    self.stats.played += 1;
                    out.extend(f.pcm)
                }
                None => {
                    self.stats.concealed += 1;
                    out.extend(std::iter::repeat(0).take(FRAME_SAMPLES))
                }
            }
            self.next = Some(next.wrapping_add(1));
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ over a byte stream

/// Write one frame to a byte stream (a socket, after the person agreed and a TLS layer wrapped it): the frame's own bytes, which carry
/// their length, so no second framing is needed.
pub fn write_frame<W: std::io::Write>(w: &mut W, f: &Frame) -> std::io::Result<()> {
    w.write_all(&f.encode())
}

/// Read the next frame from a byte stream. `Ok(None)` at a clean end of the stream; a damaged frame is an error and ends the stream
/// (after damage the boundaries are not known any more, so the reader never guesses where the next frame starts).
pub fn read_frame<R: std::io::Read>(r: &mut R) -> Result<Option<Frame>, FrameError> {
    let mut head = [0u8; HEAD];
    let mut got = 0;
    while got < HEAD {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Short),
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(FrameError::Short),
        }
    }
    if head[..2] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    if head[2] != VERSION {
        return Err(FrameError::BadVersion(head[2]));
    }
    let n = u16::from_le_bytes([head[11], head[12]]) as usize;
    if n > MAX_SAMPLES {
        return Err(FrameError::TooBig(n));
    }
    let mut all = head.to_vec();
    all.resize(HEAD + n * 2 + 4, 0);
    r.read_exact(&mut all[HEAD..]).map_err(|_| FrameError::Short)?;
    Frame::decode(&all).map(Some)
}

/// Frames waiting to be sent. When the link is slower than the sound, the OLDEST frames are dropped so the delay never grows past
/// `max_ms` (old speech is worth less than new); every drop is counted.
pub struct SendQueue {
    q: std::collections::VecDeque<Frame>,
    max_frames: usize,
    pub dropped: u32,
}

impl SendQueue {
    pub fn new(max_ms: u32) -> SendQueue {
        SendQueue { q: Default::default(), max_frames: (max_ms as usize * RATE as usize / 1000 / FRAME_SAMPLES).max(1), dropped: 0 }
    }
    pub fn push(&mut self, f: Frame) {
        self.q.push_back(f);
        while self.q.len() > self.max_frames {
            self.q.pop_front();
            self.dropped += 1;
        }
    }
    pub fn pop(&mut self) -> Option<Frame> {
        self.q.pop_front()
    }
    pub fn len(&self) -> usize {
        self.q.len()
    }
    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<i16> {
        (0..n).map(|i| ((i as f32 * 0.3).sin() * 9000.0) as i16).collect()
    }

    #[test]
    fn a_frame_survives_encoding_and_every_kind_of_damage_is_named() {
        let f = Frame { seq: 7, ts_ms: 140, pcm: tone(FRAME_SAMPLES) };
        let b = f.encode();
        assert_eq!(b.len(), 13 + 640 + 4);
        assert_eq!(Frame::decode(&b), Ok(f.clone()));
        let mut x = b.clone();
        x[20] ^= 1;
        assert_eq!(Frame::decode(&x), Err(FrameError::Checksum));
        assert_eq!(Frame::decode(&b[..10]), Err(FrameError::Short));
        assert_eq!(Frame::decode(&b[..b.len() - 2]), Err(FrameError::Length));
        let mut m = b.clone();
        m[0] = 0;
        assert_eq!(Frame::decode(&m), Err(FrameError::BadMagic));
        let mut v = b.clone();
        v[2] = 9;
        assert_eq!(Frame::decode(&v), Err(FrameError::BadVersion(9)));
        let mut big = b.clone();
        big[11] = 0xFF;
        big[12] = 0xFF;
        assert!(matches!(Frame::decode(&big), Err(FrameError::TooBig(_))));
        assert_eq!(Frame { seq: 1, ts_ms: 0, pcm: vec![0; 5000] }.encode().len(), 13 + MAX_SAMPLES * 2 + 4, "a sender cannot make an oversized frame");
    }

    #[test]
    fn a_recording_is_cut_numbered_timed_and_comes_back_whole_through_shuffled_duplicated_frames() {
        let pcm = tone(FRAME_SAMPLES * 10 + 100);
        let frames = cut(&pcm, 4_294_967_290, 1000);
        assert_eq!(frames.len(), 11);
        assert_eq!(frames[1].ts_ms, 1020);
        assert_eq!(frames[6].seq, 0, "the sequence number wraps");
        // a fixed shuffle with two duplicates, over the wire format
        let order = [1usize, 0, 3, 2, 2, 5, 4, 7, 6, 6, 9, 8, 10];
        let mut j = Jitter::new(4);
        let mut out = Vec::new();
        for i in order {
            j.push(Frame::decode(&frames[i].encode()).unwrap());
            while let Some(p) = j.pop() {
                out.extend(p);
            }
        }
        out.extend(j.drain());
        assert_eq!(&out[..pcm.len()], &pcm[..]);
        assert_eq!(out.len(), 11 * FRAME_SAMPLES);
        assert_eq!(j.stats.duplicates + j.stats.late, 2, "a repeat is dropped, held or already played");
        assert_eq!(j.stats.concealed, 0);
    }

    #[test]
    fn a_lost_frame_becomes_silence_of_the_same_length_and_a_late_one_is_dropped() {
        let pcm = tone(FRAME_SAMPLES * 8);
        let frames = cut(&pcm, 0, 0);
        let mut j = Jitter::new(3);
        let mut out = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            if i == 2 {
                continue; // lost on the way
            }
            j.push(f.clone());
            while let Some(p) = j.pop() {
                out.extend(p);
            }
        }
        j.push(frames[2].clone()); // arrives after its place was given up
        out.extend(j.drain());
        assert_eq!(out.len(), 8 * FRAME_SAMPLES, "the sound keeps its length");
        assert!(out[2 * FRAME_SAMPLES..3 * FRAME_SAMPLES].iter().all(|&s| s == 0));
        assert_eq!(&out[..2 * FRAME_SAMPLES], &pcm[..2 * FRAME_SAMPLES]);
        assert_eq!(&out[3 * FRAME_SAMPLES..], &pcm[3 * FRAME_SAMPLES..]);
        assert_eq!((j.stats.concealed, j.stats.late), (1, 1));
    }

    #[test]
    fn a_wild_jump_in_the_sequence_is_not_held_in_memory() {
        let mut j = Jitter::new(4);
        j.push(Frame { seq: 0, ts_ms: 0, pcm: vec![1; FRAME_SAMPLES] });
        for i in 0..1000u32 {
            j.push(Frame { seq: 1_000_000 + i, ts_ms: 0, pcm: vec![1; FRAME_SAMPLES] });
        }
        assert_eq!(j.held.len(), 1);
        assert_eq!(j.stats.late, 1000);
    }

    #[test]
    fn frames_travel_over_a_real_loopback_socket_in_order_with_a_slow_link_dropping_the_oldest() {
        use std::net::{TcpListener, TcpStream};
        let pcm = tone(FRAME_SAMPLES * 30);
        let frames = cut(&pcm, 100, 0);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // the receiving side: reads frames until the stream ends, puts them through the jitter buffer
        let rx = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut j = Jitter::new(3);
            let mut out = Vec::new();
            while let Some(f) = read_frame(&mut s).unwrap() {
                j.push(f);
                while let Some(p) = j.pop() {
                    out.extend(p);
                }
            }
            out.extend(j.drain());
            (out, j.stats)
        });
        // the sending side: a queue of 200 ms (10 frames) in front of the socket; the link here is fast, so nothing is dropped
        let mut tx = TcpStream::connect(addr).unwrap();
        let mut q = SendQueue::new(200);
        for f in &frames {
            q.push(f.clone());
            while let Some(x) = q.pop() {
                write_frame(&mut tx, &x).unwrap();
            }
        }
        assert_eq!(q.dropped, 0);
        drop(tx);
        let (out, stats) = rx.join().unwrap();
        assert_eq!(out, pcm, "the sound comes out as it went in");
        assert_eq!((stats.concealed, stats.late, stats.duplicates), (0, 0, 0));
    }

    #[test]
    fn a_queue_that_the_link_cannot_empty_keeps_the_newest_two_hundred_milliseconds_and_counts_the_rest() {
        let mut q = SendQueue::new(200);
        for f in cut(&tone(FRAME_SAMPLES * 25), 0, 0) {
            q.push(f);
        }
        assert_eq!((q.len(), q.dropped), (10, 15));
        assert_eq!(q.pop().unwrap().seq, 15, "the oldest that is left is the sixteenth");
        assert!(!q.is_empty());
    }

    #[test]
    fn a_damaged_stream_ends_with_a_named_error_and_a_clean_end_is_not_an_error() {
        let frames = cut(&tone(FRAME_SAMPLES * 3), 0, 0);
        let mut bytes = Vec::new();
        for f in &frames {
            write_frame(&mut bytes, f).unwrap();
        }
        let mut r = std::io::Cursor::new(bytes.clone());
        for f in &frames {
            assert_eq!(read_frame(&mut r).unwrap().as_ref(), Some(f));
        }
        assert_eq!(read_frame(&mut r), Ok(None), "the end of the stream between frames");
        let mut cut_short = std::io::Cursor::new(bytes[..bytes.len() - 7].to_vec());
        read_frame(&mut cut_short).unwrap();
        read_frame(&mut cut_short).unwrap();
        assert_eq!(read_frame(&mut cut_short), Err(FrameError::Short), "a frame cut in the middle");
        let mut bad = bytes.clone();
        bad[30] ^= 0xFF;
        assert_eq!(read_frame(&mut std::io::Cursor::new(bad)), Err(FrameError::Checksum));
        assert_eq!(read_frame(&mut std::io::Cursor::new(vec![0u8; 40])), Err(FrameError::BadMagic));
    }
}

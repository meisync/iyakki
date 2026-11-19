//! Motion-JPEG in an AVI file, the one video format the core's embedded player decodes in house (F-SDK-143): an AVI reader (the RIFF
//! chunks, written from the public container description), the picture of each frame through the browser's own JPEG decoder
//! (`iyakki_web::jpeg`), the frame pacer that picks the picture whose time has come on the sound's clock and drops the late ones, and the
//! scaling that keeps a picture under the facet's size limit. Camera recordings and most webcam clips are in this format. H.264, VP8, VP9
//! and AV1 are not: they need a decoder that does not exist in house yet, and the player says so in a sentence.
//!
//! The sound of the file (plain PCM, 8 or 16 bit) is the clock: the player plays it through the mixer and asks the pacer which picture is
//! due at the position the mixer reports as heard. A file with no sound is played with silence of the video's length, so the same clock
//! drives it.

use iyakki_core_api::media::MAX_FRAME_PIXELS;

pub const MAX_FRAMES: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VFrame {
    pub pts_ms: u64,
    pub off: usize,
    pub len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AviAudio {
    pub rate: u32,
    pub channels: usize,
    pub pcm: Vec<i16>,
}

#[derive(Clone, Debug)]
pub struct Avi {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<VFrame>,
    pub audio: Option<AviAudio>,
    pub duration_ms: u64,
}

fn u32le(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u16le(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

pub fn is_avi(head: &[u8]) -> bool {
    head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"AVI "
}

#[derive(Default)]
struct Stream {
    kind: [u8; 4],
    handler: [u8; 4],
    scale: u32,
    rate: u32,
    // video: width, height, compression; audio: tag, channels, rate, bits
    a: u32,
    b: u32,
    c: u32,
    d: u32,
}

/// Reads the headers and the chunks of the file. Errors are sentences for the person.
pub fn parse(d: &[u8]) -> Result<Avi, String> {
    if !is_avi(d) {
        return Err("this is not an AVI file".into());
    }
    let mut streams: Vec<Stream> = Vec::new();
    let mut usec = 0u32;
    let mut avih_size = (0u32, 0u32);
    // (stream number, offset, len)
    let mut video_chunks: Vec<(usize, usize)> = Vec::new();
    let mut audio_bytes: Vec<u8> = Vec::new();
    let mut vstream: Option<usize> = None;
    let mut astream: Option<usize> = None;
    let mut cur: Option<usize> = None;
    walk(d, 12, d.len().min(8 + u32le(d, 4).unwrap_or(0) as usize), &mut |id, list, off, len| {
        match (id, list) {
            (b"avih", _) => {
                usec = u32le(d, off).unwrap_or(0);
                avih_size = (u32le(d, off + 32).unwrap_or(0), u32le(d, off + 36).unwrap_or(0));
            }
            (b"strh", _) => {
                let mut s = Stream::default();
                s.kind.copy_from_slice(d.get(off..off + 4).unwrap_or(b"????"));
                s.handler.copy_from_slice(d.get(off + 4..off + 8).unwrap_or(b"????"));
                s.scale = u32le(d, off + 20).unwrap_or(0);
                s.rate = u32le(d, off + 24).unwrap_or(0);
                streams.push(s);
                cur = Some(streams.len() - 1);
                if &streams[streams.len() - 1].kind == b"vids" && vstream.is_none() {
                    vstream = Some(streams.len() - 1);
                } else if &streams[streams.len() - 1].kind == b"auds" && astream.is_none() {
                    astream = Some(streams.len() - 1);
                }
            }
            (b"strf", _) => {
                if let Some(i) = cur {
                    let s = &mut streams[i];
                    if &s.kind == b"vids" {
                        s.a = u32le(d, off + 4).unwrap_or(0);
                        s.b = u32le(d, off + 8).unwrap_or(0);
                        s.c = u32le(d, off + 16).unwrap_or(0);
                    } else if &s.kind == b"auds" {
                        s.a = u32::from(u16le(d, off).unwrap_or(0));
                        s.b = u32::from(u16le(d, off + 2).unwrap_or(0));
                        s.c = u32le(d, off + 4).unwrap_or(0);
                        s.d = u32::from(u16le(d, off + 14).unwrap_or(0));
                    }
                }
            }
            _ => {
                // a data chunk of the movi list: two digits of the stream number and two letters
                if id[0].is_ascii_digit() && id[1].is_ascii_digit() {
                    let n = usize::from(id[0] - b'0') * 10 + usize::from(id[1] - b'0');
                    if Some(n) == vstream && (&id[2..] == b"dc" || &id[2..] == b"db") {
                        if video_chunks.len() < MAX_FRAMES && len > 0 {
                            video_chunks.push((off, len));
                        }
                    } else if Some(n) == astream && &id[2..] == b"wb" {
                        audio_bytes.extend_from_slice(d.get(off..off + len).unwrap_or(&[]));
                    }
                }
            }
        }
    });
    let Some(vi) = vstream else { return Err("this AVI file has no video".into()) };
    let v = &streams[vi];
    let comp = v.c.to_le_bytes();
    let mjpeg = v.handler.eq_ignore_ascii_case(b"MJPG") || comp.eq_ignore_ascii_case(b"MJPG");
    if !mjpeg {
        let name: String = v.handler.iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '?' }).collect();
        return Err(format!("this AVI file holds {name} video: only Motion-JPEG plays so far (H.264, VP8, VP9 and AV1 need a decoder that is not here yet)"));
    }
    if video_chunks.is_empty() {
        return Err("this AVI file has no pictures in it".into());
    }
    let (w, h) = if v.a > 0 && v.b > 0 { (v.a, v.b.min(0x7FFF_FFFF)) } else { avih_size };
    let frame_ms_num = if v.rate > 0 && v.scale > 0 { 1000.0 * f64::from(v.scale) / f64::from(v.rate) } else { f64::from(usec) / 1000.0 };
    if frame_ms_num <= 0.0 {
        return Err("this AVI file does not say how fast its pictures come".into());
    }
    let frames: Vec<VFrame> = video_chunks.iter().enumerate().map(|(i, (off, len))| VFrame { pts_ms: (i as f64 * frame_ms_num) as u64, off: *off, len: *len }).collect();
    let duration_ms = (video_chunks.len() as f64 * frame_ms_num) as u64;
    let audio = match astream {
        Some(ai) if !audio_bytes.is_empty() => {
            let s = &streams[ai];
            let (tag, ch, rate, bits) = (s.a, s.b as usize, s.c, s.d);
            if tag != 1 || !matches!(bits, 8 | 16) {
                return Err("the sound of this AVI file is not plain PCM, which is all that plays so far".into());
            }
            if !(1..=2).contains(&ch) || !(8_000..=192_000).contains(&rate) {
                return Err("the sound of this AVI file has a format the player does not take (1 or 2 channels, 8 to 192 kHz)".into());
            }
            let pcm: Vec<i16> = if bits == 16 { audio_bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect() } else { audio_bytes.iter().map(|b| (i16::from(*b) - 128) << 8).collect() };
            Some(AviAudio { rate, channels: ch, pcm })
        }
        _ => None,
    };
    Ok(Avi { width: w, height: h, frames, audio, duration_ms })
}

/// Calls `f(id, list type, data offset, data length)` for every chunk between `start` and `end`, going into the lists (`LIST` and `RIFF`).
fn walk(d: &[u8], start: usize, end: usize, f: &mut dyn FnMut(&[u8; 4], &[u8; 4], usize, usize)) {
    let mut p = start;
    while p + 8 <= end.min(d.len()) {
        let id = [d[p], d[p + 1], d[p + 2], d[p + 3]];
        let len = u32le(d, p + 4).unwrap_or(0) as usize;
        let body = p + 8;
        let stop = body.saturating_add(len).min(d.len());
        if &id == b"LIST" || &id == b"RIFF" {
            if body + 4 <= d.len() {
                let kind = [d[body], d[body + 1], d[body + 2], d[body + 3]];
                walk(d, body + 4, stop, f);
                let _ = kind;
            }
        } else {
            f(&id, &[0; 4], body, stop - body);
        }
        p = body.saturating_add(len + (len & 1));
    }
}

/// Picks the picture that is due: the latest one whose time is not after the position, never the same one twice, counting the ones that
/// were skipped because they were late.
pub struct Pacer {
    pts: Vec<u64>,
    last: Option<usize>,
    pub dropped: u64,
    pub shown: u64,
}

impl Pacer {
    pub fn new(pts: Vec<u64>) -> Pacer {
        Pacer { pts, last: None, dropped: 0, shown: 0 }
    }

    /// The index of the picture due at `pos_ms`.
    pub fn due_at(&self, pos_ms: u64) -> usize {
        self.pts.partition_point(|p| *p <= pos_ms).saturating_sub(1)
    }

    /// The index of a picture to show now, or `None` when the one on show is still the one that is due.
    pub fn next(&mut self, pos_ms: u64) -> Option<usize> {
        if self.pts.is_empty() {
            return None;
        }
        let due = self.due_at(pos_ms);
        if Some(due) == self.last {
            return None;
        }
        if let Some(l) = self.last {
            if due > l {
                self.dropped += (due - l - 1) as u64;
            }
        }
        self.last = Some(due);
        self.shown += 1;
        Some(due)
    }

    /// After a seek the next picture is shown whatever it is, and nothing is counted as late.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// A JPEG as RGBA, scaled down (nearest neighbour on a box) until it has at most [`MAX_FRAME_PIXELS`] pixels.
pub fn to_rgba(jpeg: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let bm = iyakki_web::jpeg::decode(jpeg).map_err(|e| format!("a picture of the video could not be read ({e})"))?;
    let (w, h) = (bm.width, bm.height);
    if w == 0 || h == 0 || bm.rgba.len() < w as usize * h as usize * 4 {
        return Err("a picture of the video is empty".into());
    }
    if w * h <= MAX_FRAME_PIXELS {
        return Ok((w, h, bm.rgba));
    }
    let k = (f64::from(MAX_FRAME_PIXELS) / (f64::from(w) * f64::from(h))).sqrt();
    let (nw, nh) = (((f64::from(w) * k) as u32).max(1), ((f64::from(h) * k) as u32).max(1));
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    for y in 0..nh {
        let sy = (u64::from(y) * u64::from(h) / u64::from(nh)) as usize;
        for x in 0..nw {
            let sx = (u64::from(x) * u64::from(w) / u64::from(nw)) as usize;
            let i = (sy * w as usize + sx) * 4;
            out.extend_from_slice(&bm.rgba[i..i + 4]);
        }
    }
    Ok((nw, nh, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pacer_shows_the_due_picture_once_and_counts_the_late_ones() {
        // 25 pictures a second
        let pts: Vec<u64> = (0..50).map(|i| i * 40).collect();
        let mut p = Pacer::new(pts.clone());
        // a fake clock at the pace of the pictures: nothing is dropped
        let mut shown = Vec::new();
        for t in (0..400).step_by(40) {
            if let Some(i) = p.next(t) {
                shown.push(i);
            }
        }
        assert_eq!(shown, (0..10).collect::<Vec<_>>());
        assert_eq!((p.dropped, p.shown), (0, 10));
        // asking twice at the same time gives nothing new
        assert_eq!(p.next(399), None);
        // the clock jumps 240 ms (a slow screen): the due picture shows and the five between are late
        assert_eq!(p.next(600), Some(15));
        assert_eq!(p.dropped, 5);
        // a seek back shows the picture there and counts nothing
        p.reset();
        assert_eq!(p.next(80), Some(2));
        assert_eq!(p.dropped, 5);
        // before the first picture's time the first is on show; past the end the last stays
        let mut q = Pacer::new(pts);
        assert_eq!(q.next(0), Some(0));
        assert_eq!(q.next(1_000_000), Some(49));
        assert_eq!(q.next(2_000_000), None);
    }

    #[test]
    fn the_pacer_follows_a_slower_clock_without_dropping() {
        // 10 pictures a second, the clock runs at half speed (a playback rate of 0.5): every picture still shows
        let mut p = Pacer::new((0..20).map(|i| i * 100).collect());
        let mut n = 0;
        for tick in 0..400u64 {
            if p.next(tick * 5).is_some() {
                n += 1;
            }
        }
        assert_eq!((n, p.dropped), (20, 0));
    }

    #[test]
    fn a_file_that_is_not_motion_jpeg_or_is_damaged_says_so() {
        assert!(!is_avi(b"RIFF....WAVE"));
        assert!(parse(b"hello").is_err());
        let mut x = b"RIFF\x10\x00\x00\x00AVI LIST\x04\x00\x00\x00hdrl".to_vec();
        assert!(parse(&x).unwrap_err().contains("no video"));
        x.truncate(14);
        assert!(parse(&x).is_err());
    }
}

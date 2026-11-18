//! Video files for the camera: Motion-JPEG in an AVI (RIFF) container, which mpv, VLC, ffplay and the web image's player
//! all open. One video stream, every frame a key frame, an `idx1` index at the end; the header is written first with
//! zero counts and filled in by [`AviWriter::finish`] with the real frame count and the frame rate measured while
//! recording. A plain AVI stays under 1 GiB per file; the recorder starts a next part before that (no OpenDML needed).

use std::io::{Seek, SeekFrom, Write};

/// Where a recorder starts the next part.
pub const PART_LIMIT: u64 = 1_000_000_000;

// byte offsets inside the header we write (see `write_header`)
const RIFF_SIZE: u64 = 4;
const AVIH_USEC: u64 = 32;
const AVIH_FRAMES: u64 = 48;
const AVIH_SUGGESTED: u64 = 60;
const STRH_SCALE: u64 = 128;
const STRH_RATE: u64 = 132;
const STRH_LENGTH: u64 = 140;
const STRH_SUGGESTED: u64 = 144;
const MOVI_SIZE: u64 = 216;
const MOVI_START: u64 = 220;

pub struct AviWriter<W: Write + Seek> {
    w: W,
    width: u32,
    height: u32,
    /// (offset from the `movi` tag, size) of every frame.
    index: Vec<(u32, u32)>,
    pos: u64,
    biggest: u32,
}

fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

impl<W: Write + Seek> AviWriter<W> {
    /// Start a file for `width`×`height` MJPEG frames (`fps_hint` until the real rate is known).
    pub fn new(mut w: W, width: u32, height: u32, fps_hint: f32) -> std::io::Result<AviWriter<W>> {
        let h = header(width, height, fps_hint);
        debug_assert_eq!(h.len() as u64, MOVI_START + 4);
        w.write_all(&h)?;
        Ok(AviWriter { w, width, height, index: Vec::new(), pos: h.len() as u64, biggest: 0 })
    }

    /// Add one JPEG frame.
    pub fn frame(&mut self, jpeg: &[u8]) -> std::io::Result<()> {
        let off = (self.pos - MOVI_START) as u32;
        self.w.write_all(b"00dc")?;
        self.w.write_all(&le32(jpeg.len() as u32))?;
        self.w.write_all(jpeg)?;
        let mut n = 8 + jpeg.len() as u64;
        if jpeg.len() % 2 == 1 {
            self.w.write_all(&[0])?;
            n += 1;
        }
        self.index.push((off, jpeg.len() as u32));
        self.biggest = self.biggest.max(jpeg.len() as u32);
        self.pos += n;
        Ok(())
    }

    pub fn frames(&self) -> usize {
        self.index.len()
    }

    /// Bytes written so far (the index adds 16 per frame at the end).
    pub fn bytes(&self) -> u64 {
        self.pos
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Write the index, fill in the counts and the frame rate (`fps`, from the recording's real length) and give the
    /// writer back.
    pub fn finish(mut self, fps: f32) -> std::io::Result<W> {
        // the movi list's size counts from its "movi" tag
        let movi_len = self.pos - MOVI_START;
        self.w.write_all(b"idx1")?;
        self.w.write_all(&le32(self.index.len() as u32 * 16))?;
        for (off, len) in &self.index {
            self.w.write_all(b"00dc")?;
            self.w.write_all(&le32(0x10))?;
            self.w.write_all(&le32(*off))?;
            self.w.write_all(&le32(*len))?;
        }
        let end = self.pos + 8 + self.index.len() as u64 * 16;
        let fps = if fps.is_finite() && fps > 0.5 { fps.min(120.0) } else { 15.0 };
        let (rate, scale) = ((fps * 1000.0).round() as u32, 1000u32);
        let put = |w: &mut W, at: u64, v: u32| -> std::io::Result<()> {
            w.seek(SeekFrom::Start(at))?;
            w.write_all(&le32(v))
        };
        put(&mut self.w, RIFF_SIZE, (end - 8) as u32)?;
        put(&mut self.w, AVIH_USEC, (1_000_000.0 / fps).round() as u32)?;
        put(&mut self.w, AVIH_FRAMES, self.index.len() as u32)?;
        put(&mut self.w, AVIH_SUGGESTED, self.biggest + 8)?;
        put(&mut self.w, STRH_SCALE, scale)?;
        put(&mut self.w, STRH_RATE, rate)?;
        put(&mut self.w, STRH_LENGTH, self.index.len() as u32)?;
        put(&mut self.w, STRH_SUGGESTED, self.biggest + 8)?;
        put(&mut self.w, MOVI_SIZE, movi_len as u32)?;
        self.w.seek(SeekFrom::Start(end))?;
        self.w.flush()?;
        Ok(self.w)
    }
}

/// RIFF header, hdrl (avih, strl with strh and strf), and the start of the movi list.
fn header(width: u32, height: u32, fps: f32) -> Vec<u8> {
    let fps = if fps > 0.5 { fps } else { 15.0 };
    let mut avih = Vec::new();
    for v in [(1_000_000.0 / fps) as u32, 0, 0, 0x10, 0, 0, 1, 0, width, height, 0, 0, 0, 0] {
        avih.extend_from_slice(&le32(v));
    }
    let mut strh = Vec::new();
    strh.extend_from_slice(b"vids");
    strh.extend_from_slice(b"MJPG");
    strh.extend_from_slice(&le32(0)); // flags
    strh.extend_from_slice(&[0, 0, 0, 0]); // priority, language
    for v in [0, 1000, (fps * 1000.0) as u32, 0, 0, 0, u32::MAX, 0] {
        // initial frames, scale, rate, start, length, suggested buffer, quality (-1), sample size
        strh.extend_from_slice(&le32(v));
    }
    for v in [0u16, 0, width as u16, height as u16] {
        strh.extend_from_slice(&v.to_le_bytes());
    }
    let mut strf = Vec::new();
    strf.extend_from_slice(&le32(40));
    strf.extend_from_slice(&le32(width));
    strf.extend_from_slice(&le32(height));
    strf.extend_from_slice(&1u16.to_le_bytes());
    strf.extend_from_slice(&24u16.to_le_bytes());
    strf.extend_from_slice(b"MJPG");
    for v in [width * height * 3, 0, 0, 0, 0] {
        strf.extend_from_slice(&le32(v));
    }
    let chunk = |id: &[u8], body: &[u8]| {
        let mut c = id.to_vec();
        c.extend_from_slice(&le32(body.len() as u32));
        c.extend_from_slice(body);
        c
    };
    let list = |kind: &[u8], body: &[u8]| {
        let mut c = b"LIST".to_vec();
        c.extend_from_slice(&le32(body.len() as u32 + 4));
        c.extend_from_slice(kind);
        c.extend_from_slice(body);
        c
    };
    let mut strl = chunk(b"strh", &strh);
    strl.extend(chunk(b"strf", &strf));
    let mut hdrl = chunk(b"avih", &avih);
    hdrl.extend(list(b"strl", &strl));
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&le32(0));
    out.extend_from_slice(b"AVI ");
    out.extend(list(b"hdrl", &hdrl));
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&le32(4));
    out.extend_from_slice(b"movi");
    out
}

/// What [`parse`] found in an AVI file.
#[derive(Debug, Clone, PartialEq)]
pub struct AviInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: f32,
    pub handler: String,
    /// The frames found in the `movi` list, in order.
    pub chunks: Vec<Vec<u8>>,
    /// (offset, size) entries of `idx1`.
    pub index: Vec<(u32, u32)>,
}

fn rd(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("cut short at {at}"))
}

/// Read an AVI back strictly (sizes must add up), for the tests and for checking a finished recording.
pub fn parse(b: &[u8]) -> Result<AviInfo, String> {
    if b.get(..4) != Some(b"RIFF") || b.get(8..12) != Some(b"AVI ") {
        return Err("not an AVI file".into());
    }
    if rd(b, 4)? as usize + 8 != b.len() {
        return Err(format!("RIFF size {} does not match the file ({} bytes)", rd(b, 4)?, b.len()));
    }
    let mut info = AviInfo { width: 0, height: 0, frames: 0, fps: 0.0, handler: String::new(), chunks: Vec::new(), index: Vec::new() };
    let mut movi_at = None;
    // walk the top level, descending into LIST hdrl / strl / movi
    fn walk(b: &[u8], mut i: usize, end: usize, info: &mut AviInfo, movi_at: &mut Option<usize>) -> Result<(), String> {
        while i + 8 <= end {
            let id = &b[i..i + 4];
            let len = rd(b, i + 4)? as usize;
            let body = i + 8;
            if body + len > end {
                return Err(format!("chunk {} at {i} runs past its parent", String::from_utf8_lossy(id)));
            }
            match id {
                b"LIST" => {
                    let kind = &b[body..body + 4];
                    if kind == b"movi" {
                        *movi_at = Some(body);
                    }
                    walk(b, body + 4, body + len, info, movi_at)?;
                }
                b"avih" => {
                    info.frames = rd(b, body + 16)?;
                    info.width = rd(b, body + 32)?;
                    info.height = rd(b, body + 36)?;
                }
                b"strh" => {
                    info.handler = String::from_utf8_lossy(&b[body + 4..body + 8]).into_owned();
                    let (scale, rate) = (rd(b, body + 20)?, rd(b, body + 24)?);
                    info.fps = if scale > 0 { rate as f32 / scale as f32 } else { 0.0 };
                }
                b"00dc" => info.chunks.push(b[body..body + len].to_vec()),
                b"idx1" => {
                    for e in b[body..body + len].chunks_exact(16) {
                        info.index.push((rd(e, 8)?, rd(e, 12)?));
                    }
                }
                _ => {}
            }
            i = body + len + (len & 1);
        }
        Ok(())
    }
    walk(b, 12, b.len(), &mut info, &mut movi_at)?;
    let movi = movi_at.ok_or("no movi list")?;
    // every index entry points at a 00dc chunk of that size, counted from the movi tag
    for (off, len) in &info.index {
        let at = movi + *off as usize;
        if b.get(at..at + 4) != Some(b"00dc") || rd(b, at + 4)? != *len {
            return Err(format!("index entry at {off} does not point at a frame"));
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_is_a_valid_riff_file_with_its_frames_index_and_rate() {
        let frames: Vec<Vec<u8>> = (0..5).map(|i| {
            let mut f = vec![0xFF, 0xD8];
            f.extend(std::iter::repeat_n(i as u8, 10 + i));
            f.extend_from_slice(&[0xFF, 0xD9]);
            f
        }).collect();
        let mut w = AviWriter::new(std::io::Cursor::new(Vec::new()), 640, 480, 30.0).unwrap();
        for f in &frames {
            w.frame(f).unwrap();
        }
        assert_eq!(w.frames(), 5);
        let bytes = w.finish(12.5).unwrap().into_inner();
        let info = parse(&bytes).unwrap();
        assert_eq!((info.width, info.height, info.frames), (640, 480, 5));
        assert_eq!(info.handler, "MJPG");
        assert!((info.fps - 12.5).abs() < 0.01, "{}", info.fps);
        assert_eq!(info.chunks, frames, "odd-sized frames are padded but come back whole");
        assert_eq!(info.index.len(), 5);
        // the header's own fields sit where the writer patches them
        assert_eq!(u32::from_le_bytes(bytes[AVIH_USEC as usize..][..4].try_into().unwrap()), 80_000);
        assert_eq!(&bytes[MOVI_START as usize..MOVI_START as usize + 4], b"movi");
        assert_eq!(&bytes[(MOVI_SIZE - 4) as usize..MOVI_SIZE as usize], b"LIST");
        // damage is noticed
        assert!(parse(&bytes[..bytes.len() - 3]).is_err());
        assert!(parse(b"RIFF\x04\0\0\0WAVE").is_err());
    }

    #[test]
    fn an_empty_recording_is_still_a_file_players_accept() {
        let bytes = AviWriter::new(std::io::Cursor::new(Vec::new()), 320, 240, 0.0).unwrap().finish(0.0).unwrap().into_inner();
        let info = parse(&bytes).unwrap();
        assert_eq!((info.frames, info.chunks.len()), (0, 0));
        assert!((info.fps - 15.0).abs() < 0.01);
    }
}

//! What a radio stream carries: telling MP3 from AAC (ADTS) by its first bytes, reading ADTS frame headers, skipping
//! ID3 tags, and the seam an AAC decoder plugs into (`aacpipe.rs` implements it with a separate decoder program; no AAC
//! decoding code is part of IYAKKI). Everything here is written from the public format descriptions.

/// The audio format of an elementary stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Mp3,
    Aac,
}

/// What the person is told when a station sends AAC and no AAC decoder is installed. The audio add-on is `ffmpeg`, which
/// the person downloads from its builder when they ask (Store -> Setup); IYAKKI never ships it.
pub const NEEDS_AUDIO_ADDON: &str = "This station sends AAC audio, which needs the audio add-on (ffmpeg). It is downloaded from its builder when you ask; IYAKKI does not ship it.";

/// Whether a failure text is the "needs the audio add-on" one (the RADIO tile then offers to install it).
pub fn needs_addon(why: &str) -> bool {
    why.contains("needs the audio add-on")
}

/// The seam for an AAC decoder: ADTS bytes in, interleaved 16-bit PCM out. Today the decoder is a plain `ffmpeg` process
/// (`aacpipe.rs`); an in-house decoder can take its place by implementing this. It must never panic on bad input.
pub trait AacDecoder: Send {
    /// Hands over bytes of the stream. `Ok(false)`: the decoder is full, offer them again after reading some sound.
    fn feed(&mut self, bytes: &[u8]) -> Result<bool, String>;
    /// No more input will come.
    fn finish(&mut self);
    /// Reads up to `out.len()` samples (waiting up to `wait` for some).
    fn read(&mut self, out: &mut [i16], wait: std::time::Duration) -> Result<Pcm, String>;
    /// Sample rate and channel count of what `read` returns.
    fn format(&self) -> (u32, usize);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pcm {
    /// This many samples (interleaved) were written to the start of `out`.
    Samples(usize),
    /// Nothing yet.
    Pending,
    /// The decoder has finished: everything fed has been played.
    End,
}

pub type AacFactory = std::sync::Arc<dyn Fn() -> Result<Box<dyn AacDecoder>, String> + Send + Sync>;

/// Length of an ID3v2 tag at the start of `b` (header, body and footer), when `b` starts with one.
pub fn id3_len(b: &[u8]) -> Option<usize> {
    if b.len() < 10 || &b[..3] != b"ID3" || b[3] == 0xFF || b[6..10].iter().any(|x| x & 0x80 != 0) {
        return None;
    }
    let size = (b[6] as usize) << 21 | (b[7] as usize) << 14 | (b[8] as usize) << 7 | b[9] as usize;
    Some(10 + size + if b[5] & 0x10 != 0 { 10 } else { 0 })
}

/// Skips ID3 tags at the start; `None` when the data ends inside one (more is needed to know).
pub fn skip_id3(mut b: &[u8]) -> Option<&[u8]> {
    while let Some(n) = id3_len(b) {
        b = b.get(n..)?;
    }
    Some(b)
}

const SAMPLE_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
const CHANNELS: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 8];

/// An ADTS frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adts {
    /// MPEG-4 audio object type: 1 Main, 2 LC, 3 SSR, 4 LTP.
    pub object_type: u8,
    pub sample_rate: u32,
    /// 0 when the layout is in the stream (a program config element).
    pub channels: u8,
    pub frame_len: usize,
    pub header_len: usize,
}

/// Reads an ADTS header at the start of `b`; `None` when it is not one (or not valid).
pub fn parse_adts(b: &[u8]) -> Option<Adts> {
    if b.len() < 7 || b[0] != 0xFF || b[1] & 0xF6 != 0xF0 {
        return None;
    }
    let sfi = ((b[2] >> 2) & 0x0F) as usize;
    let sample_rate = *SAMPLE_RATES.get(sfi)?;
    let channels = CHANNELS[(((b[2] & 1) << 2) | (b[3] >> 6)) as usize];
    let frame_len = ((b[3] & 3) as usize) << 11 | (b[4] as usize) << 3 | (b[5] >> 5) as usize;
    let header_len = if b[1] & 1 == 0 { 9 } else { 7 };
    if frame_len < header_len {
        return None;
    }
    Some(Adts { object_type: (b[2] >> 6) + 1, sample_rate, channels, frame_len, header_len })
}

/// Result of looking for the next ADTS frame in received bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum Scan {
    /// A whole frame starts after `skip` bytes.
    Frame { skip: usize, header: Adts },
    /// A frame starts after `skip` bytes but has not fully arrived.
    NeedMore { skip: usize },
    /// No frame start in these bytes: all of them can be dropped.
    Junk(usize),
}

/// Finds the next ADTS frame. A candidate counts only when the bytes after it start another header (or the stream has
/// ended), so a stray 0xFFF in other data is not taken for audio.
pub fn scan_adts(b: &[u8], eof: bool) -> Scan {
    let mut i = 0;
    while i + 7 <= b.len() {
        if let Some(h) = parse_adts(&b[i..]) {
            let end = i + h.frame_len;
            if end > b.len() {
                if eof {
                    return Scan::Junk(b.len());
                }
                return Scan::NeedMore { skip: i };
            }
            if end + 2 > b.len() && !eof {
                return Scan::NeedMore { skip: i };
            }
            if end + 2 > b.len() || (b[end] == 0xFF && b[end + 1] & 0xF6 == 0xF0) {
                return Scan::Frame { skip: i, header: h };
            }
        }
        i += 1;
    }
    Scan::Junk(i.min(b.len()))
}

/// MPEG audio frame sync (layer I-III, not the reserved version).
fn is_mp3_sync(b: &[u8]) -> bool {
    b.len() >= 2 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0 && (b[1] >> 3) & 3 != 1 && (b[1] >> 1) & 3 != 0
}

/// The format of the audio that starts at `b` (after any ID3 tag): `None` when it is neither.
pub fn sniff(b: &[u8]) -> Option<Codec> {
    let b = skip_id3(b)?;
    if b.len() >= 2 && b[0] == 0xFF && b[1] & 0xF6 == 0xF0 {
        return Some(Codec::Aac);
    }
    is_mp3_sync(b).then_some(Codec::Mp3)
}

/// Audio content types that mean AAC.
pub fn content_type_is_aac(ct: &str) -> bool {
    ["audio/aac", "audio/aacp", "audio/x-aac", "audio/mp4a-latm", "audio/x-hx-aac-adts"].iter().any(|t| ct.starts_with(t))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An ADTS frame of `len` bytes: LC, 44.1 kHz, stereo.
    pub(crate) fn adts_frame(len: usize) -> Vec<u8> {
        let mut f = vec![0u8; len];
        f[0] = 0xFF;
        f[1] = 0xF1;
        f[2] = 0x50; // LC, index 4 (44100), channel config high bit 0
        f[3] = 0x80 | ((len >> 11) & 3) as u8; // channel config 2
        f[4] = (len >> 3) as u8;
        f[5] = (((len & 7) << 5) | 0x1F) as u8;
        f[6] = 0xFC;
        f
    }

    #[test]
    fn adts_headers_are_read_and_bad_ones_refused() {
        let h = parse_adts(&adts_frame(200)).unwrap();
        assert_eq!((h.object_type, h.sample_rate, h.channels, h.frame_len, h.header_len), (2, 44100, 2, 200, 7));
        let mut crc = adts_frame(200);
        crc[1] = 0xF0;
        assert_eq!(parse_adts(&crc).unwrap().header_len, 9);
        let mut bad_rate = adts_frame(200);
        bad_rate[2] |= 0x3C; // index 15
        assert!(parse_adts(&bad_rate).is_none());
        let mut short = adts_frame(200);
        short[4] = 0;
        short[3] &= 0xFC;
        short[5] = 0x1F | (5 << 5);
        assert!(parse_adts(&short).is_none(), "shorter than its own header");
        assert!(parse_adts(&[0xFF, 0xFB, 0x90, 0x64, 0, 0, 0]).is_none(), "an MP3 header is not ADTS");
        assert!(parse_adts(&[0xFF; 3]).is_none());
    }

    #[test]
    fn frames_are_found_after_junk_and_a_false_sync_is_ignored() {
        let mut s = vec![1, 2, 3, 4, 5, 6];
        s.extend(adts_frame(100));
        s.extend(adts_frame(100));
        assert!(matches!(scan_adts(&s, false), Scan::Frame { skip: 6, header } if header.frame_len == 100));
        // the second frame has not fully arrived: the first still plays once its successor's start is seen
        assert!(matches!(scan_adts(&s[..6 + 100 + 1], false), Scan::NeedMore { skip: 6 }));
        assert!(matches!(scan_adts(&s[..6 + 50], false), Scan::NeedMore { skip: 6 }));
        assert!(matches!(scan_adts(&s[..6 + 100], true), Scan::Frame { skip: 6, .. }), "at the end of the stream the last frame counts");
        // a header-looking pair followed by something that is not a header
        let mut f = adts_frame(100);
        f.extend(vec![0u8; 50]);
        assert!(matches!(scan_adts(&f, false), Scan::Junk(_)));
        assert_eq!(scan_adts(&[], false), Scan::Junk(0));
    }

    #[test]
    fn formats_are_told_apart_by_their_first_bytes() {
        assert_eq!(sniff(&adts_frame(100)), Some(Codec::Aac));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x64]), Some(Codec::Mp3));
        assert_eq!(sniff(&[0xFF, 0xF3, 0x90, 0x64]), Some(Codec::Mp3), "MPEG-2 layer III");
        assert_eq!(sniff(&[0xFF, 0xE9, 0, 0]), None, "reserved version");
        assert_eq!(sniff(b"RIFF....."), None);
        let mut tagged = b"ID3\x04\x00\x00\x00\x00\x00\x05hello".to_vec();
        tagged.extend(adts_frame(30));
        assert_eq!(sniff(&tagged), Some(Codec::Aac));
        assert_eq!(sniff(&tagged[..12]), None, "the tag is not finished");
        assert_eq!(id3_len(b"ID3\x04\x00\x10\x00\x00\x00\x05"), Some(25), "a footer adds ten bytes");
        assert!(id3_len(b"ID3\x04\x00\x00\x80\x00\x00\x05").is_none(), "size bytes are 7-bit");
        assert!(content_type_is_aac("audio/aacp") && content_type_is_aac("audio/mp4a-latm") && !content_type_is_aac("audio/mpeg"));
    }

    #[test]
    fn the_add_on_message_is_recognised() {
        assert!(needs_addon(NEEDS_AUDIO_ADDON) && !needs_addon("the station answered 404"));
    }

    #[test]
    fn random_bytes_never_panic_the_scanner() {
        let mut x = 0x1234_5678u32;
        for n in 0..300 {
            let b: Vec<u8> = (0..n * 3).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; if x % 7 == 0 { 0xFF } else { (x >> 8) as u8 } }).collect();
            let _ = scan_adts(&b, n % 2 == 0);
            let _ = sniff(&b);
            let _ = skip_id3(&b);
        }
    }
}

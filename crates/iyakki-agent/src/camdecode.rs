//! Camera video and sound that IYAKKI does not decode itself (F-CAM-20): H.265 (and H.264) video and AAC sound go through the person's own
//! FFmpeg, the audio and video add-on that they install at their click (D-AAC-ADDON, LICENSING.md items 10 and 11). No codec is shipped
//! and no codec table is typed in here.
//!
//! * video: Annex-B access units on the program's standard input, a stream of JPEG pictures on its standard output (`image2pipe`, `mjpeg`).
//!   [`JpegSplitter`] cuts the stream into whole pictures; the newest one is decoded to RGBA by the in-house JPEG decoder;
//! * sound: the camera's raw AAC frames get an ADTS header made from the track's `AudioSpecificConfig` ([`parse_asc`], [`adts_wrap`]) and go
//!   through the same program the radio uses ([`crate::aacpipe::aac_decoder`]); the stereo it writes is mixed down to mono for the listener.
//!
//! The program starts with a fixed argument list and no shell, only when a camera that needs it is opened (the person added the camera and
//! opened it or asked for its recording or sound). It is killed when the feed ends. Nothing is started while the add-on is not installed:
//! the feed says so in words ([`H264Support::hint`]).

use std::sync::Arc;
use std::time::Duration;

use iyakki_rtsp::rtp::AccessUnit;

use crate::cameras::{H264Decoder, H264Support};

/// The video codecs a camera may send that need the add-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoKind {
    H264,
    H265,
}

/// The fixed arguments of the video decoder: raw Annex-B in, one JPEG per picture out, as little buffering as the program allows.
pub fn video_args(kind: VideoKind) -> Vec<String> {
    let fmt = match kind {
        VideoKind::H264 => "h264",
        VideoKind::H265 => "hevc",
    };
    ["-hide_banner", "-loglevel", "error", "-fflags", "nobuffer", "-flags", "low_delay", "-probesize", "32768", "-analyzeduration", "0", "-f", fmt, "-i", "pipe:0", "-an", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2pipe", "pipe:1"].map(String::from).to_vec()
}

// ------------------------------------------------------------------------------------------------ pictures

/// Cuts a stream of JPEG pictures into whole files. Bytes that belong to no picture (a program that began mid-way, output that was dropped
/// because nobody read it) are skipped up to the next start of image; nothing grows without bound.
#[derive(Default)]
pub struct JpegSplitter {
    buf: Vec<u8>,
}

/// The longest picture kept (a 4K camera at a high quality stays well below).
pub const MAX_JPEG: usize = 4 << 20;

impl JpegSplitter {
    pub fn new() -> JpegSplitter {
        JpegSplitter::default()
    }

    /// Hands over bytes; returns the pictures that are complete now.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            // the start of an image: FF D8 FF
            let Some(s) = self.buf.windows(3).position(|w| w == [0xff, 0xd8, 0xff]) else {
                // keep at most the last two bytes: they may be the start of the next marker
                let keep = self.buf.len().min(2);
                let n = self.buf.len() - keep;
                self.buf.drain(..n);
                break;
            };
            self.buf.drain(..s);
            match jpeg_end(&self.buf) {
                Some(e) => out.push(self.buf.drain(..e).collect()),
                None => {
                    if self.buf.len() > MAX_JPEG {
                        // a picture that never ends: drop its start so the search goes on after it
                        self.buf.drain(..3);
                        continue;
                    }
                    break;
                }
            }
        }
        out
    }
}

/// The length of the JPEG at the start of `b`, found by walking its marker segments and then its entropy-coded data up to the end of image
/// marker; `None` while it is not complete.
fn jpeg_end(b: &[u8]) -> Option<usize> {
    let mut i = 2;
    // marker segments up to the start of scan
    loop {
        if *b.get(i)? != 0xff {
            return Some(find_soi_after(b));
        }
        let m = *b.get(i + 1)?;
        match m {
            0xff => i += 1,
            0xd8 | 0x01 | 0xd0..=0xd7 => i += 2,
            0xd9 => return Some(i + 2),
            _ => {
                let len = u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]) as usize;
                if len < 2 {
                    return Some(find_soi_after(b));
                }
                i += 2 + len;
                if m == 0xda {
                    break;
                }
            }
        }
    }
    // entropy-coded data: a 0xFF is followed by 0x00 (stuffing), a restart marker or the end of image
    while i + 1 < b.len() {
        if b[i] == 0xff {
            match b[i + 1] {
                0x00 | 0xd0..=0xd7 | 0xff => {}
                0xd9 => return Some(i + 2),
                // another marker segment (a further scan or tables): walk it
                _ => {
                    let len = u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]) as usize;
                    if len < 2 {
                        return Some(find_soi_after(b));
                    }
                    i += 2 + len;
                    continue;
                }
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

/// A damaged picture: where the next one starts (the damaged bytes go with it), or the whole buffer.
fn find_soi_after(b: &[u8]) -> usize {
    b.windows(3).skip(1).position(|w| w == [0xff, 0xd8, 0xff]).map(|p| p + 1).unwrap_or(b.len())
}

// ------------------------------------------------------------------------------------------------ the video decoder

/// What turns a JPEG into straight RGBA (width, height, bytes).
pub type ToRgba = fn(&[u8]) -> Option<(u32, u32, Vec<u8>)>;

/// The in-house JPEG decoder.
pub fn jpeg_to_rgba(j: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let b = iyakki_web::jpeg::decode(j).ok()?;
    Some((b.width as u32, b.height as u32, b.rgba))
}

/// The add-on's FFmpeg as a camera decoder: access units in, the newest finished picture out.
pub struct AddonVideoDecoder {
    pipe: crate::btaac::Pipe,
    split: JpegSplitter,
    to_rgba: ToRgba,
}

impl AddonVideoDecoder {
    pub fn spawn(program: &str, kind: VideoKind) -> Result<AddonVideoDecoder, String> {
        Self::spawn_with(program, &video_args(kind), jpeg_to_rgba)
    }

    /// A program and argument list of the caller's (tests stand a script in for FFmpeg).
    pub fn spawn_with(program: &str, args: &[String], to_rgba: ToRgba) -> Result<AddonVideoDecoder, String> {
        Ok(AddonVideoDecoder { pipe: crate::btaac::Pipe::spawn(program, args)?, split: JpegSplitter::new(), to_rgba })
    }

    /// Waits until the program has finished its output (tests and orderly ends).
    pub fn finish_and_wait(&mut self, limit: Duration) -> bool {
        self.pipe.finish();
        self.pipe.wait_end(limit)
    }

    /// The newest finished picture the program has written so far.
    pub fn newest(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        let bytes = self.pipe.take()?;
        let pics = self.split.push(&bytes);
        Ok(pics.iter().rev().find_map(|p| (self.to_rgba)(p)))
    }
}

impl H264Decoder for AddonVideoDecoder {
    fn decode(&mut self, au: &AccessUnit) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        self.pipe.put(au.annex_b())?;
        self.newest()
    }
}

/// The FFmpeg of the audio and video add-on, if one is installed and starts.
pub fn ffmpeg() -> Option<String> {
    crate::player::find_program("ffmpeg").filter(|p| crate::player::runs(p))
}

/// The camera hub's support for codecs that need the add-on: H.264 and H.265 pictures and AAC sound through the person's FFmpeg. With none
/// installed every method says so and nothing starts.
pub struct AddonCodecs;

pub const H264_HINT: &str = "This camera sends H.264 video. Showing it needs the audio and video add-on (Store, Setup), which you download yourself. Until then, switch the camera to Motion-JPEG or use its snapshot address.";
pub const H265_HINT: &str = "This camera sends H.265 video. Showing it needs the audio and video add-on (Store, Setup), which you download yourself. Until then, switch the camera to Motion-JPEG or H.264 or use its snapshot address.";

impl H264Support for AddonCodecs {
    fn decoder(&self) -> Option<Box<dyn H264Decoder>> {
        self.video_decoder(VideoKind::H264)
    }
    fn video_decoder(&self, kind: VideoKind) -> Option<Box<dyn H264Decoder>> {
        let f = ffmpeg()?;
        AddonVideoDecoder::spawn(&f, kind).ok().map(|d| Box::new(d) as Box<dyn H264Decoder>)
    }
    fn aac_decoder(&self) -> Option<Box<dyn crate::audiofmt::AacDecoder>> {
        crate::aacpipe::aac_decoder().ok()
    }
    fn hint(&self) -> String {
        H264_HINT.into()
    }
    fn hint_for(&self, kind: VideoKind) -> String {
        match kind {
            VideoKind::H264 => H264_HINT.into(),
            VideoKind::H265 => H265_HINT.into(),
        }
    }
}

// ------------------------------------------------------------------------------------------------ AAC sound

/// What the `AudioSpecificConfig` of an AAC track says (ISO 14496-3): enough for an ADTS header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asc {
    /// The audio object type (1 main, 2 LC, 3 SSR, 4 LTP).
    pub object: u8,
    /// The index in the sampling-rate table.
    pub rate_index: u8,
    /// The channel configuration (1 to 7).
    pub channels: u8,
}

const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

impl Asc {
    pub fn rate(&self) -> u32 {
        RATES[self.rate_index as usize]
    }
}

/// Reads the first bits of an `AudioSpecificConfig`. Object types that an ADTS header cannot carry (SBR, escapes), an explicit sampling
/// rate and a channel configuration of 0 are refused: the person is then told the track is not one this route can carry.
pub fn parse_asc(config: &[u8]) -> Option<Asc> {
    if config.len() < 2 {
        return None;
    }
    let bits = u16::from_be_bytes([config[0], config[1]]);
    let object = (bits >> 11) as u8;
    let rate_index = ((bits >> 7) & 0xf) as u8;
    let channels = ((bits >> 3) & 0xf) as u8;
    ((1..=4).contains(&object) && (rate_index as usize) < RATES.len() && (1..=7).contains(&channels)).then_some(Asc { object, rate_index, channels })
}

/// A raw AAC frame with its ADTS header (no CRC). `None` for a frame that does not fit the 13-bit length.
pub fn adts_wrap(asc: &Asc, frame: &[u8]) -> Option<Vec<u8>> {
    let total = frame.len() + 7;
    if frame.is_empty() || total > 0x1fff {
        return None;
    }
    let (profile, sf, ch) = (asc.object - 1, asc.rate_index, asc.channels);
    let mut v = vec![0xff, 0xf1, profile << 6 | sf << 2 | (ch >> 2) & 1, (ch & 3) << 6 | (total >> 11) as u8 & 3, (total >> 3) as u8, ((total & 7) as u8) << 5 | 0x1f, 0xfc];
    v.extend_from_slice(frame);
    Some(v)
}

/// Stereo (or more) interleaved samples mixed down to one channel.
pub fn to_mono(pcm: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return pcm.to_vec();
    }
    pcm.chunks_exact(channels).map(|f| (f.iter().map(|s| *s as i32).sum::<i32>() / channels as i32) as i16).collect()
}

/// One camera's AAC sound: the frames go in, mono samples come out. A decoder that stops is dropped and the sound ends with its words.
pub struct AacTrack {
    asc: Asc,
    dec: Box<dyn crate::audiofmt::AacDecoder>,
}

impl AacTrack {
    /// `config` is the track's `AudioSpecificConfig` from the camera's description.
    pub fn new(config: &[u8], dec: Box<dyn crate::audiofmt::AacDecoder>) -> Result<AacTrack, String> {
        let asc = parse_asc(config).ok_or("this camera's AAC sound is not a kind the add-on route can take (SBR, an unusual rate or channel layout)")?;
        Ok(AacTrack { asc, dec })
    }

    /// Gives one frame; returns what has been decoded so far as (rate, mono samples).
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<(u32, Vec<i16>)>, String> {
        if let Some(adts) = adts_wrap(&self.asc, frame) {
            self.dec.feed(&adts)?;
        }
        let mut out = Vec::new();
        let mut buf = vec![0i16; 8192];
        loop {
            match self.dec.read(&mut buf, Duration::ZERO)? {
                crate::audiofmt::Pcm::Samples(n) => out.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }
        if out.is_empty() {
            return Ok(None);
        }
        let (rate, ch) = self.dec.format();
        Ok(Some((rate, to_mono(&out, ch.max(1)))))
    }
}

/// For callers that hold the support as a trait object.
pub fn addon() -> Arc<dyn H264Support> {
    Arc::new(AddonCodecs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiofmt::{AacDecoder, Pcm};

    /// A small JPEG-shaped file: tables, a start of scan, entropy data with a stuffed 0xFF and a restart marker, the end of image.
    fn pic(tag: u8) -> Vec<u8> {
        let mut v = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x01, 0x02, 0xff, 0xdb, 0x00, 0x05, 1, 2, 3, 0xff, 0xda, 0x00, 0x04, 9, 9];
        v.extend_from_slice(&[tag, 0xff, 0x00, 0x11, 0xff, 0xd0, 0x22, 0xff, 0xd9]);
        v
    }

    #[test]
    fn the_video_program_gets_a_fixed_argument_list_for_each_codec() {
        let h = video_args(VideoKind::H265);
        assert!(h.windows(2).any(|w| w == ["-f", "hevc"]) && h.windows(2).any(|w| w == ["-i", "pipe:0"]) && h.ends_with(&["-f".to_string(), "image2pipe".to_string(), "pipe:1".to_string()]));
        assert!(video_args(VideoKind::H264).windows(2).any(|w| w == ["-f", "h264"]));
        assert!(h.contains(&"-an".to_string()), "the video program never decodes the camera's sound");
    }

    #[test]
    fn a_stream_of_pictures_is_cut_whole_wherever_the_chunks_end() {
        let all: Vec<u8> = [pic(1), pic(2), pic(3)].concat();
        for cut in 0..=all.len() {
            let mut s = JpegSplitter::new();
            let mut got = s.push(&all[..cut]);
            got.extend(s.push(&all[cut..]));
            assert_eq!(got, vec![pic(1), pic(2), pic(3)], "split at {cut}");
        }
        // byte by byte
        let mut s = JpegSplitter::new();
        let got: Vec<Vec<u8>> = all.iter().flat_map(|b| s.push(&[*b])).collect();
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn junk_before_a_picture_an_unfinished_one_and_a_flood_without_pictures_do_not_confuse_or_grow() {
        let mut s = JpegSplitter::new();
        let mut data = vec![0x00, 0x01, 0xff, 0xd9, 0x55];
        data.extend(pic(7));
        data.extend_from_slice(&pic(8)[..10]);
        assert_eq!(s.push(&data), vec![pic(7)], "the unfinished picture waits");
        assert_eq!(s.push(&pic(8)[10..]), vec![pic(8)]);
        let mut flood = JpegSplitter::new();
        for _ in 0..64 {
            assert!(flood.push(&vec![0xab; 1 << 20]).is_empty());
        }
        assert!(flood.buf.len() <= 2, "bytes that start no picture are not kept: {}", flood.buf.len());
        let mut never = JpegSplitter::new();
        let mut start = vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0x04, 0, 0, 0xff, 0xda, 0x00, 0x02];
        start.extend(vec![0x41; MAX_JPEG + 10]);
        never.push(&start);
        assert!(never.buf.len() <= MAX_JPEG + 20, "a picture that never ends is bounded: {}", never.buf.len());
        let mut hurt = JpegSplitter::new();
        let mut d = pic(1)[..12].to_vec();
        d.extend(pic(2));
        let out = hurt.push(&d);
        assert_eq!(out.last(), Some(&pic(2)), "a damaged picture is skipped and the next one is whole: {} pictures", out.len());
    }

    #[test]
    fn the_audio_config_of_an_aac_track_makes_the_adts_header_and_odd_ones_are_refused() {
        // AAC-LC, 44.1 kHz, stereo: 00010 0100 0010 000
        let asc = parse_asc(&[0x12, 0x10]).unwrap();
        assert_eq!(asc, Asc { object: 2, rate_index: 4, channels: 2 });
        assert_eq!(asc.rate(), 44100);
        let w = adts_wrap(&asc, &[0u8; 10]).unwrap();
        assert_eq!(&w[..7], &[0xff, 0xf1, 0x50, 0x80, 0x02, 0x3f, 0xfc], "header of a 17 byte frame");
        assert_eq!(w.len(), 17);
        assert!(adts_wrap(&asc, &[]).is_none() && adts_wrap(&asc, &vec![0; 0x2000]).is_none());
        // 48 kHz mono LC: 00010 0011 0001 000
        assert_eq!(parse_asc(&[0x11, 0x88]).unwrap(), Asc { object: 2, rate_index: 3, channels: 1 });
        // SBR (object 5), channel configuration 0, an explicit rate (index 15) and a short config
        for bad in [&[0x2b, 0x10][..], &[0x12, 0x00], &[0x17, 0x90], &[0x12]] {
            assert!(parse_asc(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn channels_are_mixed_down_to_one() {
        assert_eq!(to_mono(&[100, 300, -50, 50], 2), vec![200, 0]);
        assert_eq!(to_mono(&[1, 2, 3], 1), vec![1, 2, 3]);
        assert!(to_mono(&[], 2).is_empty());
    }

    struct FakeAac {
        fed: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        out: Vec<i16>,
    }
    impl AacDecoder for FakeAac {
        fn feed(&mut self, b: &[u8]) -> Result<bool, String> {
            self.fed.lock().unwrap().push(b.to_vec());
            Ok(true)
        }
        fn finish(&mut self) {}
        fn read(&mut self, out: &mut [i16], _: Duration) -> Result<Pcm, String> {
            if self.out.is_empty() {
                return Ok(Pcm::Pending);
            }
            let n = self.out.len().min(out.len());
            out[..n].copy_from_slice(&self.out[..n]);
            self.out.drain(..n);
            Ok(Pcm::Samples(n))
        }
        fn format(&self) -> (u32, usize) {
            (48000, 2)
        }
    }

    #[test]
    fn camera_aac_goes_through_the_decoder_with_an_adts_header_and_comes_out_mono() {
        let fed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut t = AacTrack::new(&[0x12, 0x10], Box::new(FakeAac { fed: fed.clone(), out: vec![10, 30, 100, 300] })).unwrap();
        let (rate, pcm) = t.push(&[1, 2, 3]).unwrap().unwrap();
        assert_eq!((rate, pcm), (48000, vec![20, 200]));
        let f = fed.lock().unwrap();
        assert_eq!((f[0].len(), &f[0][7..]), (10, &[1u8, 2, 3][..]), "the decoder gets an ADTS frame");
        drop(f);
        assert!(t.push(&[4]).unwrap().is_none(), "nothing decoded yet is not an error");
        assert!(AacTrack::new(&[0x2b, 0x10], Box::new(FakeAac { fed, out: vec![] })).is_err(), "a track the route cannot carry is refused in words");
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_writes_pictures_gives_the_newest_whole_one_and_a_missing_program_is_told_in_words() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testdir::unique("camdecode", "script");
        std::fs::create_dir_all(&dir).unwrap();
        let data = dir.join("out.bin");
        std::fs::write(&data, [pic(1), pic(2)].concat()).unwrap();
        let script = dir.join("fakeffmpeg");
        std::fs::write(&script, format!("#!/bin/sh\ncat '{}'\n", data.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // the stand-in for the JPEG decoder reads the tag byte, so the test knows which picture came out
        fn tag(j: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
            Some((1, 1, vec![j[21], 0, 0, 255]))
        }
        let mut d = AddonVideoDecoder::spawn_with(script.to_str().unwrap(), &[], tag).unwrap();
        assert!(d.finish_and_wait(Duration::from_secs(20)), "the program ended its output");
        let got = d.newest().unwrap().unwrap();
        assert_eq!(got, (1, 1, vec![2, 0, 0, 255]), "the newest of two pictures");
        assert!(AddonVideoDecoder::spawn_with("/nonexistent/iyakki-no-ffmpeg", &[], tag).err().is_some_and(|e| e.contains("add-on")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_the_add_on_nothing_starts_and_the_person_is_told_which_codec_needs_it() {
        struct Nothing;
        impl H264Support for Nothing {
            fn decoder(&self) -> Option<Box<dyn H264Decoder>> {
                None
            }
            fn hint(&self) -> String {
                H264_HINT.into()
            }
        }
        assert!(Nothing.video_decoder(VideoKind::H265).is_none() && Nothing.aac_decoder().is_none());
        assert!(Nothing.hint_for(VideoKind::H265).contains("H.265") && Nothing.hint_for(VideoKind::H264).contains("H.264"));
        assert!(H265_HINT.contains("add-on") && H265_HINT.contains("download yourself"));
    }
}

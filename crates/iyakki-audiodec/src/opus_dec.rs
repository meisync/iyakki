//! The Opus decoder of a whole stream (RFC 6716 section 4, RFC 7845): takes the packets of an Ogg Opus file in order and makes 48 kHz
//! sound, joining the CELT decoder ([`crate::opus_celt`]) and the SILK decoder ([`crate::opus_silk`]).
//!
//! What this layer decides: which decoder reads a frame (SILK-only, CELT-only, or both in hybrid mode where SILK carries the speech
//! band below 8 kHz and CELT the rest on top of it); how the two states are reset when the mode changes; the short redundant CELT
//! frames the encoder adds around a mode change (their range-coded bytes sit after the speech data and are cross-faded in); the five
//! millisecond smoothing at a change between CELT and the other modes; the concealment of a frame that is missing (empty or one byte
//! long, or a packet that did not arrive); and the output gain of the stream header.

use crate::opus::{parse_packet, Bandwidth, Head, Mode, OpusError, RangeDecoder};
use crate::opus_celt::{CeltDecoder, FrameParams};
use crate::opus_silk::SilkDecoder;

const F20: usize = 960;
#[allow(dead_code)] // the 10 ms frame size: kept so the table of frame sizes (20, 10, 5, 2.5 ms) stays complete
const F10: usize = 480;
const F5: usize = 240;
const F2_5: usize = 120;

fn celt_end_band(b: Bandwidth) -> usize {
    match b {
        Bandwidth::Narrow => 13,
        Bandwidth::Medium | Bandwidth::Wide => 17,
        Bandwidth::SuperWide => 19,
        Bandwidth::Full => 21,
    }
}

fn lm_of(samples: usize) -> usize {
    match samples {
        0..=120 => 0,
        121..=240 => 1,
        241..=480 => 2,
        _ => 3,
    }
}

/// `out = w * in2 + (1 - w) * in1` with the power-complementary window over `overlap` samples per channel.
fn smooth_fade(in1: &[f32], in2: &[f32], out: &mut [f32], overlap: usize, channels: usize, window: &[f32]) {
    for c in 0..channels {
        for i in 0..overlap {
            let w = window[i] * window[i];
            let (a, b) = (in1[i * channels + c], in2[i * channels + c]);
            out[i * channels + c] = w * b + (1.0 - w) * a;
        }
    }
}

/// What the table of contents of the last real packet said.
#[derive(Clone, Copy)]
struct Current {
    mode: Mode,
    bandwidth: Bandwidth,
    frame_size: usize,
    stream_channels: usize,
}

pub struct OpusDecoder {
    channels: usize,
    celt: CeltDecoder,
    silk: SilkDecoder,
    cur: Option<Current>,
    prev_mode: Option<Mode>,
    prev_redundancy: bool,
    /// Output gain, linear.
    gain: f32,
    window: [f32; 120],
}

impl OpusDecoder {
    pub fn new(head: &Head) -> OpusDecoder {
        let channels = (head.channels as usize).clamp(1, 2);
        let window = std::array::from_fn(|i| {
            let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / 120.0).sin();
            (0.5 * std::f64::consts::PI * s * s).sin() as f32
        });
        OpusDecoder { channels, celt: CeltDecoder::new(channels), silk: SilkDecoder::new(), cur: None, prev_mode: None, prev_redundancy: false, gain: 10f32.powf(head.gain as f32 / (20.0 * 256.0)), window }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Decodes one packet; returns the sound at 48 kHz, interleaved, plus-minus one full scale. Pre-skip is not removed here.
    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<f32>, OpusError> {
        let pk = parse_packet(packet)?;
        self.cur = Some(Current { mode: pk.toc.mode, bandwidth: pk.toc.bandwidth, frame_size: pk.toc.frame_samples(), stream_channels: if pk.toc.stereo { 2 } else { 1 } });
        let mut out = Vec::with_capacity(pk.samples() * self.channels);
        for f in &pk.frames {
            let fs = pk.toc.frame_samples();
            let data = if f.len() <= 1 { None } else { Some(*f) };
            out.extend(self.decode_frame(data, fs));
        }
        self.apply_gain(&mut out);
        Ok(out)
    }

    /// Makes `samples` samples (per channel, at 48 kHz) for a packet that did not arrive.
    pub fn conceal(&mut self, samples: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(samples * self.channels);
        let mut left = samples;
        while left > 0 {
            let n = left.min(F20);
            out.extend(self.decode_frame(None, n));
            left -= n;
        }
        self.apply_gain(&mut out);
        out
    }

    fn apply_gain(&self, out: &mut [f32]) {
        if (self.gain - 1.0).abs() > 1e-6 {
            out.iter_mut().for_each(|v| *v *= self.gain);
        }
    }

    fn silk_rate(mode: Mode, bw: Bandwidth) -> i32 {
        if mode == Mode::Hybrid {
            return 16000;
        }
        match bw {
            Bandwidth::Narrow => 8000,
            Bandwidth::Medium => 12000,
            _ => 16000,
        }
    }

    /// One frame (the standard's `opus_decode_frame`): `data` is None for a missing frame.
    fn decode_frame(&mut self, data: Option<&[u8]>, frame_size_in: usize) -> Vec<f32> {
        let ch = self.channels;
        let cur = match self.cur {
            Some(c) => c,
            None => return vec![0.0; frame_size_in * ch],
        };
        let (audiosize, mode, mut frame_size) = if data.is_some() {
            (cur.frame_size, cur.mode, frame_size_in)
        } else {
            let fs = frame_size_in;
            match self.prev_mode {
                None => return vec![0.0; fs * ch],
                Some(m) => (fs, m, fs),
            }
        };
        // a missing frame is concealed in pieces of at most 20 ms in the modes that have CELT
        if data.is_none() && frame_size > F20 && mode != Mode::Silk {
            let mut out = Vec::new();
            let mut done = 0;
            while done < frame_size {
                out.extend(self.decode_frame(None, F20));
                done += F20;
            }
            return out;
        }
        let stream_ch = cur.stream_channels;
        let mut transition = false;
        let mut pcm_transition = Vec::new();
        if data.is_some() {
            if let Some(pm) = self.prev_mode {
                if (mode == Mode::Celt && pm != Mode::Celt && !self.prev_redundancy) || (mode != Mode::Celt && pm == Mode::Celt) {
                    transition = true;
                    if mode == Mode::Celt {
                        pcm_transition = self.decode_frame(None, F5.min(audiosize));
                    }
                }
            }
        }
        frame_size = audiosize;
        let mut dec = RangeDecoder::new(data.unwrap_or(&[]));
        let mut len = data.map_or(0, |d| d.len());
        // the speech layer
        let mut pcm_silk: Vec<i16> = Vec::new();
        if mode != Mode::Celt {
            if self.prev_mode == Some(Mode::Celt) {
                self.silk = SilkDecoder::new();
            }
            let rate = Self::silk_rate(cur.mode, cur.bandwidth);
            pcm_silk = self.silk.decode_opus_frame(&mut dec, rate, stream_ch, ch, frame_size, data.is_none());
            pcm_silk.resize(frame_size * ch, 0);
        }
        // a redundant CELT frame after the speech data
        let (mut redundancy, mut celt_to_silk, mut redundancy_bytes) = (false, false, 0usize);
        if data.is_some() && mode != Mode::Celt && (dec.tell() + 17 + if cur.mode == Mode::Hybrid { 20 } else { 0 }) as usize <= 8 * len {
            redundancy = if mode == Mode::Hybrid { dec.bit_logp(12) } else { true };
            if redundancy {
                celt_to_silk = dec.bit_logp(1);
                redundancy_bytes = if mode == Mode::Hybrid { dec.uint(256) as usize + 2 } else { len.saturating_sub(((dec.tell() + 7) >> 3) as usize) };
                if redundancy_bytes > len {
                    redundancy_bytes = len + 1;
                }
                if len < redundancy_bytes || (len - redundancy_bytes) * 8 < dec.tell() as usize {
                    len = 0;
                    redundancy_bytes = 0;
                    redundancy = false;
                } else {
                    len -= redundancy_bytes;
                }
                dec.truncate_end(redundancy_bytes);
            }
        }
        let start_band = if mode != Mode::Celt { 17 } else { 0 };
        let end_band = celt_end_band(cur.bandwidth);
        if redundancy {
            transition = false;
        }
        if transition && mode != Mode::Celt {
            pcm_transition = self.decode_frame(None, F5.min(audiosize));
        }
        let redundant_data: &[u8] = match data {
            Some(d) if redundancy => &d[len..len + redundancy_bytes],
            _ => &[],
        };
        let decode_redundant = |celt: &mut CeltDecoder| -> Vec<f32> { celt.decode(redundant_data, FrameParams { stream_channels: stream_ch, end_band, lm: 1, start: 0 }) };
        let mut redundant_audio = Vec::new();
        if redundancy && celt_to_silk {
            redundant_audio = decode_redundant(&mut self.celt);
        }
        // the CELT part (alone, or on top of the speech layer)
        let mut pcm: Vec<f32>;
        if mode != Mode::Silk {
            let celt_frame = F20.min(frame_size);
            let lm = lm_of(celt_frame);
            if Some(mode) != self.prev_mode && self.prev_mode.is_some() && !self.prev_redundancy {
                self.celt.reset();
            }
            pcm = if data.is_some() {
                self.celt.decode_frame(&mut dec, FrameParams { stream_channels: stream_ch, end_band, lm, start: start_band })
            } else {
                self.celt.decode_lost(lm, start_band, end_band)
            };
            pcm.resize(frame_size * ch, 0.0);
        } else {
            pcm = vec![0.0; frame_size * ch];
            if self.prev_mode == Some(Mode::Hybrid) && !(redundancy && celt_to_silk && self.prev_redundancy) {
                let silence = self.celt.decode(&[0xFF, 0xFF], FrameParams { stream_channels: stream_ch, end_band, lm: 0, start: 0 });
                let n = silence.len().min(pcm.len());
                pcm[..n].copy_from_slice(&silence[..n]);
            }
        }
        if mode != Mode::Celt {
            for (p, &s) in pcm.iter_mut().zip(&pcm_silk) {
                *p += s as f32 / 32768.0;
            }
        }
        if redundancy && !celt_to_silk {
            self.celt.reset();
            redundant_audio = decode_redundant(&mut self.celt);
            if frame_size >= F2_5 && redundant_audio.len() >= F5 * ch {
                let at = (frame_size - F2_5) * ch;
                let src: Vec<f32> = pcm[at..at + F2_5 * ch].to_vec();
                smooth_fade(&src, &redundant_audio[F2_5 * ch..], &mut pcm[at..at + F2_5 * ch], F2_5, ch, &self.window);
            }
        }
        if redundancy && celt_to_silk && redundant_audio.len() >= F5 * ch && frame_size >= F5 {
            pcm[..F2_5 * ch].copy_from_slice(&redundant_audio[..F2_5 * ch]);
            let src: Vec<f32> = pcm[F2_5 * ch..2 * F2_5 * ch].to_vec();
            smooth_fade(&redundant_audio[F2_5 * ch..], &src, &mut pcm[F2_5 * ch..2 * F2_5 * ch], F2_5, ch, &self.window);
        }
        if transition && pcm_transition.len() >= F2_5 * ch {
            if audiosize >= F5 && pcm_transition.len() >= F5 * ch {
                pcm[..F2_5 * ch].copy_from_slice(&pcm_transition[..F2_5 * ch]);
                let src: Vec<f32> = pcm[F2_5 * ch..2 * F2_5 * ch].to_vec();
                smooth_fade(&pcm_transition[F2_5 * ch..], &src, &mut pcm[F2_5 * ch..2 * F2_5 * ch], F2_5, ch, &self.window);
            } else if pcm.len() >= F2_5 * ch {
                let src: Vec<f32> = pcm[..F2_5 * ch].to_vec();
                smooth_fade(&pcm_transition, &src, &mut pcm[..F2_5 * ch], F2_5, ch, &self.window);
            }
        }
        self.prev_mode = Some(mode);
        self.prev_redundancy = redundancy && !celt_to_silk;
        pcm
    }
}

/// Decodes a whole list of Opus audio packets (everything after the two header packets), removing the pre-skip: for tests and tools.
pub fn decode_packets(head: &Head, packets: &[Vec<u8>]) -> Result<Vec<f32>, OpusError> {
    let mut d = OpusDecoder::new(head);
    let mut out = Vec::new();
    for p in packets {
        out.extend(d.decode_packet(p)?);
    }
    let skip = head.pre_skip as usize * d.channels();
    Ok(out.split_off(skip.min(out.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn fixture(name: &str) -> (Head, Vec<Vec<u8>>, Vec<i16>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opus");
        let bytes = std::fs::read(dir.join(format!("{name}.opus"))).unwrap();
        let raw = std::fs::read(dir.join(format!("{name}.ref.s16"))).unwrap();
        let pcm: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let mut d = crate::ogg::Demuxer::new();
        let mut packets: Vec<Vec<u8>> = d.push(&bytes).into_iter().flat_map(|p| p.packets).collect();
        let head = crate::opus::parse_head(&packets[0]).unwrap();
        let audio = packets.split_off(2);
        (head, audio, pcm)
    }

    pub fn compare(got: &[f32], want: &[i16]) -> (f64, f64) {
        let n = got.len().min(want.len());
        let (mut s, mut e, mut gg, mut gw) = (0f64, 0f64, 0f64, 0f64);
        for i in 0..n {
            let (g, w) = (got[i] as f64 * 32768.0, want[i] as f64);
            s += w * w;
            e += (g - w) * (g - w);
            gg += g * g;
            gw += g * w;
        }
        (10.0 * (s / e.max(1e-9)).log10(), gw / (gg * s).sqrt().max(1e-9))
    }

    #[test]
    fn every_fixture_decodes_through_the_full_decoder_to_the_reference() {
        // (name, least SNR in dB): one mode per file, so the match is exact (CELT about 88 dB because of the 16-bit reference)
        for (name, min_snr) in [
            ("celt-mono-20ms", 70.0), ("celt-stereo-20ms", 70.0), ("celt-mono-10ms-wb", 70.0), ("celt-stereo-5ms", 70.0), ("celt-mono-2p5ms", 70.0), ("celt-stereo-clicks", 70.0), ("celt-mono-clicks", 70.0),
            ("silk-mono-wb-20ms", 100.0), ("silk-mono-nb-20ms", 100.0), ("silk-mono-mb-20ms", 100.0), ("silk-mono-wb-10ms", 100.0), ("silk-mono-wb-60ms", 100.0), ("silk-stereo-wb-20ms", 100.0),
            ("silk-mono-wb-fec", 100.0), ("silk-stereo-wb-fec", 100.0),
            ("switch-mono-audio", 70.0), ("hybrid-mono-fb-20ms", 70.0), ("hybrid-mono-swb-10ms", 70.0), ("hybrid-stereo-fb-20ms", 70.0),
        ] {
            let (head, audio, want) = fixture(name);
            let got = decode_packets(&head, &audio).unwrap();
            let (snr, corr) = compare(&got, &want);
            eprintln!("{name}: {} samples, want {}, snr {snr:.1} dB, corr {corr:.6}", got.len(), want.len());
            assert!(got.len() >= want.len(), "{name}: too few samples");
            assert!(snr > min_snr && corr > 0.9999, "{name}: snr {snr:.1} corr {corr:.6}");
        }
    }

    #[test]
    fn the_stream_decoder_plays_opus_files_to_the_reference_length_and_sound_however_the_bytes_are_cut() {
        for name in ["celt-stereo-20ms", "silk-mono-wb-20ms", "hybrid-mono-fb-20ms", "switch-mono-audio"] {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/opus");
            let bytes = std::fs::read(dir.join(format!("{name}.opus"))).unwrap();
            let raw = std::fs::read(dir.join(format!("{name}.ref.s16"))).unwrap();
            let want: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            for chunk in [1usize, 7, 300, 100_000] {
                let mut d = crate::StreamDecoder::new();
                let mut pcm = Vec::new();
                for c in bytes.chunks(chunk) {
                    pcm.extend(d.push(c).unwrap());
                }
                assert_eq!((d.rate, d.channels), (48000, want.len() / (pcm.len() / d.channels.max(1)).max(1)), "{name} {chunk}");
                assert_eq!(pcm.len(), want.len(), "{name} {chunk}: the end of the stream is cut by the last granule position");
                let max = pcm.iter().zip(&want).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
                assert!(max <= 2, "{name} {chunk}: largest step difference {max}");
            }
        }
    }

    fn rms(v: &[f32]) -> f64 {
        (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    }

    /// Decodes a fixture, replacing the packets `lost` by concealment of the same length; returns the sound and where each packet starts.
    fn decode_with_losses(name: &str, lost: &[usize]) -> (Vec<f32>, Vec<usize>, Vec<f32>, usize) {
        let (head, audio, _) = fixture(name);
        let mut d = OpusDecoder::new(&head);
        let (mut out, mut starts) = (Vec::new(), Vec::new());
        let mut clean_dec = OpusDecoder::new(&head);
        let mut clean = Vec::new();
        for (i, p) in audio.iter().enumerate() {
            starts.push(out.len());
            clean.extend(clean_dec.decode_packet(p).unwrap());
            if lost.contains(&i) {
                let n = crate::opus::parse_packet(p).unwrap().samples();
                out.extend(d.conceal(n));
            } else {
                out.extend(d.decode_packet(p).unwrap());
            }
        }
        (out, starts, clean, head.channels as usize)
    }

    #[test]
    fn a_lost_packet_is_concealed_with_sound_of_the_right_size_and_level_and_the_stream_recovers() {
        // (fixture, lost packet, how many packets after it the sound must be close again)
        for (name, lost, recover) in [("celt-mono-20ms", 6usize, 4usize), ("silk-mono-wb-20ms", 3, 4), ("hybrid-mono-fb-20ms", 5, 4), ("celt-stereo-20ms", 2, 3), ("silk-stereo-wb-20ms", 2, 3)] {
            let (out, starts, clean, ch) = decode_with_losses(name, &[lost]);
            assert_eq!(out.len(), clean.len(), "{name}: the same number of samples");
            assert!(out.iter().all(|v| v.is_finite()), "{name}");
            let (a, b) = (starts[lost], starts[lost + 1]);
            let (before, during, want) = (rms(&clean[starts[lost - 1]..a]), rms(&out[a..b]), rms(&clean[a..b]));
            eprintln!("{name}: packet {lost} lost: level of the clean sound {want:.4}, of the concealed {during:.4}, of the packet before {before:.4}");
            assert!(during > 0.0 && during < 2.0 * before.max(want), "{name}: the concealed stretch has a sane level");
            let end = starts.get(lost + 1 + recover).copied().unwrap_or(out.len());
            let from = starts[(lost + 1 + recover).min(starts.len() - 1)];
            if end > from {
                let err: f64 = out[from..end].iter().zip(&clean[from..end]).map(|(x, y)| ((x - y) as f64).powi(2)).sum();
                let sig: f64 = clean[from..end].iter().map(|y| (*y as f64).powi(2)).sum();
                eprintln!("  {recover} packets after it the sound is {:.1} dB above the difference to the clean decode", 10.0 * (sig / err.max(1e-12)).log10());
                assert!(sig / err.max(1e-12) > 10.0, "{name}: the stream recovered");
            }
            let _ = ch;
        }
    }

    #[test]
    fn a_long_loss_fades_and_never_blows_up() {
        for name in ["celt-mono-20ms", "silk-mono-wb-20ms", "hybrid-mono-fb-20ms"] {
            let (out, starts, clean, _) = decode_with_losses(name, &[3, 4, 5, 6, 7, 8, 9, 10]);
            assert!(out.iter().all(|v| v.is_finite() && v.abs() < 2.0), "{name}");
            let n = starts.len();
            let (early, late) = (rms(&out[starts[3]..starts[4.min(n - 1)]]), rms(&out[starts[(n - 1).min(9)]..]));
            eprintln!("{name}: eight lost packets: first concealed {early:.4}, last {late:.4}, clean {:.4}", rms(&clean[starts[3]..]));
            assert!(late <= rms(&clean[starts[3]..]) * 1.5 + 1e-4 && early <= rms(&clean[starts[3]..]) * 1.5 + 1e-4, "{name}: the concealed sound stays at or below the level of the sound it replaces");
        }
    }

    #[test]
    fn empty_and_one_byte_frames_are_concealed_and_garbage_never_panics() {
        let (head, audio, _) = fixture("celt-stereo-20ms");
        let mut d = OpusDecoder::new(&head);
        d.decode_packet(&audio[0]).unwrap();
        // a packet that is just its table of contents (an empty frame) and one with a single byte
        for p in [vec![audio[0][0]], vec![audio[0][0], 0u8]] {
            let out = d.decode_packet(&p).unwrap();
            assert_eq!(out.len(), 960 * 2);
            assert!(out.iter().all(|v| v.is_finite()));
        }
        // random bytes behind every kind of table of contents byte, in every state the decoder can be in
        let mut x = 99u64;
        let mut next = move || {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) as u32
        };
        for channels in [1u8, 2] {
            let mut h = head.clone();
            h.channels = channels;
            let mut d = OpusDecoder::new(&h);
            for _ in 0..1500 {
                let len = 2 + (next() % 200) as usize;
                let mut p: Vec<u8> = (0..len).map(|_| next() as u8).collect();
                p[0] = next() as u8;
                if let Ok(out) = d.decode_packet(&p) {
                    assert!(out.iter().all(|v| v.is_finite()));
                }
            }
        }
    }

    #[test]
    #[ignore = "a speed measurement: run with --release --ignored --nocapture"]
    fn how_much_faster_than_real_time_each_mode_decodes() {
        for name in ["celt-stereo-20ms", "celt-mono-2p5ms", "silk-stereo-wb-20ms", "silk-mono-nb-20ms", "hybrid-mono-fb-20ms", "hybrid-stereo-fb-20ms"] {
            let (head, audio, _) = fixture(name);
            let seconds: f64 = audio.iter().map(|p| crate::opus::parse_packet(p).unwrap().samples() as f64 / 48000.0).sum();
            let start = std::time::Instant::now();
            let mut total = 0.0;
            while start.elapsed().as_secs_f64() < 1.0 {
                let mut d = OpusDecoder::new(&head);
                for p in &audio {
                    let _ = d.decode_packet(p).unwrap();
                }
                total += seconds;
            }
            eprintln!("{name}: {:.0} times faster than real time ({:.0} frames of the file's size per second)", total / start.elapsed().as_secs_f64(), total / start.elapsed().as_secs_f64() / (seconds / audio.len() as f64));
        }
    }
}

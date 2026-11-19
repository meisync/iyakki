use super::*;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::var("IYAKKI_SBC_SCRATCH").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    d.join(name)
}

fn ffmpeg_present() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

// ------------------------------------------------------------------------------------------------ the codec

fn p(freq: u8, blocks: u8, mode: ChannelMode, loudness: bool, subbands: u8, bitpool: u8) -> Params {
    Params { freq, blocks, mode, loudness, subbands, bitpool }
}

fn tone(n: usize, ch: usize, rate: f64) -> Vec<i16> {
    (0..n * ch).map(|i| {
        let t = (i / ch) as f64 / rate;
        let c = (i % ch) as f64;
        ((t * 440.0 * std::f64::consts::TAU).sin() * 9000.0 + (t * (1250.0 + 300.0 * c) * std::f64::consts::TAU).sin() * 4000.0 + (t * 6100.0 * std::f64::consts::TAU).sin() * 1500.0) as i16
    }).collect()
}

/// The signal to noise ratio in dB of `out` against `src` after the best delay (the filter bank delays by a fixed number of samples).
fn snr_db(src: &[i16], out: &[i16], ch: usize, skip: usize) -> (f64, usize) {
    let mut best = (f64::MIN, 0);
    for lag in 0..400usize {
        let (mut sig, mut err) = (0.0, 0.0);
        let n = (src.len() / ch).min(out.len() / ch);
        for i in skip..n.saturating_sub(lag + 1) {
            for c in 0..ch {
                let s = src[i * ch + c] as f64;
                let o = out[(i + lag) * ch + c] as f64;
                sig += s * s;
                err += (s - o) * (s - o);
            }
        }
        let snr = 10.0 * (sig / err.max(1e-9)).log10();
        if snr > best.0 {
            best = (snr, lag);
        }
    }
    best
}

fn round_trip(p: Params, frames: usize) -> (f64, usize) {
    let ch = p.channels();
    let mut e = Encoder::new(p).unwrap();
    let mut d = Decoder::new();
    let frames = frames.max(8000 / p.frame_samples() + 2);
    let src = tone(e.frame_input_len() / ch * frames, ch, p.sample_rate() as f64);
    let mut out = Vec::new();
    for f in src.chunks(e.frame_input_len()) {
        let bytes = e.encode(f).unwrap();
        assert_eq!(bytes.len(), p.frame_len());
        let dec = d.decode(&bytes).unwrap();
        assert_eq!((dec.used, dec.params), (bytes.len(), p));
        out.extend(dec.pcm);
    }
    snr_db(&src, &out, ch, 2000)
}

#[test]
fn frame_lengths_and_headers_match_the_well_known_a2dp_numbers() {
    // high quality and "mid quality" joint stereo at 44.1 kHz: the 119 and 83 byte frames
    let hq = p(2, 16, ChannelMode::Joint, true, 8, 53);
    assert_eq!(hq.frame_len(), 119);
    assert_eq!(p(2, 16, ChannelMode::Joint, true, 8, 35).frame_len(), 83);
    assert_eq!(p(2, 16, ChannelMode::Mono, true, 8, 31).frame_len(), 4 + 4 + 62);
    assert_eq!(hq.frame_samples(), 128);
    assert_eq!(hq.sample_rate(), 44100);
    // the header bytes of the high quality stream: sync, 44.1 kHz / 16 blocks / joint / loudness / 8 subbands, bitpool
    assert_eq!(&hq.header_bytes(), &[0x9c, 0xbd, 0x35]);
    let mut e = Encoder::new(hq).unwrap();
    let f = e.encode(&vec![0i16; 256]).unwrap();
    assert_eq!(&f[..3], &[0x9c, 0xbd, 0x35]);
    assert_eq!(parse_header(&f).unwrap(), hq);
}

#[test]
fn the_checksum_is_the_crc8_of_the_specification_over_header_and_scale_factors() {
    // CRC-8 poly 0x1D start 0x0F, MSB first: a message of zero bits keeps the start value shifting through the polynomial
    assert_eq!(crc8(&[], 0), 0x0f);
    assert_eq!(crc8(&[0x00], 8), crc8(&[0x00, 0x00], 8));
    assert_ne!(crc8(&[0x01], 8), crc8(&[0x02], 8));
    // a single flipped bit changes the checksum
    let mut e = Encoder::new(p(2, 16, ChannelMode::Joint, true, 8, 53)).unwrap();
    let f = e.encode(&tone(128, 2, 44100.0)).unwrap();
    let mut g = f.clone();
    g[6] ^= 0x10;
    assert_ne!(frame_crc(&f, &parse_header(&f).unwrap()), frame_crc(&g, &parse_header(&g).unwrap()));
    assert_eq!(Decoder::new().decode(&g).err(), Some(SbcError::BadCrc));
}

#[test]
fn the_bit_allocation_never_exceeds_the_bitpool_and_uses_only_allowed_widths() {
    let mut seed = 99u64;
    let mut rnd = |m: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % m
    };
    for mode in [ChannelMode::Mono, ChannelMode::Dual, ChannelMode::Stereo, ChannelMode::Joint] {
        for sbn in [4u8, 8] {
            for loud in [false, true] {
                for freq in 0..4u8 {
                    for _ in 0..40 {
                        let max = p(freq, 16, mode, loud, sbn, 2).max_bitpool() as u64;
                        let bp = (2 + rnd(max - 1)) as u8;
                        let pr = p(freq, 16, mode, loud, sbn, bp);
                        let mut sf = [[0u8; 8]; 2];
                        for c in sf.iter_mut() {
                            for s in c.iter_mut().take(sbn as usize) {
                                *s = rnd(16) as u8;
                            }
                        }
                        let bits = allocate(&pr, &sf);
                        let total = |chs: &[usize]| chs.iter().map(|&c| bits[c][..sbn as usize].iter().map(|b| *b as u32).sum::<u32>()).sum::<u32>();
                        for c in 0..pr.channels() {
                            for b in &bits[c][..sbn as usize] {
                                assert!(*b <= 16, "{pr:?} {sf:?} {bits:?}");
                            }
                        }
                        let (pools, groups): (u32, Vec<Vec<usize>>) = match mode {
                            ChannelMode::Mono => (bp as u32, vec![vec![0]]),
                            ChannelMode::Dual => (bp as u32, vec![vec![0], vec![1]]),
                            _ => (bp as u32, vec![vec![0, 1]]),
                        };
                        for g in groups {
                            assert!(total(&g) <= pools, "{pr:?} {sf:?} {bits:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn encoding_and_decoding_a_tone_gives_it_back_in_every_mode_within_an_error_bound() {
    // bitpool 53 / 35 are what phones use; the bound is far under what the codec gives and far over a broken filter bank
    let cases = [
        (p(2, 16, ChannelMode::Joint, true, 8, 53), 30.0),
        (p(2, 16, ChannelMode::Stereo, true, 8, 53), 30.0),
        (p(3, 16, ChannelMode::Joint, true, 8, 35), 25.0),
        (p(2, 16, ChannelMode::Dual, false, 8, 40), 25.0),
        (p(2, 16, ChannelMode::Mono, true, 8, 31), 25.0),
        (p(1, 8, ChannelMode::Joint, false, 8, 40), 25.0),
        (p(2, 12, ChannelMode::Joint, true, 4, 32), 22.0),
        (p(0, 4, ChannelMode::Stereo, true, 4, 32), 22.0),
        (p(2, 16, ChannelMode::Joint, true, 4, 32), 22.0),
    ];
    for (pr, bound) in cases {
        let (snr, lag) = round_trip(pr, 60);
        eprintln!("{pr:?}: snr {snr:.1} dB at delay {lag}");
        assert!(snr > bound, "{pr:?}: {snr:.1} dB at delay {lag}");
    }
}

#[test]
fn a_larger_bitpool_gives_a_better_signal_to_noise_ratio() {
    let low = round_trip(p(2, 16, ChannelMode::Joint, true, 8, 20), 40).0;
    let high = round_trip(p(2, 16, ChannelMode::Joint, true, 8, 53), 40).0;
    assert!(high > low + 5.0, "{low:.1} dB -> {high:.1} dB");
}

#[test]
fn silence_is_silence_and_full_scale_does_not_wrap() {
    let pr = p(2, 16, ChannelMode::Joint, true, 8, 53);
    let mut e = Encoder::new(pr).unwrap();
    let mut d = Decoder::new();
    for _ in 0..4 {
        let out = d.decode(&e.encode(&vec![0i16; 256]).unwrap()).unwrap().pcm;
        assert!(out.iter().all(|x| x.abs() <= 1));
    }
    let loud: Vec<i16> = (0..256).map(|i| if (i / 2) % 20 < 10 { 32767 } else { -32768 }).collect();
    let mut last = Vec::new();
    for _ in 0..6 {
        last = d.decode(&e.encode(&loud).unwrap()).unwrap().pcm;
    }
    assert!(last.iter().any(|x| x.abs() > 20000), "the square wave is still loud");
}

#[test]
fn malformed_frames_are_errors_and_never_panic() {
    let pr = p(2, 16, ChannelMode::Joint, true, 8, 53);
    let mut e = Encoder::new(pr).unwrap();
    let good = e.encode(&tone(128, 2, 44100.0)).unwrap();
    let mut d = Decoder::new();
    assert_eq!(d.decode(&[]).err(), Some(SbcError::TooShort));
    assert_eq!(d.decode(&good[..3]).err(), Some(SbcError::TooShort));
    assert_eq!(d.decode(&good[..good.len() - 1]).err(), Some(SbcError::TooShort));
    let mut bad = good.clone();
    bad[0] = 0x9d;
    assert_eq!(d.decode(&bad).err(), Some(SbcError::BadSync));
    // bitpool 0, 1 and above the limit for the mode
    for bp in [0u8, 1] {
        let mut b = good.clone();
        b[2] = bp;
        assert!(matches!(d.decode(&b).err(), Some(SbcError::BadParams(_))), "bitpool {bp}");
    }
    let mut b = good.clone();
    b[1] &= !0x0c; // mono ...
    b[2] = 200; // ... with a stereo-sized bitpool (mono allows 128 with 8 subbands)
    assert!(matches!(d.decode(&b), Err(SbcError::BadParams(_))));
    // one bit flipped anywhere: a checksum error or a (possibly different) valid decode, never a panic
    for i in 0..good.len() * 8 {
        let mut b = good.clone();
        b[i / 8] ^= 1 << (i % 8);
        let _ = d.decode(&b);
    }
    // random bytes behind a good-looking header, in every mode
    let mut seed = 1u64;
    for _ in 0..3000 {
        let mut b: Vec<u8> = (0..160).map(|_| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) as u8 }).collect();
        b[0] = SYNC;
        b[2] = 2 + (b[2] % 60);
        if let Ok(h) = parse_header(&b) {
            if b.len() >= h.frame_len() {
                b[3] = frame_crc(&b, &h);
                let r = d.decode(&b).unwrap();
                assert_eq!(r.pcm.len(), h.frame_samples() * h.channels());
            }
        }
        let _ = d.decode(&b);
    }
    assert!(Encoder::new(p(2, 16, ChannelMode::Joint, true, 8, 1)).is_err());
    assert_eq!(Encoder::new(pr).unwrap().encode(&[0; 10]).err(), Some(SbcError::TooShort));
}

#[test]
fn the_decoder_follows_a_change_of_channels_or_subbands_between_frames() {
    let mut d = Decoder::new();
    for pr in [p(2, 16, ChannelMode::Joint, true, 8, 40), p(2, 16, ChannelMode::Mono, true, 4, 20), p(3, 8, ChannelMode::Stereo, false, 8, 30)] {
        let mut e = Encoder::new(pr).unwrap();
        let out = d.decode(&e.encode(&tone(pr.frame_samples(), pr.channels(), pr.sample_rate() as f64)).unwrap()).unwrap();
        assert_eq!(out.pcm.len(), pr.frame_samples() * pr.channels());
    }
}

// ------------------------------------------------------------------------------------------------ an independent implementation as oracle

/// BLACK-BOX INTEROPERABILITY CHECK ONLY. Runs ffmpeg (a development tool, never shipped, never linked: a second, independent SBC
/// implementation) as a separate process and returns `Some` when it ran, or `None` when ffmpeg is not installed (the test then skips).
/// It only compares audio SNR. No constant in this repository derives from ffmpeg's source, tables or output (docs/development/SBC-TABLES.md).
fn ffmpeg(args: &[&str]) -> Option<()> {
    if !ffmpeg_present() {
        eprintln!("ffmpeg is not installed: the oracle test is skipped");
        return None;
    }
    let st = std::process::Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).status().ok()?;
    assert!(st.success(), "ffmpeg {args:?}");
    Some(())
}

fn to_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn from_bytes(b: &[u8]) -> Vec<i16> {
    b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

#[test]
fn oracle_our_frames_are_decoded_by_an_independent_decoder_to_the_same_sound() {
    for (pr, bound) in [(p(2, 16, ChannelMode::Joint, true, 8, 53), 30.0), (p(2, 16, ChannelMode::Stereo, false, 8, 40), 28.0), (p(3, 8, ChannelMode::Joint, true, 4, 32), 22.0), (p(2, 16, ChannelMode::Mono, true, 8, 31), 25.0)] {
        let ch = pr.channels();
        let mut e = Encoder::new(pr).unwrap();
        let src = tone(e.frame_input_len() / ch * 80, ch, pr.sample_rate() as f64);
        let mut stream = Vec::new();
        for f in src.chunks(e.frame_input_len()) {
            stream.extend(e.encode(f).unwrap());
        }
        std::fs::write(scratch("ours.sbc"), &stream).unwrap();
        let Some(()) = ffmpeg(&["-f", "sbc", "-i", scratch("ours.sbc").to_str().unwrap(), "-f", "s16le", "-ac", &ch.to_string(), scratch("theirs.raw").to_str().unwrap()]) else { return };
        let out = from_bytes(&std::fs::read(scratch("theirs.raw")).unwrap());
        let (snr, lag) = snr_db(&src, &out, ch, 2000);
        eprintln!("{pr:?}: ffmpeg decodes our stream at {snr:.1} dB, delay {lag}");
        assert!(snr > bound, "{pr:?}: {snr:.1} dB at delay {lag}");
    }
}

#[test]
fn oracle_an_independent_encoder_makes_frames_that_our_decoder_reads_to_the_same_sound() {
    for (name, extra, ch) in [("joint", Vec::<&str>::new(), 2usize), ("mono", Vec::<&str>::new(), 1)] {
        let rate = 44100.0;
        let src = tone(44100, ch, rate);
        std::fs::write(scratch("src.raw"), to_bytes(&src)).unwrap();
        let (src_path, sbc_path) = (scratch("src.raw"), scratch("theirs.sbc"));
        let (src_s, sbc_s) = (src_path.to_str().unwrap().to_string(), sbc_path.to_str().unwrap().to_string());
        let mut args = vec!["-f", "s16le", "-ar", "44100", "-ac"];
        let chs = ch.to_string();
        args.push(&chs);
        args.extend(["-i", src_s.as_str(), "-c:a", "sbc"]);
        args.extend(extra.iter().copied());
        args.extend(["-f", "sbc", sbc_s.as_str()]);
        let Some(()) = ffmpeg(&args) else { return };
        let bytes = std::fs::read(scratch("theirs.sbc")).unwrap();
        let mut d = Decoder::new();
        let (mut at, mut out, mut frames) = (0, Vec::new(), 0);
        while at < bytes.len() {
            let Ok(r) = d.decode(&bytes[at..]) else { break };
            at += r.used;
            out.extend(r.pcm);
            frames += 1;
        }
        assert!(frames > 100, "{name}: only {frames} frames were readable");
        assert_eq!(at, bytes.len(), "{name}: every byte of the independent stream is in a frame we read");
        let (snr, lag) = snr_db(&src, &out, ch, 4000);
        eprintln!("{name}: we decode ffmpeg's stream at {snr:.1} dB, delay {lag}");
        assert!(snr > 20.0, "{name}: {snr:.1} dB at delay {lag}");
    }
}


/// Prints the designed tables in the form `sbc_tables.rs` holds them: `IYAKKI_SBC_OUT=path cargo test -p iyakki-devctl print_designed_tables -- --ignored`.
#[test]
#[ignore]
fn print_designed_tables() {
    let out = std::env::var("IYAKKI_SBC_OUT").expect("IYAKKI_SBC_OUT");
    let mut text = String::new();
    for m in [4usize, 8] {
        let w = crate::sbc_design::designed_window(m);
        text.push_str(&format!("pub static D{m}_DESIGNED: [f64; {}] = [\n", 10 * m));
        for row in w.chunks(m.max(8).min(8)) {
            text.push_str("    ");
            for v in row {
                text.push_str(&format!("{:.9}, ", if *v == 0.0 { 0.0 } else { *v }));
            }
            text.push('\n');
        }
        text.push_str("];\n\n");
    }
    std::fs::write(out, text).unwrap();
}

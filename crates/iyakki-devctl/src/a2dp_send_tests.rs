use super::*;
use crate::a2dp::{parse_rtp, StreamDecoder};
use crate::avdtp::{Action, Sink};
use std::sync::{Arc, Mutex};

/// The signalling channel of a scripted device: commands go to this crate's own sink and its answers come back.
struct SinkSig {
    sink: Arc<Mutex<Sink>>,
    out: VecDeque<Vec<u8>>,
    re: Reassembler,
    delay_after_start: Option<u16>,
    closed: bool,
}

impl Channel for SinkSig {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        let m = self.re.push(data).unwrap().unwrap();
        let (reply, acts) = self.sink.lock().unwrap().handle(&m);
        if let Some(r) = reply {
            self.out.push_back(r.encode());
        }
        if acts.contains(&Action::Started) {
            if let Some(d) = self.delay_after_start {
                if let Some(rep) = self.sink.lock().unwrap().delay_report(d) {
                    self.out.push_back(rep.encode());
                }
            }
        }
        if acts.contains(&Action::Ended) {
            self.closed = true;
        }
        Ok(())
    }
    fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
        Ok(match self.out.pop_front() {
            Some(d) => Recv::Data(d),
            None => Recv::Timeout,
        })
    }
}

/// The media channel of a scripted device: packets are kept.
#[derive(Clone, Default)]
struct Wire(Arc<Mutex<Vec<Vec<u8>>>>);
impl Channel for Wire {
    fn send(&mut self, data: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().push(data.to_vec());
        Ok(())
    }
    fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
        Ok(Recv::Timeout)
    }
}

fn open(delay: Option<u16>) -> (Session<SinkSig, Wire>, Wire, Arc<Mutex<Sink>>) {
    let sink = Arc::new(Mutex::new(if delay.is_some() { Sink::new().with_delay_reporting() } else { Sink::new() }));
    let sig = SinkSig { sink: sink.clone(), out: VecDeque::new(), re: Reassembler::default(), delay_after_start: delay, closed: false };
    let wire = Wire::default();
    let w2 = wire.clone();
    let s = Session::open(sig, move || Ok(w2), false, Duration::from_millis(10)).unwrap();
    (s, wire, sink)
}

fn tone(rate: u32, ch: usize, secs: f64, hz: f64) -> Vec<i16> {
    let n = (rate as f64 * secs) as usize;
    (0..n).flat_map(|i| std::iter::repeat_n(((i as f64 / rate as f64 * hz * std::f64::consts::TAU).sin() * 12000.0) as i16, ch)).collect()
}

fn decode_all(w: &Wire) -> (Vec<i16>, u32, usize) {
    let mut d = StreamDecoder::new();
    let (mut pcm, mut rate, mut ch) = (vec![], 0, 0);
    for p in w.0.lock().unwrap().iter() {
        for b in d.on_media(p) {
            rate = b.rate;
            ch = b.channels;
            pcm.extend(b.pcm);
        }
    }
    (pcm, rate, ch)
}

/// Frequency by zero crossings of channel 0.
fn freq(pcm: &[i16], ch: usize, rate: u32) -> f64 {
    let m: Vec<i16> = pcm.iter().step_by(ch).copied().collect();
    let z = m.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
    z as f64 / 2.0 / (m.len() as f64 / rate as f64)
}

#[test]
fn the_set_up_runs_to_streaming_and_a_tone_arrives_decoded_at_the_right_pitch() {
    let (mut s, wire, sink) = open(None);
    assert_eq!(s.state(), SrcState::Streaming);
    assert_eq!(sink.lock().unwrap().state(), crate::avdtp::StreamState::Streaming);
    let p = *s.params().unwrap();
    assert_eq!(p.sample_rate(), 44100);
    s.write(&tone(44100, 2, 1.0, 440.0), 44100, 2).unwrap();
    let (pcm, rate, ch) = decode_all(&wire);
    assert_eq!((rate, ch), (44100, 2));
    assert!(pcm.len() >= 2 * 43000, "{}", pcm.len());
    let f = freq(&pcm[2 * 2000..], 2, 44100);
    assert!((f - 440.0).abs() < 8.0, "{f}");
    // loudness is about right (no silence, no clipping): peak of the input is 12000
    let peak = pcm.iter().map(|v| v.unsigned_abs()).max().unwrap();
    assert!((9000..15000).contains(&peak), "{peak}");
    assert!(s.info().packets > 10 && s.info().codec.contains("SBC"));
}

#[test]
fn sound_of_another_rate_and_mono_is_converted_to_the_speakers_settings() {
    let (mut s, wire, _) = open(None);
    s.write(&tone(48000, 1, 1.0, 1000.0), 48000, 1).unwrap();
    let (pcm, rate, ch) = decode_all(&wire);
    assert_eq!((rate, ch), (44100, 2));
    // about a second at the speaker's rate came out
    let secs = pcm.len() as f64 / ch as f64 / rate as f64;
    assert!((0.9..1.01).contains(&secs), "{secs}");
    let f = freq(&pcm[2 * 2000..], 2, 44100);
    assert!((f - 1000.0).abs() < 15.0, "{f}");
    // both channels carry the same mono sound
    let diff = pcm.chunks(2).skip(2000).take(5000).map(|c| (c[0] as i32 - c[1] as i32).abs()).max().unwrap();
    assert!(diff < 2500, "{diff}");
}

#[test]
fn small_writes_make_the_same_stream_as_one_big_one_and_the_rtp_numbers_run_on() {
    let (mut a, wa, _) = open(None);
    let (mut b, wb, _) = open(None);
    let t = tone(44100, 2, 0.5, 300.0);
    a.write(&t, 44100, 2).unwrap();
    for c in t.chunks(2 * 333) {
        b.write(c, 44100, 2).unwrap();
    }
    let (pa, _, _) = decode_all(&wa);
    let (pb, _, _) = decode_all(&wb);
    let n = pa.len().min(pb.len());
    assert!(n > 40000);
    let e: f64 = pa[..n].iter().zip(&pb[..n]).map(|(x, y)| ((*x as f64) - (*y as f64)).abs()).sum::<f64>() / n as f64;
    assert!(e < 1.0, "{e}");
    let pk = wb.0.lock().unwrap();
    let mut last: Option<(u16, u32)> = None;
    for p in pk.iter() {
        let r = parse_rtp(p).unwrap();
        if let Some((seq, ts)) = last {
            assert_eq!(r.seq, seq.wrapping_add(1));
            assert!(r.timestamp > ts);
        }
        last = Some((r.seq, r.timestamp));
    }
    assert!(pk.iter().all(|p| p.len() <= MEDIA_MTU + 13));
}

#[test]
fn the_delay_the_speaker_reports_is_kept_and_close_ends_the_stream() {
    let (mut s, _, sink) = open(Some(250));
    s.write(&tone(44100, 2, 0.1, 440.0), 44100, 2).unwrap();
    assert_eq!(s.info().delay, Some(250));
    s.close();
    assert!(s.ended());
    assert_eq!(sink.lock().unwrap().state(), crate::avdtp::StreamState::Idle);
    assert!(s.write(&[0; 10], 44100, 2).is_err());
}

#[test]
fn a_silent_or_garbled_device_gives_a_plain_failure() {
    struct Dead;
    impl Channel for Dead {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
            Ok(Recv::Timeout)
        }
    }
    let e = Session::<Dead, Wire>::open(Dead, || Ok(Wire::default()), false, Duration::from_millis(5)).err().unwrap();
    assert!(e.contains("did not answer"), "{e}");
    struct Closed;
    impl Channel for Closed {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
            Ok(Recv::Closed)
        }
    }
    let e = Session::<Closed, Wire>::open(Closed, || Ok(Wire::default()), false, Duration::from_millis(5)).err().unwrap();
    assert!(e.contains("closed"), "{e}");
    struct Junk;
    impl Channel for Junk {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            Ok(())
        }
        fn recv(&mut self, _w: Duration) -> io::Result<Recv> {
            Ok(Recv::Data(vec![0xff, 0x01, 0x02]))
        }
    }
    assert!(Session::<Junk, Wire>::open(Junk, || Ok(Wire::default()), false, Duration::from_millis(5)).is_err());
}

#[test]
fn pacing_keeps_sound_from_going_out_faster_than_it_plays() {
    let sink = Arc::new(Mutex::new(Sink::new()));
    let sig = SinkSig { sink, out: VecDeque::new(), re: Reassembler::default(), delay_after_start: None, closed: false };
    let wire = Wire::default();
    let w2 = wire.clone();
    let mut s = Session::open(sig, move || Ok(w2), true, Duration::from_millis(10)).unwrap();
    let t0 = Instant::now();
    s.write(&tone(44100, 2, 0.6, 440.0), 44100, 2).unwrap();
    let took = t0.elapsed();
    assert!(took >= Duration::from_millis(400), "{took:?}");
    assert!(took < Duration::from_millis(1500), "{took:?}");
}

#[test]
fn the_sender_shows_its_state_and_refuses_a_second_stream_or_a_stop_with_nothing_running() {
    let mk = || {
        let sink = Arc::new(Mutex::new(Sink::new()));
        let wire = Wire::default();
        let w = wire.clone();
        let snd = Sender::<SinkSig, Wire>::new(false, move |addr| {
            if addr == "00:00:00:00:00:00" {
                return Err("could not connect to the device for audio: timed out".into());
            }
            let sig = SinkSig { sink: sink.clone(), out: VecDeque::new(), re: Reassembler::default(), delay_after_start: None, closed: false };
            let w = w.clone();
            let m: Box<dyn FnOnce() -> io::Result<Wire> + Send> = Box::new(move || Ok(w));
            Ok((sig, m))
        });
        (snd, wire)
    };
    let (s, wire) = mk();
    assert!(s.stop().is_err() && s.write(&[0; 4], 44100, 2).is_err());
    let e = s.start("00:00:00:00:00:00", "x").unwrap_err();
    assert!(e.contains("timed out") && s.view().line().contains("not sending: could not connect"));
    s.start("AA:BB:CC:DD:EE:FF", "Headset").unwrap();
    assert!(s.view().active && s.view().line().contains("Headset"));
    assert!(s.start("AA:BB:CC:DD:EE:FF", "Headset").is_err());
    s.write(&tone(44100, 2, 0.2, 440.0), 44100, 2).unwrap();
    assert!(!wire.0.lock().unwrap().is_empty() && s.view().packets > 0);
    s.stop().unwrap();
    assert!(!s.view().active);
}

#[test]
fn the_converter_keeps_pitch_and_length_and_never_panics_on_odd_input() {
    let mut c = Converter::new(48000, 2, 44100, 2);
    let out = c.run(&tone(48000, 2, 0.5, 440.0));
    assert!((out.len() as f64 / 2.0 - 22050.0).abs() < 3.0);
    let mut c = Converter::new(8000, 1, 44100, 2);
    assert_eq!(c.run(&[]).len(), 0);
    let o = c.run(&[1000; 80]);
    assert!(o.len() > 400 && o.iter().all(|v| *v <= 1000));
    // a lone odd sample count for stereo is cut, not read past
    let mut c = Converter::new(44100, 2, 44100, 1);
    assert_eq!(c.run(&[1, 2, 3]).len(), 1);
}

#[test]
fn an_independent_decoder_reads_the_frames_the_stream_sent_to_the_same_sound() {
    // BLACK-BOX CHECK ONLY: ffmpeg (a development tool, never shipped or linked) decodes the SBC frames taken out of the RTP packets
    // the session sent, and its sound is compared with this crate's own decode of the same packets.
    if !std::process::Command::new("ffmpeg").arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false) {
        eprintln!("ffmpeg is not installed: the check is skipped");
        return;
    }
    let (mut s, wire, _) = open(None);
    s.write(&tone(48000, 2, 1.0, 523.0), 48000, 2).unwrap();
    let mut frames = vec![];
    for p in wire.0.lock().unwrap().iter() {
        let r = parse_rtp(p).unwrap();
        frames.extend_from_slice(crate::a2dp::parse_sbc_payload(r.payload).unwrap().data);
    }
    let dir = std::env::temp_dir().join(format!("iyakki-a2dp-send-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (sbc, raw) = (dir.join("sent.sbc"), dir.join("theirs.raw"));
    std::fs::write(&sbc, &frames).unwrap();
    let st = std::process::Command::new("ffmpeg").args(["-y", "-loglevel", "error", "-f", "sbc", "-i"]).arg(&sbc).args(["-f", "s16le", "-ac", "2"]).arg(&raw).status().unwrap();
    assert!(st.success());
    let theirs: Vec<i16> = std::fs::read(&raw).unwrap().chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let (ours, _, _) = decode_all(&wire);
    let _ = std::fs::remove_dir_all(&dir);
    let n = ours.len().min(theirs.len());
    assert!(n > 80_000, "{n}");
    assert!(ours.len().abs_diff(theirs.len()) < 2 * 512, "{} against {}", ours.len(), theirs.len());
    let max = ours[..n].iter().zip(&theirs[..n]).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    eprintln!("stream frames decoded by an independent decoder against ours: {n} samples, largest difference {max}");
    // two independent decoders of a lossy codec differ by a few steps (fixed against floating point arithmetic)
    assert!(max <= 8, "{max}");
}

// ------------------------------------------------------------------------------------------------ AAC through the add-on's coders

mod aac {
    use super::*;
    use crate::a2dp_aac::{latm_unwrap, AacBackend, AacConfig, AacDecode, AacEncode, AacStream, FRAME_SAMPLES};

    /// An encoder that makes one 10-byte "frame" for every 1024 sample frames fed (the first byte tells which), and a decoder for it.
    struct FakeEnc {
        ch: usize,
        pend: Vec<i16>,
        n: u8,
    }
    impl AacEncode for FakeEnc {
        fn feed(&mut self, pcm: &[i16]) -> Result<(), String> {
            self.pend.extend_from_slice(pcm);
            Ok(())
        }
        fn drain(&mut self) -> Result<Vec<Vec<u8>>, String> {
            let mut out = Vec::new();
            while self.pend.len() >= FRAME_SAMPLES * self.ch {
                self.pend.drain(..FRAME_SAMPLES * self.ch);
                self.n = self.n.wrapping_add(1);
                out.push(vec![self.n; 10]);
            }
            Ok(out)
        }
    }
    struct NullDec;
    impl AacDecode for NullDec {
        fn feed(&mut self, _: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn drain(&mut self) -> Result<Vec<i16>, String> {
            Ok(vec![])
        }
    }
    struct Backend {
        fail_encoder: bool,
        configs: Mutex<Vec<AacConfig>>,
    }
    impl AacBackend for Backend {
        fn available(&self) -> bool {
            true
        }
        fn decoder(&self, _: &AacConfig) -> Result<Box<dyn AacDecode>, String> {
            Ok(Box::new(NullDec))
        }
        fn encoder(&self, c: &AacConfig) -> Result<Box<dyn AacEncode>, String> {
            if self.fail_encoder {
                return Err("the audio add-on could not be started".into());
            }
            self.configs.lock().unwrap().push(*c);
            Ok(Box::new(FakeEnc { ch: c.channels as usize, pend: vec![], n: 0 }))
        }
    }
    fn backend(fail: bool) -> Arc<Backend> {
        Arc::new(Backend { fail_encoder: fail, configs: Mutex::default() })
    }

    fn open_with(sink: Sink, b: Option<Arc<dyn AacBackend>>) -> (Session<SinkSig, Wire>, Wire, Arc<Mutex<Sink>>) {
        let sink = Arc::new(Mutex::new(sink));
        let sig = SinkSig { sink: sink.clone(), out: VecDeque::new(), re: Reassembler::default(), delay_after_start: None, closed: false };
        let wire = Wire::default();
        let w2 = wire.clone();
        let s = Session::open_with(sig, move || Ok(w2), false, Duration::from_millis(10), b).unwrap();
        (s, wire, sink)
    }

    #[test]
    fn headphones_that_offer_aac_get_aac_frames_in_latm_packets_when_the_add_on_is_there() {
        let b = backend(false);
        let (mut s, wire, sink) = open_with(Sink::new().with_aac(), Some(b.clone()));
        assert_eq!(s.state(), SrcState::Streaming);
        assert!(s.params().is_none());
        let c = *s.aac_config().unwrap();
        assert_eq!(c, AacConfig { rate: 48000, channels: 2, bit_rate: 128_000 });
        assert_eq!(sink.lock().unwrap().config_aac().map(|c| (c.rate, c.channels)), Some((48000, 2)));
        assert_eq!(s.info().codec, "AAC 48 kHz stereo");
        s.write(&tone(48000, 2, 1.0, 440.0), 48000, 2).unwrap();
        let pk = wire.0.lock().unwrap().clone();
        assert_eq!(pk.len(), 46);
        // the receiving side reads the packets back: one frame each, in order, 1024 samples apart on the clock
        let mut stream = AacStream::new(c, Box::new(NullDec));
        for (i, p) in pk.iter().enumerate() {
            let r = parse_rtp(p).unwrap();
            assert_eq!((r.seq as usize, r.timestamp as usize), (i, i * 1024));
            assert_eq!(latm_unwrap(r.payload).unwrap(), &[i as u8 + 1; 10][..]);
            stream.on_media(p);
        }
        assert_eq!(stream.stats.frames, 46);
        // sound at another rate and channel count is converted first
        s.write(&tone(44100, 1, 0.5, 440.0), 44100, 1).unwrap();
        assert!(wire.0.lock().unwrap().len() > 46);
        assert_eq!(s.info().packets as usize, wire.0.lock().unwrap().len());
    }

    #[test]
    fn headphones_that_offer_only_sbc_or_a_source_without_the_add_on_use_sbc() {
        let (s, _, _) = open_with(Sink::new(), Some(backend(false)));
        assert!(s.aac_config().is_none());
        assert_eq!(s.params().unwrap().sample_rate(), 44100);
        let (s, _, sink) = open_with(Sink::new().with_aac(), None);
        assert!(s.aac_config().is_none());
        assert!(sink.lock().unwrap().config_aac().is_none());
        assert!(s.info().codec.starts_with("SBC"));
    }

    #[test]
    fn when_the_add_on_cannot_start_its_encoder_the_sender_falls_back_to_sbc_and_says_so_in_the_view() {
        let b = backend(true);
        let b2 = b.clone();
        let sender = Sender::<SinkSig, Wire>::new(false, |_| {
            let sig = SinkSig { sink: Arc::new(Mutex::new(Sink::new().with_aac())), out: VecDeque::new(), re: Reassembler::default(), delay_after_start: None, closed: false };
            let media: Box<dyn FnOnce() -> io::Result<Wire> + Send> = Box::new(|| Ok(Wire::default()));
            Ok((sig, media))
        })
        .with_aac(move || Some(b2.clone() as Arc<dyn AacBackend>));
        sender.start("00:11:22:33:44:55", "Buds").unwrap();
        let v = sender.view();
        assert!(v.active);
        assert!(v.codec.starts_with("SBC"), "{}", v.codec);
        assert!(b.configs.lock().unwrap().is_empty());
    }
}

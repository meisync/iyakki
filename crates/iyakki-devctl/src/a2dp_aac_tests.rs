use super::*;

/// A decoder that makes a frame of the first payload byte for every frame fed (golden, no real coding), and one that can fail.
pub(crate) struct FakeDec {
    pub ch: usize,
    pub ready: Vec<i16>,
    pub fail: bool,
    pub fed: Vec<Vec<u8>>,
}
impl AacDecode for FakeDec {
    fn feed(&mut self, adts: &[u8]) -> Result<(), String> {
        if self.fail {
            return Err("the decoder stopped".into());
        }
        self.fed.push(adts.to_vec());
        self.ready.extend(std::iter::repeat_n(adts[7] as i16, FRAME_SAMPLES * self.ch));
        Ok(())
    }
    fn drain(&mut self) -> Result<Vec<i16>, String> {
        Ok(std::mem::take(&mut self.ready))
    }
}

fn cfg() -> AacConfig {
    AacConfig { rate: 44100, channels: 2, bit_rate: 128_000 }
}

fn rtp(seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x80, 96];
    v.extend_from_slice(&seq.to_be_bytes());
    v.extend_from_slice(&[0, 0, 0, 0, 1, 2, 3, 4]);
    v.extend_from_slice(payload);
    v
}

#[test]
fn the_capability_and_a_configuration_are_the_bytes_of_the_specification() {
    let cap = sink_capability();
    let i = parse_aac(&cap).unwrap();
    assert_eq!(i.object_types, 0xc0);
    assert_eq!(i.sample_rates, vec![44100, 48000]);
    assert_eq!(i.max_channels, 2);
    assert!(i.variable_bit_rate);
    assert_eq!(sink_codec_element(), [7, 8, 0, 2, 0xc0, 0x01, 0x8c, 0x80, 0, 0]);
    let c = AacConfig { rate: 48000, channels: 2, bit_rate: 0x01_f4_00 };
    let b = c.to_config_bytes(true);
    assert_eq!(b, [0x80, 0x00, 0x84, 0x01, 0xf4, 0x00]);
    assert_eq!(AacConfig::from_config_bytes(&b), Ok(c));
    assert_eq!(AacConfig::from_config_bytes(&cfg().to_config_bytes(false)), Ok(cfg()));
}

#[test]
fn a_configuration_with_two_values_or_a_foreign_kind_is_refused_with_a_code() {
    use crate::avdtp::*;
    assert_eq!(AacConfig::from_config_bytes(&[0xc0, 0x01, 0x84, 0, 0, 0]), Err(ERR_INVALID_OBJECT_TYPE));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x01, 0x84, 0, 0, 0]), Err(ERR_INVALID_SAMPLING_FREQUENCY));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x01, 0x04, 0, 0, 0]), Ok(AacConfig { rate: 44100, channels: 2, bit_rate: 0 }));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x01, 0x00, 0, 0, 0]), Err(ERR_INVALID_CHANNELS));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x01, 0x0c, 0, 0, 0]), Err(ERR_INVALID_CHANNELS));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x03, 0x84, 0, 0, 0]), Err(ERR_INVALID_SAMPLING_FREQUENCY));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x00, 0x04, 0, 0, 0]), Err(ERR_INVALID_SAMPLING_FREQUENCY));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x01, 0x04, 0x80, 0, 0]), Err(ERR_INVALID_CAPABILITIES));
    assert_eq!(AacConfig::from_config_bytes(&[0x80, 0x01]), Err(ERR_BAD_PAYLOAD_FORMAT));
}

#[test]
fn the_best_setting_within_the_offer_is_chosen_and_nothing_is_invented() {
    let offer = |o: u8, rates: &[u32], ch: u8, br: u32| AacInfo { object_types: o, sample_rates: rates.to_vec(), max_channels: ch, variable_bit_rate: false, bit_rate: br };
    assert_eq!(choose_aac(&offer(0xf0, &[44100, 48000], 2, 320_000)), Ok((AacConfig { rate: 48000, channels: 2, bit_rate: 128_000 }, true)));
    assert_eq!(choose_aac(&offer(0x40, &[44100], 1, 96_000)), Ok((AacConfig { rate: 44100, channels: 1, bit_rate: 96_000 }, false)));
    assert_eq!(choose_aac(&offer(0x80, &[44100], 2, 0)).unwrap().0.bit_rate, 128_000);
    assert!(choose_aac(&offer(0x20, &[44100], 2, 0)).is_err());
    assert!(choose_aac(&offer(0x80, &[96000], 2, 0)).is_err());
    assert!(choose_aac(&offer(0x80, &[44100], 0, 0)).is_err());
}

#[test]
fn latm_carries_one_frame_with_its_length_in_runs_of_ff() {
    assert_eq!(latm_wrap(&[9, 8, 7]), vec![3, 9, 8, 7]);
    let big = vec![5u8; 300];
    let w = latm_wrap(&big);
    assert_eq!(&w[..2], &[0xff, 45]);
    assert_eq!(latm_unwrap(&w).unwrap(), &big[..]);
    let exact = vec![1u8; 255];
    assert_eq!(&latm_wrap(&exact)[..2], &[0xff, 0]);
    assert_eq!(latm_unwrap(&latm_wrap(&exact)).unwrap(), &exact[..]);
    assert!(latm_unwrap(&[]).is_err());
    assert!(latm_unwrap(&[0xff, 0xff]).is_err());
    assert!(latm_unwrap(&[5, 1, 2]).is_err());
    assert!(latm_unwrap(&[0]).is_err());
    assert!(latm_unwrap(&[0xff; 20]).is_err());
}

#[test]
fn the_adts_header_is_the_golden_one_for_44_1_khz_stereo() {
    assert_eq!(adts_header(&cfg(), 100), Some([0xff, 0xf1, 0x50, 0x80, 0x0d, 0x7f, 0xfc]));
    assert_eq!(adts_header(&AacConfig { rate: 48000, channels: 1, bit_rate: 0 }, 1).unwrap()[2], 0x4c);
    assert_eq!(adts_header(&AacConfig { rate: 12345, channels: 2, bit_rate: 0 }, 1), None);
    assert_eq!(adts_header(&cfg(), 0x2000), None);
}

#[test]
fn an_adts_stream_is_cut_into_frames_whatever_the_chunks_and_junk_is_skipped() {
    let c = cfg();
    let f1 = vec![1u8; 100];
    let f2 = vec![2u8; 40];
    let mut stream = vec![0x12, 0x34];
    stream.extend(adts_wrap(&c, &f1).unwrap());
    stream.extend(adts_wrap(&c, &f2).unwrap());
    let mut buf = Vec::new();
    let mut got = Vec::new();
    for ch in stream.chunks(13) {
        buf.extend_from_slice(ch);
        got.extend(adts_take_frames(&mut buf));
    }
    assert_eq!(got, vec![f1, f2]);
    assert!(buf.len() <= 1);
    // a header that claims less than itself is skipped, not looped on
    let mut bad = vec![0xff, 0xf1, 0x50, 0x80, 0x00, 0x1f, 0xfc, 0, 0, 0, 0];
    assert!(adts_take_frames(&mut bad).is_empty());
}

fn payload(b: u8) -> Vec<u8> {
    latm_wrap(&[b, 1, 2, 3])
}

#[test]
fn packets_in_order_become_adts_frames_in_the_decoder_and_its_sound_comes_out() {
    let mut s = AacStream::new(cfg(), Box::new(FakeDec { ch: 2, ready: vec![], fail: false, fed: vec![] }));
    assert!(s.on_media(&rtp(10, &payload(7))).is_empty());
    assert!(s.on_media(&rtp(11, &payload(9))).is_empty());
    let b = s.poll().unwrap().unwrap();
    assert_eq!((b.rate, b.channels, b.pcm.len()), (44100, 2, 2 * 2048));
    assert_eq!((b.pcm[0], b.pcm[2048 * 2 - 1]), (7, 9));
    assert!(s.poll().unwrap().is_none());
    assert_eq!((s.stats.packets, s.stats.frames), (2, 2));
}

#[test]
fn a_lost_packet_is_one_frame_of_silence_and_a_damaged_one_does_not_stop_the_stream() {
    let mut s = AacStream::new(cfg(), Box::new(FakeDec { ch: 2, ready: vec![], fail: false, fed: vec![] }));
    let mut silence = 0;
    s.on_media(&rtp(1, &payload(1)));
    // 2 is missing; four later packets declare it lost
    for q in 3..8 {
        for b in s.on_media(&rtp(q, &payload(1))) {
            assert!(b.pcm.iter().all(|x| *x == 0));
            silence += b.pcm.len();
        }
    }
    assert_eq!(silence, FRAME_SAMPLES * 2);
    assert_eq!(s.stats.lost_packets, 1);
    let bad = s.on_media(&rtp(8, &[9, 1]));
    assert_eq!(bad.len(), 1);
    assert_eq!(s.stats.bad_frames, 1);
    assert!(s.on_media(&[1, 2, 3]).is_empty());
    assert_eq!(s.stats.bad_packets, 1);
    s.on_media(&rtp(9, &payload(4)));
    assert!(s.poll().unwrap().is_some());
}

#[test]
fn a_decoder_that_stops_is_counted_and_its_reason_is_given_by_poll() {
    struct Dead;
    impl AacDecode for Dead {
        fn feed(&mut self, _: &[u8]) -> Result<(), String> {
            Err("gone".into())
        }
        fn drain(&mut self) -> Result<Vec<i16>, String> {
            Err("the audio decoder stopped".into())
        }
    }
    let mut s = AacStream::new(cfg(), Box::new(Dead));
    s.on_media(&rtp(1, &payload(1)));
    assert_eq!(s.stats.bad_frames, 1);
    assert_eq!(s.poll().unwrap_err(), "the audio decoder stopped");
}

#[test]
fn what_the_packetizer_sends_the_receiving_side_reads_back() {
    let mut p = AacPacketizer::new(0xabcd);
    let frames: Vec<Vec<u8>> = (1..=3u8).map(|i| vec![i; 20 + i as usize]).collect();
    let pk: Vec<Vec<u8>> = frames.iter().map(|f| p.packet(f)).collect();
    let r = parse_rtp(&pk[1]).unwrap();
    assert_eq!((r.seq, r.timestamp, r.ssrc), (1, 1024, 0xabcd));
    let mut s = AacStream::new(cfg(), Box::new(FakeDec { ch: 2, ready: vec![], fail: false, fed: vec![] }));
    for k in &pk {
        s.on_media(k);
    }
    let b = s.poll().unwrap().unwrap();
    assert_eq!(b.pcm.len(), 3 * 2048);
    assert_eq!(b.pcm[0], 1);
}

struct Fb(bool);
impl AacBackend for Fb {
    fn available(&self) -> bool {
        self.0
    }
    fn decoder(&self, c: &AacConfig) -> Result<Box<dyn AacDecode>, String> {
        Ok(Box::new(FakeDec { ch: c.channels as usize, ready: vec![], fail: false, fed: vec![] }))
    }
    fn encoder(&self, _: &AacConfig) -> Result<Box<dyn AacEncode>, String> {
        Err("no".into())
    }
}

// the only test that touches the process-wide backend registry (no other test in this crate sets or reads it)
#[test]
fn aac_is_offered_only_while_a_backend_says_the_add_on_is_installed() {
    clear_backend();
    assert!(!aac_ready());
    assert_eq!(status_line(), NEEDS_ADDON);
    assert!(note_line().contains("needs the audio and video add-on"));
    set_backend(Arc::new(Fb(false)));
    assert!(!aac_ready());
    set_backend(Arc::new(Fb(true)));
    assert!(aac_ready());
    assert!(status_line().contains("available"));
    assert!(note_line().contains("AAC works"));
    clear_backend();
}

// ------------------------------------------------------------------------------------------------ the sink endpoint

mod sink {
    use super::*;
    use crate::avdtp::*;

    fn cmd(signal: u8, params: &[u8]) -> Msg {
        Msg { label: 1, kind: MsgType::Command, signal, params: params.to_vec() }
    }
    fn ask(s: &mut Sink, m: Msg) -> (Msg, Vec<Action>) {
        let (r, a) = s.handle(&m);
        (r.expect("answered"), a)
    }
    fn aac_config(seid: u8, bytes: [u8; 6]) -> Vec<u8> {
        let mut v = vec![seid << 2, 5 << 2, CAT_MEDIA_TRANSPORT, 0, CAT_MEDIA_CODEC, 8, 0, CODEC_AAC];
        v.extend_from_slice(&bytes);
        v
    }
    const CAT_MEDIA_TRANSPORT: u8 = 1;
    const CAT_MEDIA_CODEC: u8 = 7;

    #[test]
    fn without_the_add_on_only_the_sbc_endpoint_exists_and_an_aac_one_is_refused() {
        let mut s = Sink::new();
        assert_eq!(ask(&mut s, cmd(SIG_DISCOVER, &[])).0.params, vec![0x04, 0x08]);
        let (r, _) = ask(&mut s, cmd(SIG_GET_CAPABILITIES, &[AAC_SEID << 2]));
        assert_eq!((r.kind, r.params), (MsgType::Reject, vec![ERR_BAD_ACP_SEID]));
        let (r, a) = ask(&mut s, cmd(SIG_SET_CONFIGURATION, &aac_config(AAC_SEID, AacConfig { rate: 44100, channels: 2, bit_rate: 0 }.to_config_bytes(true))));
        assert_eq!((r.kind, r.params, a), (MsgType::Reject, vec![0, ERR_BAD_ACP_SEID], vec![]));
    }

    #[test]
    fn with_the_add_on_a_second_endpoint_offers_aac_and_a_phone_can_set_it_up_and_stream() {
        let mut s = Sink::new().with_aac();
        assert_eq!(ask(&mut s, cmd(SIG_DISCOVER, &[])).0.params, vec![0x04, 0x08, 0x08, 0x08]);
        let (r, _) = ask(&mut s, cmd(SIG_GET_ALL_CAPABILITIES, &[AAC_SEID << 2]));
        assert_eq!(r.params, [1, 0, 7, 8, 0, 2, 0xc0, 0x01, 0x8c, 0x80, 0, 0]);
        // the SBC endpoint still answers as before
        assert_eq!(ask(&mut s, cmd(SIG_GET_CAPABILITIES, &[OUR_SEID << 2])).0.params, our_capabilities());
        let c = AacConfig { rate: 48000, channels: 2, bit_rate: 0 };
        let (r, a) = ask(&mut s, cmd(SIG_SET_CONFIGURATION, &aac_config(AAC_SEID, c.to_config_bytes(true))));
        assert_eq!((r.kind, a), (MsgType::Accept, vec![Action::ConfiguredAac(c)]));
        assert_eq!(s.config_aac(), Some(&c));
        // the endpoint in use is flagged, the other is refused while it is
        assert_eq!(ask(&mut s, cmd(SIG_DISCOVER, &[])).0.params, vec![0x04, 0x08, 0x0a, 0x08]);
        let (r, _) = ask(&mut s, cmd(SIG_SET_CONFIGURATION, &aac_config(AAC_SEID, c.to_config_bytes(true))));
        assert_eq!((r.kind, r.params), (MsgType::Reject, vec![0, ERR_SEP_IN_USE]));
        // the configuration can be read back
        let (r, _) = ask(&mut s, cmd(SIG_GET_CONFIGURATION, &[AAC_SEID << 2]));
        assert_eq!(&r.params[..6], &[1, 0, 7, 8, 0, 2]);
        assert_eq!(AacConfig::from_config_bytes(&r.params[6..]), Ok(c));
        assert_eq!(ask(&mut s, cmd(SIG_OPEN, &[AAC_SEID << 2])).1, vec![Action::Opened]);
        // start names the wrong endpoint: refused; the right one: started
        let (r, _) = ask(&mut s, cmd(SIG_START, &[OUR_SEID << 2]));
        assert_eq!((r.kind, r.params), (MsgType::Reject, vec![OUR_SEID << 2, ERR_BAD_ACP_SEID]));
        assert_eq!(ask(&mut s, cmd(SIG_START, &[AAC_SEID << 2])).1, vec![Action::Started]);
        assert_eq!(ask(&mut s, cmd(SIG_SUSPEND, &[AAC_SEID << 2])).1, vec![Action::Suspended]);
        assert_eq!(ask(&mut s, cmd(SIG_CLOSE, &[AAC_SEID << 2])).1, vec![Action::Ended]);
        // after the close everything is as at the start, and the SBC endpoint can be chosen
        assert_eq!(s.config_aac(), None);
        assert_eq!(ask(&mut s, cmd(SIG_DISCOVER, &[])).0.params, vec![0x04, 0x08, 0x08, 0x08]);
        let sbc = vec![OUR_SEID << 2, 5 << 2, 1, 0, 7, 6, 0, 0, 0x21, 0x15, 2, 53];
        let (r, a) = ask(&mut s, cmd(SIG_SET_CONFIGURATION, &sbc));
        assert_eq!(r.kind, MsgType::Accept);
        assert!(matches!(a[0], Action::Configured(_)));
    }

    #[test]
    fn a_bad_aac_configuration_is_refused_with_the_specifications_code() {
        let mut s = Sink::new().with_aac();
        let both = [0x80, 0x01, 0x84, 0, 0, 0];
        let (r, _) = ask(&mut s, cmd(SIG_SET_CONFIGURATION, &aac_config(AAC_SEID, both)));
        assert_eq!((r.kind, r.params), (MsgType::Reject, vec![CAT_MEDIA_CODEC, ERR_INVALID_SAMPLING_FREQUENCY]));
        let ltp = [0x20, 0x01, 0x04, 0, 0, 0];
        assert_eq!(ask(&mut s, cmd(SIG_SET_CONFIGURATION, &aac_config(AAC_SEID, ltp))).0.params, vec![CAT_MEDIA_CODEC, ERR_INVALID_OBJECT_TYPE]);
        // an SBC element sent to the AAC endpoint
        let mut v = vec![AAC_SEID << 2, 5 << 2, 1, 0, 7, 6, 0, 0, 0x21, 0x15, 2, 53];
        assert_eq!(ask(&mut s, cmd(SIG_SET_CONFIGURATION, &v)).0.params, vec![CAT_MEDIA_CODEC, ERR_INVALID_CODEC_TYPE]);
        // no transport element
        v = vec![AAC_SEID << 2, 5 << 2, 7, 8, 0, 2, 0x80, 0x01, 0x04, 0, 0, 0];
        assert_eq!(ask(&mut s, cmd(SIG_SET_CONFIGURATION, &v)).0.params, vec![CAT_MEDIA_CODEC, ERR_INVALID_CAPABILITIES]);
        assert_eq!(s.state(), StreamState::Idle);
    }
}

// ------------------------------------------------------------------------------------------------ the audio path

mod path {
    use super::*;
    use crate::a2dp::{AudioPath, Output, OutputFactory};

    type Shared = Arc<Mutex<(Vec<(u32, usize)>, Vec<i16>)>>;
    struct Out(Shared);
    impl Output for Out {
        fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
            self.0.lock().unwrap().1.extend_from_slice(pcm);
            Ok(())
        }
    }
    struct Fac(Shared);
    impl OutputFactory for Fac {
        fn open(&mut self, rate: u32, ch: usize, _: &str) -> Result<Box<dyn Output>, String> {
            self.0.lock().unwrap().0.push((rate, ch));
            Ok(Box::new(Out(self.0.clone())))
        }
    }

    #[test]
    fn an_aac_stream_is_decoded_by_the_add_on_and_played_and_stopping_frees_the_speaker() {
        let seen: Shared = Arc::default();
        let mut p = AudioPath::new(Box::new(Fac(seen.clone())), "phone");
        p.set_aac(cfg(), Arc::new(Fb(true)));
        assert_eq!(p.aac_config(), Some(cfg()));
        // not started: packets are dropped
        p.on_media(&rtp(1, &payload(3)));
        assert!(seen.lock().unwrap().0.is_empty());
        p.start();
        p.on_media(&rtp(2, &payload(5)));
        p.poll();
        let g = seen.lock().unwrap();
        assert_eq!(g.0, vec![(44100, 2)]);
        assert_eq!(g.1.len(), 2048);
        assert_eq!(g.1[0], 5);
        drop(g);
        assert!(p.playing());
        assert_eq!(p.stats().packets, 1);
        p.stop();
        assert!(!p.playing());
    }

    #[test]
    fn without_a_decoder_the_path_says_why_and_a_decoder_that_stops_ends_the_sound_with_its_words() {
        struct NoDec;
        impl AacBackend for NoDec {
            fn available(&self) -> bool {
                true
            }
            fn decoder(&self, _: &AacConfig) -> Result<Box<dyn AacDecode>, String> {
                Err("the audio add-on could not be started".into())
            }
            fn encoder(&self, _: &AacConfig) -> Result<Box<dyn AacEncode>, String> {
                Err("no".into())
            }
        }
        let mut p = AudioPath::new(Box::new(Fac(Arc::default())), "phone");
        p.set_aac(cfg(), Arc::new(NoDec));
        p.start();
        assert!(!p.playing());
        assert_eq!(p.error.as_deref(), Some("the audio add-on could not be started"));
        // SBC again: the in-house decoder is used
        p.set_sbc();
        p.start();
        assert!(p.error.is_none());
        assert_eq!(p.aac_config(), None);
    }
}

mod source {
    use super::*;
    use crate::avdtp::*;
    use crate::avdtp_src::{Source, SrcAction, SrcError};

    fn reply(m: &Msg, params: Vec<u8>) -> Msg {
        Msg { label: m.label, kind: MsgType::Accept, signal: m.signal, params }
    }
    fn aac_caps(info: [u8; 6]) -> Vec<u8> {
        let mut v = vec![1, 0, 7, 8, 0, 2];
        v.extend_from_slice(&info);
        v
    }
    fn sbc_caps() -> Vec<u8> {
        our_capabilities()
    }

    /// Plays a sink with these endpoints (their capability bytes), answering the source until it stops asking; returns the Set
    /// Configuration it sent (if any), the actions and the error (if any).
    fn run(src: &mut Source, eps: &[Vec<u8>]) -> (Option<Msg>, Vec<SrcAction>, Option<SrcError>) {
        let mut acts_all = vec![];
        let mut cfg = None;
        let mut cmd = Some(src.begin());
        for _ in 0..20 {
            let Some(c) = cmd.take() else { break };
            let r = match c.signal {
                SIG_DISCOVER => reply(&c, eps.iter().enumerate().flat_map(|(i, _)| [(i as u8 + 1) << 2, 0x08]).collect()),
                SIG_GET_ALL_CAPABILITIES => reply(&c, eps[(c.params[0] >> 2) as usize - 1].clone()),
                SIG_SET_CONFIGURATION => {
                    cfg = Some(c.clone());
                    reply(&c, vec![])
                }
                SIG_OPEN => reply(&c, vec![]),
                _ => break,
            };
            match src.on_msg(&r) {
                Ok((next, acts)) => {
                    cmd = next;
                    acts_all.extend(acts);
                }
                Err(e) => return (cfg, acts_all, Some(e)),
            }
        }
        (cfg, acts_all, None)
    }

    #[test]
    fn aac_wins_over_an_sbc_endpoint_that_comes_first_when_the_add_on_is_there() {
        let mut s = Source::new().with_aac();
        let (cfg, acts, e) = run(&mut s, &[sbc_caps(), aac_caps([0xf0, 0x01, 0x8c, 0, 0, 0])]);
        assert!(e.is_none());
        let cfg = cfg.unwrap();
        // the sink's endpoint 2, our endpoint 1; transport, then the AAC element with MPEG-2 LC, 48 kHz, stereo, 128 kbit/s
        assert_eq!(cfg.params, [2 << 2, 1 << 2, 1, 0, 7, 8, 0, 2, 0x80, 0x00, 0x84, 0x01, 0xf4, 0x00]);
        assert_eq!(acts[0], SrcAction::ConfiguredAac(AacConfig { rate: 48000, channels: 2, bit_rate: 128_000 }));
        assert_eq!(s.chosen_aac().map(|c| c.rate), Some(48000));
    }

    #[test]
    fn without_the_add_on_the_same_headphones_get_sbc_as_before() {
        let mut s = Source::new();
        let (cfg, acts, e) = run(&mut s, &[aac_caps([0xf0, 0x01, 0x8c, 0, 0, 0]), sbc_caps()]);
        assert!(e.is_none());
        assert_eq!(cfg.unwrap().params[0], 2 << 2);
        assert!(matches!(acts[0], SrcAction::Configured(_)));
        assert!(s.chosen_aac().is_none());
    }

    #[test]
    fn with_the_add_on_but_no_aac_endpoint_or_an_unusable_one_sbc_is_chosen_and_nothing_else_ends_in_a_plain_reason() {
        let mut s = Source::new().with_aac();
        let (cfg, acts, e) = run(&mut s, &[sbc_caps()]);
        assert!(e.is_none() && matches!(acts[0], SrcAction::Configured(_)) && cfg.is_some());
        // AAC-LTP only (not the kind sent) beside SBC: SBC
        let mut s = Source::new().with_aac();
        let (_, acts, _) = run(&mut s, &[aac_caps([0x20, 0x01, 0x04, 0, 0, 0]), sbc_caps()]);
        assert!(matches!(acts[0], SrcAction::Configured(_)));
        // AAC only and not usable: the message names AAC, as before
        let mut s = Source::new().with_aac();
        let (_, _, e) = run(&mut s, &[aac_caps([0x20, 0x01, 0x04, 0, 0, 0])]);
        assert_eq!(e, Some(SrcError::NoSbc(vec!["AAC"])));
        // AAC only, usable, and the source has no add-on: still the plain refusal naming AAC
        let mut s = Source::new();
        let (_, _, e) = run(&mut s, &[aac_caps([0xf0, 0x01, 0x8c, 0, 0, 0])]);
        assert_eq!(e, Some(SrcError::NoSbc(vec!["AAC"])));
        // AAC only, usable, add-on there: AAC
        let mut s = Source::new().with_aac();
        let (_, acts, e) = run(&mut s, &[aac_caps([0x40, 0x00, 0x84, 0, 0, 0])]);
        assert!(e.is_none());
        assert_eq!(acts[0], SrcAction::ConfiguredAac(AacConfig { rate: 48000, channels: 2, bit_rate: 128_000 }));
    }
}

#[test]
fn the_settings_line_says_about_aac_only_what_is_true() {
    let v = crate::a2dp::AudioView { ready: true, aac_note: "AAC needs the audio and video add-on (Store); SBC is used.".into(), ..Default::default() };
    assert!(v.line().starts_with("ready: a paired phone") && v.line().ends_with("SBC is used."));
    let v = crate::a2dp::AudioView { ready: true, ..Default::default() };
    assert!(!v.line().contains("AAC"));
}

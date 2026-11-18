use super::*;
use crate::a2dp::{parse_rtp, parse_sbc_payload, split_frames};
use crate::sbc::{ChannelMode, Encoder};

fn acc(m: &Msg, params: &[u8]) -> Msg {
    Msg { label: m.label, kind: MsgType::Accept, signal: m.signal, params: params.to_vec() }
}

fn sbc_caps(extra: &[u8]) -> Vec<u8> {
    let mut v = vec![1, 0, 7, 6, 0, 0];
    v.extend_from_slice(&SbcInfo { freq: 0x30, mode: 0x0b, blocks: 0xf0, subbands: 0x0c, alloc: 0x03, min_bitpool: 2, max_bitpool: 75 }.to_bytes());
    v.extend_from_slice(extra);
    v
}

#[test]
fn the_source_talks_to_our_own_sink_from_discover_to_close_with_a_delay_report() {
    let mut src = Source::new();
    let mut sink = Sink::new().with_delay_reporting();
    let mut acts = Vec::new();
    let mut next = Some(src.begin());
    while let Some(cmd) = next.take() {
        let (reply, a) = sink.handle(&cmd);
        assert!(a.len() <= 1);
        let (n, sa) = src.on_msg(&reply.expect("answered")).unwrap();
        acts.extend(sa);
        next = n;
    }
    let Some(SrcAction::Configured(p)) = acts.first().cloned() else { panic!("{acts:?}") };
    assert_eq!((p.sample_rate(), p.channels(), p.blocks, p.subbands, p.loudness, p.bitpool), (44100, 2, 16, 8, true, 53));
    assert_eq!(acts[1], SrcAction::Opened);
    assert_eq!(sink.state(), StreamState::Open);
    let go = src.start().unwrap();
    let (r, a) = sink.handle(&go);
    assert_eq!(a, vec![Action::Started]);
    assert_eq!(src.on_msg(&r.unwrap()).unwrap().1, vec![SrcAction::Started]);
    // the sink tells how long it takes: 25.0 ms = 250 tenths
    let d = sink.delay_report(250).unwrap();
    assert_eq!((d.signal, d.params.clone()), (SIG_DELAY_REPORT, vec![1 << 2, 0x00, 0xfa]));
    let (reply, acts) = src.on_msg(&d).unwrap();
    assert_eq!((reply.unwrap().kind, acts), (MsgType::Accept, vec![SrcAction::Delay(250)]));
    // pause, resume and close
    let sus = src.suspend().unwrap();
    let (r, a) = sink.handle(&sus);
    assert_eq!(a, vec![Action::Suspended]);
    assert_eq!(src.on_msg(&r.unwrap()).unwrap().1, vec![SrcAction::Suspended]);
    assert!(src.suspend().is_none());
    let go = src.start().unwrap();
    src.on_msg(&sink.handle(&go).0.unwrap()).unwrap();
    let cl = src.close().unwrap();
    let (r, a) = sink.handle(&cl);
    assert_eq!(a, vec![Action::Ended]);
    assert_eq!(src.on_msg(&r.unwrap()).unwrap().1, vec![SrcAction::Ended]);
    assert_eq!((src.state(), sink.state()), (SrcState::Idle, StreamState::Idle));
}

#[test]
fn a_sink_without_delay_reporting_gets_no_delay_element_and_sends_no_report() {
    let mut sink = Sink::new();
    assert!(sink.delay_report(100).is_none());
    assert_eq!(sink.capabilities(), our_capabilities());
    // a configuration that asks for it anyway is refused as before
    let mut p = vec![OUR_SEID << 2, 2 << 2, 1, 0, 7, 6, 0, 0];
    p.extend_from_slice(&SbcInfo { freq: 0x20, mode: 0x01, blocks: 0x80, subbands: 0x04, alloc: 0x01, min_bitpool: 2, max_bitpool: 53 }.to_bytes());
    p.extend_from_slice(&[CAT_DELAY_REPORTING, 0]);
    let (r, _) = sink.handle(&Msg { label: 1, kind: MsgType::Command, signal: SIG_SET_CONFIGURATION, params: p.clone() });
    assert_eq!(r.unwrap().params, vec![CAT_DELAY_REPORTING, ERR_INVALID_CAPABILITIES]);
    // a sink that offers it, configured without it, sends none
    let mut sink = Sink::new().with_delay_reporting();
    let caps = sbc_caps(&[]);
    assert!(scan_capabilities(&caps).is_ok());
    assert!(scan_capabilities(&sink.capabilities()).unwrap().iter().any(|(c, _)| *c == CAT_DELAY_REPORTING));
    p.truncate(p.len() - 2);
    let (r, _) = sink.handle(&Msg { label: 2, kind: MsgType::Command, signal: SIG_SET_CONFIGURATION, params: p });
    assert_eq!(r.unwrap().kind, MsgType::Accept);
    assert!(sink.delay_report(100).is_none());
}

#[test]
fn endpoints_in_use_and_sources_are_skipped_and_an_aac_endpoint_is_passed_over_for_an_sbc_one() {
    let mut src = Source::new();
    let d = src.begin();
    assert_eq!((d.signal, d.label), (SIG_DISCOVER, 0));
    // seid 1 sink, seid 2 sink in use, seid 3 source, seid 4 sink
    let (m, _) = src.on_msg(&acc(&d, &[0x04, 0x08, 0x0a, 0x08, 0x0c, 0x00, 0x10, 0x08])).unwrap();
    let m = m.unwrap();
    assert_eq!((m.signal, m.params.clone()), (SIG_GET_ALL_CAPABILITIES, vec![1 << 2]));
    // seid 1 is an AAC endpoint
    let aac = [1, 0, 7, 8, 0, 2, 0xc0, 0xff, 0xfc, 0x80, 0x00, 0x00];
    let (m, a) = src.on_msg(&acc(&m, &aac)).unwrap();
    let m = m.unwrap();
    assert!(a.is_empty());
    assert_eq!(m.params, vec![4 << 2]);
    // seid 4 takes SBC with delay reporting
    let (m, _) = src.on_msg(&acc(&m, &sbc_caps(&[CAT_DELAY_REPORTING, 0]))).unwrap();
    let m = m.unwrap();
    assert_eq!(m.signal, SIG_SET_CONFIGURATION);
    let want_info = SbcInfo { freq: 0x20, mode: 0x01, blocks: 0x80, subbands: 0x04, alloc: 0x01, min_bitpool: 2, max_bitpool: 53 }.to_bytes();
    let mut want = vec![4 << 2, 1 << 2, 1, 0, 7, 6, 0, 0];
    want.extend_from_slice(&want_info);
    want.extend_from_slice(&[8, 0]);
    assert_eq!(m.params, want);
    let (open, a) = src.on_msg(&acc(&m, &[])).unwrap();
    assert!(matches!(a[0], SrcAction::Configured(_)));
    assert_eq!(open.unwrap().params, vec![4 << 2]);
}

#[test]
fn no_sink_no_sbc_a_refusal_and_garbage_each_end_with_a_plain_reason() {
    // nothing that takes sound
    let mut s = Source::new();
    let d = s.begin();
    let e = s.on_msg(&acc(&d, &[0x0c, 0x00])).unwrap_err();
    assert_eq!(e, SrcError::NoSink);
    assert!(e.text().contains("no audio endpoint"));
    assert_eq!(s.state(), SrcState::Idle);
    // an aptX only speaker
    let mut s = Source::new();
    let d = s.begin();
    let (m, _) = s.on_msg(&acc(&d, &[0x04, 0x08])).unwrap();
    let aptx = [1, 0, 7, 9, 0, 0xff, 0x4f, 0, 0, 0, 0x01, 0x00, 0x3c];
    let e = s.on_msg(&acc(&m.unwrap(), &aptx)).unwrap_err();
    assert_eq!(e, SrcError::NoSbc(vec!["aptX"]));
    assert!(e.text().contains("aptX") && e.text().contains("SBC"));
    // the speaker rejects the configuration with a codec error
    let mut s = Source::new();
    let d = s.begin();
    let (m, _) = s.on_msg(&acc(&d, &[0x04, 0x08])).unwrap();
    let (m, _) = s.on_msg(&acc(&m.unwrap(), &sbc_caps(&[]))).unwrap();
    let m = m.unwrap();
    let rej = Msg { label: m.label, kind: MsgType::Reject, signal: SIG_SET_CONFIGURATION, params: vec![7, ERR_INVALID_SAMPLING_FREQUENCY] };
    let e = s.on_msg(&rej).unwrap_err();
    assert_eq!(e, SrcError::Refused { signal: SIG_SET_CONFIGURATION, code: ERR_INVALID_SAMPLING_FREQUENCY });
    assert!(e.text().contains("choosing the audio settings"));
    // an odd-length endpoint list, and a capability list that does not add up
    let mut s = Source::new();
    let d = s.begin();
    assert_eq!(s.on_msg(&acc(&d, &[0x04])).unwrap_err(), SrcError::Garbled);
    let mut s = Source::new();
    let d = s.begin();
    let (m, _) = s.on_msg(&acc(&d, &[0x04, 0x08])).unwrap();
    assert_eq!(s.on_msg(&acc(&m.unwrap(), &[7, 9, 0])).unwrap_err(), SrcError::Garbled);
    // a general reject
    let mut s = Source::new();
    let d = s.begin();
    let g = Msg { label: d.label, kind: MsgType::GeneralReject, signal: SIG_DISCOVER, params: vec![] };
    assert!(matches!(s.on_msg(&g), Err(SrcError::Refused { .. })));
}

#[test]
fn a_refused_start_leaves_the_stream_open_and_old_answers_and_wrong_state_commands_are_ignored() {
    let mut src = Source::new();
    let mut sink = Sink::new();
    let mut next = Some(src.begin());
    while let Some(c) = next.take() {
        next = src.on_msg(&sink.handle(&c).0.unwrap()).unwrap().0;
    }
    let go = src.start().unwrap();
    assert_eq!(src.state(), SrcState::Starting);
    let rej = Msg { label: go.label, kind: MsgType::Reject, signal: SIG_START, params: vec![1 << 2, ERR_BAD_STATE] };
    assert_eq!(src.on_msg(&rej).unwrap_err(), SrcError::Refused { signal: SIG_START, code: ERR_BAD_STATE });
    assert_eq!(src.state(), SrcState::Open);
    // an answer with a label nobody waits for is ignored
    assert_eq!(src.on_msg(&Msg { label: 9, kind: MsgType::Accept, signal: SIG_START, params: vec![] }).unwrap(), (None, vec![]));
    // a delay report that was not agreed is refused, an unknown command is a general reject
    let d = Msg { label: 3, kind: MsgType::Command, signal: SIG_DELAY_REPORT, params: vec![4, 0, 1] };
    assert_eq!(src.on_msg(&d).unwrap().0.unwrap().params, vec![ERR_BAD_STATE]);
    let u = Msg { label: 4, kind: MsgType::Command, signal: 0x20, params: vec![] };
    assert_eq!(src.on_msg(&u).unwrap().0.unwrap().kind, MsgType::GeneralReject);
    // the sink ends the stream; the link going away ends it too
    let c = Msg { label: 5, kind: MsgType::Command, signal: SIG_CLOSE, params: vec![4] };
    assert_eq!(src.on_msg(&c).unwrap().1, vec![SrcAction::Ended]);
    assert!(src.link_lost().is_empty());
    assert!(src.start().is_none() && src.close().is_none());
}

#[test]
fn codecs_are_named_from_their_capability_bytes() {
    assert_eq!(codec_of(&[0, 0, 1, 2, 3, 4]), Some(Codec::Sbc));
    assert_eq!(codec_of(&[0, 1]), Some(Codec::Mpeg12Audio));
    assert_eq!(codec_of(&[0, 2, 0xc0]), Some(Codec::Aac));
    assert_eq!(codec_of(&[0, 0xff, 0x4f, 0, 0, 0, 1, 0, 0x3c]), Some(Codec::AptX));
    assert_eq!(codec_of(&[0, 0xff, 0xd7, 0, 0, 0, 0x24, 0, 0]), Some(Codec::AptXHd));
    assert_eq!(codec_of(&[0, 0xff, 0x2d, 0x01, 0, 0, 0xaa, 0, 0]), Some(Codec::Ldac));
    assert_eq!(codec_of(&[0, 0xff, 1, 2, 3, 4, 5, 6]), Some(Codec::Vendor { vendor: 0x04030201, id: 0x0605 }));
    assert_eq!(codec_of(&[0, 0xff, 1, 2]), None, "vendor codec without its identifiers");
    assert_eq!(codec_of(&[0x10, 0]), None, "video is not audio");
    assert_eq!(codec_of(&[0]), None);
    assert_eq!(codec_of(&[0, 7]), Some(Codec::Other(7)));
    assert!(Codec::Sbc.we_can_encode() && !Codec::Aac.we_can_encode() && !Codec::AptX.we_can_encode());
    assert_eq!(Codec::AptXHd.name(), "aptX HD");
}

#[test]
fn an_aac_offer_is_read_with_its_rates_channels_and_bit_rate() {
    // MPEG-2 and MPEG-4 LC, 44.1 and 48 kHz and more, stereo and mono, variable bit rate, 320000 bit/s
    let i = parse_aac(&[0xc0, 0x01, 0xf4, 0x84, 0xe2, 0x00]).unwrap();
    assert_eq!(i.object_types, 0xc0);
    assert_eq!(i.sample_rates, vec![44100, 48000, 64000, 88200, 96000]);
    assert_eq!(i.max_channels, 2);
    assert!(i.variable_bit_rate);
    assert_eq!(i.bit_rate, 4 << 16 | 0xe200);
    let j = parse_aac(&[0x40, 0x00, 0x88, 0x00, 0x00, 0x00]).unwrap();
    assert_eq!((j.sample_rates, j.max_channels, j.variable_bit_rate, j.bit_rate), (vec![48000], 1, false, 0));
    assert!(parse_aac(&[0; 5]).is_none() && parse_aac(&[0; 7]).is_none());
}

#[test]
fn the_best_sbc_setting_within_the_limits_is_chosen_and_nothing_is_invented() {
    let all = SbcInfo { freq: 0xf0, mode: 0x0f, blocks: 0xf0, subbands: 0x0c, alloc: 0x03, min_bitpool: 2, max_bitpool: 250 };
    let c = choose_sbc(&all).unwrap();
    assert_eq!((c.freq, c.mode, c.blocks, c.subbands, c.alloc, c.max_bitpool), (0x20, 0x01, 0x80, 0x04, 0x01, 53));
    assert!(c.to_params().is_ok());
    // only 48 kHz mono, 4 blocks, 4 subbands, SNR, a narrow bitpool range above 53
    let weak = SbcInfo { freq: 0x10, mode: 0x08, blocks: 0x10, subbands: 0x08, alloc: 0x02, min_bitpool: 60, max_bitpool: 64 };
    let c = choose_sbc(&weak).unwrap();
    assert_eq!((c.freq, c.mode, c.blocks, c.subbands, c.alloc, c.min_bitpool, c.max_bitpool), (0x10, 0x08, 0x10, 0x08, 0x02, 60, 60));
    // an empty field is an error with a plain sentence; a backwards bitpool range too
    assert!(choose_sbc(&SbcInfo { freq: 0, ..all }).unwrap_err().contains("sampling frequency"));
    assert!(choose_sbc(&SbcInfo { min_bitpool: 9, max_bitpool: 3, ..all }).unwrap_err().contains("bitpool"));
}

#[test]
fn encoded_frames_travel_in_rtp_packets_that_the_sink_side_reads_back() {
    let p = Params { freq: 2, blocks: 16, mode: ChannelMode::Joint, loudness: true, subbands: 8, bitpool: 53 };
    let mut enc = Encoder::new(p).unwrap();
    let pcm = vec![0i16; enc.frame_input_len()];
    let frames: Vec<Vec<u8>> = (0..3).map(|_| enc.encode(&pcm).unwrap()).collect();
    assert!(frames.iter().all(|f| f.len() == p.frame_len()));
    let mut pk = Packetizer::new(0x01020304, &p);
    let a = pk.packet(&frames);
    let b = pk.packet(&frames[..1]);
    let r = parse_rtp(&a).unwrap();
    assert_eq!((r.seq, r.timestamp, r.ssrc, a[1]), (0, 0, 0x01020304, 96));
    let pl = parse_sbc_payload(r.payload).unwrap();
    assert_eq!((pl.fragmented, pl.frames), (false, 3));
    assert_eq!(split_frames(pl.data, 3).unwrap().len(), 3);
    let r2 = parse_rtp(&b).unwrap();
    assert_eq!((r2.seq, r2.timestamp), (1, 128 * 3), "128 samples per frame, three frames sent first");
    assert_eq!(frames_per_packet(672, p.frame_len()), (672 - 13) / p.frame_len());
    assert_eq!(frames_per_packet(10, 100), 1);
    assert_eq!(frames_per_packet(100_000, 10), 15);
    assert_eq!(frames_per_packet(672, 0), 1);
    // the sequence number wraps
    let mut pk = Packetizer::new(1, &p);
    pk.seq = 0xffff;
    pk.packet(&frames[..1]);
    assert_eq!(parse_rtp(&pk.packet(&frames[..1])).unwrap().seq, 0);
}

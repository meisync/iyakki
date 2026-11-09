use super::*;

fn cmd(label: u8, signal: u8, params: &[u8]) -> Msg {
    Msg { label, kind: MsgType::Command, signal, params: params.to_vec() }
}

fn config_params(freq: u8, mode: u8, blocks: u8, subbands: u8, alloc: u8) -> Vec<u8> {
    let mut v = vec![OUR_SEID << 2, 2 << 2, CAT_MEDIA_TRANSPORT, 0, CAT_MEDIA_CODEC, 6, 0, CODEC_SBC];
    v.extend_from_slice(&SbcInfo { freq, mode, blocks, subbands, alloc, min_bitpool: 2, max_bitpool: 53 }.to_bytes());
    v
}

fn ask(s: &mut Sink, m: Msg) -> (Msg, Vec<Action>) {
    let (r, a) = s.handle(&m);
    (r.expect("every command is answered"), a)
}

#[test]
fn a_phone_discovers_asks_for_capabilities_configures_opens_starts_suspends_and_closes() {
    let mut s = Sink::new();
    // Discover: one audio sink endpoint, not in use
    let (r, _) = ask(&mut s, cmd(1, SIG_DISCOVER, &[]));
    assert_eq!((r.kind, r.label, r.signal), (MsgType::Accept, 1, SIG_DISCOVER));
    assert_eq!(r.params, vec![0x04, 0x08]);
    // capabilities: media transport and the SBC codec element (6 bytes: audio, SBC, 4 bytes)
    for sig in [SIG_GET_CAPABILITIES, SIG_GET_ALL_CAPABILITIES] {
        let (r, _) = ask(&mut s, cmd(2, sig, &[OUR_SEID << 2]));
        assert_eq!(r.kind, MsgType::Accept);
        assert_eq!(&r.params[..6], &[1, 0, 7, 6, 0, 0]);
        let (t, sbc) = parse_capabilities(&r.params).unwrap();
        assert!(t);
        assert_eq!(sbc.unwrap(), SbcInfo::ours());
    }
    // configure: 44.1 kHz, joint stereo, 16 blocks, 8 subbands, loudness
    let (r, a) = ask(&mut s, cmd(3, SIG_SET_CONFIGURATION, &config_params(0x20, 0x01, 0x80, 0x04, 0x01)));
    assert_eq!(r.kind, MsgType::Accept);
    assert!(r.params.is_empty());
    let Action::Configured(p) = &a[0] else { panic!("{a:?}") };
    assert_eq!((p.sample_rate(), p.channels(), p.blocks, p.subbands, p.loudness), (44100, 2, 16, 8, true));
    assert_eq!(s.state(), StreamState::Configured);
    // the endpoint now shows as in use
    assert_eq!(ask(&mut s, cmd(4, SIG_DISCOVER, &[])).0.params, vec![0x06, 0x08]);
    let (r, a) = ask(&mut s, cmd(5, SIG_OPEN, &[OUR_SEID << 2]));
    assert_eq!((r.kind, a), (MsgType::Accept, vec![Action::Opened]));
    let (r, a) = ask(&mut s, cmd(6, SIG_START, &[OUR_SEID << 2]));
    assert_eq!((r.kind, a), (MsgType::Accept, vec![Action::Started]));
    assert_eq!(s.state(), StreamState::Streaming);
    let (r, a) = ask(&mut s, cmd(7, SIG_SUSPEND, &[OUR_SEID << 2]));
    assert_eq!((r.kind, a), (MsgType::Accept, vec![Action::Suspended]));
    assert_eq!(s.state(), StreamState::Open);
    ask(&mut s, cmd(8, SIG_START, &[OUR_SEID << 2]));
    let (r, a) = ask(&mut s, cmd(9, SIG_CLOSE, &[OUR_SEID << 2]));
    assert_eq!((r.kind, a), (MsgType::Accept, vec![Action::Ended]));
    assert_eq!(s.state(), StreamState::Idle);
    assert!(s.config().is_none());
}

#[test]
fn commands_in_the_wrong_state_are_refused_with_bad_state_and_abort_always_works() {
    let mut s = Sink::new();
    let seid = [OUR_SEID << 2];
    for sig in [SIG_OPEN, SIG_START, SIG_SUSPEND, SIG_CLOSE] {
        let (r, a) = ask(&mut s, cmd(1, sig, &seid));
        assert_eq!(r.kind, MsgType::Reject, "signal {sig}");
        assert_eq!(*r.params.last().unwrap(), ERR_BAD_STATE);
        assert!(a.is_empty());
    }
    ask(&mut s, cmd(2, SIG_SET_CONFIGURATION, &config_params(0x10, 0x08, 0x10, 0x08, 0x02)));
    // configuring twice: the endpoint is in use
    let (r, _) = ask(&mut s, cmd(3, SIG_SET_CONFIGURATION, &config_params(0x10, 0x08, 0x10, 0x08, 0x02)));
    assert_eq!((r.kind, r.params), (MsgType::Reject, vec![0, ERR_SEP_IN_USE]));
    // start before open, suspend while open
    assert_eq!(ask(&mut s, cmd(4, SIG_START, &seid)).0.kind, MsgType::Reject);
    ask(&mut s, cmd(5, SIG_OPEN, &seid));
    assert_eq!(ask(&mut s, cmd(6, SIG_SUSPEND, &seid)).0.kind, MsgType::Reject);
    let (r, a) = ask(&mut s, cmd(7, SIG_ABORT, &seid));
    assert_eq!((r.kind, a), (MsgType::Accept, vec![Action::Ended]));
    // an abort while idle is accepted too and does nothing
    let (r, a) = ask(&mut s, cmd(8, SIG_ABORT, &seid));
    assert_eq!((r.kind, a), (MsgType::Accept, vec![]));
    assert_eq!(s.state(), StreamState::Idle);
}

#[test]
fn a_wrong_endpoint_or_a_configuration_with_several_values_is_refused_with_the_codes_of_the_specification() {
    let mut s = Sink::new();
    assert_eq!(ask(&mut s, cmd(1, SIG_GET_CAPABILITIES, &[9 << 2])).0.params, vec![ERR_BAD_ACP_SEID]);
    assert_eq!(ask(&mut s, cmd(1, SIG_OPEN, &[9 << 2])).0.params, vec![ERR_BAD_ACP_SEID]);
    let mut bad_seid = config_params(0x20, 0x01, 0x80, 0x04, 0x01);
    bad_seid[0] = 9 << 2;
    assert_eq!(ask(&mut s, cmd(2, SIG_SET_CONFIGURATION, &bad_seid)).0.params, vec![0, ERR_BAD_ACP_SEID]);
    let cases = [
        (config_params(0x30, 0x01, 0x80, 0x04, 0x01), ERR_INVALID_SAMPLING_FREQUENCY),
        (config_params(0x20, 0x03, 0x80, 0x04, 0x01), ERR_INVALID_CHANNEL_MODE),
        (config_params(0x20, 0x01, 0xc0, 0x04, 0x01), ERR_INVALID_BLOCK_LENGTH),
        (config_params(0x20, 0x01, 0x80, 0x0c, 0x01), ERR_INVALID_SUBBANDS),
        (config_params(0x20, 0x01, 0x80, 0x04, 0x03), ERR_INVALID_ALLOCATION_METHOD),
    ];
    for (p, code) in cases {
        let (r, a) = ask(&mut s, cmd(3, SIG_SET_CONFIGURATION, &p));
        assert_eq!((r.kind, r.params), (MsgType::Reject, vec![CAT_MEDIA_CODEC, code]));
        assert!(a.is_empty());
        assert_eq!(s.state(), StreamState::Idle);
    }
    // another codec (MPEG-2,4 AAC is 2)
    let mut aac = config_params(0x20, 0x01, 0x80, 0x04, 0x01);
    aac[7] = 2;
    assert_eq!(ask(&mut s, cmd(4, SIG_SET_CONFIGURATION, &aac)).0.params, vec![CAT_MEDIA_CODEC, ERR_INVALID_CODEC_TYPE]);
    // an element we never offered (delay reporting is 8)
    let mut extra = config_params(0x20, 0x01, 0x80, 0x04, 0x01);
    extra.extend_from_slice(&[8, 0]);
    assert_eq!(ask(&mut s, cmd(5, SIG_SET_CONFIGURATION, &extra)).0.params, vec![8, ERR_INVALID_CAPABILITIES]);
    // truncated and without the codec
    assert_eq!(ask(&mut s, cmd(6, SIG_SET_CONFIGURATION, &[OUR_SEID << 2, 8, 7, 6, 0])).0.kind, MsgType::Reject);
    assert_eq!(ask(&mut s, cmd(7, SIG_SET_CONFIGURATION, &[OUR_SEID << 2, 8, 1, 0])).0.kind, MsgType::Reject);
    assert_eq!(ask(&mut s, cmd(8, SIG_SET_CONFIGURATION, &[OUR_SEID << 2])).0.kind, MsgType::Reject);
    // start with a wrong endpoint in the list names the endpoint
    let (r, _) = ask(&mut s, cmd(9, SIG_START, &[OUR_SEID << 2, 5 << 2]));
    assert_eq!(r.params, vec![5 << 2, ERR_BAD_ACP_SEID]);
}

#[test]
fn unknown_commands_get_a_general_reject_and_replies_from_the_phone_are_ignored() {
    let mut s = Sink::new();
    let (r, a) = s.handle(&cmd(4, 0x0b, &[1, 2, 3]));
    let r = r.unwrap();
    assert_eq!((r.kind, r.label, r.signal), (MsgType::GeneralReject, 4, 0x0b));
    assert!(a.is_empty());
    assert_eq!(s.handle(&Msg { label: 1, kind: MsgType::Accept, signal: SIG_START, params: vec![] }), (None, vec![]));
    assert_eq!(s.handle(&Msg { label: 1, kind: MsgType::Reject, signal: SIG_START, params: vec![0x31] }), (None, vec![]));
}

#[test]
fn losing_the_link_ends_a_stream_like_an_abort() {
    let mut s = Sink::new();
    assert!(s.link_lost().is_empty());
    ask(&mut s, cmd(1, SIG_SET_CONFIGURATION, &config_params(0x20, 0x01, 0x80, 0x04, 0x01)));
    ask(&mut s, cmd(2, SIG_OPEN, &[OUR_SEID << 2]));
    assert_eq!(s.link_lost(), vec![Action::Ended]);
    assert_eq!(s.state(), StreamState::Idle);
}

#[test]
fn the_wire_format_has_the_label_the_packet_type_and_the_message_type_in_the_right_bits() {
    let m = Msg { label: 0x0a, kind: MsgType::Accept, signal: SIG_DISCOVER, params: vec![0x04, 0x08] };
    assert_eq!(m.encode(), vec![0xa2, 0x01, 0x04, 0x08]);
    let mut r = Reassembler::default();
    assert_eq!(r.push(&[0xa2, 0x01, 0x04, 0x08]).unwrap().unwrap(), m);
    let rej = Msg { label: 3, kind: MsgType::Reject, signal: SIG_OPEN, params: vec![ERR_BAD_STATE] };
    assert_eq!(rej.encode(), vec![0x33, 0x06, 0x31]);
}

#[test]
fn fragmented_messages_are_put_together_and_broken_ones_are_errors() {
    let mut r = Reassembler::default();
    // start (2 packets in all), end
    assert_eq!(r.push(&[0x34 | 0, 2, 0x03, 1, 2]), Ok(None)); // label 3, packet type 1 (start), command
    let m = r.push(&[0x3c, 3, 4]).unwrap().unwrap(); // packet type 3 (end)
    assert_eq!(m, Msg { label: 3, kind: MsgType::Command, signal: 3, params: vec![1, 2, 3, 4] });
    // three packets: start, continue, end
    assert_eq!(r.push(&[0x54, 3, 0x0c, 9]), Ok(None));
    assert_eq!(r.push(&[0x58, 8]), Ok(None));
    assert_eq!(r.push(&[0x5c, 7]).unwrap().unwrap().params, vec![9, 8, 7]);
    // errors
    assert!(r.push(&[]).is_err());
    assert!(r.push(&[0x5c, 7]).is_err(), "an end without a start");
    assert!(r.push(&[0x00]).is_err(), "a single packet without a signal");
    r.push(&[0x34, 3, 0x03, 1]).unwrap();
    assert!(r.push(&[0x4c, 1]).is_err(), "another transaction");
    r.push(&[0x34, 3, 0x03, 1]).unwrap();
    assert!(r.push(&[0x3c, 1]).is_err(), "the count says more packets follow");
    r.push(&[0x34, 2, 0x03]).unwrap();
    let big = vec![0u8; 1500];
    let mut cont = vec![0x38];
    cont.extend(&big);
    assert!(r.push(&cont).is_err(), "a message that is too long");
}

#[test]
fn hostile_signalling_never_panics() {
    let mut s = Sink::new();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut r = Reassembler::default();
    for len in 0..40 {
        for _ in 0..300 {
            let b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if let Ok(Some(m)) = r.push(&b) {
                let _ = s.handle(&m);
            }
            let _ = parse_capabilities(&b);
        }
    }
    for sig in 0..64u8 {
        for len in 0..12 {
            let _ = s.handle(&cmd(1, sig, &vec![OUR_SEID << 2; len]));
        }
    }
}

#[test]
fn the_codec_information_round_trips_and_gives_the_codec_settings() {
    let i = SbcInfo { freq: 0x10, mode: 0x02, blocks: 0x20, subbands: 0x08, alloc: 0x02, min_bitpool: 2, max_bitpool: 40 };
    assert_eq!(SbcInfo::from_bytes(&i.to_bytes()), Some(i));
    assert_eq!(i.to_bytes(), [0x12, 0x2a, 2, 40]);
    let p = i.to_params().unwrap();
    assert_eq!((p.sample_rate(), p.channels(), p.blocks, p.subbands, p.loudness, p.mode), (48000, 2, 8, 4, false, ChannelMode::Stereo));
    assert_eq!(SbcInfo::from_bytes(&[1, 2, 3]), None);
    assert_eq!(SbcInfo { min_bitpool: 1, ..i }.to_params(), Err(ERR_INVALID_MINIMUM_BITPOOL));
    assert_eq!(SbcInfo { max_bitpool: 1, ..i }.to_params(), Err(ERR_INVALID_MAXIMUM_BITPOOL));
    assert!(error_text(ERR_BAD_STATE).contains("disagree"));
}

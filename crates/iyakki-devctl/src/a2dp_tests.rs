use super::*;
use crate::sbc::{ChannelMode, Encoder};
use std::sync::{Arc, Mutex};

fn params() -> Params {
    Params { freq: 2, blocks: 16, mode: ChannelMode::Joint, loudness: true, subbands: 8, bitpool: 32 }
}

fn frame(n: usize) -> Vec<u8> {
    let mut e = Encoder::new(params()).unwrap();
    let pcm: Vec<i16> = (0..e.frame_input_len()).map(|i| ((((i / 2 + n * 128) as f64) * 0.05).sin() * 8000.0) as i16).collect();
    e.encode(&pcm).unwrap()
}

fn rtp(seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x80, 0x60];
    v.extend_from_slice(&seq.to_be_bytes());
    v.extend_from_slice(&(seq as u32 * 128).to_be_bytes());
    v.extend_from_slice(&0x1234_5678u32.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

fn media(seq: u16, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut p = vec![frames.len() as u8];
    for f in frames {
        p.extend_from_slice(f);
    }
    rtp(seq, &p)
}

// ------------------------------------------------------------------------------------------------ SDP

fn sdp_req(id: u8, tid: u16, params: &[u8]) -> Vec<u8> {
    sdp_pdu(id, tid, params)
}

fn pattern(u: u16) -> Vec<u8> {
    e_seq(&[e_uuid16(u)])
}

fn all_attrs() -> Vec<u8> {
    e_seq(&[e_uint32(0x0000_ffff)])
}

#[test]
fn a_service_search_for_the_audio_sink_finds_the_one_record_and_other_searches_find_nothing() {
    let mut p = pattern(0x110b);
    p.extend_from_slice(&[0, 10, 0]);
    let r = sdp_respond(&sdp_req(0x02, 7, &p), "IYAKKI");
    assert_eq!(r[0], 0x03);
    assert_eq!(u16::from_be_bytes([r[1], r[2]]), 7);
    assert_eq!(&r[5..9], &[0, 1, 0, 1]);
    assert_eq!(&r[9..13], &SDP_RECORD_HANDLE.to_be_bytes());
    for other in [0x1124u16, 0x1108, 0x1200] {
        let mut p = pattern(other);
        p.extend_from_slice(&[0, 10, 0]);
        let r = sdp_respond(&sdp_req(0x02, 8, &p), "IYAKKI");
        assert_eq!(&r[5..9], &[0, 0, 0, 0], "{other:#x}");
    }
    // the public browse root and the L2CAP protocol are in the record too
    for u in [0x1002u16, 0x0100, 0x0019] {
        let mut p = pattern(u);
        p.extend_from_slice(&[0, 10, 0]);
        assert_eq!(&sdp_respond(&sdp_req(0x02, 9, &p), "IYAKKI")[5..9], &[0, 1, 0, 1]);
    }
}

fn attribute_pairs(r: &[u8], skip: usize) -> Vec<(u16, Elem)> {
    // response: pdu, tid, len, byte count, list ..., continuation
    let n = u16::from_be_bytes([r[5], r[6]]) as usize;
    let (e, _) = parse_elem(&r[7 + skip..7 + n]).unwrap();
    let l = e.seq().unwrap();
    l.chunks(2).map(|c| (c[0].uint().unwrap() as u16, c[1].clone())).collect()
}

#[test]
fn the_attributes_are_the_class_the_protocol_stack_the_profile_and_the_name() {
    let mut p = SDP_RECORD_HANDLE.to_be_bytes().to_vec();
    p.extend_from_slice(&[0xff, 0xff]);
    p.extend_from_slice(&all_attrs());
    p.push(0);
    let r = sdp_respond(&sdp_req(0x04, 1, &p), "IYAKKI Audio");
    assert_eq!(r[0], 0x05);
    assert_eq!(*r.last().unwrap(), 0, "no continuation");
    let a = attribute_pairs(&r, 0);
    assert_eq!(a.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0x0000, 0x0001, 0x0004, 0x0005, 0x0009, 0x0100, 0x0311]);
    assert_eq!(a[1].1.seq().unwrap()[0].uuid16(), Some(0x110b));
    // protocol list: L2CAP with PSM 0x19, then AVDTP 1.3
    let proto = a[2].1.seq().unwrap();
    assert_eq!(proto[0].seq().unwrap()[0].uuid16(), Some(0x0100));
    assert_eq!(proto[0].seq().unwrap()[1].uint(), Some(0x19));
    assert_eq!(proto[1].seq().unwrap()[0].uuid16(), Some(0x0019));
    assert_eq!(proto[1].seq().unwrap()[1].uint(), Some(0x0103));
    assert_eq!(a[3].1.seq().unwrap()[0].uuid16(), Some(0x1002));
    assert_eq!(a[4].1.seq().unwrap()[0].seq().unwrap()[0].uuid16(), Some(0x110d));
    assert_eq!(a[5].1, Elem::Str(b"IYAKKI Audio".to_vec()));
    // only the ids asked for come back (a single attribute, then a range)
    let mut p = SDP_RECORD_HANDLE.to_be_bytes().to_vec();
    p.extend_from_slice(&[0xff, 0xff]);
    p.extend_from_slice(&e_seq(&[e_uint16(0x0001), e_uint32(0x0100_0100)]));
    p.push(0);
    let a = attribute_pairs(&sdp_respond(&sdp_req(0x04, 2, &p), "x"), 0);
    assert_eq!(a.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0x0001, 0x0100]);
}

#[test]
fn a_small_size_limit_gives_the_answer_in_pieces_with_a_continuation_that_joins_up() {
    let mut whole = Vec::new();
    let mut cont = vec![0u8];
    let mut rounds = 0;
    loop {
        let mut p = pattern(0x110b);
        p.extend_from_slice(&[0, 12]); // twelve bytes at a time
        p.extend_from_slice(&all_attrs());
        p.extend_from_slice(&cont);
        let r = sdp_respond(&sdp_req(0x06, 5, &p), "IYAKKI");
        assert_eq!(r[0], 0x07, "{r:?}");
        let n = u16::from_be_bytes([r[5], r[6]]) as usize;
        assert!(n <= 12);
        whole.extend_from_slice(&r[7..7 + n]);
        let c = &r[7 + n..];
        rounds += 1;
        if c == [0] {
            break;
        }
        assert_eq!(c[0], 2);
        cont = c.to_vec();
        assert!(rounds < 50);
    }
    assert!(rounds > 3);
    let (e, used) = parse_elem(&whole).unwrap();
    assert_eq!(used, whole.len());
    let lists = e.seq().unwrap();
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].seq().unwrap().len(), 14);
}

#[test]
fn bad_sdp_requests_get_the_sdp_error_response() {
    let code = |r: Vec<u8>| {
        assert_eq!(r[0], 0x01, "{r:?}");
        u16::from_be_bytes([r[5], r[6]])
    };
    assert_eq!(code(sdp_respond(&[], "x")), SDP_ERR_INVALID_PDU_SIZE);
    assert_eq!(code(sdp_respond(&[0x02, 0, 1, 0, 9, 1], "x")), SDP_ERR_INVALID_PDU_SIZE);
    assert_eq!(code(sdp_respond(&sdp_req(0x02, 1, &[1, 2, 3]), "x")), SDP_ERR_INVALID_SYNTAX);
    assert_eq!(code(sdp_respond(&sdp_req(0x09, 1, &[]), "x")), SDP_ERR_INVALID_SYNTAX);
    let mut p = 0x0002_0002u32.to_be_bytes().to_vec();
    p.extend_from_slice(&[0xff, 0xff]);
    p.extend_from_slice(&all_attrs());
    p.push(0);
    assert_eq!(code(sdp_respond(&sdp_req(0x04, 1, &p), "x")), SDP_ERR_INVALID_HANDLE);
    // a continuation that points past the answer, and one with an odd length
    let mut p = pattern(0x110b);
    p.extend_from_slice(&[0xff, 0xff]);
    p.extend_from_slice(&all_attrs());
    p.extend_from_slice(&[2, 0x7f, 0xff]);
    assert_eq!(code(sdp_respond(&sdp_req(0x06, 1, &p), "x")), SDP_ERR_INVALID_CONTINUATION);
    let mut p = pattern(0x110b);
    p.extend_from_slice(&[0xff, 0xff]);
    p.extend_from_slice(&all_attrs());
    p.extend_from_slice(&[3, 1, 2, 3]);
    assert_eq!(code(sdp_respond(&sdp_req(0x06, 1, &p), "x")), SDP_ERR_INVALID_CONTINUATION);
    // every request id with garbage of every length: an answer, never a panic
    let mut seed = 77u64;
    for id in 0..=255u8 {
        for len in 0..24 {
            let p: Vec<u8> = (0..len).map(|_| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) as u8 }).collect();
            let r = sdp_respond(&sdp_req(id, 3, &p), "x");
            assert!(r.len() >= 5);
        }
    }
}

// ------------------------------------------------------------------------------------------------ RTP and order

#[test]
fn the_rtp_header_is_read_with_csrc_extension_and_padding_and_garbage_is_refused() {
    let p = rtp(513, &[1, 2, 3]);
    let r = parse_rtp(&p).unwrap();
    assert_eq!((r.seq, r.timestamp, r.ssrc, r.payload), (513, 513 * 128, 0x1234_5678, &[1u8, 2, 3][..]));
    // two CSRC entries
    let mut c = p.clone();
    c[0] |= 2;
    c.splice(12..12, [0u8; 8]);
    assert_eq!(parse_rtp(&c).unwrap().payload, &[1, 2, 3]);
    // an extension of one word
    let mut e = p.clone();
    e[0] |= 0x10;
    e.splice(12..12, [0xbe, 0xde, 0, 1, 9, 9, 9, 9]);
    assert_eq!(parse_rtp(&e).unwrap().payload, &[1, 2, 3]);
    // padding of two bytes
    let mut d = p.clone();
    d[0] |= 0x20;
    d.extend_from_slice(&[0, 2]);
    assert_eq!(parse_rtp(&d).unwrap().payload, &[1, 2, 3]);
    assert!(parse_rtp(&[0x80; 5]).is_err());
    assert!(parse_rtp(&[0x40; 20]).is_err());
    assert!(parse_rtp(&rtp(1, &[])).is_err());
    let mut bad = p.clone();
    bad[0] |= 0x20;
    *bad.last_mut().unwrap() = 200;
    assert!(parse_rtp(&bad).is_err());
    let mut ext = p.clone();
    ext[0] |= 0x10;
    assert!(parse_rtp(&ext).is_err());
    let mut seed = 5u64;
    for len in 0..80 {
        for _ in 0..100 {
            let b: Vec<u8> = (0..len).map(|_| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1); (seed >> 33) as u8 }).collect();
            let _ = parse_rtp(&b);
            let _ = parse_sbc_payload(&b);
            let _ = split_frames(&b, 3);
        }
    }
}

fn seqs(o: Vec<Ordered>) -> Vec<String> {
    o.into_iter().map(|x| match x { Ordered::Packet(p) => format!("p{}", p[0]), Ordered::Lost(n) => format!("L{n}") }).collect()
}

#[test]
fn packets_in_order_pass_straight_through_and_swapped_ones_are_put_right() {
    let mut r = Reorder::default();
    assert_eq!(seqs(r.push(100, vec![0])), ["p0"]);
    assert_eq!(seqs(r.push(101, vec![1])), ["p1"]);
    // 103 before 102
    assert!(r.push(103, vec![3]).is_empty());
    assert_eq!(seqs(r.push(102, vec![2])), ["p2", "p3"]);
    // a copy of an old one and a copy of a held one
    assert!(r.push(101, vec![1]).is_empty());
    assert!(r.push(105, vec![5]).is_empty());
    assert!(r.push(105, vec![5]).is_empty());
    assert_eq!(r.late, 2);
    assert_eq!(seqs(r.push(104, vec![4])), ["p4", "p5"]);
}

#[test]
fn a_gap_that_does_not_fill_is_declared_lost_after_a_few_later_packets_and_the_rest_follows() {
    let mut r = Reorder::default();
    r.push(10, vec![0]);
    // 11 and 12 never come
    assert!(r.push(13, vec![3]).is_empty());
    assert!(r.push(14, vec![4]).is_empty());
    assert!(r.push(15, vec![5]).is_empty());
    assert!(r.push(16, vec![6]).is_empty());
    assert_eq!(seqs(r.push(17, vec![7])), ["L2", "p3", "p4", "p5", "p6", "p7"]);
    assert_eq!(seqs(r.push(18, vec![8])), ["p8"]);
    // the late packet that was given up on is dropped
    assert!(r.push(11, vec![1]).is_empty());
}

#[test]
fn the_sequence_number_wraps_and_a_restarted_sender_resynchronises() {
    let mut r = Reorder::default();
    r.push(65534, vec![0]);
    assert_eq!(seqs(r.push(65535, vec![1])), ["p1"]);
    assert_eq!(seqs(r.push(0, vec![2])), ["p2"]);
    assert!(r.push(2, vec![4]).is_empty());
    assert_eq!(seqs(r.push(1, vec![3])), ["p3", "p4"]);
    // far away in either direction: a new stream
    assert_eq!(seqs(r.push(30000, vec![9])), ["p9"]);
    assert_eq!(seqs(r.push(30001, vec![10])), ["p10"]);
    assert_eq!(seqs(r.push(100, vec![11])), ["p11"]);
}

// ------------------------------------------------------------------------------------------------ stream and audio path

#[test]
fn media_packets_with_several_frames_decode_in_order_and_a_lost_packet_becomes_silence() {
    let mut s = StreamDecoder::new();
    let f = |n| frame(n);
    let b = s.on_media(&media(1, &[f(0), f(1)]));
    assert_eq!(b.len(), 2);
    assert_eq!((b[0].rate, b[0].channels, b[0].pcm.len()), (44100, 2, 128 * 2));
    // packet 2 is lost, 3 to 6 arrive and wait, 7 pushes the gap out
    for q in 3..=6u16 {
        assert!(s.on_media(&media(q, &[f(2), f(3)])).is_empty());
    }
    let out = s.on_media(&media(7, &[f(4), f(5)]));
    assert_eq!(out.len(), 1 + 5 * 2);
    assert!(out[0].pcm.iter().all(|x| *x == 0), "the lost packet is silence");
    assert_eq!(out[0].pcm.len(), 2 * 128 * 2, "two frames of silence, as long as the packet it replaces");
    assert_eq!(s.stats.lost_packets, 1);
    assert_eq!(s.stats.frames, 12);
    // an old packet is counted and dropped
    assert!(s.on_media(&media(2, &[f(0)])).is_empty());
    assert_eq!(s.stats.late_packets, 1);
}

#[test]
fn a_damaged_frame_is_one_frame_of_silence_and_garbage_does_not_stop_the_stream() {
    let mut s = StreamDecoder::new();
    s.on_media(&media(1, &[frame(0)]));
    let mut bad = frame(1);
    bad[10] ^= 0xff; // the checksum no longer matches
    let out = s.on_media(&media(2, &[bad]));
    assert_eq!(out.len(), 1);
    assert!(out[0].pcm.iter().all(|x| *x == 0));
    assert_eq!(s.stats.bad_frames, 1);
    // not RTP, an empty payload, a payload that is not audio, a cut frame
    assert!(s.on_media(&[1, 2, 3]).is_empty());
    assert!(s.on_media(&rtp(3, &[1])).is_empty());
    assert!(s.on_media(&rtp(4, &[1, 0xaa, 0xbb, 0xcc, 0xdd])).is_empty());
    let f = frame(2);
    assert!(s.on_media(&rtp(5, &[[1u8].as_slice(), &f[..f.len() - 3]].concat())).is_empty());
    assert!(s.stats.bad_packets >= 4);
    assert_eq!(s.on_media(&media(6, &[frame(3)])).len(), 1);
}

#[test]
fn a_frame_split_over_packets_is_joined_and_decoded_once() {
    let mut s = StreamDecoder::new();
    let f = frame(0);
    let (a, b) = f.split_at(f.len() / 2);
    let start = rtp(1, &[[0xc0u8].as_slice(), a].concat()); // fragmented, first
    let last = rtp(2, &[[0xa0u8].as_slice(), b].concat()); // fragmented, last
    assert!(s.on_media(&start).is_empty());
    let out = s.on_media(&last);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].pcm.len(), 128 * 2);
    // a middle piece with no start is ignored
    assert!(s.on_media(&rtp(3, &[[0x80u8].as_slice(), b].concat())).is_empty());
}

#[derive(Default, Clone)]
struct Log {
    opened: Arc<Mutex<Vec<(u32, usize, String)>>>,
    written: Arc<Mutex<usize>>,
    fail_write: Arc<Mutex<bool>>,
    fail_open: Arc<Mutex<bool>>,
    closed: Arc<Mutex<usize>>,
}

struct FakeOut(Log);
impl Output for FakeOut {
    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        if *self.0.fail_write.lock().unwrap() {
            return Err("the speaker went away".into());
        }
        *self.0.written.lock().unwrap() += pcm.len();
        Ok(())
    }
}
impl Drop for FakeOut {
    fn drop(&mut self) {
        *self.0.closed.lock().unwrap() += 1;
    }
}
struct FakeFactory(Log);
impl OutputFactory for FakeFactory {
    fn open(&mut self, rate: u32, channels: usize, label: &str) -> Result<Box<dyn Output>, String> {
        if *self.0.fail_open.lock().unwrap() {
            return Err("the speaker is busy".into());
        }
        self.0.opened.lock().unwrap().push((rate, channels, label.to_string()));
        Ok(Box::new(FakeOut(self.0.clone())))
    }
}

#[test]
fn the_audio_path_opens_the_output_named_after_the_device_when_the_stream_starts_and_closes_it_when_it_stops() {
    let log = Log::default();
    let mut p = AudioPath::new(Box::new(FakeFactory(log.clone())), "Pixel 8");
    // before Start nothing is played
    p.on_media(&media(1, &[frame(0)]));
    assert!(log.opened.lock().unwrap().is_empty() && !p.playing());
    p.start();
    p.on_media(&media(2, &[frame(0), frame(1)]));
    assert_eq!(log.opened.lock().unwrap().clone(), vec![(44100, 2, "Pixel 8".to_string())]);
    assert_eq!(*log.written.lock().unwrap(), 2 * 128 * 2);
    assert!(p.playing());
    p.on_media(&media(3, &[frame(2)]));
    assert_eq!(log.opened.lock().unwrap().len(), 1, "one output for the whole stream");
    assert_eq!(p.params().unwrap().sample_rate(), 44100);
    // suspend frees the speaker at once
    p.stop();
    assert_eq!(*log.closed.lock().unwrap(), 1);
    assert!(!p.playing());
    p.on_media(&media(4, &[frame(3)]));
    assert_eq!(*log.written.lock().unwrap(), 3 * 128 * 2, "nothing played after stop");
    p.start();
    p.on_media(&media(5, &[frame(3)]));
    assert_eq!(log.opened.lock().unwrap().len(), 2);
}

#[test]
fn the_audio_path_says_why_when_the_speaker_cannot_be_opened_or_stops_taking_sound() {
    let log = Log::default();
    *log.fail_open.lock().unwrap() = true;
    let mut p = AudioPath::new(Box::new(FakeFactory(log.clone())), "Laptop");
    p.start();
    p.on_media(&media(1, &[frame(0)]));
    assert_eq!(p.error.as_deref(), Some("the speaker is busy"));
    assert!(!p.playing());
    *log.fail_open.lock().unwrap() = false;
    p.start();
    assert!(p.error.is_none());
    p.on_media(&media(2, &[frame(0)]));
    assert!(p.playing());
    *log.fail_write.lock().unwrap() = true;
    p.on_media(&media(3, &[frame(1)]));
    assert_eq!(p.error.as_deref(), Some("the speaker went away"));
}

//! A live check of the client against a public gortsplib RTSP test stream (H.264 video and AAC audio). It is ignored: it needs the
//! internet and the address of the stream in the environment variable `IYAKKI_RTSP_LIVE_URL` (an address with a token in it: it is
//! never written to a file, and everything this test prints has it taken out). Run with
//! `IYAKKI_RTSP_LIVE_URL=rtsp://... cargo test -p iyakki-rtsp --test live_rtsp -- --ignored --nocapture`.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use iyakki_rtsp::client::{Client, Media};
use iyakki_rtsp::sdp::Codec;
use iyakki_rtsp::{url, Error};

const WANT_UNITS: usize = 30;
const WITHIN: Duration = Duration::from_secs(30);

#[test]
#[ignore = "needs the internet and IYAKKI_RTSP_LIVE_URL"]
fn a_public_gortsplib_stream_is_read_end_to_end() {
    let Ok(raw) = std::env::var("IYAKKI_RTSP_LIVE_URL") else {
        eprintln!("skipped: set IYAKKI_RTSP_LIVE_URL to the address of an RTSP test stream to run this check");
        return;
    };
    // nothing printed may carry the address or its token
    let secret_bits: Vec<String> = raw.split(['?', '&', '=']).skip(1).filter(|s| s.len() >= 8).map(str::to_string).chain([raw.clone()]).collect();
    let clean = move |s: &str| {
        let mut o = s.to_string();
        for b in &secret_bits {
            o = o.replace(b.as_str(), "<hidden>");
        }
        o
    };
    let u = url::parse(&raw).unwrap_or_else(|e| panic!("the address does not parse: {}", clean(&e)));
    let t0 = Instant::now();
    let addr = (u.host.as_str(), u.port).to_socket_addrs().unwrap_or_else(|e| panic!("no address for the host: {e}")).next().expect("an address");
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).unwrap_or_else(|e| panic!("connect: {e}"));
    sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    sock.set_nodelay(true).ok();
    // DESCRIBE, SETUP over TCP (interleaved), PLAY
    let mut c = Client::open(sock, &u, u.credentials()).unwrap_or_else(|e| panic!("open: {}", clean(&e.to_string())));
    let opened = t0.elapsed();
    let Codec::H264 { sps, pps, packetization_mode } = c.codec.clone() else { panic!("the SDP is not H.264: {:?}", c.codec) };
    assert!(!sps.is_empty() && !pps.is_empty(), "the SDP carries the SPS and the PPS");
    assert_eq!(sps[0] & 0x1f, 7, "sprop SPS is a sequence parameter set");
    assert_eq!(pps[0] & 0x1f, 8, "sprop PPS is a picture parameter set");
    eprintln!("opened in {opened:?}: H.264, packetization-mode {packetization_mode}, profile-level-id bytes {:02x?}, SPS {} bytes, PPS {} bytes, payload type {}", &sps[1..4.min(sps.len())], sps.len(), pps.len(), c.payload_type);
    let mut units = 0usize;
    let mut keys = 0usize;
    let mut first_key: Option<(Vec<u8>, usize)> = None;
    let mut in_band_params_before_key = false;
    let mut seen_params = false;
    let mut timeouts = 0;
    let mut last = t0;
    let mut max_gap = Duration::ZERO;
    let start = Instant::now();
    while units < WANT_UNITS && start.elapsed() < WITHIN {
        match c.next_media() {
            Ok(Media::H264(au)) => {
                let types: Vec<u8> = au.nals.iter().map(|n| n[0] & 0x1f).collect();
                max_gap = max_gap.max(last.elapsed());
                last = Instant::now();
                units += 1;
                if types.iter().any(|t| *t == 7 || *t == 8) {
                    seen_params = true;
                }
                if au.is_keyframe() {
                    keys += 1;
                    if first_key.is_none() {
                        in_band_params_before_key = seen_params;
                        first_key = Some((types.clone(), units));
                    }
                }
                assert!(!au.nals.is_empty() && au.nals.iter().all(|n| !n.is_empty()), "a whole access unit has NAL units");
                assert!(au.annex_b().starts_with(&[0, 0, 0, 1]));
            }
            Ok(Media::Jpeg(_)) => panic!("a JPEG from an H.264 stream"),
            Ok(Media::H265(_)) => panic!("H.265 from an H.264 stream"),
            Ok(Media::Audio(_)) => {}
            Err(Error::Timeout) => timeouts += 1,
            Err(e) => panic!("the stream ended after {units} access units: {}", clean(&e.to_string())),
        }
    }
    let took = start.elapsed();
    eprintln!("{units} access units in {took:?} ({keys} key pictures, {timeouts} read timeouts, longest gap {max_gap:?}, dropped {}, bad RTP packets {}, frames counted {})", c.dropped(), c.bad_packets, c.frames);
    assert!(units >= WANT_UNITS, "at least {WANT_UNITS} access units within {WITHIN:?}: got {units}");
    let (types, at) = first_key.expect("a key picture arrived");
    assert!(types.contains(&5), "the first key picture has an IDR slice (NAL type 5): {types:?}");
    // the parameter sets came with the SDP, and a stream that repeats them in band shows them in front of the IDR; either way the decoder has them
    eprintln!("first key picture: access unit {at}, NAL types {types:?}, parameter sets in band before it: {in_band_params_before_key}");
    // TEARDOWN must not hang or fail the connection
    let t = Instant::now();
    let code = c.teardown_checked().unwrap_or_else(|e| panic!("TEARDOWN: {}", clean(&e.to_string())));
    eprintln!("TEARDOWN answered {code} in {:?}", t.elapsed());
    assert!((200..300).contains(&code), "the server accepted the TEARDOWN: {code}");
}

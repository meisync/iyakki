//! The client against scripted cameras on the loopback address (no real camera was used).

use std::net::TcpStream;
use std::time::Duration;

use iyakki_rtsp::client::{Client, Media};
use iyakki_rtsp::fakeserver::{Config, FakeCamera};
use iyakki_rtsp::httpmjpeg::HttpMjpeg;
use iyakki_rtsp::packetize::split_jpeg;
use iyakki_rtsp::url;
use iyakki_rtsp::Error;

const JPG: &[u8] = include_bytes!("data/test-48x32.jpg");

fn conn(port: u16, ms: u64) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(ms))).unwrap();
    s
}

fn cfg(user: Option<(&str, &str)>, digest: bool) -> Config {
    Config { user: user.map(|(u, p)| (u.to_string(), p.to_string())), digest, frames: vec![JPG.to_vec()], interval_ms: 10, ..Default::default() }
}

fn sof_dims(j: &[u8]) -> (u16, u16) {
    let i = j.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    (u16::from_be_bytes([j[i + 7], j[i + 8]]), u16::from_be_bytes([j[i + 5], j[i + 6]]))
}

#[test]
fn rtsp_mjpeg_with_digest_sign_in_delivers_the_pictures_scan_intact() {
    let cam = FakeCamera::rtsp(cfg(Some(("admin", "s3cret")), true));
    let u = url::parse(&format!("rtsp://admin:s3cret@127.0.0.1:{}/live", cam.port)).unwrap();
    let mut c = Client::open(conn(cam.port, 2000), &u, u.credentials()).unwrap();
    assert!(!c.used_basic_auth());
    let (_, _, _, scan) = split_jpeg(JPG).unwrap();
    for _ in 0..3 {
        let Media::Jpeg(j) = c.next_media().unwrap() else { panic!("expected a JPEG") };
        assert_eq!(sof_dims(&j), (48, 32));
        let sos = j.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap() + 14;
        assert_eq!(&j[sos..j.len() - 2], scan.as_slice(), "the entropy-coded data survives the trip");
    }
    assert_eq!(c.frames, 3);
    c.teardown();
    let log = cam.requests();
    assert_eq!(log.iter().filter(|l| l.starts_with("DESCRIBE")).count(), 2, "401 then the signed request: {log:?}");
    assert!(log.iter().any(|l| l.starts_with("SETUP")) && log.iter().any(|l| l.starts_with("PLAY")));
    assert!(log.iter().all(|l| !l.contains("s3cret") && !l.contains("admin")), "the credentials never appear in an address");
    assert!(cam.refused.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}

#[test]
fn basic_sign_in_works_and_is_reported() {
    let cam = FakeCamera::rtsp(cfg(Some(("u", "p")), false));
    let u = url::parse(&format!("rtsp://127.0.0.1:{}/live", cam.port)).unwrap();
    let mut c = Client::open(conn(cam.port, 2000), &u, Some(("u".into(), "p".into()))).unwrap();
    assert!(c.used_basic_auth());
    assert!(matches!(c.next_media().unwrap(), Media::Jpeg(_)));
}

#[test]
fn a_wrong_password_and_a_missing_one_are_told_apart_and_never_loop() {
    let cam = FakeCamera::rtsp(cfg(Some(("u", "p")), true));
    let u = url::parse(&format!("rtsp://127.0.0.1:{}/live", cam.port)).unwrap();
    match Client::open(conn(cam.port, 2000), &u, Some(("u".into(), "wrong".into()))) {
        Err(Error::Auth(m)) => assert!(m.contains("refused")),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("signed in with a wrong password"),
    }
    match Client::open(conn(cam.port, 2000), &u, None) {
        Err(Error::Auth(m)) => assert!(m.contains("needs a user name")),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("signed in without a password"),
    }
    assert!(cam.requests().len() < 8, "no endless retries: {:?}", cam.requests());
}

#[test]
fn h264_arrives_as_access_units_with_the_parameter_sets_and_a_key_picture() {
    let cam = FakeCamera::rtsp(Config { h264: true, interval_ms: 10, ..Default::default() });
    let u = url::parse(&format!("rtsp://127.0.0.1:{}/live", cam.port)).unwrap();
    let mut c = Client::open(conn(cam.port, 2000), &u, None).unwrap();
    assert!(matches!(c.codec, iyakki_rtsp::sdp::Codec::H264 { .. }));
    let Media::H264(au) = c.next_media().unwrap() else { panic!("expected H.264") };
    assert!(au.is_keyframe());
    assert_eq!(au.nals.len(), 3, "SPS, PPS and the IDR slice: {:?}", au.nals.iter().map(|n| n[0]).collect::<Vec<_>>());
    assert!(au.annex_b().starts_with(&[0, 0, 0, 1, 0x67]));
}

#[test]
fn a_silent_camera_times_out_and_a_closing_one_ends_the_stream() {
    let cam = FakeCamera::rtsp(Config { frames: vec![JPG.to_vec()], interval_ms: 600, max_frames: Some(1), ..Default::default() });
    let u = url::parse(&format!("rtsp://127.0.0.1:{}/live", cam.port)).unwrap();
    let mut c = Client::open(conn(cam.port, 100), &u, None).unwrap();
    // the first picture is sent at once; the stream then ends
    let mut got = false;
    let mut end = None;
    for _ in 0..40 {
        match c.next_media() {
            Ok(_) => got = true,
            Err(Error::Timeout) => {}
            Err(e) => {
                end = Some(e);
                break;
            }
        }
    }
    assert!(got);
    assert_eq!(end, Some(Error::Closed));
}

#[test]
fn a_hostile_describe_answer_is_an_error_not_a_hang_or_a_crash() {
    for raw in [
        b"garbage garbage garbage\r\n\r\n".to_vec(),
        b"RTSP/1.0 200 OK\r\nContent-Length: 999999999\r\n\r\n".to_vec(),
        b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 20\r\n\r\nv=0\r\nm=audio 0 RTP/A".to_vec(),
        b"RTSP/1.0 302 Moved\r\nLocation: rtsp://evil/\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"RTSP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
        vec![0xff; 40_000],
    ] {
        let cam = FakeCamera::rtsp(Config { raw_describe: Some(raw.clone()), ..Default::default() });
        let u = url::parse(&format!("rtsp://127.0.0.1:{}/live", cam.port)).unwrap();
        let r = Client::open(conn(cam.port, 1000), &u, None);
        assert!(r.is_err(), "{:?}", String::from_utf8_lossy(&raw[..raw.len().min(30)]));
    }
}

#[test]
fn http_motion_jpeg_streams_with_and_without_part_lengths_and_chunked() {
    for path in ["/video.mjpg", "/video-nolength.mjpg", "/video-chunked.mjpg"] {
        let cam = FakeCamera::http(cfg(Some(("u", "p")), true));
        let u = url::parse(&format!("http://127.0.0.1:{}{path}", cam.port)).unwrap();
        let port = cam.port;
        let mut h = HttpMjpeg::open(Box::new(move || Ok(conn(port, 2000))), &u, Some(("u".into(), "p".into()))).unwrap();
        assert!(!h.is_snapshot() && !h.basic);
        for _ in 0..3 {
            let j = h.next_jpeg().unwrap();
            assert_eq!(j, JPG, "{path}");
        }
    }
}

#[test]
fn http_snapshot_addresses_are_fetched_again_for_every_picture_with_basic_sign_in() {
    let cam = FakeCamera::http(cfg(Some(("u", "p")), false));
    let u = url::parse(&format!("http://127.0.0.1:{}/snapshot.jpg", cam.port)).unwrap();
    let port = cam.port;
    let mut h = HttpMjpeg::open(Box::new(move || Ok(conn(port, 2000))), &u, Some(("u".into(), "p".into()))).unwrap();
    assert!(h.is_snapshot() && h.basic);
    assert_eq!(h.next_jpeg().unwrap(), JPG);
    assert_eq!(h.next_jpeg().unwrap(), JPG);
    let gets = cam.requests().iter().filter(|l| l.starts_with("GET /snapshot.jpg")).count();
    assert_eq!(gets, 3, "an unsigned try, the signed one, and one more for the second picture");
}

#[test]
fn http_errors_are_plain_sentences() {
    let cam = FakeCamera::http(cfg(Some(("u", "p")), true));
    let port = cam.port;
    let mk = move || -> Box<dyn FnMut() -> Result<TcpStream, Error> + Send> { Box::new(move || Ok(conn(port, 2000))) };
    let u = url::parse(&format!("http://127.0.0.1:{port}/nothing")).unwrap();
    assert!(matches!(HttpMjpeg::open(mk(), &u, Some(("u".into(), "p".into()))), Err(Error::Protocol(m)) if m.contains("404")));
    let u = url::parse(&format!("http://127.0.0.1:{port}/video.mjpg")).unwrap();
    assert!(matches!(HttpMjpeg::open(mk(), &u, Some(("u".into(), "bad".into()))), Err(Error::Auth(_))));
    assert!(matches!(HttpMjpeg::open(mk(), &u, None), Err(Error::Auth(_))));
}

//! A scripted camera on the loopback address, for tests and demos: an RTSP server that streams Motion-JPEG or H.264 over TCP
//! (interleaved), with Basic or Digest sign-in, and an HTTP server for `multipart/x-mixed-replace` and single pictures. It speaks just
//! enough of each protocol to exercise a client; it is not a camera.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::auth::{digest_response, params};
use crate::hash::base64_encode;
use crate::packetize::{interleave, packetize_jpeg, rtp_packet, split_jpeg};

#[derive(Clone)]
pub struct Config {
    /// Require this user and password.
    pub user: Option<(String, String)>,
    /// Digest (true) or Basic (false) when `user` is set.
    pub digest: bool,
    /// Complete baseline 4:2:0 JPEG files, sent in turn.
    pub frames: Vec<Vec<u8>>,
    pub interval_ms: u64,
    /// Stream H.264 (canned NAL units) instead of Motion-JPEG.
    pub h264: bool,
    /// Stop (close the connection) after this many pictures.
    pub max_frames: Option<usize>,
    pub path: String,
    /// Send this instead of the RTSP answer to DESCRIBE (hostile server tests): raw bytes.
    pub raw_describe: Option<Vec<u8>>,
}

impl Default for Config {
    fn default() -> Config {
        Config { user: None, digest: true, frames: vec![], interval_ms: 20, h264: false, max_frames: None, path: "/live".into(), raw_describe: None }
    }
}

pub struct FakeCamera {
    pub port: u16,
    stop: Arc<AtomicBool>,
    /// `METHOD uri` of every request, in order (never the Authorization header).
    pub log: Arc<Mutex<Vec<String>>>,
    /// Connections that got as far as receiving pictures.
    pub streams: Arc<AtomicU64>,
    /// Requests that failed the sign-in check.
    pub refused: Arc<AtomicU64>,
}

const NONCE: &str = "0123456789abcdef0123456789abcdef";
const REALM: &str = "FakeCam";

impl Drop for FakeCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn read_head(s: &mut TcpStream) -> Option<(String, Vec<(String, String)>)> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).ok()? == 0 || buf.len() > 16384 {
            return None;
        }
        buf.push(b[0]);
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = text.split("\r\n");
    let first = lines.next()?.to_string();
    let hs = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Some((first, hs))
}

fn authorized(cfg: &Config, method: &str, hs: &[(String, String)]) -> bool {
    let Some((u, p)) = &cfg.user else { return true };
    let Some(h) = hs.iter().find(|h| h.0 == "authorization").map(|h| h.1.clone()) else { return false };
    if !cfg.digest {
        return h == format!("Basic {}", base64_encode(format!("{u}:{p}").as_bytes()));
    }
    let Some(rest) = h.strip_prefix("Digest ") else { return false };
    let pr = params(rest);
    let get = |k: &str| pr.iter().find(|x| x.0 == k).map(|x| x.1.clone()).unwrap_or_default();
    if get("username") != *u || get("nonce") != NONCE {
        return false;
    }
    let qop = if get("qop").is_empty() { None } else { Some((format!("{}:{}", get("nc"), get("cnonce")), get("qop"))) };
    let want = digest_response(u, REALM, p, method, &get("uri"), NONCE, qop.as_ref().map(|(a, b)| (a.as_str(), b.as_str())));
    get("response") == want
}

fn challenge(cfg: &Config) -> String {
    if cfg.digest { format!("WWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"{NONCE}\", qop=\"auth\"\r\n") } else { format!("WWW-Authenticate: Basic realm=\"{REALM}\"\r\n") }
}

impl FakeCamera {
    fn start(cfg: Config, serve: fn(TcpStream, Config, Arc<AtomicBool>, Arc<Mutex<Vec<String>>>, Arc<AtomicU64>, Arc<AtomicU64>)) -> FakeCamera {
        let l = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
        let cam = FakeCamera { port, stop: Arc::new(AtomicBool::new(false)), log: Arc::new(Mutex::new(vec![])), streams: Arc::new(AtomicU64::new(0)), refused: Arc::new(AtomicU64::new(0)) };
        let (stop, log, streams, refused) = (cam.stop.clone(), cam.log.clone(), cam.streams.clone(), cam.refused.clone());
        std::thread::spawn(move || {
            for c in l.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(c) = c else { continue };
                let (cfg, stop, log, streams, refused) = (cfg.clone(), stop.clone(), log.clone(), streams.clone(), refused.clone());
                std::thread::spawn(move || serve(c, cfg, stop, log, streams, refused));
            }
        });
        cam
    }

    pub fn rtsp(cfg: Config) -> FakeCamera {
        FakeCamera::start(cfg, serve_rtsp)
    }

    /// `/video.mjpg` is a multipart stream, `/snapshot.jpg` a single picture, anything else 404.
    pub fn http(cfg: Config) -> FakeCamera {
        FakeCamera::start(cfg, serve_http)
    }

    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn reply(s: &mut TcpStream, code: u16, reason: &str, cseq: &str, extra: &str, body: &str) {
    let r = format!("RTSP/1.0 {code} {reason}\r\nCSeq: {cseq}\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = s.write_all(r.as_bytes());
}

fn serve_rtsp(mut s: TcpStream, cfg: Config, stop: Arc<AtomicBool>, log: Arc<Mutex<Vec<String>>>, streams: Arc<AtomicU64>, refused: Arc<AtomicU64>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut sdp_sent = false;
    while let Some((first, hs)) = read_head(&mut s) {
        let mut it = first.split(' ');
        let (method, uri) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
        log.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{method} {uri}"));
        let cseq = hs.iter().find(|h| h.0 == "cseq").map(|h| h.1.clone()).unwrap_or_default();
        if matches!(method.as_str(), "DESCRIBE" | "SETUP" | "PLAY") && !authorized(&cfg, &method, &hs) {
            refused.fetch_add(1, Ordering::SeqCst);
            reply(&mut s, 401, "Unauthorized", &cseq, &challenge(&cfg), "");
            continue;
        }
        match method.as_str() {
            "OPTIONS" => reply(&mut s, 200, "OK", &cseq, "Public: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN\r\n", ""),
            "DESCRIBE" => {
                if let Some(raw) = &cfg.raw_describe {
                    let _ = s.write_all(raw);
                    return;
                }
                let sdp = if cfg.h264 {
                    "v=0\r\ns=fake\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z2QAH6zZQFAFuhAAAAMAEAAAAwPA8YMZYA==,aOvjyyLA\r\na=control:track1\r\n"
                } else {
                    "v=0\r\ns=fake\r\nm=video 0 RTP/AVP 26\r\na=control:track1\r\n"
                };
                sdp_sent = true;
                reply(&mut s, 200, "OK", &cseq, &format!("Content-Base: rtsp://127.0.0.1/{}/\r\nContent-Type: application/sdp\r\n", cfg.path.trim_start_matches('/')), sdp);
            }
            "SETUP" if sdp_sent => {
                let tr = hs.iter().find(|h| h.0 == "transport").map(|h| h.1.clone()).unwrap_or_default();
                if !tr.contains("TCP") {
                    reply(&mut s, 461, "Unsupported Transport", &cseq, "", "");
                    continue;
                }
                reply(&mut s, 200, "OK", &cseq, "Session: 4711;timeout=60\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n", "");
            }
            "PLAY" if sdp_sent => {
                reply(&mut s, 200, "OK", &cseq, "Session: 4711\r\n", "");
                streams.fetch_add(1, Ordering::SeqCst);
                stream_rtp(&mut s, &cfg, &stop);
                return;
            }
            "TEARDOWN" => {
                reply(&mut s, 200, "OK", &cseq, "", "");
                return;
            }
            _ => reply(&mut s, 455, "Method Not Valid in This State", &cseq, "", ""),
        }
    }
}

fn stream_rtp(s: &mut TcpStream, cfg: &Config, stop: &AtomicBool) {
    let (mut seq, mut ts, mut n) = (1000u16, 90_000u32, 0usize);
    loop {
        if stop.load(Ordering::SeqCst) || cfg.max_frames.is_some_and(|m| n >= m) {
            return;
        }
        let mut wire: Vec<Vec<u8>> = Vec::new();
        if cfg.h264 {
            let (sps, pps) = (vec![0x67u8, 0x64, 0, 0x1f], vec![0x68u8, 0xeb, 0xe3]);
            let idr: Vec<u8> = std::iter::once(0x65u8).chain((0..2500u32).map(|i| (i % 200) as u8 + 1)).collect();
            let mut stap = vec![24u8];
            for x in [&sps, &pps] {
                stap.extend_from_slice(&(x.len() as u16).to_be_bytes());
                stap.extend_from_slice(x);
            }
            wire.push(rtp_packet(96, false, seq, ts, 9, &stap));
            seq = seq.wrapping_add(1);
            let chunks: Vec<&[u8]> = idr[1..].chunks(1000).collect();
            for (i, c) in chunks.iter().enumerate() {
                let mut v = vec![(idr[0] & 0xe0) | 28, (if i == 0 { 0x80 } else { 0 }) | (if i == chunks.len() - 1 { 0x40 } else { 0 }) | (idr[0] & 0x1f)];
                v.extend_from_slice(c);
                wire.push(rtp_packet(96, i == chunks.len() - 1, seq, ts, 9, &v));
                seq = seq.wrapping_add(1);
            }
        } else if let Some(j) = cfg.frames.get(n % cfg.frames.len().max(1)) {
            let Some((w, h, qt, scan)) = split_jpeg(j) else { return };
            for (last, p) in packetize_jpeg(w, h, 1, 255, Some(&qt), &scan, 1000) {
                wire.push(rtp_packet(26, last, seq, ts, 9, &p));
                seq = seq.wrapping_add(1);
            }
        } else {
            return;
        }
        // an RTCP-looking packet on the odd channel now and then, which a client must skip
        let mut out = vec![];
        if n % 3 == 0 {
            out.extend(interleave(1, &[0x80, 200, 0, 6, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        }
        for p in &wire {
            out.extend(interleave(0, p));
        }
        if s.write_all(&out).is_err() {
            return;
        }
        n += 1;
        ts = ts.wrapping_add(3000);
        std::thread::sleep(Duration::from_millis(cfg.interval_ms));
    }
}

fn serve_http(mut s: TcpStream, cfg: Config, stop: Arc<AtomicBool>, log: Arc<Mutex<Vec<String>>>, streams: Arc<AtomicU64>, refused: Arc<AtomicU64>) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let Some((first, hs)) = read_head(&mut s) else { return };
    let mut it = first.split(' ');
    let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
    log.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{method} {path}"));
    if !authorized(&cfg, &method, &hs) {
        refused.fetch_add(1, Ordering::SeqCst);
        let _ = s.write_all(format!("HTTP/1.1 401 Unauthorized\r\n{}Content-Length: 0\r\nConnection: close\r\n\r\n", challenge(&cfg)).as_bytes());
        return;
    }
    let Some(first_frame) = cfg.frames.first() else { return };
    match path.as_str() {
        "/snapshot.jpg" => {
            streams.fetch_add(1, Ordering::SeqCst);
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", first_frame.len()).as_bytes());
            let _ = s.write_all(first_frame);
        }
        "/video.mjpg" | "/video-nolength.mjpg" | "/video-chunked.mjpg" => {
            streams.fetch_add(1, Ordering::SeqCst);
            let chunked = path.contains("chunked");
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=\"frame\"\r\n{}Connection: close\r\n\r\n", if chunked { "Transfer-Encoding: chunked\r\n" } else { "" }).as_bytes());
            let mut n = 0usize;
            loop {
                if stop.load(Ordering::SeqCst) || cfg.max_frames.is_some_and(|m| n >= m) {
                    return;
                }
                let f = &cfg.frames[n % cfg.frames.len()];
                let head = if path.contains("nolength") { "--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_string() } else { format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", f.len()) };
                let mut part = head.into_bytes();
                part.extend_from_slice(f);
                part.extend_from_slice(b"\r\n");
                let ok = if chunked {
                    // odd chunk sizes on purpose: a chunk boundary can fall anywhere
                    part.chunks(331).all(|c| s.write_all(format!("{:x}\r\n", c.len()).as_bytes()).is_ok() && s.write_all(c).is_ok() && s.write_all(b"\r\n").is_ok())
                } else {
                    s.write_all(&part).is_ok()
                };
                if !ok {
                    return;
                }
                n += 1;
                std::thread::sleep(Duration::from_millis(cfg.interval_ms));
            }
        }
        _ => {
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
}

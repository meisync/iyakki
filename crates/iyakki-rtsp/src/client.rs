//! The RTSP client: OPTIONS-free `DESCRIBE`, `SETUP` (TCP, interleaved) and `PLAY`, then pictures. It works over any
//! `Read + Write`; the caller sets a read timeout on the socket so `next_media` returns `Err(Timeout)` now and then (the caller checks
//! its stop flag and calls again; keep-alives are sent meanwhile).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::auth::{self, Auth};
use crate::msg::{request, Incoming, Reader, Response};
use crate::rtp::{parse_rtp, AccessUnit, H264Depay, MjpegDepay};
use crate::sdp::{self, Codec};
use crate::url::Url;
use crate::Error;

/// One decoded-by-the-caller unit of video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Media {
    /// A complete JPEG file.
    Jpeg(Vec<u8>),
    H264(AccessUnit),
}

pub struct Client<T: Read + Write> {
    rd: Reader<T>,
    uri: String,
    base: String,
    cseq: u32,
    session: Option<String>,
    auth: Option<Auth>,
    creds: Option<(String, String)>,
    pub codec: Codec,
    pub payload_type: u8,
    channel: u8,
    mjpeg: MjpegDepay,
    h264: H264Depay,
    keepalive_every: Duration,
    last_sent: Instant,
    /// Frames seen so far.
    pub frames: u64,
    /// RTP packets that could not be parsed.
    pub bad_packets: u64,
}

fn resolve(base: &str, control: Option<&str>) -> String {
    match control.map(str::trim) {
        None | Some("") | Some("*") => base.to_string(),
        Some(c) if c.to_ascii_lowercase().starts_with("rtsp://") => c.to_string(),
        Some(c) => format!("{}/{}", base.trim_end_matches('/'), c.trim_start_matches('/')),
    }
}

impl<T: Read + Write> Client<T> {
    /// Connect the protocol over `transport`. `creds` are used only if the camera asks for them.
    pub fn open(transport: T, url: &Url, creds: Option<(String, String)>) -> Result<Client<T>, Error> {
        let uri = url.request_uri();
        let mut c = Client { rd: Reader::new(transport), uri: uri.clone(), base: uri.clone(), cseq: 0, session: None, auth: None, creds, codec: Codec::Mjpeg, payload_type: 26, channel: 0, mjpeg: MjpegDepay::new(), h264: H264Depay::new(), keepalive_every: Duration::from_secs(25), last_sent: Instant::now(), frames: 0, bad_packets: 0 };
        let d = c.round_trip("DESCRIBE", &uri, &[("Accept", "application/sdp".to_string())])?;
        if let Some(b) = d.header("content-base") {
            if b.to_ascii_lowercase().starts_with("rtsp://") {
                c.base = b.to_string();
            }
        }
        let text = String::from_utf8_lossy(&d.body).into_owned();
        let track = sdp::parse(&text).video().ok_or_else(|| Error::Unsupported("the camera offers no video stream at this address".into()))?;
        if let Codec::Other(n) = &track.codec {
            return Err(Error::Unsupported(format!("{n} video is not supported")));
        }
        c.codec = track.codec.clone();
        c.payload_type = track.payload_type;
        let setup_uri = resolve(&c.base, track.control.as_deref());
        let s = c.round_trip("SETUP", &setup_uri, &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1".to_string())])?;
        let tr = s.header("transport").unwrap_or("").to_ascii_lowercase();
        if !tr.contains("tcp") {
            return Err(Error::Unsupported("the camera did not agree to send video over the same connection (TCP); UDP is not supported".into()));
        }
        c.channel = tr.split(';').find_map(|p| p.trim().strip_prefix("interleaved=")).and_then(|v| v.split('-').next()).and_then(|v| v.parse().ok()).unwrap_or(0);
        if let Some(sess) = s.header("session") {
            let (id, rest) = sess.split_once(';').unwrap_or((sess, ""));
            c.session = Some(id.trim().chars().filter(|ch| ch.is_ascii_graphic()).take(64).collect());
            if let Some(t) = rest.split(';').find_map(|p| p.trim().strip_prefix("timeout=")).and_then(|v| v.parse::<u64>().ok()) {
                c.keepalive_every = Duration::from_secs((t / 2).clamp(5, 60));
            }
        }
        let base = c.base.clone();
        c.round_trip("PLAY", &base, &[("Range", "npt=0.000-".to_string())])?;
        Ok(c)
    }

    fn send(&mut self, method: &str, uri: &str, extra: &[(&str, String)]) -> Result<u32, Error> {
        self.cseq += 1;
        let mut h: Vec<(&str, String)> = extra.to_vec();
        if let Some(s) = &self.session {
            h.push(("Session", s.clone()));
        }
        if let Some(a) = self.auth.as_mut() {
            h.push(("Authorization", a.header(method, uri)));
        }
        let bytes = request(method, uri, self.cseq, &h);
        let w = self.rd.get_mut();
        w.write_all(&bytes)?;
        w.flush()?;
        self.last_sent = Instant::now();
        Ok(self.cseq)
    }

    fn read_response(&mut self) -> Result<Response, Error> {
        // the camera may already be sending pictures (after PLAY) or its own requests: skip them until the answer
        for _ in 0..10_000 {
            if let Incoming::Response(r) = self.rd.next()? {
                return Ok(r);
            }
        }
        Err(Error::Protocol("the camera never answered".into()))
    }

    fn round_trip(&mut self, method: &str, uri: &str, extra: &[(&str, String)]) -> Result<Response, Error> {
        let mut tried_auth = false;
        loop {
            self.send(method, uri, extra)?;
            let r = self.read_response()?;
            match r.code {
                200..=299 => return Ok(r),
                401 => {
                    let stale = r.all("www-authenticate").iter().any(|h| h.to_ascii_lowercase().contains("stale=true"));
                    if tried_auth && !stale {
                        return Err(Error::Auth("the camera refused the user name or password".into()));
                    }
                    let Some((u, p)) = self.creds.clone() else { return Err(Error::Auth("the camera needs a user name and password".into())) };
                    let ch = auth::choose(&r.all("www-authenticate")).map_err(Error::Unsupported)?;
                    self.auth = Some(Auth::new(ch, &u, &p));
                    tried_auth = true;
                }
                403 => return Err(Error::Auth("the camera does not allow this account to watch".into())),
                404 => return Err(Error::Protocol("the camera has no stream at this address (404); check the path".into())),
                code => return Err(Error::Protocol(format!("the camera answered {code} {} to {method}", r.reason))),
            }
        }
    }

    pub fn used_basic_auth(&self) -> bool {
        self.auth.as_ref().is_some_and(Auth::is_basic)
    }

    /// The next picture. `Err(Timeout)` when the socket's read timeout passed with nothing complete (call again); other errors end the
    /// stream.
    pub fn next_media(&mut self) -> Result<Media, Error> {
        loop {
            if self.last_sent.elapsed() > self.keepalive_every {
                let u = self.uri.clone();
                self.send("OPTIONS", &u, &[])?;
            }
            match self.rd.next()? {
                Incoming::Interleaved(ch, data) if ch == self.channel => {
                    let Ok(p) = parse_rtp(&data) else {
                        self.bad_packets += 1;
                        continue;
                    };
                    if p.payload_type != self.payload_type {
                        continue;
                    }
                    match self.codec {
                        Codec::Mjpeg => {
                            if let Some(j) = self.mjpeg.push(&p) {
                                self.frames += 1;
                                return Ok(Media::Jpeg(j));
                            }
                        }
                        Codec::H264 { .. } => {
                            if let Some(a) = self.h264.push(&p) {
                                self.frames += 1;
                                return Ok(Media::H264(a));
                            }
                        }
                        Codec::Other(_) => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Pictures lost to gaps or bad data so far.
    pub fn dropped(&self) -> u64 {
        self.mjpeg.dropped + self.h264.dropped
    }

    /// Tell the camera we are leaving (best effort).
    pub fn teardown(&mut self) {
        let b = self.base.clone();
        let _ = self.send("TEARDOWN", &b, &[]);
    }
}

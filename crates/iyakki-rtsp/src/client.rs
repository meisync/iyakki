//! The RTSP client: OPTIONS-free `DESCRIBE`, `SETUP` (TCP, interleaved; UDP when the camera refuses TCP, see [`Client::open_with`]) and `PLAY`, then pictures. It works over any
//! `Read + Write`; the caller sets a read timeout on the socket so `next_media` returns `Err(Timeout)` now and then (the caller checks
//! its stop flag and calls again; keep-alives are sent meanwhile).

use std::io::{Read, Write};
use std::net::{IpAddr, UdpSocket};
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

/// The answer text of a camera that does not take the way of sending video that was asked for (`461 Unsupported Transport`).
pub const UNSUPPORTED_TRANSPORT: &str = "the camera does not support this way of sending video (461)";

/// Where the pictures come from when the camera sends them over UDP (RFC 2326, `RTP/AVP;unicast;client_port=`): our RTP socket, and the
/// camera's address, from which alone packets are taken.
struct UdpIn {
    rtp: UdpSocket,
    /// The odd neighbour port, held so the camera's reports do not bounce (ICMP port unreachable makes some cameras stop); never read.
    _rtcp: UdpSocket,
    peer: IpAddr,
    /// The camera's RTP port when it said (`server_port=`); `None`: any port of the camera's address.
    from_port: Option<u16>,
    buf: Vec<u8>,
}

/// UDP packets that came from somewhere else than the camera, or were too large (counted, never read).
pub const MAX_UDP_PACKET: usize = 2048;
/// The most redirects of the stream's address that are followed.
const MAX_MOVES: usize = 3;
const MOVED: &str = "the camera moved the stream";

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
    udp: Option<UdpIn>,
    /// UDP packets from another address than the camera's (dropped).
    pub foreign_packets: u64,
    /// Where the camera said the stream moved to (set while a DESCRIBE answer is a redirect).
    moved_to: Option<String>,
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
        Client::open_with(transport, url, creds, None)
    }

    /// Like [`Client::open`]. `udp_peer` is the address the control connection runs to: when it is given and the camera refuses the
    /// video over the same connection (`461`), the client asks for UDP instead, opens its own pair of ports for it and takes packets
    /// only from that address.
    pub fn open_with(transport: T, url: &Url, creds: Option<(String, String)>, udp_peer: Option<IpAddr>) -> Result<Client<T>, Error> {
        let uri = url.request_uri();
        let mut c = Client { rd: Reader::new(transport), uri: uri.clone(), base: uri.clone(), cseq: 0, session: None, auth: None, creds, codec: Codec::Mjpeg, payload_type: 26, channel: 0, mjpeg: MjpegDepay::new(), h264: H264Depay::new(), keepalive_every: Duration::from_secs(25), last_sent: Instant::now(), frames: 0, bad_packets: 0, udp: None, foreign_packets: 0, moved_to: None };
        let d = c.describe(url)?;
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
        let mut rtp_socket = None;
        let first = c.round_trip("SETUP", &setup_uri, &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1".to_string())]);
        let s = match (first, udp_peer) {
            (Err(Error::Unsupported(m)), Some(_)) if m == UNSUPPORTED_TRANSPORT => {
                let (sock, rtcp, port) = open_udp_pair()?;
                let r = c.round_trip("SETUP", &setup_uri, &[("Transport", format!("RTP/AVP;unicast;client_port={}-{}", port, port + 1))])?;
                rtp_socket = Some((sock, rtcp));
                r
            }
            (r, _) => r?,
        };
        let tr = s.header("transport").unwrap_or("").to_ascii_lowercase();
        if let (Some((sock, rtcp)), Some(peer)) = (rtp_socket, udp_peer) {
            if tr.contains("tcp") {
                // the camera chose the same connection after all: the sockets are not needed
                c.channel = tr.split(';').find_map(|p| p.trim().strip_prefix("interleaved=")).and_then(|v| v.split('-').next()).and_then(|v| v.parse().ok()).unwrap_or(0);
            } else {
                let ports = tr.split(';').find_map(|p| p.trim().strip_prefix("server_port=")).map(|v| v.to_string());
                let from_port = ports.as_deref().and_then(|v| v.split('-').next()).and_then(|v| v.parse::<u16>().ok()).filter(|p| *p != 0);
                let rtcp_port = ports.as_deref().and_then(|v| v.split('-').nth(1)).and_then(|v| v.parse::<u16>().ok()).filter(|p| *p != 0);
                // a camera behind a firewall sends nothing until it has heard from us: one empty receiver report to its RTCP port
                if let Some(p) = rtcp_port.or(from_port.map(|p| p.wrapping_add(1))) {
                    let _ = rtcp.send_to(&[0x80, 201, 0, 1, 0, 0, 0, 0], (peer, p));
                }
                let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
                c.udp = Some(UdpIn { rtp: sock, _rtcp: rtcp, peer, from_port, buf: vec![0; MAX_UDP_PACKET] });
            }
        } else {
            if !tr.contains("tcp") {
                return Err(Error::Unsupported("the camera did not agree to send video over the same connection (TCP); UDP is not supported here".into()));
            }
            c.channel = tr.split(';').find_map(|p| p.trim().strip_prefix("interleaved=")).and_then(|v| v.split('-').next()).and_then(|v| v.parse().ok()).unwrap_or(0);
        }
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

    /// DESCRIBE, following a redirect to another path on the same camera (a few steps at most). A different machine or port, or an
    /// address that is not RTSP, is refused with a sentence: this connection runs to one camera only.
    fn describe(&mut self, url: &Url) -> Result<Response, Error> {
        let mut uri = self.uri.clone();
        for _ in 0..=MAX_MOVES {
            match self.round_trip("DESCRIBE", &uri, &[("Accept", "application/sdp".to_string())]) {
                Err(Error::Protocol(m)) if m == MOVED => {
                    let loc = self.moved_to.take().ok_or_else(|| Error::Protocol("the camera moved the stream but did not say where".into()))?;
                    if loc.chars().any(|c| c.is_control() || c == ' ') {
                        return Err(Error::Protocol("the camera moved the stream to an address that cannot be used".into()));
                    }
                    let text = if loc.starts_with('/') && !loc.starts_with("//") { format!("rtsp://{}:{}{}", url.host, url.port, loc) } else { loc };
                    let to = crate::url::parse(&text).map_err(|_| Error::Protocol("the camera moved the stream to an address that cannot be used".into()))?;
                    if to.scheme != crate::url::Scheme::Rtsp || !to.host.eq_ignore_ascii_case(&url.host) || to.port != url.port {
                        return Err(Error::Protocol("the camera moved the stream to another machine or port; the address was not followed (enter the new address instead)".into()));
                    }
                    uri = to.request_uri();
                    self.uri = uri.clone();
                    self.base = uri.clone();
                }
                other => return other,
            }
        }
        Err(Error::Protocol("the camera kept moving the stream; stopped after a few steps".into()))
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
                461 => return Err(Error::Unsupported(UNSUPPORTED_TRANSPORT.into())),
                301 | 302 | 303 | 307 | 308 if method == "DESCRIBE" => {
                    self.moved_to = r.header("location").map(str::trim).filter(|l| !l.is_empty()).map(|l| l.chars().take(1100).collect());
                    return Err(Error::Protocol(MOVED.into()));
                }
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
                if self.udp.is_some() {
                    // the answer arrives on the control connection, which nothing else reads in this mode: take it so it cannot pile up
                    match self.rd.next() {
                        Ok(_) | Err(Error::Timeout) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            if self.udp.is_some() {
                let data = {
                    let u = self.udp.as_mut().expect("checked");
                    match u.rtp.recv_from(&mut u.buf) {
                        Ok((n, from)) => {
                            if from.ip() != u.peer || u.from_port.is_some_and(|p| p != from.port()) {
                                self.foreign_packets += 1;
                                continue;
                            }
                            u.buf[..n].to_vec()
                        }
                        Err(e) => return Err(Error::from(e)),
                    }
                };
                if let Some(m) = self.on_rtp(&data) {
                    return Ok(m);
                }
                continue;
            }
            match self.rd.next()? {
                Incoming::Interleaved(ch, data) if ch == self.channel => {
                    if let Some(m) = self.on_rtp(&data) {
                        return Ok(m);
                    }
                }
                _ => {}
            }
        }
    }

    /// One RTP packet, from either way of arriving: a whole picture when it completes one.
    fn on_rtp(&mut self, data: &[u8]) -> Option<Media> {
        let Ok(p) = parse_rtp(data) else {
            self.bad_packets += 1;
            return None;
        };
        if p.payload_type != self.payload_type {
            return None;
        }
        match self.codec {
            Codec::Mjpeg => {
                let j = self.mjpeg.push(&p)?;
                self.frames += 1;
                Some(Media::Jpeg(j))
            }
            Codec::H264 { .. } => {
                let a = self.h264.push(&p)?;
                self.frames += 1;
                Some(Media::H264(a))
            }
            Codec::Other(_) => None,
        }
    }

    /// Pictures lost to gaps or bad data so far.
    pub fn dropped(&self) -> u64 {
        self.mjpeg.dropped + self.h264.dropped
    }

    /// Tell the camera we are leaving and read its answer: the status code it gave (200 when it took the TEARDOWN). Pictures that arrive
    /// before the answer are skipped.
    pub fn teardown_checked(&mut self) -> Result<u16, Error> {
        let b = self.base.clone();
        self.send("TEARDOWN", &b, &[])?;
        Ok(self.read_response()?.code)
    }

    /// Tell the camera we are leaving (best effort).
    pub fn teardown(&mut self) {
        let b = self.base.clone();
        let _ = self.send("TEARDOWN", &b, &[]);
    }
}

/// Two sockets on neighbouring ports, the first even (RTP) and the second the RTCP port next to it.
fn open_udp_pair() -> Result<(UdpSocket, UdpSocket, u16), Error> {
    for _ in 0..40 {
        let rtp = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| Error::Io(e.to_string()))?;
        let port = rtp.local_addr().map_err(|e| Error::Io(e.to_string()))?.port();
        if port % 2 == 0 && port < u16::MAX {
            if let Ok(rtcp) = UdpSocket::bind(("0.0.0.0", port + 1)) {
                return Ok((rtp, rtcp, port));
            }
        }
    }
    Err(Error::Io("no free pair of ports for UDP".into()))
}

//! An RTSP client written for IYAKKI's Cameras tile (F-CAM-02), with no dependencies.
//!
//! * [`url`]: `rtsp://` and `http://` addresses; credentials are split off and can be stripped for display.
//! * [`msg`]: RTSP messages and interleaved `$` frames from a byte stream, with hard limits (hostile servers).
//! * [`sdp`]: the parts of an SDP description a camera client needs.
//! * [`auth`]: Basic and Digest (MD5) sign-in; [`hash`] has the MD5 and Base64 they need.
//! * [`rtp`]: RTP packets, Motion-JPEG (RFC 2435, complete) and H.264 (RFC 6184 single NAL, STAP-A, FU-A) depacketisers.
//! * [`client`]: DESCRIBE, SETUP (TCP interleaved), PLAY and the media loop over any `Read + Write`.
//! * [`httpmjpeg`]: `multipart/x-mixed-replace` Motion-JPEG and single-picture URLs over plain HTTP.
//! * [`packetize`]: the other direction (a JPEG into RFC 2435 packets), for simulators and tests.
//!
//! Decoding H.264 needs a codec, which this crate does not have: it hands out whole access units (NAL units, Annex-B bytes) and the
//! caller says "needs the video add-on". Motion-JPEG frames come out as complete JPEG files (the standard Huffman tables are written
//! in, as RFC 2435 requires receivers to assume them).

pub mod auth;
pub mod client;
pub mod hash;
pub mod httpmjpeg;
pub mod jpegtab;
pub mod msg;
pub mod packetize;
pub mod rtp;
pub mod sdp;
pub mod url;

/// Everything that can go wrong, in words a person can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The connection failed or broke.
    Io(String),
    /// Nothing arrived in time (the transport's read timeout).
    Timeout,
    /// The camera said something that is not valid RTSP, or answered an error.
    Protocol(String),
    /// The user name or password was refused.
    Auth(String),
    /// A valid but unsupported feature (an encoding, a transport, https).
    Unsupported(String),
    /// The other end closed the stream.
    Closed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(m) => write!(f, "connection problem: {m}"),
            Error::Timeout => write!(f, "no data in time"),
            Error::Protocol(m) => write!(f, "{m}"),
            Error::Auth(m) => write!(f, "{m}"),
            Error::Unsupported(m) => write!(f, "{m}"),
            Error::Closed => write!(f, "the camera closed the stream"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::Timeout,
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe => Error::Closed,
            _ => Error::Io(e.to_string()),
        }
    }
}

pub mod fakeserver;

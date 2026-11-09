//! RTSP messages and interleaved frames from a byte stream. Everything is bounded: a hostile server cannot make the reader allocate
//! more than [`MAX_HEAD`] + [`MAX_BODY`] bytes for a message or loop forever on junk.

use crate::Error;
use std::io::Read;

pub const MAX_HEAD: usize = 16 * 1024;
pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_HEADERS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Response {
    pub code: u16,
    pub reason: String,
    /// Header names are lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub fn all(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    Response(Response),
    /// `$` + channel + length + data: RTP on even channels, RTCP on odd ones.
    Interleaved(u8, Vec<u8>),
    /// A request the server sent us (ANNOUNCE, a keep-alive probe ...). Read and ignored.
    ServerRequest(String),
}

/// A buffered reader that keeps what it has read when the transport times out, so a call can simply be repeated.
pub struct Reader<T: Read> {
    t: T,
    buf: Vec<u8>,
}

impl<T: Read> Reader<T> {
    pub fn new(t: T) -> Reader<T> {
        Reader { t, buf: Vec::new() }
    }
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.t
    }
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    fn fill(&mut self) -> Result<(), Error> {
        let mut tmp = [0u8; 8192];
        let n = self.t.read(&mut tmp)?;
        if n == 0 {
            return Err(Error::Closed);
        }
        self.buf.extend_from_slice(&tmp[..n]);
        Ok(())
    }

    fn need(&mut self, n: usize) -> Result<(), Error> {
        while self.buf.len() < n {
            self.fill()?;
        }
        Ok(())
    }

    /// The next message. `Err(Timeout)` leaves everything read so far in place.
    pub fn next(&mut self) -> Result<Incoming, Error> {
        self.need(1)?;
        if self.buf[0] == b'$' {
            self.need(4)?;
            let len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
            self.need(4 + len)?;
            let ch = self.buf[1];
            let data = self.buf[4..4 + len].to_vec();
            self.buf.drain(..4 + len);
            return Ok(Incoming::Interleaved(ch, data));
        }
        // a header block ends with a blank line
        let end = loop {
            if let Some(i) = find(&self.buf, b"\r\n\r\n").map(|i| (i, 4)).or_else(|| find(&self.buf, b"\n\n").map(|i| (i, 2))) {
                break i;
            }
            if self.buf.len() > MAX_HEAD {
                return Err(Error::Protocol("the camera sent a message header that is far too long".into()));
            }
            // junk that cannot start a message: give up rather than wait for a blank line that never comes
            if !self.buf[0].is_ascii_alphabetic() {
                return Err(Error::Protocol("the camera sent data that is not RTSP".into()));
            }
            self.fill()?;
        };
        let (pos, sep) = end;
        if pos + sep > MAX_HEAD {
            return Err(Error::Protocol("the camera sent a message header that is far too long".into()));
        }
        let head = String::from_utf8_lossy(&self.buf[..pos]).into_owned();
        let mut lines = head.split('\n').map(|l| l.trim_end_matches('\r'));
        let first = lines.next().unwrap_or("").to_string();
        let mut headers: Vec<(String, String)> = Vec::new();
        for l in lines {
            if headers.len() >= MAX_HEADERS {
                return Err(Error::Protocol("the camera sent too many header lines".into()));
            }
            if l.starts_with(' ') || l.starts_with('\t') {
                if let Some(last) = headers.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(l.trim());
                }
                continue;
            }
            let Some((k, v)) = l.split_once(':') else { continue };
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
        let clen = match headers.iter().find(|(k, _)| k == "content-length") {
            Some((_, v)) => v.trim().parse::<usize>().map_err(|_| Error::Protocol("the camera sent a bad Content-Length".into()))?,
            None => 0,
        };
        if clen > MAX_BODY {
            return Err(Error::Protocol("the camera sent a message body that is far too long".into()));
        }
        self.need(pos + sep + clen)?;
        let body = self.buf[pos + sep..pos + sep + clen].to_vec();
        self.buf.drain(..pos + sep + clen);
        if let Some(rest) = first.strip_prefix("RTSP/") {
            let mut it = rest.splitn(3, ' ');
            let _ver = it.next();
            let code = it.next().and_then(|c| c.parse::<u16>().ok()).filter(|c| (100..600).contains(c)).ok_or_else(|| Error::Protocol("the camera sent a status line that is not RTSP".into()))?;
            let reason: String = it.next().unwrap_or("").chars().filter(|c| !c.is_control()).take(100).collect();
            return Ok(Incoming::Response(Response { code, reason, headers, body }));
        }
        Ok(Incoming::ServerRequest(first.chars().filter(|c| !c.is_control()).take(100).collect()))
    }
}

pub(crate) fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    h.windows(n.len()).position(|w| w == n)
}

/// A request, ready to send.
pub fn request(method: &str, uri: &str, cseq: u32, headers: &[(&str, String)]) -> Vec<u8> {
    let mut s = format!("{method} {uri} RTSP/1.0\r\nCSeq: {cseq}\r\nUser-Agent: iyakki\r\n");
    for (k, v) in headers {
        // a header value never carries a line break (a server value copied back could try)
        let v: String = v.chars().filter(|c| *c != '\r' && *c != '\n').collect();
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str("\r\n");
    s.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A reader that gives one byte per call, then a timeout, then the rest: a message arriving in pieces.
    struct Drip {
        data: Vec<u8>,
        pos: usize,
        stall_at: usize,
        stalled: bool,
    }
    impl Read for Drip {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.pos == self.stall_at && !self.stalled {
                self.stalled = true;
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            b[0] = self.data[self.pos];
            self.pos += 1;
            Ok(1)
        }
    }

    #[test]
    fn a_response_with_a_body_and_an_interleaved_frame_are_read_in_turn() {
        let mut d = b"RTSP/1.0 200 OK\r\nCSeq: 2\r\nContent-Length: 5\r\nSession: ABC;timeout=60\r\n\r\nhello".to_vec();
        d.extend_from_slice(&[b'$', 0, 0, 3, 1, 2, 3]);
        let mut r = Reader::new(Cursor::new(d));
        match r.next().unwrap() {
            Incoming::Response(x) => {
                assert_eq!((x.code, x.reason.as_str(), x.body.as_slice()), (200, "OK", &b"hello"[..]));
                assert_eq!(x.header("session"), Some("ABC;timeout=60"));
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(r.next().unwrap(), Incoming::Interleaved(0, vec![1, 2, 3]));
        assert_eq!(r.next(), Err(Error::Closed));
    }

    #[test]
    fn a_timeout_in_the_middle_loses_nothing() {
        let d = b"RTSP/1.0 401 Unauthorized\r\nCSeq: 1\r\n\r\n".to_vec();
        let mut r = Reader::new(Drip { data: d, pos: 0, stall_at: 20, stalled: false });
        assert_eq!(r.next(), Err(Error::Timeout));
        assert!(r.buffered() > 0);
        match r.next().unwrap() {
            Incoming::Response(x) => assert_eq!(x.code, 401),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn requests_from_the_server_are_read_and_marked() {
        let mut r = Reader::new(Cursor::new(b"ANNOUNCE rtsp://x/y RTSP/1.0\r\nCSeq: 9\r\nContent-Length: 2\r\n\r\nhi".to_vec()));
        assert!(matches!(r.next().unwrap(), Incoming::ServerRequest(s) if s.starts_with("ANNOUNCE")));
    }

    #[test]
    fn hostile_input_is_refused_without_growing_without_bound() {
        let mut r = Reader::new(Cursor::new(vec![b'A'; 100_000]));
        assert!(matches!(r.next(), Err(Error::Protocol(_))), "header never ends");
        let mut r = Reader::new(Cursor::new(b"RTSP/1.0 200 OK\r\nContent-Length: 99999999\r\n\r\n".to_vec()));
        assert!(matches!(r.next(), Err(Error::Protocol(_))), "huge body");
        let mut r = Reader::new(Cursor::new(b"RTSP/1.0 200 OK\r\nContent-Length: -1\r\n\r\n".to_vec()));
        assert!(matches!(r.next(), Err(Error::Protocol(_))));
        let mut r = Reader::new(Cursor::new(b"RTSP/1.0 999 Odd\r\n\r\n".to_vec()));
        assert!(matches!(r.next(), Err(Error::Protocol(_))), "status out of range");
        let mut r = Reader::new(Cursor::new(b"RTSP/1.0 abc\r\n\r\n".to_vec()));
        assert!(matches!(r.next(), Err(Error::Protocol(_))));
        let mut r = Reader::new(Cursor::new(vec![0u8, 1, 2, 3]));
        assert!(matches!(r.next(), Err(Error::Protocol(_))), "binary junk");
        let many: String = (0..300).map(|i| format!("X-{i}: v\r\n")).collect();
        let mut r = Reader::new(Cursor::new(format!("RTSP/1.0 200 OK\r\n{many}\r\n").into_bytes()));
        assert!(matches!(r.next(), Err(Error::Protocol(_))), "too many headers");
        // a frame that stops short is a closed stream, not a hang
        let mut r = Reader::new(Cursor::new(vec![b'$', 0, 0, 200, 1, 2]));
        assert_eq!(r.next(), Err(Error::Closed));
    }

    #[test]
    fn header_values_cannot_inject_lines_into_a_request() {
        let r = String::from_utf8(request("DESCRIBE", "rtsp://h/x", 3, &[("Accept", "a\r\nX-Evil: 1".into())])).unwrap();
        assert!(r.starts_with("DESCRIBE rtsp://h/x RTSP/1.0\r\nCSeq: 3\r\n"));
        assert!(!r.contains("\r\nX-Evil"));
        assert!(r.ends_with("\r\n\r\n"));
    }
}

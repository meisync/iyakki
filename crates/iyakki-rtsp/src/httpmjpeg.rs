//! Motion-JPEG over plain HTTP: `multipart/x-mixed-replace` streams (most cameras' `/video.mjpg`) and single-picture addresses
//! (`/snapshot.jpg`, fetched again for every picture). Basic and Digest sign-in. No TLS and no chunked encoding.

use std::io::{Read, Write};

use crate::auth::{self, Auth};
use crate::msg::find;
use crate::url::Url;
use crate::Error;

pub const MAX_HEAD: usize = 16 * 1024;
pub const MAX_PICTURE: usize = 8 * 1024 * 1024;

type Connect<T> = Box<dyn FnMut() -> Result<T, Error> + Send>;

enum Mode {
    Multipart { boundary: Vec<u8> },
    /// A single-picture address; `first` is the body length announced on the connection `open` already made (`None`: read to the end).
    Single { first: Option<Option<usize>> },
}

pub struct HttpMjpeg<T: Read + Write> {
    connect: Connect<T>,
    url: Url,
    auth: Option<Auth>,
    t: T,
    buf: Vec<u8>,
    mode: Mode,
    /// Basic sign-in was needed (the password crossed the network readable).
    pub basic: bool,
    dechunk: Option<Dechunk>,
}

/// Undoes `Transfer-Encoding: chunked` (some servers, ffmpeg's among them, send a Motion-JPEG stream that way).
#[derive(Default)]
struct Dechunk {
    raw: Vec<u8>,
    left: usize,
    need_crlf: bool,
}

impl Dechunk {
    /// Move what is complete in `raw` into `out`.
    fn feed(&mut self, out: &mut Vec<u8>) -> Result<(), Error> {
        loop {
            if self.left > 0 {
                let n = self.left.min(self.raw.len());
                if n == 0 {
                    return Ok(());
                }
                out.extend_from_slice(&self.raw[..n]);
                self.raw.drain(..n);
                self.left -= n;
                if self.left == 0 {
                    self.need_crlf = true;
                }
                continue;
            }
            if self.need_crlf {
                if self.raw.len() < 2 {
                    return Ok(());
                }
                self.raw.drain(..2);
                self.need_crlf = false;
                continue;
            }
            let Some(i) = find(&self.raw, b"\r\n") else {
                if self.raw.len() > 64 {
                    return Err(Error::Protocol("the camera sent a broken chunk size".into()));
                }
                return Ok(());
            };
            let line = String::from_utf8_lossy(&self.raw[..i]).into_owned();
            let n = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| Error::Protocol("the camera sent a broken chunk size".into()))?;
            self.raw.drain(..i + 2);
            if n == 0 {
                return Err(Error::Closed);
            }
            if n > MAX_PICTURE {
                return Err(Error::Protocol("the camera sent a chunk that is far too large".into()));
            }
            self.left = n;
        }
    }
}

struct Head {
    code: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn get(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|h| h.0 == k).map(|h| h.1.as_str())
    }
}

fn fill<T: Read>(t: &mut T, buf: &mut Vec<u8>) -> Result<(), Error> {
    let mut tmp = [0u8; 16384];
    let n = t.read(&mut tmp)?;
    if n == 0 {
        return Err(Error::Closed);
    }
    buf.extend_from_slice(&tmp[..n]);
    Ok(())
}

fn read_head<T: Read>(t: &mut T, buf: &mut Vec<u8>) -> Result<Head, Error> {
    let end = loop {
        if let Some(i) = find(buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(Error::Protocol("the camera sent a header that is far too long".into()));
        }
        fill(t, buf)?;
    };
    let text = String::from_utf8_lossy(&buf[..end]).into_owned();
    buf.drain(..end + 4);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let code = first.strip_prefix("HTTP/").and_then(|r| r.split(' ').nth(1)).and_then(|c| c.parse::<u16>().ok()).ok_or_else(|| Error::Protocol("the camera did not answer like a web server".into()))?;
    let headers = lines.take(100).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Ok(Head { code, headers })
}

fn get_request(url: &Url, auth: Option<&mut Auth>) -> Vec<u8> {
    let mut s = format!("GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: iyakki\r\nAccept: multipart/x-mixed-replace, image/jpeg\r\nConnection: close\r\n", url.path, if url.port == 80 { url.host.clone() } else { format!("{}:{}", url.host, url.port) });
    if let Some(a) = auth {
        s.push_str(&format!("Authorization: {}\r\n", a.header("GET", &url.path)));
    }
    s.push_str("\r\n");
    s.into_bytes()
}

fn content_length(h: &Head) -> Result<Option<usize>, Error> {
    match h.get("content-length") {
        None => Ok(None),
        Some(v) => v.trim().parse::<usize>().ok().filter(|n| *n <= MAX_PICTURE).map(Some).ok_or_else(|| Error::Protocol("the camera announced a picture that is far too large".into())),
    }
}

/// Read a body of `len` bytes, or to the end of the connection, bounded.
fn read_body<T: Read>(t: &mut T, buf: &mut Vec<u8>, len: Option<usize>) -> Result<Vec<u8>, Error> {
    match len {
        Some(n) => {
            while buf.len() < n {
                fill(t, buf)?;
            }
            Ok(buf.drain(..n).collect())
        }
        None => {
            loop {
                match fill(t, buf) {
                    Ok(()) if buf.len() > MAX_PICTURE => return Err(Error::Protocol("the camera sent a picture that is far too large".into())),
                    Ok(()) => {}
                    Err(Error::Closed) => break,
                    Err(e) => return Err(e),
                }
            }
            Ok(std::mem::take(buf))
        }
    }
}

fn is_jpeg(b: &[u8]) -> bool {
    b.len() > 4 && b[0] == 0xFF && b[1] == 0xD8
}

impl<T: Read + Write> HttpMjpeg<T> {
    pub fn open(mut connect: Connect<T>, url: &Url, creds: Option<(String, String)>) -> Result<HttpMjpeg<T>, Error> {
        let mut auth: Option<Auth> = None;
        for attempt in 0..2 {
            let mut t = connect()?;
            t.write_all(&get_request(url, auth.as_mut()))?;
            let mut buf = Vec::new();
            let head = read_head(&mut t, &mut buf)?;
            match head.code {
                200 => {
                    let ctype = head.get("content-type").unwrap_or("").to_ascii_lowercase();
                    let mode = if ctype.starts_with("multipart/") {
                        let b = ctype.split(';').find_map(|p| p.trim().strip_prefix("boundary=")).map(|b| b.trim_matches('"').trim_start_matches("--").to_string()).filter(|b| !b.is_empty() && b.len() < 100).ok_or_else(|| Error::Protocol("the camera's stream has no boundary".into()))?;
                        Mode::Multipart { boundary: format!("--{b}").into_bytes() }
                    } else if ctype.starts_with("image/jp") {
                        if head.get("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
                            return Err(Error::Unsupported("this camera sends single pictures in chunks, which is not supported".into()));
                        }
                        Mode::Single { first: Some(content_length(&head)?) }
                    } else if ctype.is_empty() {
                        return Err(Error::Protocol("the camera did not say what kind of picture it sends".into()));
                    } else {
                        return Err(Error::Unsupported(format!("this address sends {}, not Motion-JPEG", ctype.chars().filter(|c| c.is_ascii_graphic()).take(40).collect::<String>())));
                    };
                    let basic = auth.as_ref().is_some_and(Auth::is_basic);
                    let chunked = matches!(mode, Mode::Multipart { .. }) && head.get("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
                    let mut dechunk = None;
                    if chunked {
                        let mut d = Dechunk { raw: std::mem::take(&mut buf), ..Default::default() };
                        d.feed(&mut buf)?;
                        dechunk = Some(d);
                    }
                    return Ok(HttpMjpeg { connect, url: url.clone(), auth, t, buf, mode, basic, dechunk });
                }
                401 if attempt == 0 => {
                    let Some((u, p)) = creds.clone() else { return Err(Error::Auth("the camera needs a user name and password".into())) };
                    let hs: Vec<&str> = head.headers.iter().filter(|h| h.0 == "www-authenticate").map(|h| h.1.as_str()).collect();
                    auth = Some(Auth::new(auth::choose(&hs).map_err(Error::Unsupported)?, &u, &p));
                }
                401 => return Err(Error::Auth("the camera refused the user name or password".into())),
                404 => return Err(Error::Protocol("the camera has no picture at this address (404); check the path".into())),
                c => return Err(Error::Protocol(format!("the camera answered {c}"))),
            }
        }
        Err(Error::Auth("the camera refused the user name or password".into()))
    }

    /// Read more of a multipart stream into `buf`, undoing chunked encoding when the server uses it.
    fn more(&mut self) -> Result<(), Error> {
        match &mut self.dechunk {
            None => fill(&mut self.t, &mut self.buf),
            Some(d) => {
                let mut tmp = [0u8; 16384];
                let n = self.t.read(&mut tmp)?;
                if n == 0 {
                    return Err(Error::Closed);
                }
                d.raw.extend_from_slice(&tmp[..n]);
                d.feed(&mut self.buf)
            }
        }
    }

    /// Whether every picture is a new request (a snapshot address) rather than one long stream.
    pub fn is_snapshot(&self) -> bool {
        matches!(self.mode, Mode::Single { .. })
    }

    /// The next picture as a JPEG file. `Err(Timeout)` keeps what was read.
    pub fn next_jpeg(&mut self) -> Result<Vec<u8>, Error> {
        match &mut self.mode {
            Mode::Single { first } => {
                let len = match first.take() {
                    Some(l) => l,
                    None => {
                        self.t = (self.connect)()?;
                        self.buf.clear();
                        let req = get_request(&self.url, self.auth.as_mut());
                        self.t.write_all(&req)?;
                        let head = read_head(&mut self.t, &mut self.buf)?;
                        if head.code != 200 {
                            return Err(if head.code == 401 { Error::Auth("the camera refused the user name or password".into()) } else { Error::Protocol(format!("the camera answered {}", head.code)) });
                        }
                        content_length(&head)?
                    }
                };
                let body = read_body(&mut self.t, &mut self.buf, len)?;
                if is_jpeg(&body) { Ok(body) } else { Err(Error::Protocol("the camera's answer is not a JPEG picture".into())) }
            }
            Mode::Multipart { boundary } => {
                let boundary = boundary.clone();
                loop {
                    // up to and including a boundary line
                    let bpos = loop {
                        if let Some(i) = find(&self.buf, &boundary) {
                            break i;
                        }
                        if self.buf.len() > MAX_PICTURE {
                            return Err(Error::Protocol("no picture boundary in the camera's stream".into()));
                        }
                        self.more()?;
                    };
                    // the part header: after the boundary line, up to a blank line
                    let hstart = bpos + boundary.len();
                    let hend = loop {
                        if let Some(i) = find(&self.buf[hstart..], b"\r\n\r\n") {
                            break hstart + i;
                        }
                        if self.buf.len() - hstart > MAX_HEAD {
                            return Err(Error::Protocol("a picture header in the camera's stream is far too long".into()));
                        }
                        self.more()?;
                    };
                    let hdr = String::from_utf8_lossy(&self.buf[hstart..hend]).to_ascii_lowercase();
                    if hdr.trim_start().starts_with("--") {
                        // the closing boundary
                        return Err(Error::Closed);
                    }
                    let clen = hdr.lines().find_map(|l| l.trim().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()));
                    let body_start = hend + 4;
                    let body = match clen {
                        Some(n) if n > MAX_PICTURE => return Err(Error::Protocol("the camera announced a picture that is far too large".into())),
                        Some(n) => {
                            while self.buf.len() < body_start + n {
                                self.more()?;
                            }
                            let b = self.buf[body_start..body_start + n].to_vec();
                            self.buf.drain(..body_start + n);
                            b
                        }
                        None => {
                            // no length: the picture ends where the next boundary begins (after a line break)
                            let end = loop {
                                if let Some(i) = find(&self.buf[body_start..], &boundary) {
                                    break body_start + i;
                                }
                                if self.buf.len() - body_start > MAX_PICTURE {
                                    return Err(Error::Protocol("a picture in the camera's stream is far too large".into()));
                                }
                                self.more()?;
                            };
                            let mut stop = end;
                            if self.buf[body_start..stop].ends_with(b"\r\n") {
                                stop -= 2;
                            }
                            let b = self.buf[body_start..stop].to_vec();
                            self.buf.drain(..end);
                            b
                        }
                    };
                    if is_jpeg(&body) {
                        return Ok(body);
                    }
                }
            }
        }
    }
}

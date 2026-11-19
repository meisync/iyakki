//! Streaming downloads (F-BROW-94): a file is fetched in ranged pieces and written to disk as it arrives, so a large file does not
//! have to fit in memory, the progress is known after every piece, and Esc stops it between pieces (the part written so far is deleted).
//!
//! The transport answers whole bodies, so the pieces are `Range: bytes=a-b` requests, a megabyte each. A server that ignores `Range`
//! (answers 200 with everything) is handled as before: the body is written in one go, and a file larger than the transport's own
//! limit is refused in plain words. Nothing is written under the final name until the file is complete.

use iyakki_web::fetch::{Fetch, Request, Response};
use iyakki_web::url::Url;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Bytes asked for at a time.
pub const PIECE: u64 = 1 << 20;
/// Larger files are refused (the disk is shared, and a runaway server must not fill it).
pub const MAX_FILE: u64 = 4 << 30;
/// Leave at least this much room on the drive.
pub const KEEP_FREE: u64 = 64 << 20;

/// How far a download is, read by the screen while another thread writes.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    /// 0 while the size is not known.
    pub total: AtomicU64,
}

impl Progress {
    pub fn percent(&self) -> Option<u8> {
        let t = self.total.load(Ordering::Relaxed);
        (t > 0).then(|| ((self.done.load(Ordering::Relaxed) as u128 * 100 / t as u128).min(100)) as u8)
    }

    /// `[#####-----] 50%  3.1 MB of 6.2 MB`, or `3.1 MB` while the size is unknown.
    pub fn line(&self) -> String {
        let d = self.done.load(Ordering::Relaxed);
        let t = self.total.load(Ordering::Relaxed);
        match self.percent() {
            Some(p) => {
                let filled = (p as usize * 10) / 100;
                format!("[{}{}] {p}%  {} of {}", "#".repeat(filled), "-".repeat(10 - filled), human(d), human(t))
            }
            None => human(d),
        }
    }
}

pub fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else if b >= 1 << 10 {
        format!("{:.0} KB", b as f64 / 1024.0)
    } else {
        format!("{b} bytes")
    }
}

/// `bytes 0-1048575/12345678` or `bytes 0-9/*` → (first, last, total if known).
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let (a, b) = range.trim().split_once('-')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse::<u64>().ok()?),
    };
    let (a, b) = (a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?);
    (a <= b).then_some((a, b, total))
}

fn header<'a>(r: &'a Response, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

fn ask(web: &dyn Fetch, url: &Url, from: u64, to: u64, referer: &str) -> Result<Response, String> {
    let mut req = Request::get(url);
    req.range = format!("bytes={from}-{to}");
    req.referer = referer.to_string();
    web.send(&req)
}

/// Fetch `url` into `dir`, never over an older file. Returns the final name and the size. `cancel` is looked at between pieces.
/// `free` is the room left on the drive when it is known (`None`: nothing is refused for it).
pub fn download(web: &dyn Fetch, url: &Url, dir: &Path, name: &str, cancel: &AtomicBool, progress: &Progress, free: Option<u64>) -> Result<(String, u64), String> {
    let part = dir.join(format!(".{name}.part"));
    let _ = std::fs::remove_file(&part);
    let result = pieces(web, url, &part, cancel, progress, free);
    let written = match result {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    // never overwrite: name (1).ext, name (2).ext …
    let (stem, ext) = match name.rsplit_once('.') {
        Some((a, b)) if !a.is_empty() => (a.to_string(), format!(".{b}")),
        _ => (name.to_string(), String::new()),
    };
    let mut final_name = name.to_string();
    let mut n = 1;
    while dir.join(&final_name).exists() && n < 1000 {
        final_name = format!("{stem} ({n}){ext}");
        n += 1;
    }
    std::fs::rename(&part, dir.join(&final_name)).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        e.to_string()
    })?;
    Ok((final_name, written))
}

fn pieces(web: &dyn Fetch, url: &Url, part: &Path, cancel: &AtomicBool, progress: &Progress, free: Option<u64>) -> Result<u64, String> {
    let referer = String::new();
    let mut file = std::fs::File::create(part).map_err(|e| e.to_string())?;
    let mut offset: u64 = 0;
    let mut total: Option<u64> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let resp = ask(web, url, offset, offset + PIECE - 1, &referer)?;
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match resp.status {
            206 => {
                let cr = header(&resp, "content-range").and_then(parse_content_range).ok_or("the server's answer to a partial request was not understood")?;
                if cr.0 != offset || cr.1 < cr.0 || (cr.1 - cr.0 + 1) != resp.body.len() as u64 {
                    return Err("the server sent the wrong part of the file".into());
                }
                if resp.body.len() as u64 > PIECE {
                    return Err("the server sent more than was asked for".into());
                }
                if let Some(t) = cr.2 {
                    total = Some(t);
                }
            }
            416 if offset > 0 || total == Some(0) => break, // past the end: finished
            416 => break,                                   // an empty file
            200 => {
                // the server ignores ranges: everything came in one answer
                if offset != 0 {
                    return Err("the server changed its answer in the middle of the download".into());
                }
                if resp.content_length.is_some_and(|n| n > resp.body.len() as u64) {
                    return Err(format!("too large to save here (over {}) from a server that cannot send it in parts", human(resp.body.len() as u64)));
                }
                if let Some(f) = free {
                    if (resp.body.len() as u64).saturating_add(KEEP_FREE) > f {
                        return Err("there is not enough room on the drive".into());
                    }
                }
                progress.total.store(resp.body.len() as u64, Ordering::Relaxed);
                file.write_all(&resp.body).map_err(|e| e.to_string())?;
                progress.done.store(resp.body.len() as u64, Ordering::Relaxed);
                file.flush().map_err(|e| e.to_string())?;
                return Ok(resp.body.len() as u64);
            }
            s if s >= 400 => return Err(format!("the server answered {s}")),
            s => return Err(format!("the server answered {s}, which a download cannot use")),
        }
        if let Some(t) = total {
            if t > MAX_FILE {
                return Err(format!("too large ({}); files over {} are not saved here", human(t), human(MAX_FILE)));
            }
            if let Some(f) = free {
                if t.saturating_add(KEEP_FREE) > f {
                    return Err(format!("not enough room on the drive for {}", human(t)));
                }
            }
            progress.total.store(t, Ordering::Relaxed);
        }
        if resp.body.is_empty() {
            return Err("the server stopped sending before the file was complete".into());
        }
        file.write_all(&resp.body).map_err(|e| e.to_string())?;
        offset += resp.body.len() as u64;
        progress.done.store(offset, Ordering::Relaxed);
        if offset > MAX_FILE {
            return Err(format!("files over {} are not saved here", human(MAX_FILE)));
        }
        match total {
            Some(t) if offset >= t => break,
            None if (resp.body.len() as u64) < PIECE => break, // a short piece with no announced size is the end
            _ => {}
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    if total.is_some_and(|t| t != offset) {
        return Err("the file is incomplete".into());
    }
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A server that serves ranges (or not), can fail on the n-th request and records what was asked.
    struct Server {
        body: Vec<u8>,
        ranges: bool,
        lie_about_total: Option<u64>,
        fail_on: Option<usize>,
        asked: Mutex<Vec<String>>,
        on_request: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    }

    impl Server {
        fn new(len: usize, ranges: bool) -> Server {
            Server { body: (0..len).map(|i| (i % 251) as u8).collect(), ranges, lie_about_total: None, fail_on: None, asked: Mutex::new(vec![]), on_request: None }
        }
    }

    impl Fetch for Server {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.send(&Request::get(url))
        }
        fn send(&self, req: &Request) -> Result<Response, String> {
            let n = {
                let mut a = self.asked.lock().unwrap();
                a.push(req.range.clone());
                a.len()
            };
            if let Some(f) = &self.on_request {
                f(n);
            }
            if self.fail_on == Some(n) {
                return Ok(Response { status: 503, ..Response::default() });
            }
            if !self.ranges || req.range.is_empty() {
                return Ok(Response { status: 200, body: self.body.clone(), content_length: Some(self.body.len() as u64), ..Response::default() });
            }
            let (a, b) = req.range.strip_prefix("bytes=").and_then(|r| r.split_once('-')).map(|(a, b)| (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap())).unwrap();
            if a >= self.body.len() {
                return Ok(Response { status: 416, ..Response::default() });
            }
            let b = b.min(self.body.len() - 1);
            let total = self.lie_about_total.map_or(self.body.len().to_string(), |t| t.to_string());
            Ok(Response { status: 206, body: self.body[a..=b].to_vec(), headers: vec![("Content-Range".into(), format!("bytes {a}-{b}/{total}"))], ..Response::default() })
        }
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let base = std::env::var("TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let d = std::path::PathBuf::from(base).join(format!("iyakki-stream-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn url() -> Url {
        Url::parse("https://files.org/big.bin").unwrap()
    }

    #[test]
    fn content_range_is_read_and_nonsense_is_refused() {
        assert_eq!(parse_content_range("bytes 0-1048575/12345678"), Some((0, 1048575, Some(12345678))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("bytes 9-5/20"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
        assert_eq!(parse_content_range("bytes a-b/c"), None);
    }

    #[test]
    fn a_big_file_arrives_in_pieces_with_progress_and_is_complete() {
        let d = dir("pieces");
        let s = Server::new(2 * PIECE as usize + 12345, true);
        let p = Progress::default();
        let (name, n) = download(&s, &url(), &d, "big.bin", &AtomicBool::new(false), &p, Some(u64::MAX)).unwrap();
        assert_eq!((name.as_str(), n), ("big.bin", s.body.len() as u64));
        assert_eq!(std::fs::read(d.join("big.bin")).unwrap(), s.body);
        assert_eq!(s.asked.lock().unwrap().len(), 3, "three pieces");
        assert_eq!(s.asked.lock().unwrap()[1], format!("bytes={}-{}", PIECE, 2 * PIECE - 1));
        assert_eq!(p.percent(), Some(100));
        assert!(p.line().starts_with("[##########] 100%"), "{}", p.line());
        assert!(!d.join(".big.bin.part").exists(), "the part file is gone");
        // a second download never overwrites the first
        let (name2, _) = download(&s, &url(), &d, "big.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap();
        assert_eq!(name2, "big (1).bin");
    }

    #[test]
    fn progress_is_visible_between_pieces_and_cancel_stops_the_download_and_removes_the_part() {
        let d = dir("cancel");
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let mut s = Server::new(5 * PIECE as usize, true);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (c2, p2, seen2) = (cancel.clone(), progress.clone(), seen.clone());
        s.on_request = Some(Arc::new(move |n| {
            seen2.lock().unwrap().push(p2.percent());
            if n == 3 {
                c2.store(true, Ordering::Relaxed); // the person presses Esc during the third piece
            }
        }));
        let r = download(&s, &url(), &d, "big.bin", &cancel, &progress, Some(u64::MAX));
        assert_eq!(r.unwrap_err(), "cancelled");
        assert_eq!(s.asked.lock().unwrap().len(), 3, "no piece is asked for after Esc");
        assert_eq!(*seen.lock().unwrap(), vec![None, Some(20), Some(40)], "the screen could show 20% and 40% on the way");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "nothing is left on the drive");
    }

    #[test]
    fn a_server_that_ignores_ranges_is_written_in_one_piece_and_a_cut_one_is_refused() {
        let d = dir("whole");
        let s = Server::new(300_000, false);
        let (name, n) = download(&s, &url(), &d, "f.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap();
        assert_eq!((name.as_str(), n), ("f.bin", 300_000));
        // the transport gave only the start of something larger
        struct Cut;
        impl Fetch for Cut {
            fn get(&self, _: &Url) -> Result<Response, String> {
                Ok(Response { status: 200, body: vec![1; 100], content_length: Some(1 << 30), ..Response::default() })
            }
        }
        let e = download(&Cut, &url(), &d, "g.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap_err();
        assert!(e.contains("too large"), "{e}");
        assert!(!d.join("g.bin").exists());
    }

    #[test]
    fn errors_in_the_middle_leave_nothing_and_say_why() {
        let d = dir("errors");
        let mut s = Server::new(3 * PIECE as usize, true);
        s.fail_on = Some(2);
        assert_eq!(download(&s, &url(), &d, "a.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap_err(), "the server answered 503");
        // a server that announces more than it delivers
        let mut s = Server::new(PIECE as usize, true);
        s.lie_about_total = Some(3 * PIECE);
        let e = download(&s, &url(), &d, "b.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap_err();
        assert!(e.contains("stopped") || e.contains("incomplete") || e.contains("wrong part"), "{e}");
        // too big for the rules, and not enough room
        let mut s = Server::new(10, true);
        s.lie_about_total = Some(MAX_FILE + 1);
        assert!(download(&s, &url(), &d, "c.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap_err().contains("too large"));
        let s = Server::new(10 * PIECE as usize, true);
        assert!(download(&s, &url(), &d, "d.bin", &AtomicBool::new(false), &Progress::default(), Some(PIECE)).unwrap_err().contains("room"));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "no part file is left behind");
        // an empty file
        let s = Server::new(0, true);
        let (n, size) = download(&s, &url(), &d, "e.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap();
        assert_eq!((n.as_str(), size), ("e.bin", 0));
    }

    /// A real HTTP/1.1 server on the loopback: serves ranges (or ignores them) from real sockets, and records the `Range` header of each
    /// request as the wire carried it.
    fn real_server(body: Vec<u8>, ranges: bool) -> (u16, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let a2 = asked.clone();
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (body, a3) = (body.clone(), a2.clone());
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c.try_clone().unwrap());
                    let mut w = c;
                    loop {
                        let mut range = String::new();
                        let mut first = String::new();
                        if r.read_line(&mut first).unwrap_or(0) == 0 {
                            return;
                        }
                        loop {
                            let mut l = String::new();
                            if r.read_line(&mut l).unwrap_or(0) == 0 {
                                return;
                            }
                            if l == "\r\n" {
                                break;
                            }
                            if let Some(v) = l.to_ascii_lowercase().strip_prefix("range:") {
                                range = v.trim().to_string();
                            }
                        }
                        a3.lock().unwrap().push(range.clone());
                        let (status, extra, part) = match range.strip_prefix("bytes=").and_then(|x| x.split_once('-')).filter(|_| ranges) {
                            Some((a, b)) => {
                                let a: usize = a.parse().unwrap();
                                let b: usize = b.parse::<usize>().unwrap().min(body.len() - 1);
                                ("206 Partial Content", format!("Content-Range: bytes {a}-{b}/{}\r\n", body.len()), &body[a..=b])
                            }
                            None => ("200 OK", String::new(), &body[..]),
                        };
                        let head = format!("HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n{extra}Connection: keep-alive\r\n\r\n", part.len());
                        if w.write_all(head.as_bytes()).and_then(|_| w.write_all(part)).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (port, asked)
    }

    #[test]
    fn a_big_file_arrives_in_pieces_over_the_real_transport_from_a_real_server_and_a_server_without_ranges_still_gives_the_whole_file() {
        let web = iyakki_agent::iyakki_proxy::https::BrowserFetch::default().http1_only();
        let body: Vec<u8> = (0..2 * PIECE as usize + 777).map(|i| (i * 7 % 253) as u8).collect();
        let (port, asked) = real_server(body.clone(), true);
        let d = dir("real-pieces");
        let u = Url::parse(&format!("http://127.0.0.1:{port}/big.bin")).unwrap();
        let p = Progress::default();
        let (name, n) = download(&web, &u, &d, "big.bin", &AtomicBool::new(false), &p, Some(u64::MAX)).unwrap();
        assert_eq!((name.as_str(), n), ("big.bin", body.len() as u64));
        assert_eq!(std::fs::read(d.join("big.bin")).unwrap(), body, "the bytes are the server's");
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert_eq!(asked[0], format!("bytes=0-{}", PIECE - 1), "the Range header reached the wire as the code wrote it");
        assert_eq!(asked[1], format!("bytes={}-{}", PIECE, 2 * PIECE - 1));
        assert_eq!(p.percent(), Some(100));
        // a server that ignores Range: the whole file in one answer, written whole
        let body2: Vec<u8> = (0..300_000usize).map(|i| (i % 249) as u8).collect();
        let (port2, asked2) = real_server(body2.clone(), false);
        let u2 = Url::parse(&format!("http://127.0.0.1:{port2}/plain.bin")).unwrap();
        let (_, n2) = download(&web, &u2, &d, "plain.bin", &AtomicBool::new(false), &Progress::default(), Some(u64::MAX)).unwrap();
        assert_eq!(n2, body2.len() as u64);
        assert_eq!(std::fs::read(d.join("plain.bin")).unwrap(), body2);
        assert_eq!(asked2.lock().unwrap().len(), 1, "one request for the whole file");
        // a cancel in the middle leaves no part file
        let cancel = AtomicBool::new(true);
        let u3 = Url::parse(&format!("http://127.0.0.1:{port}/big.bin")).unwrap();
        assert!(download(&web, &u3, &d, "gone.bin", &cancel, &Progress::default(), Some(u64::MAX)).is_err());
        assert!(!d.join("gone.bin").exists() && !d.join(".gone.bin.part").exists());
    }
}

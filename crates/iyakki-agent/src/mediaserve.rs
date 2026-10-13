//! Playing one local video in the embedded full web engine (the `web` image's helper) without giving the engine the file
//! system. The engine may only open `http` and `https` addresses (see [`crate::webengine::check_url`]), and a `file:` address
//! would let a page read whatever the helper can reach. So the shell starts a tiny server for the duration of one playback:
//!
//! * it listens on **127.0.0.1 only**, on an ephemeral port, and is stopped (and the port closed) when [`LocalServe`] is dropped;
//! * it serves exactly **two things**, both under a path that starts with a random 128-bit token read from `/dev/urandom`:
//!   `/<token>/` (a small generated player page with a `<video controls autoplay>`) and `/<token>/media` (the one file chosen,
//!   opened before the server started, with `Range` requests so the player can seek). Every other path is a 404 and gives
//!   nothing away;
//! * the `Host` header must be the loopback address and port (a web page cannot reach it through a rebound DNS name), only `GET`
//!   and `HEAD` are answered, and replies carry `no-store`, `nosniff`, a strict Content-Security-Policy and no referrer.
//!
//! The caller checks that the account may read the file before starting; the helper is given `127.0.0.1` as its only allowed
//! host, so a page cannot navigate anywhere else. See `docs/MEDIA-SEARCH.md` ("Video on the web image").

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const MAX_CONNECTIONS: usize = 8;
const CHUNK: usize = 64 * 1024;

#[derive(Debug)]
pub struct LocalServe {
    /// The address to give the engine: `http://127.0.0.1:<port>/<token>/`.
    pub url: String,
    port: u16,
    stop: Arc<AtomicBool>,
}

/// Content type by extension.
fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("webm") => "video/webm",
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        _ => "application/octet-stream",
    }
}

fn token() -> Result<String, String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("no random numbers for the player address: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn escape(s: &str) -> String {
    s.chars().map(|c| match c {
        '&' => "&amp;".to_string(),
        '<' => "&lt;".to_string(),
        '>' => "&gt;".to_string(),
        '"' => "&quot;".to_string(),
        c if c.is_control() => " ".to_string(),
        c => c.to_string(),
    }).collect()
}

/// The page the engine shows: black, the video filling it, Space pauses, the arrows seek.
pub fn player_page(title: &str, audio_only: bool) -> String {
    let tag = if audio_only { "audio" } else { "video" };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{t}</title><style>html,body{{margin:0;height:100%;background:#000;color:#ddd;font:20px sans-serif;overflow:hidden}}\
         {tag}{{width:100%;height:100%;background:#000;outline:none}}#msg{{position:fixed;left:0;right:0;bottom:56px;text-align:center;text-shadow:0 0 6px #000}}</style></head>\
         <body><{tag} id=\"v\" controls autoplay src=\"media\"></{tag}><div id=\"msg\"></div><script>\
         var v=document.getElementById('v'),m=document.getElementById('msg');v.focus();\
         function say(t){{m.textContent=t}}\
         v.addEventListener('error',function(){{say('This file cannot be played here (the codec may be missing).')}});\
         v.addEventListener('ended',function(){{say('The end. Esc goes back.')}});\
         v.addEventListener('playing',function(){{say('')}});\
         var p=v.play();if(p&&p.catch){{p.catch(function(){{say('Press Space to play.')}})}}\
         document.addEventListener('keydown',function(e){{\
         if(e.key===' '){{e.preventDefault();if(v.paused){{v.play()}}else{{v.pause()}}}}\
         else if(e.key==='ArrowRight'){{e.preventDefault();v.currentTime=Math.min(v.duration||1e9,v.currentTime+5)}}\
         else if(e.key==='ArrowLeft'){{e.preventDefault();v.currentTime=Math.max(0,v.currentTime-5)}}\
         else if(e.key==='m'){{v.muted=!v.muted}}}});\
         </script></body></html>",
        t = escape(title)
    )
}

impl LocalServe {
    /// Serve `file` (a regular file the caller has checked) for a player page titled `title`.
    pub fn start(file: &Path, title: &str) -> Result<LocalServe, String> {
        let f = std::fs::File::open(file).map_err(|e| format!("cannot open the file: {e}"))?;
        let md = f.metadata().map_err(|e| e.to_string())?;
        if !md.is_file() || md.len() == 0 {
            return Err("that is not a playable file".into());
        }
        let f = Arc::new(f);
        let size = md.len();
        let ctype = content_type(file);
        let audio = ctype.starts_with("audio/");
        let page = Arc::new(player_page(title, audio));
        let token = token()?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("cannot open a local port: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let open = Arc::new(AtomicUsize::new(0));
        let (stop2, token2) = (stop.clone(), token.clone());
        std::thread::Builder::new()
            .name("iyakki-media-serve".into())
            .spawn(move || {
                for s in listener.incoming() {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(s) = s else { continue };
                    if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                        let _ = s.shutdown(Shutdown::Both);
                        continue;
                    }
                    open.fetch_add(1, Ordering::SeqCst);
                    let (f, page, token, stop3, open2) = (f.clone(), page.clone(), token2.clone(), stop2.clone(), open.clone());
                    std::thread::spawn(move || {
                        let _ = handle(s, &f, size, ctype, &page, &token, port, &stop3);
                        open2.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(LocalServe { url: format!("http://127.0.0.1:{port}/{token}/"), port, stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for LocalServe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // wake the accept loop so it sees the flag and closes the port
        let _ = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_millis(500));
    }
}

fn reply(s: &mut TcpStream, status: &str, headers: &[(&str, String)], body_len: u64) -> std::io::Result<()> {
    let mut h = format!("HTTP/1.1 {status}\r\nContent-Length: {body_len}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n");
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    s.write_all(h.as_bytes())
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` against a file of `size` bytes: the inclusive range, or None when it cannot be met.
pub fn parse_range(h: &str, size: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        (size.saturating_sub(n), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end: u64 = if b.is_empty() { size - 1 } else { b.parse().ok()? };
        (start, end.min(size - 1))
    };
    (start <= end && start < size).then_some((start, end))
}

#[allow(clippy::too_many_arguments)]
fn handle(mut s: TcpStream, f: &std::fs::File, size: u64, ctype: &str, page: &str, token: &str, port: u16, stop: &AtomicBool) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut chunk)?;
        if n == 0 || buf.len() > 8192 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let header = |name: &str| lines.clone().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    // the page behind this port is for the engine on this machine only: a rebound name is not accepted
    if header("host").as_deref() != Some(&format!("127.0.0.1:{port}")) {
        return reply(&mut s, "403 Forbidden", &[], 0);
    }
    if method != "GET" && method != "HEAD" {
        return reply(&mut s, "405 Method Not Allowed", &[("Allow", "GET, HEAD".into())], 0);
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let rest = path.strip_prefix('/').and_then(|p| p.strip_prefix(token));
    match rest {
        Some("/") => {
            let h = [("Content-Type", "text/html; charset=utf-8".to_string()), ("Content-Security-Policy", "default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'".to_string())];
            reply(&mut s, "200 OK", &h, page.len() as u64)?;
            if method == "GET" {
                s.write_all(page.as_bytes())?;
            }
            Ok(())
        }
        Some("/media") => {
            let range = header("range");
            let (status, start, end, mut h) = match range.as_deref().map(|r| parse_range(r, size)) {
                Some(Some((a, b))) => ("206 Partial Content", a, b, vec![("Content-Range", format!("bytes {a}-{b}/{size}"))]),
                Some(None) => return reply(&mut s, "416 Range Not Satisfiable", &[("Content-Range", format!("bytes */{size}"))], 0),
                None => ("200 OK", 0, size - 1, vec![]),
            };
            h.push(("Content-Type", ctype.to_string()));
            h.push(("Accept-Ranges", "bytes".to_string()));
            reply(&mut s, status, &h, end - start + 1)?;
            if method == "HEAD" {
                return Ok(());
            }
            let mut pos = start;
            let mut data = vec![0u8; CHUNK];
            while pos <= end && !stop.load(Ordering::SeqCst) {
                let want = ((end - pos + 1) as usize).min(CHUNK);
                let n = f.read_at(&mut data[..want], pos)?;
                if n == 0 {
                    break;
                }
                s.write_all(&data[..n])?;
                pos += n as u64;
            }
            Ok(())
        }
        _ => reply(&mut s, "404 Not Found", &[], 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, host: &str, request: &str) -> (String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = host;
        s.write_all(request.as_bytes()).unwrap();
        let mut out = vec![];
        let _ = s.read_to_end(&mut out);
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(out.len());
        (String::from_utf8_lossy(&out[..split]).into_owned(), out.get(split + 4..).unwrap_or(&[]).to_vec())
    }

    fn req(method: &str, path: &str, host: &str, extra: &str) -> String {
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n")
    }

    fn sample_file(tag: &str, len: usize) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-serve-{}-{}", std::process::id(), tag));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("clip one.webm");
        std::fs::write(&p, (0..len).map(|i| (i % 251) as u8).collect::<Vec<u8>>()).unwrap();
        p
    }

    #[test]
    fn it_serves_the_page_and_the_one_file_with_ranges_to_loopback_only() {
        let file = sample_file("ok", 300_000);
        let srv = LocalServe::start(&file, "Clip <one> & \"two\"").unwrap();
        let port = srv.port();
        assert!(srv.url.starts_with(&format!("http://127.0.0.1:{port}/")) && srv.url.ends_with('/'));
        let token = srv.url.trim_end_matches('/').rsplit('/').next().unwrap().to_string();
        assert_eq!(token.len(), 32);
        let host = format!("127.0.0.1:{port}");
        // the page
        let (h, body) = get(port, &host, &req("GET", &format!("/{token}/"), &host, ""));
        assert!(h.starts_with("HTTP/1.1 200") && h.contains("text/html") && h.contains("no-store") && h.contains("Content-Security-Policy"), "{h}");
        let page = String::from_utf8(body).unwrap();
        assert!(page.contains("<video id=\"v\" controls autoplay src=\"media\">") && page.contains("Clip &lt;one&gt; &amp; &quot;two&quot;") && !page.contains("Clip <one>"), "{page}");
        // the whole file, a range, a suffix range, an open range
        let media = format!("/{token}/media");
        let (h, body) = get(port, &host, &req("GET", &media, &host, ""));
        assert!(h.starts_with("HTTP/1.1 200") && h.contains("video/webm") && h.contains("Accept-Ranges: bytes"), "{h}");
        assert_eq!(body, std::fs::read(&file).unwrap());
        let all = std::fs::read(&file).unwrap();
        let (h, body) = get(port, &host, &req("GET", &media, &host, "Range: bytes=100-199\r\n"));
        assert!(h.starts_with("HTTP/1.1 206") && h.contains("Content-Range: bytes 100-199/300000"), "{h}");
        assert_eq!(body, all[100..200]);
        let (_, body) = get(port, &host, &req("GET", &media, &host, "Range: bytes=-50\r\n"));
        assert_eq!(body, all[all.len() - 50..]);
        let (_, body) = get(port, &host, &req("GET", &media, &host, "Range: bytes=299990-\r\n"));
        assert_eq!(body, all[299_990..]);
        let (h, _) = get(port, &host, &req("GET", &media, &host, "Range: bytes=999999-\r\n"));
        assert!(h.starts_with("HTTP/1.1 416"), "{h}");
        // HEAD has no body
        let (h, body) = get(port, &host, &req("HEAD", &media, &host, ""));
        assert!(h.starts_with("HTTP/1.1 200") && h.contains("Content-Length: 300000") && body.is_empty());
    }

    #[test]
    fn everything_else_is_refused() {
        let file = sample_file("deny", 1000);
        let srv = LocalServe::start(&file, "x").unwrap();
        let port = srv.port();
        let token = srv.url.trim_end_matches('/').rsplit('/').next().unwrap().to_string();
        let host = format!("127.0.0.1:{port}");
        let status = |r: String| get(port, &host, &r).0.lines().next().unwrap_or("").to_string();
        // no token, a wrong token, other files, folder tricks
        for path in ["/", "/media", "/wrong/media", &format!("/{token}x/media"), &format!("/{token}/../etc/passwd"), &format!("/{token}/media/extra"), &format!("/{token}/clip one.webm"), "/etc/passwd", "/favicon.ico"] {
            assert!(status(req("GET", path, &host, "")).contains("404"), "{path}");
        }
        // a rebound or foreign Host name, no Host at all
        assert!(status(req("GET", &format!("/{token}/media"), "evil.example:80", "")).contains("403"));
        assert!(status(req("GET", &format!("/{token}/media"), "localhost", "")).contains("403"));
        assert!(status("GET /x HTTP/1.1\r\n\r\n".to_string()).contains("403"));
        // writing methods
        for m in ["POST", "PUT", "DELETE"] {
            assert!(status(req(m, &format!("/{token}/media"), &host, "")).contains("405"), "{m}");
        }
        // two servers have different ports and tokens
        let other = LocalServe::start(&file, "x").unwrap();
        assert_ne!((srv.port(), &srv.url), (other.port(), &other.url));
    }

    #[test]
    fn the_server_stops_and_closes_its_port_when_dropped_and_odd_files_are_refused() {
        let file = sample_file("drop", 100);
        let srv = LocalServe::start(&file, "x").unwrap();
        let port = srv.port();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        drop(srv);
        std::thread::sleep(Duration::from_millis(100));
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "the port is closed");
        assert!(LocalServe::start(Path::new("/nonexistent/x.webm"), "x").is_err());
        assert!(LocalServe::start(file.parent().unwrap(), "x").unwrap_err().contains("not a playable"), "a folder is not a file");
        let empty = file.with_file_name("empty.webm");
        std::fs::write(&empty, b"").unwrap();
        assert!(LocalServe::start(&empty, "x").is_err());
    }

    #[test]
    fn range_headers_are_read_strictly() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-5000", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=9-3", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=-0", 100), None);
        assert_eq!(parse_range("bytes=a-b", 100), None);
    }
}

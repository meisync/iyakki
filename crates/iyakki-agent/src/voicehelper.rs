//! The browser voice helper: reading aloud in the desktop window when it has no speech of its own.
//!
//! The window's web engine often has no `speechSynthesis` (WebKitGTK built without it), but the person's web browser does.
//! This starts a tiny listener on `127.0.0.1` (a random port), opens the default browser on a minimal page that connects back
//! over a WebSocket, and speaks the pieces that are being read with the browser's own voices. It is the same protocol and the
//! same page code as the web front end's browser voice (`hostvoice`, `speech.js`, `docs/REMOTE-PROTOCOL.md`); the only thing
//! here is the meeting point.
//!
//! Safety, because it is a listener:
//!
//! * it only listens on `127.0.0.1`, on a random port, and says nothing before the link with its secret is opened;
//! * the link carries a one-time token: the first `GET /` with it gets the page (and a session secret inside the page), a
//!   second try gets "already used". The WebSocket needs that session secret, the `Host` must be `127.0.0.1:port` (against DNS
//!   rebinding) and the `Origin` must be the page's own (`http://127.0.0.1:port`), so a page of another site cannot connect;
//! * one tab at a time; it closes when the tab closes, when the tab stops answering for a while, when nobody opened the link
//!   in time, or when the person presses Disconnect;
//! * only the text of the pieces being read goes to the page, and nothing leaves the machine through this listener.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hostvoice::{FromClient, HostVoice};
use crate::httpmini::{self as http, Parse, Request, Response};
use crate::wsproto::{self as ws, Message, Parser};

/// How long the link may wait for the browser to open it.
pub const OPEN_WAIT: Duration = Duration::from_secs(120);
/// How long a connected page may be silent (it sends a heartbeat every few seconds).
pub const IDLE: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelperState {
    /// Listening; the browser has not connected yet.
    Waiting,
    Connected,
    /// Over, and why.
    Closed(String),
}

struct Sh {
    hv: Arc<HostVoice>,
    port: u16,
    token: Mutex<Option<String>>,
    session: String,
    stop: AtomicBool,
    state: Mutex<HelperState>,
    idle: Duration,
}

impl Sh {
    fn set(&self, s: HelperState) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    fn state(&self) -> HelperState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn closed(&self) -> bool {
        matches!(self.state(), HelperState::Closed(_))
    }

    fn close(&self, why: &str) {
        if !self.closed() {
            self.set(HelperState::Closed(why.to_string()));
        }
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub struct VoiceHelper {
    sh: Arc<Sh>,
    url: String,
    addr: SocketAddr,
}

fn hex_token() -> Result<String, String> {
    let mut r = [0u8; 16];
    iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
    Ok(r.iter().map(|b| format!("{b:02x}")).collect())
}

/// Constant-time comparison of secrets.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

impl VoiceHelper {
    /// Start listening for `hv` (the speech bridge of the person's shell). Nothing is opened yet: see [`VoiceHelper::url`]
    /// and [`open_browser`].
    pub fn start(hv: Arc<HostVoice>) -> Result<VoiceHelper, String> {
        VoiceHelper::start_with(hv, OPEN_WAIT, IDLE)
    }

    pub fn start_with(hv: Arc<HostVoice>, open_wait: Duration, idle: Duration) -> Result<VoiceHelper, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot listen on this computer: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let token = hex_token()?;
        let sh = Arc::new(Sh { hv, port: addr.port(), token: Mutex::new(Some(token.clone())), session: hex_token()?, stop: AtomicBool::new(false), state: Mutex::new(HelperState::Waiting), idle });
        let s = sh.clone();
        std::thread::Builder::new().name("iyakki-voice-helper".into()).spawn(move || accept_loop(listener, s, open_wait)).map_err(|e| e.to_string())?;
        Ok(VoiceHelper { url: format!("http://127.0.0.1:{}/?t={token}", addr.port()), sh, addr })
    }

    /// The link to open in the browser (it works once).
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn state(&self) -> HelperState {
        self.sh.state()
    }

    /// Disconnect: the page is told, the listener closes.
    pub fn stop(&self) {
        self.sh.close("Disconnected.");
    }
}

impl Drop for VoiceHelper {
    fn drop(&mut self) {
        self.sh.close("Disconnected.");
    }
}

fn accept_loop(listener: TcpListener, sh: Arc<Sh>, open_wait: Duration) {
    let started = Instant::now();
    while !sh.stop.load(Ordering::SeqCst) {
        if sh.state() == HelperState::Waiting && started.elapsed() > open_wait {
            sh.close("The browser did not open the link in time.");
            break;
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let s = sh.clone();
                let _ = std::thread::Builder::new().name("iyakki-voice-helper-conn".into()).spawn(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(stream, &s)));
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    // the listener is dropped here: the port is closed
}

fn read_request(stream: &mut TcpStream) -> Result<Request, Option<Response>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let started = Instant::now();
    loop {
        match http::parse(&buf) {
            Parse::Done(r) => return Ok(r),
            Parse::Bad(code, msg) => return Err(Some(Response::error(code, msg))),
            Parse::Incomplete => {}
        }
        if started.elapsed() > Duration::from_secs(5) {
            return Err(None);
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn connection(mut stream: TcpStream, sh: &Sh) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_nodelay(true);
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(Some(r)) => {
            let _ = stream.write_all(&r.to_bytes());
            return;
        }
        Err(None) => return,
    };
    // the name the browser used must be the one it was given: a page of another site that was pointed at 127.0.0.1 by its
    // DNS sends its own name here
    let host_ok = req.header("host").is_some_and(|h| h == format!("127.0.0.1:{}", sh.port));
    if !host_ok {
        let _ = stream.write_all(&Response::error(403, "not allowed from here").to_bytes());
        return;
    }
    if req.wants_websocket() {
        websocket(stream, &req, sh);
        return;
    }
    let resp = route(&req, sh);
    let _ = stream.write_all(&resp.to_bytes());
    let _ = stream.flush();
}

fn route(req: &Request, sh: &Sh) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => {
            let Some(t) = req.query.get("t") else { return Response::error(403, "this link needs its secret") };
            let mut slot = sh.token.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_deref() {
                Some(expected) if same(expected, t) => {
                    *slot = None; // one time
                    Response::html(&page(&sh.session))
                }
                None => Response::error(410, "this link was already used; press Open voice helper in IYAKKI again"),
                _ => Response::error(403, "this link needs its secret"),
            }
        }
        ("GET", _) => Response::error(404, "not found"),
        _ => Response::error(405, "method not allowed"),
    }
}

/// The page: a status line, a button that satisfies the browsers that only speak after a click, and the shared speech code.
fn page(session: &str) -> String {
    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>IYAKKI voice helper</title>
<style>
  :root {{ color-scheme: dark; }}
  body {{ margin: 0; min-height: 100vh; display: flex; align-items: center; justify-content: center; background: #090b14; color: #e6e9f5; font-family: system-ui, "Segoe UI", Roboto, sans-serif; }}
  main {{ width: 440px; max-width: calc(100vw - 32px); background: #1e253e; border-radius: 14px; padding: 28px; box-shadow: 0 10px 40px #0008; }}
  h1 {{ margin: 0 0 6px; font-size: 22px; letter-spacing: 2px; }}
  p {{ margin: 0 0 14px; color: #9aa3c4; font-size: 14px; line-height: 1.5; }}
  #st {{ font-size: 16px; margin: 14px 0; }}
  button {{ padding: 10px 16px; border: 0; border-radius: 8px; background: #5b7cff; color: #fff; font-size: 15px; cursor: pointer; }}
</style></head><body><main>
<h1>IYAKKI voice helper</h1>
<p>This tab lets IYAKKI read stories and replies aloud with the voices of this browser. Nothing leaves this computer. Keep the tab open while you listen; close it to disconnect.</p>
<div id="st" role="status">Connecting…</div>
<button id="b" type="button">Click once to allow sound</button>
</main>
<script>
"use strict";
const S = {session:?};
let ws = null, closed = false;
const st = (t) => {{ document.getElementById("st").textContent = t; }};
function send(o) {{ if (ws && ws.readyState === 1) ws.send(JSON.stringify(o)); }}
{js}
document.getElementById("b").addEventListener("click", () => {{ spPrime(); st(ws && ws.readyState === 1 ? "Voice helper connected. You can go back to IYAKKI." : "Connecting…"); }});
try {{ history.replaceState(null, "", "/"); }} catch (e) {{}}
function connect() {{
  ws = new WebSocket("ws://" + location.host + "/ws?s=" + encodeURIComponent(S));
  ws.onopen = () => {{ st("Voice helper connected. You can go back to IYAKKI."); spVoices(); }};
  ws.onmessage = (m) => {{
    if (typeof m.data !== "string") return;
    const j = JSON.parse(m.data);
    if (j.t === "bye") {{ closed = true; st("Disconnected. You can close this tab."); try {{ ws.close(); }} catch (e) {{}} return; }}
    if (typeof j.t === "string" && j.t.indexOf("speak") === 0) spMessage(j);
  }};
  ws.onclose = () => {{ if (!closed) {{ closed = true; st("Disconnected. You can close this tab."); }} }};
}}
setInterval(() => send({{ t: "hb" }}), 5000);
window.addEventListener("beforeunload", () => {{ try {{ closed = true; ws.close(); }} catch (e) {{}} }});
connect();
</script></body></html>
"##,
        session = session,
        js = crate::hostvoice::SPEECH_JS
    )
}

fn websocket(mut stream: TcpStream, req: &Request, sh: &Sh) {
    let refuse = |s: &mut TcpStream, r: Response| {
        let _ = s.write_all(&r.to_bytes());
    };
    // a browser always sends the origin of the page that opens a WebSocket; it must be this helper's own page
    let origin_ok = req.header("origin").is_some_and(|o| o == format!("http://127.0.0.1:{}", sh.port));
    if req.method != "GET" || !origin_ok {
        return refuse(&mut stream, Response::error(403, "not allowed from here"));
    }
    if !req.query.get("s").is_some_and(|s| same(s, &sh.session)) {
        return refuse(&mut stream, Response::error(403, "not allowed from here"));
    }
    let Some(key) = req.header("sec-websocket-key") else { return refuse(&mut stream, Response::error(400, "no key")) };
    {
        let mut st = sh.state.lock().unwrap_or_else(|e| e.into_inner());
        match *st {
            HelperState::Connected => {
                drop(st);
                return refuse(&mut stream, Response::error(409, "a tab is already connected"));
            }
            HelperState::Closed(_) => {
                drop(st);
                return refuse(&mut stream, Response::error(410, "the voice helper is closed"));
            }
            HelperState::Waiting => *st = HelperState::Connected,
        }
    }
    // the window's speech bridge talks to the page from the moment the tab is told it is connected (101), not a moment later
    sh.hv.attach_helper();
    let hello = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept_key(key));
    if stream.write_all(hello.as_bytes()).is_err() {
        sh.hv.detach_helper();
        sh.close("The tab could not connect.");
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
    let why = serve(&mut stream, sh);
    sh.hv.detach_helper();
    sh.close(&why);
}

/// Carry the speech messages until the tab goes away. Returns why it ended.
fn serve(stream: &mut TcpStream, sh: &Sh) -> String {
    let mut parser = Parser::new(256 * 1024);
    let mut buf = vec![0u8; 8192];
    let mut last = Instant::now();
    let send = |s: &mut TcpStream, bytes: Vec<u8>| s.write_all(&bytes).and_then(|_| s.flush()).is_ok();
    loop {
        if sh.stop.load(Ordering::SeqCst) {
            let _ = send(stream, ws::text_frame(r#"{"t":"bye"}"#));
            let _ = send(stream, ws::close_frame());
            return "Disconnected.".into();
        }
        if last.elapsed() > sh.idle {
            let _ = send(stream, ws::close_frame());
            return "The browser tab stopped answering.".into();
        }
        match stream.read(&mut buf) {
            Ok(0) => return "The browser tab was closed.".into(),
            Ok(n) => parser.push(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => return "The browser tab was closed.".into(),
        }
        loop {
            match parser.next() {
                Ok(Some(Message::Text(t))) => {
                    last = Instant::now();
                    if let Some(e) = FromClient::decode(&t) {
                        sh.hv.event(e);
                    }
                }
                Ok(Some(Message::Ping(p))) => {
                    last = Instant::now();
                    if !send(stream, ws::pong_frame(&p)) {
                        return "The browser tab was closed.".into();
                    }
                }
                Ok(Some(Message::Close)) => {
                    let _ = send(stream, ws::close_frame());
                    return "The browser tab was closed.".into();
                }
                Ok(Some(_)) => last = Instant::now(),
                Ok(None) => break,
                Err(_) => {
                    let _ = send(stream, ws::close_frame());
                    return "The browser tab sent something that was not understood.".into();
                }
            }
        }
        for m in sh.hv.take_outgoing() {
            if !send(stream, ws::text_frame(&m.encode())) {
                return "The browser tab was closed.".into();
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------ opening the browser

/// The command that opens `url` in the default browser: `xdg-open`, `open`, or Windows' URL handler. The address is one
/// argument; there is no shell.
pub fn browser_command(os: crate::hostengines::Os, url: &str) -> Vec<String> {
    use crate::hostengines::Os;
    match os {
        Os::Linux => vec!["xdg-open".into(), url.into()],
        Os::Mac => vec!["open".into(), url.into()],
        Os::Windows => vec!["rundll32".into(), "url.dll,FileProtocolHandler".into(), url.into()],
    }
}

/// Open the default browser on `url`. `opener` replaces the usual command (tests, and people whose desktop has another
/// one); the url is its last argument. Returns an error that names what to do when there is none.
pub fn open_browser(url: &str, opener: Option<&[String]>, path: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let argv: Vec<String> = match opener {
        Some(o) if !o.is_empty() => o.iter().cloned().chain([url.to_string()]).collect(),
        _ => browser_command(crate::hostengines::Os::current(), url),
    };
    let prog = crate::story::which(&argv[0], path).ok_or_else(|| format!("no program to open a browser was found ({}); open this address in your browser yourself: {url}", argv[0]))?;
    let mut c = std::process::Command::new(prog);
    c.args(&argv[1..]).process_group(0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = c.spawn().map_err(|e| format!("could not open the browser: {e}; open this address yourself: {url}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostvoice::{Front, Support, ToClient, UttState};

    fn get(addr: SocketAddr, path: &str, host: Option<&str>) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let host = host.map(str::to_string).unwrap_or_else(|| format!("127.0.0.1:{}", addr.port()));
        write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let t = String::from_utf8_lossy(&out).to_string();
        (t.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0), t)
    }

    struct Tab {
        s: TcpStream,
        rest: Vec<u8>,
    }

    fn masked(op: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x11, 0x22, 0x33, 0x44];
        let mut f = vec![0x80 | op, 0x80 | payload.len() as u8];
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    /// A page, as raw bytes: the handshake a browser does, with the origin it would send.
    fn connect(addr: SocketAddr, session: &str, origin: Option<&str>, host: Option<&str>) -> Result<Tab, u16> {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let host = host.map(str::to_string).unwrap_or_else(|| format!("127.0.0.1:{}", addr.port()));
        let o = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
        write!(s, "GET /ws?s={session} HTTP/1.1\r\nHost: {host}\r\n{o}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        let t = Instant::now();
        while !head.ends_with(b"\r\n\r\n") && t.elapsed() < Duration::from_secs(5) {
            match s.read(&mut b) {
                Ok(0) => break,
                Ok(_) => head.push(b[0]),
                Err(_) => {}
            }
        }
        let text = String::from_utf8_lossy(&head).to_string();
        let code: u16 = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        if code == 101 {
            Ok(Tab { s, rest: Vec::new() })
        } else {
            Err(code)
        }
    }

    impl Tab {
        fn send(&mut self, text: &str) {
            self.s.write_all(&masked(0x1, text.as_bytes())).unwrap();
        }

        /// The text frames that arrived (unmasked server frames), waiting up to `ms`.
        fn texts(&mut self, ms: u64, want: usize) -> Vec<String> {
            let t = Instant::now();
            let mut out = Vec::new();
            let mut b = [0u8; 4096];
            loop {
                match self.s.read(&mut b) {
                    Ok(0) => break,
                    Ok(n) => self.rest.extend_from_slice(&b[..n]),
                    Err(_) => {}
                }
                while self.rest.len() >= 2 {
                    let (op, len) = (self.rest[0] & 0x0f, (self.rest[1] & 0x7f) as usize);
                    if self.rest.len() < 2 + len {
                        break;
                    }
                    let payload = self.rest[2..2 + len].to_vec();
                    self.rest.drain(..2 + len);
                    if op == 1 {
                        out.push(String::from_utf8_lossy(&payload).to_string());
                    }
                }
                if out.len() >= want || t.elapsed() > Duration::from_millis(ms) {
                    break;
                }
            }
            out
        }
    }

    fn token_of(h: &VoiceHelper) -> String {
        h.url().split("?t=").nth(1).unwrap().to_string()
    }

    fn session_of(page: &str) -> String {
        let i = page.find("const S = \"").unwrap() + 11;
        page[i..].split('"').next().unwrap().to_string()
    }

    fn wait(f: impl Fn() -> bool) -> bool {
        for _ in 0..300 {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn the_link_works_once_and_needs_its_secret_and_its_own_host() {
        let hv = Arc::new(HostVoice::new());
        let h = VoiceHelper::start(hv).unwrap();
        assert!(h.url().starts_with("http://127.0.0.1:") && h.addr().ip().is_loopback());
        let (c, _) = get(h.addr(), "/", None);
        assert_eq!(c, 403, "no token");
        let (c, _) = get(h.addr(), "/?t=wrong", None);
        assert_eq!(c, 403, "wrong token");
        let t = token_of(&h);
        let (c, _) = get(h.addr(), &format!("/?t={t}"), Some("evil.example:80"));
        assert_eq!(c, 403, "another host name (DNS rebinding) is refused, and does not use up the token");
        let (c, page) = get(h.addr(), &format!("/?t={t}"), None);
        assert_eq!(c, 200);
        assert!(page.contains("IYAKKI voice helper") && page.contains("speechSynthesis") && page.contains("Content-Security-Policy"));
        assert!(!page.contains(&t), "the token is not repeated in the page");
        let (c, _) = get(h.addr(), &format!("/?t={t}"), None);
        assert_eq!(c, 410, "the second use of the link");
        let (c, _) = get(h.addr(), "/other", None);
        assert_eq!(c, 404);
    }

    #[test]
    fn the_socket_needs_the_session_the_right_origin_and_one_tab() {
        let hv = Arc::new(HostVoice::new());
        let h = VoiceHelper::start(hv.clone()).unwrap();
        let (_, page) = get(h.addr(), &format!("/?t={}", token_of(&h)), None);
        let session = session_of(&page);
        let own = format!("http://127.0.0.1:{}", h.addr().port());
        assert_eq!(connect(h.addr(), &session, None, None).err(), Some(403), "no Origin");
        assert_eq!(connect(h.addr(), &session, Some("https://evil.example"), None).err(), Some(403), "a page of another site");
        assert_eq!(connect(h.addr(), &session, Some("http://localhost:1"), None).err(), Some(403));
        assert_eq!(connect(h.addr(), "wrong", Some(&own), None).err(), Some(403), "wrong session");
        assert_eq!(connect(h.addr(), &session, Some(&own), Some("evil.example")).err(), Some(403), "wrong Host");
        assert_eq!(h.state(), HelperState::Waiting, "refused attempts change nothing");
        let mut tab = connect(h.addr(), &session, Some(&own), None).unwrap();
        assert!(wait(|| h.state() == HelperState::Connected));
        assert_eq!(connect(h.addr(), &session, Some(&own), None).err(), Some(409), "one tab at a time");
        // the window's speech bridge now talks to the page
        assert!(hv.helper_active() && hv.front() == Front::Browser && hv.support() == Support::Checking);
        let first = tab.texts(2000, 1);
        assert_eq!(ToClient::decode(&first[0]), Some(ToClient::ListVoices), "the page is asked for its voices");
        tab.send(r#"{"t":"sp_voices","ok":true,"voices":[{"n":"Test Voice","l":"ta-IN","d":true}]}"#);
        assert!(wait(|| hv.is_ready()), "{:?}", hv.support());
        assert_eq!(hv.voices()[0].name, "Test Voice");
        // speaking: the piece goes to the page and its events come back
        let id = hv.speak("Hello there", None, Some("ta-IN"), 1.0).unwrap();
        let got = tab.texts(2000, 1);
        assert!(matches!(ToClient::decode(&got[0]), Some(ToClient::Speak { id: i, ref text, .. }) if i == id && text == "Hello there"), "{got:?}");
        tab.send(&format!(r#"{{"t":"sp_start","id":{id}}}"#));
        assert!(wait(|| hv.state(id) == UttState::Started));
        tab.send("garbage that is not json");
        tab.send(r#"{"t":"hb"}"#);
        tab.send(&format!(r#"{{"t":"sp_end","id":{id}}}"#));
        assert!(wait(|| hv.state(id) == UttState::Ended));
        assert_eq!(h.state(), HelperState::Connected, "garbage text is ignored, not fatal");
        // closing the tab closes the helper and gives the window its own state back
        drop(tab);
        assert!(wait(|| matches!(h.state(), HelperState::Closed(_))), "{:?}", h.state());
        assert!(wait(|| !hv.helper_active() && hv.front() == Front::Console));
        assert!(wait(|| TcpStream::connect(h.addr()).is_err()), "the port is closed");
    }

    #[test]
    fn disconnect_idle_and_unopened_links_close_the_listener() {
        // the person presses Disconnect
        let hv = Arc::new(HostVoice::new());
        hv.connect(Front::Window);
        hv.set_unsupported("no speech in this engine");
        let h = VoiceHelper::start(hv.clone()).unwrap();
        let (_, page) = get(h.addr(), &format!("/?t={}", token_of(&h)), None);
        let own = format!("http://127.0.0.1:{}", h.addr().port());
        let mut tab = connect(h.addr(), &session_of(&page), Some(&own), None).unwrap();
        assert!(wait(|| h.state() == HelperState::Connected));
        assert!(hv.front() == Front::Browser);
        hv.set_unsupported("a late probe result must not break the helper");
        assert_eq!(hv.support(), Support::Checking);
        h.stop();
        let bye = tab.texts(2000, 2);
        assert_eq!(bye.last().map(String::as_str), Some(r#"{"t":"bye"}"#), "{bye:?}");
        assert!(wait(|| matches!(h.state(), HelperState::Closed(_))));
        assert!(wait(|| !hv.helper_active()));
        assert_eq!((hv.front(), hv.support()), (Front::Window, Support::No("a late probe result must not break the helper".into())), "the window's own state is back");
        // a tab that goes silent
        let hv = Arc::new(HostVoice::new());
        let h = VoiceHelper::start_with(hv.clone(), OPEN_WAIT, Duration::from_millis(300)).unwrap();
        let (_, page) = get(h.addr(), &format!("/?t={}", token_of(&h)), None);
        let own = format!("http://127.0.0.1:{}", h.addr().port());
        let _tab = connect(h.addr(), &session_of(&page), Some(&own), None).unwrap();
        assert!(wait(|| matches!(h.state(), HelperState::Closed(ref m) if m.contains("stopped answering"))), "{:?}", h.state());
        // a link nobody opens
        let h = VoiceHelper::start_with(Arc::new(HostVoice::new()), Duration::from_millis(200), IDLE).unwrap();
        assert!(wait(|| matches!(h.state(), HelperState::Closed(ref m) if m.contains("did not open"))), "{:?}", h.state());
        assert!(wait(|| TcpStream::connect(h.addr()).is_err()));
    }

    #[test]
    fn the_browser_is_opened_without_a_shell() {
        use crate::hostengines::Os;
        assert_eq!(browser_command(Os::Linux, "http://127.0.0.1:1/?t=a b;c"), ["xdg-open", "http://127.0.0.1:1/?t=a b;c"]);
        assert_eq!(browser_command(Os::Mac, "u")[0], "open");
        assert_eq!(browser_command(Os::Windows, "u")[0], "rundll32");
        let d = std::env::temp_dir().join(format!("iyakki-vh-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("opened");
        let fake = d.join("fakeopen");
        std::fs::write(&fake, format!("#!/bin/sh\nprintf '%s' \"$1\" > {}\n", log.display())).unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let url = "http://127.0.0.1:9/?t=$(touch /tmp/pwned2)";
        open_browser(url, Some(&[fake.to_string_lossy().to_string()]), "").unwrap();
        assert!(wait(|| log.exists() && std::fs::metadata(&log).map(|m| m.len() > 0).unwrap_or(false)));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), url);
        assert!(!Path::new("/tmp/pwned2").exists());
        let e = open_browser("http://127.0.0.1:9/", None, "/nonexistent").unwrap_err();
        assert!(e.contains("open this address in your browser yourself"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    use std::path::Path;
}

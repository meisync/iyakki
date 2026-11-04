//! The listener: one thread per connection, TLS when an identity is given, a handful of routes, and the WebSocket that
//! carries the screen.

use crate::adapt::{Choice, Controller};
use crate::codec::{Caps, Level, Opts, Streamer};
use crate::http::{self, Parse, Request, Response};
use crate::input::{self, Input};
use crate::page;
use crate::screen::{Gate, LoginError};
use crate::session::{self, RateLimit, Sessions, COOKIE};
use crate::ws::{self, Message, Parser};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_CONNECTIONS: usize = 128;
const MAX_WS_MESSAGE: usize = iyakki_agent::browsermedia::MAX_CLIP_IMAGE + 64 * 1024;
const FIRST_BYTES_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Config {
    pub addr: String,
    pub tls: Option<Arc<rustls::ServerConfig>>,
}

struct Shared {
    gate: Arc<dyn Gate>,
    sessions: Sessions,
    logins: RateLimit,
    tls: Option<Arc<rustls::ServerConfig>>,
    open: AtomicUsize,
    stop: AtomicBool,
}

pub struct Server {
    shared: Arc<Shared>,
    addr: SocketAddr,
}

enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>),
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(s) => &s.sock,
        }
    }
}

impl Read for Conn {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(b),
            Conn::Tls(s) => s.read(b),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(b),
            Conn::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

impl Server {
    pub fn start(cfg: Config, gate: Arc<dyn Gate>) -> std::io::Result<Server> {
        let listener = TcpListener::bind(&cfg.addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared { gate, sessions: Sessions::new(), logins: RateLimit::new(10), tls: cfg.tls, open: AtomicUsize::new(0), stop: AtomicBool::new(false) });
        let s = shared.clone();
        std::thread::Builder::new().name("iyakki-web-accept".into()).spawn(move || accept_loop(listener, s))?;
        Ok(Server { shared, addr })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn sessions(&self) -> usize {
        self.shared.sessions.count()
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if shared.open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue; // drop it: refusing is cheaper than serving it
                }
                shared.open.fetch_add(1, Ordering::Relaxed);
                let s = shared.clone();
                let spawned = std::thread::Builder::new().name("iyakki-web-conn".into()).stack_size(8 * 1024 * 1024).spawn(move || {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(stream, peer, &s)));
                    if r.is_err() {
                        log::error!("web: a connection handler crashed");
                    }
                    s.open.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    shared.open.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn read_request(conn: &mut Conn) -> Result<Request, Option<Response>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let started = Instant::now();
    loop {
        match http::parse(&buf) {
            Parse::Done(r) => return Ok(r),
            Parse::Bad(code, msg) => return Err(Some(Response::error(code, msg))),
            Parse::Incomplete => {}
        }
        if started.elapsed() > FIRST_BYTES_TIMEOUT {
            return Err(None);
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn connection(stream: TcpStream, peer: SocketAddr, shared: &Shared) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(FIRST_BYTES_TIMEOUT));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut conn = match &shared.tls {
        Some(cfg) => match rustls::ServerConnection::new(cfg.clone()) {
            Ok(c) => Conn::Tls(Box::new(rustls::StreamOwned::new(c, stream))),
            Err(_) => return,
        },
        None => Conn::Plain(stream),
    };
    let req = match read_request(&mut conn) {
        Ok(r) => r,
        Err(Some(resp)) => {
            let _ = conn.write_all(&resp.to_bytes());
            return;
        }
        Err(None) => return,
    };
    if req.wants_websocket() {
        websocket(conn, &req, shared);
        return;
    }
    let resp = route(&req, peer, shared);
    let _ = conn.write_all(&resp.to_bytes());
    let _ = conn.flush();
    if let Conn::Tls(t) = &mut conn {
        t.conn.send_close_notify();
        let _ = t.flush();
    }
}

/// The page may only be driven from the address it was served from.
fn same_origin(req: &Request) -> bool {
    match (req.header("origin"), req.header("host")) {
        (None, _) => true, // not sent by browsers for plain same-site GETs
        (Some(o), Some(h)) => o.split_once("://").is_some_and(|(_, rest)| rest == h),
        (Some(_), None) => false,
    }
}

fn cookie_header(token: &str, secure: bool) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict{}", if secure { "; Secure" } else { "" })
}

fn route(req: &Request, peer: SocketAddr, sh: &Shared) -> Response {
    let secure = sh.tls.is_some();
    let session = req.cookie(COOKIE).and_then(|t| sh.sessions.find(t));
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => Response::html(page::INDEX),
        ("GET", "/decoder.js") => Response::new(200, "text/javascript; charset=utf-8", page::DECODER_JS),
        ("GET", "/media.js") => Response::new(200, "text/javascript; charset=utf-8", page::MEDIA_JS),
        ("GET", "/healthz") => Response::new(200, "text/plain", "ok"),
        ("GET", "/api/session") => match session {
            Some(s) => Response::json(200, &serde_json::json!({ "user": s.user, "csrf": s.csrf })),
            None => Response::error(401, "not signed in"),
        },
        ("POST", "/api/login") => {
            if !same_origin(req) || req.header("content-type").is_none_or(|c| !c.starts_with("application/json")) {
                return Response::error(403, "not allowed from here");
            }
            if !sh.logins.allow(&peer.ip().to_string()) {
                return Response::error(429, "too many tries, wait a minute");
            }
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&req.body) else { return Response::error(400, "bad request") };
            let (Some(user), Some(pass)) = (v.get("user").and_then(|x| x.as_str()), v.get("password").and_then(|x| x.as_str())) else {
                return Response::error(400, "name and password are needed");
            };
            match sh.gate.login(user, pass) {
                Ok(screen) => match sh.sessions.create(user.trim(), screen) {
                    Ok((token, csrf)) => Response::json(200, &serde_json::json!({ "user": user.trim(), "csrf": csrf })).header("Set-Cookie", &cookie_header(&token, secure)),
                    Err(e) => Response::error(503, &e),
                },
                Err(LoginError::Invalid) => Response::error(401, "wrong name or password"),
                Err(LoginError::Throttled(s)) => Response::error(429, &format!("too many wrong passwords, try again in {s} s")),
                Err(LoginError::Other(e)) => Response::error(503, &e),
            }
        }
        ("POST", "/api/logout") => {
            let Some(token) = req.cookie(COOKIE) else { return Response::error(401, "not signed in") };
            match session {
                Some(s) if same_origin(req) && req.header("x-csrf").is_some_and(|c| session::same(c, &s.csrf)) => {
                    sh.sessions.end(token);
                    Response::json(200, &serde_json::json!({ "ok": true })).header("Set-Cookie", &format!("{COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict"))
                }
                Some(_) => Response::error(403, "not allowed from here"),
                None => Response::error(401, "not signed in"),
            }
        }
        // the sign-in relay page (a login waiting for a code to be pasted from a phone): answered only to the local network
        ("GET" | "POST", p) if p.starts_with("/login/") => crate::relay::respond(req, peer.ip()).unwrap_or_else(|| Response::error(404, "not found")),
        ("GET" | "POST", _) => Response::error(404, "not found"),
        _ => Response::error(405, "method not allowed"),
    }
}

fn websocket(mut conn: Conn, req: &Request, sh: &Shared) {
    let refuse = |conn: &mut Conn, r: Response| {
        let _ = conn.write_all(&r.to_bytes());
    };
    if req.method != "GET" || !same_origin(req) {
        return refuse(&mut conn, Response::error(403, "not allowed from here"));
    }
    let Some(found) = req.cookie(COOKIE).and_then(|t| sh.sessions.find(t)) else {
        return refuse(&mut conn, Response::error(401, "not signed in"));
    };
    if !req.query.get("csrf").is_some_and(|c| session::same(c, &found.csrf)) {
        return refuse(&mut conn, Response::error(403, "not allowed from here"));
    }
    let Some(key) = req.header("sec-websocket-key") else { return refuse(&mut conn, Response::error(400, "no key")) };
    // one person's screen is watched from one place at a time
    let Ok(mut screen) = found.screen.try_lock() else {
        return refuse(&mut conn, Response::error(409, "this session is already open in another window"));
    };
    let hello = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept_key(key));
    if conn.write_all(hello.as_bytes()).is_err() {
        return;
    }
    let _ = conn.tcp().set_read_timeout(Some(Duration::from_millis(5)));
    let _ = conn.tcp().set_write_timeout(Some(Duration::from_secs(5)));
    screen.viewing(true);
    let media = screen.media();
    if let Some(m) = &media {
        m.connection(true);
    }
    let finished = serve_screen(&mut conn, &mut **screen, sh);
    // whatever the page supplied (microphone, camera, sound) ends with its connection
    if let Some(m) = &media {
        m.connection(false);
    }
    screen.viewing(false);
    if finished {
        drop(screen);
        if let Some(t) = req.cookie(COOKIE) {
            sh.sessions.end(t);
        }
    }
}

/// Protocol version 2 state of one connection (present once the page said `hello`).
struct V2 {
    ctl: Controller,
    st: Streamer,
    caps: Caps,
    armed: bool,
    last_stats: u64,
    last_level: Level,
    last_pong: u64,
}

/// What the server speaks: the page's `hello` may ask for less, never for more.
const PROTOCOL_VERSION: u32 = 2;

/// Carry one person's screen over the connection until it closes. True when the person signed out.
fn serve_screen(conn: &mut Conn, screen: &mut dyn crate::screen::Screen, sh: &Shared) -> bool {
    let mut parser = Parser::new(MAX_WS_MESSAGE);
    let mut buf = vec![0u8; 16 * 1024];
    let t0 = Instant::now();
    let mut v2: Option<V2> = None;
    let media = screen.media();
    // everything one pass of the loop wants to say goes out in one write (small messages are batched, one packet)
    let mut outbox: Vec<u8> = Vec::new();
    loop {
        if sh.stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = t0.elapsed().as_millis() as u64;
        match conn.read(&mut buf) {
            Ok(0) => return false,
            Ok(n) => parser.push(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => return false,
        }
        loop {
            match parser.next() {
                Ok(Some(Message::Text(t))) => {
                    let Some(i) = input::parse(&t) else { continue };
                    match i {
                        Input::Hello { v, deflate } => {
                            let version = v.min(PROTOCOL_VERSION);
                            if version >= 2 && v2.is_none() {
                                let ctl = Controller::new(now);
                                let level = ctl.level();
                                v2 = Some(V2 { ctl, st: Streamer::new(), caps: Caps { deflate }, armed: false, last_stats: now, last_level: level, last_pong: 0 });
                                screen.set_stream(true);
                            }
                            let m = serde_json::json!({ "t": "hello_ok", "v": if v2.is_some() { version } else { 1 }, "tile": crate::codec::TILE, "levels": Level::ALL.map(|l| l.name()), "max_message": MAX_WS_MESSAGE });
                            outbox.extend(ws::text_frame(&m.to_string()));
                        }
                        Input::Ack(n) => {
                            if let Some(v) = v2.as_mut() {
                                v.ctl.on_ack(n, now);
                            }
                        }
                        Input::Quality { level, saver } => {
                            if let Some(v) = v2.as_mut() {
                                v.ctl.set_choice(level.map_or(Choice::Auto, Choice::Fixed), saver);
                            }
                        }
                        Input::Ping { ts, rtt } => {
                            // at most about twenty answers a second: a flood of pings costs the server little
                            if now.saturating_sub(v2.as_ref().map_or(0, |v| v.last_pong)) >= 50 || v2.is_none() {
                                if let Some(v) = v2.as_mut() {
                                    v.last_pong = now;
                                    if let Some(r) = rtt {
                                        v.ctl.on_rtt(r);
                                    }
                                }
                                outbox.extend(ws::text_frame(&format!(r#"{{"t":"pong","ts":{ts}}}"#)));
                            }
                        }
                        Input::Media { kind, on } => {
                            if let Some(m) = &media {
                                if m.set(kind, on).is_err() {
                                    outbox.extend(ws::text_frame(&iyakki_agent::browsermedia::BrowserMedia::denial_json(kind)));
                                }
                            }
                        }
                        Input::AudioOut(on) => {
                            if let Some(m) = &media {
                                m.set_audio_out(on);
                            }
                        }
                        Input::Rich { .. } => {}
                        Input::Full => {
                            if let Some(v) = v2.as_mut() {
                                v.st.reset();
                            }
                            screen.input(Input::Full);
                        }
                        other => {
                            if let (Some(v), Input::Event(e)) = (v2.as_mut(), &other) {
                                use iyakki_shell::ui::Event;
                                if matches!(e, Event::Key(_) | Event::MouseDown(..) | Event::MouseUp(..) | Event::MouseRight(..) | Event::Scroll(_) | Event::Paste(_)) {
                                    v.ctl.on_input();
                                }
                            }
                            screen.input(other);
                        }
                    }
                }
                Ok(Some(Message::Ping(p))) => outbox.extend(ws::pong_frame(&p)),
                Ok(Some(Message::Close)) => {
                    let _ = flush(conn, &mut outbox);
                    let _ = send(conn, ws::close_frame());
                    return false;
                }
                Ok(Some(Message::Binary(b))) => {
                    // the page's microphone, camera and clipboard items; the media state checks, limits and audits them
                    use iyakki_agent::browsermedia::Accepted;
                    if let Some(m) = &media {
                        if let Accepted::Clip(kind) = m.binary(&b) {
                            screen.input(Input::Rich { kind, bytes: b[1..].to_vec() });
                        }
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    let _ = send(conn, ws::close_frame());
                    return false;
                }
            }
        }
        // the picture: PNG pieces for a page that did not say hello, otherwise tiles of the difference
        let legacy = screen.step();
        match v2.as_mut() {
            None => {
                if let Some(frame) = legacy {
                    outbox.extend(ws::binary_frame(&frame.to_message()));
                }
            }
            Some(v) => {
                v.ctl.tick(now);
                if let Some(p) = screen.take_pixels() {
                    v.armed = false;
                    v.st.set_source(p.w as usize, p.h as usize, p.rgba);
                }
                if v.ctl.can_send(now) {
                    if v.st.wants_encode(now) {
                        let o = Opts { level: v.ctl.level(), caps: v.caps, budget: v.ctl.budget(), now_ms: now };
                        if let Some(e) = v.st.encode(&o) {
                            v.ctl.on_send(e.seq, e.msg.len(), now);
                            outbox.extend(ws::binary_frame(&e.msg));
                        }
                    }
                }
                if !v.armed && v.ctl.can_send(now) {
                    v.armed = true;
                    screen.arm();
                }
                let level = v.ctl.level();
                if level != v.last_level || now.saturating_sub(v.last_stats) >= 1000 {
                    v.last_level = level;
                    v.last_stats = now;
                    outbox.extend(ws::text_frame(&v.ctl.stats_json().to_string()));
                }
            }
        }
        if let Some(c) = screen.take_cursor() {
            outbox.extend(ws::text_frame(&serde_json::json!({ "t": "cursor", "c": c }).to_string()));
        }
        for m in screen.take_speech() {
            outbox.extend(ws::text_frame(&m));
        }
        if let Some(m) = &media {
            // (state messages go to a page that said hello; an older page knows nothing of them)
            if v2.is_some() {
                for t in m.take_messages() {
                    outbox.extend(ws::text_frame(&t));
                }
            }
            // sound and clipboard items are binary messages that only a version 2 page understands (it has said hello)
            if v2.is_some() {
                for a in m.take_audio() {
                    outbox.extend(ws::binary_frame(&a));
                }
                if let Some(r) = screen.take_rich_copy() {
                    outbox.extend(ws::binary_frame(&r));
                }
            }
        }
        if screen.take_clip_request() {
            outbox.extend(ws::text_frame(r#"{"t":"readclip"}"#));
        }
        if let Some(text) = screen.take_copy() {
            outbox.extend(ws::text_frame(&serde_json::json!({ "t": "copy", "s": text }).to_string()));
        }
        let done = screen.finished();
        if done {
            outbox.extend(ws::text_frame(r#"{"t":"bye"}"#));
            outbox.extend(ws::close_frame());
        }
        if !flush(conn, &mut outbox) {
            return false;
        }
        if done {
            return true;
        }
    }
}

fn send(conn: &mut Conn, bytes: Vec<u8>) -> bool {
    conn.write_all(&bytes).and_then(|_| conn.flush()).is_ok()
}

/// Write everything waiting in one go.
fn flush(conn: &mut Conn, outbox: &mut Vec<u8>) -> bool {
    if outbox.is_empty() {
        return true;
    }
    let r = conn.write_all(outbox).and_then(|_| conn.flush()).is_ok();
    outbox.clear();
    r
}

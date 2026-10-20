//! The listener: one thread per connection, TLS when an identity is given, a handful of routes, and the WebSocket that
//! carries the screen.

use crate::http::{self, Parse, Request, Response};
use crate::input;
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
const MAX_WS_MESSAGE: usize = 512 * 1024;
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
    let finished = serve_screen(&mut conn, &mut **screen, sh);
    screen.viewing(false);
    if finished {
        drop(screen);
        if let Some(t) = req.cookie(COOKIE) {
            sh.sessions.end(t);
        }
    }
}

/// Carry one person's screen over the connection until it closes. True when the person signed out.
fn serve_screen(conn: &mut Conn, screen: &mut dyn crate::screen::Screen, sh: &Shared) -> bool {
    let mut parser = Parser::new(MAX_WS_MESSAGE);
    let mut buf = vec![0u8; 16 * 1024];
    let send = |conn: &mut Conn, bytes: Vec<u8>| conn.write_all(&bytes).and_then(|_| conn.flush()).is_ok();
    loop {
        if sh.stop.load(Ordering::Relaxed) {
            return false;
        }
        match conn.read(&mut buf) {
            Ok(0) => return false,
            Ok(n) => parser.push(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => return false,
        }
        loop {
            match parser.next() {
                Ok(Some(Message::Text(t))) => {
                    if let Some(i) = input::parse(&t) {
                        screen.input(i);
                    }
                }
                Ok(Some(Message::Ping(p))) => {
                    if !send(conn, ws::pong_frame(&p)) {
                        return false;
                    }
                }
                Ok(Some(Message::Close)) => {
                    let _ = send(conn, ws::close_frame());
                    return false;
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    let _ = send(conn, ws::close_frame());
                    return false;
                }
            }
        }
        if let Some(frame) = screen.step() {
            if !send(conn, ws::binary_frame(&frame.to_message())) {
                return false;
            }
        }
        if let Some(c) = screen.take_cursor() {
            let m = serde_json::json!({ "t": "cursor", "c": c }).to_string();
            if !send(conn, ws::text_frame(&m)) {
                return false;
            }
        }
        for m in screen.take_speech() {
            if !send(conn, ws::text_frame(&m)) {
                return false;
            }
        }
        if screen.take_clip_request() {
            if !send(conn, ws::text_frame(r#"{"t":"readclip"}"#)) {
                return false;
            }
        }
        if let Some(text) = screen.take_copy() {
            let m = serde_json::json!({ "t": "copy", "s": text }).to_string();
            if !send(conn, ws::text_frame(&m)) {
                return false;
            }
        }
        if screen.finished() {
            let _ = send(conn, ws::text_frame(r#"{"t":"bye"}"#));
            let _ = send(conn, ws::close_frame());
            return true;
        }
    }
}

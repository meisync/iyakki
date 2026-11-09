//! The listener: HTTPS only (a plain connection never gets an answer), one thread per connection with a cap, request size and
//! time limits, then the table-driven pipeline: route -> origin -> sign-in -> role -> rate limit -> handler -> audit.
//! Shutdown is graceful: stop accepting, let running requests finish for a grace period, stop the runs, join.

use super::auth::{Limiter, Tokens};
use super::config::Checked;
use super::deps::Deps;
use super::handlers;
use super::routes::{self, Access, Found, Need, Route};
use super::runs::RunManager;
use super::tls::Identity;
use super::Profile;
use crate::httpmini::{parse_limited, Parse, Request, Response};
use iyakki_core_api::{ApiError, Principal};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FIRST_BYTES_TIMEOUT: Duration = Duration::from_secs(10);

/// A refusal with its HTTP status and a stable code.
pub struct Fail {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl Fail {
    pub fn new(status: u16, code: &'static str, message: &str) -> Fail {
        Fail { status, code, message: message.into() }
    }
}

impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Fail {
        match e {
            ApiError::NotFound(m) => Fail { status: 404, code: "not_found", message: m },
            ApiError::Denied(m) => Fail { status: 403, code: "denied", message: m },
            ApiError::Unsupported(m) => Fail { status: 501, code: "unsupported", message: m },
            ApiError::InvalidInput(m) => Fail { status: 400, code: "invalid_input", message: m },
            ApiError::Busy(m) => Fail { status: 429, code: "busy", message: m },
            ApiError::Io(m) => {
                // what failed inside is for the log, not for the client
                log::warn!("service: io error: {m}");
                Fail { status: 500, code: "io", message: "internal error".into() }
            }
        }
    }
}

pub struct Shared {
    pub profile: Profile,
    pub cfg: super::config::ServiceConfig,
    pub deps: Arc<Deps>,
    pub tokens: Tokens,
    pub runs: RunManager,
    /// WASM tiles run for browsers (`tiles.rs`).
    pub tiles: super::tiles::TileHost,
    pub allowed_tools: BTreeSet<String>,
    pub fingerprint: String,
    pub(super) addr_limit: Limiter,
    login_limit: Limiter,
    pub(super) user_limit: Limiter,
    /// Tenant switches per person (a switch locks and clears things, so it is limited more tightly than other calls).
    pub(super) switch_limit: Limiter,
    /// `None` for the embedded service (`embed.rs`): it has no listener of its own.
    tls: Option<Arc<rustls::ServerConfig>>,
    open: AtomicUsize,
    stop: AtomicBool,
}

enum Conn {
    Tls(Box<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>),
}

impl Read for Conn {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tls(s) => s.read(b),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Tls(s) => s.flush(),
        }
    }
}

/// A running service. Dropping it stops it (without a grace period); [`Service::shutdown`] is the graceful way.
pub struct Service {
    shared: Arc<Shared>,
    addr: SocketAddr,
    accept: Mutex<Option<std::thread::JoinHandle<()>>>,
}

pub fn start(chk: &Checked, deps: Arc<Deps>, tls: Identity) -> Result<Service, String> {
    let cfg = chk.cfg.clone();
    let allowed_tools = super::tools::allowed_set(chk.profile, &cfg.enable_tools, &[])?;
    let listener = crate::sharedsite::bind_retry(chk.addr).map_err(|e| format!("cannot listen on {}: {e}", chk.addr))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let l = cfg.limits.clone();
    let store = if cfg.token_store.is_empty() { None } else { Some(std::path::PathBuf::from(&cfg.token_store)) };
    let shared = Arc::new(Shared {
        profile: chk.profile,
        deps: deps.clone(),
        tokens: Tokens::new(store, Duration::from_secs(l.token_idle_secs), l.token_lifetime_secs),
        runs: RunManager::new(deps, Duration::from_secs(l.approval_timeout_secs)),
        tiles: super::tiles::TileHost::new(super::tiles::builtin()),
        allowed_tools,
        fingerprint: tls.fingerprint.clone(),
        addr_limit: Limiter::new(600),
        login_limit: Limiter::new(l.logins_per_minute),
        user_limit: Limiter::new(l.requests_per_minute),
        switch_limit: Limiter::new(10),
        tls: Some(tls.config),
        open: AtomicUsize::new(0),
        stop: AtomicBool::new(false),
        cfg,
    });
    let s = shared.clone();
    let h = std::thread::Builder::new().name("iyakki-svc-accept".into()).spawn(move || accept_loop(listener, s)).map_err(|e| e.to_string())?;
    log::info!("service: profile {} on https://{addr} (certificate SHA-256 {})", chk.profile.name(), shared.fingerprint);
    Ok(Service { shared, addr, accept: Mutex::new(Some(h)) })
}

impl Shared {
    /// The pipeline without a listener, for a server that already exists (the desktop's browser server, `embed.rs`). Same limits,
    /// same tokens, same rate limits; the caller's own listener does the connections.
    pub(super) fn embedded(profile: Profile, cfg: super::config::ServiceConfig, deps: Arc<Deps>) -> Result<Shared, String> {
        let allowed_tools = super::tools::allowed_set(profile, &cfg.enable_tools, &[])?;
        let l = cfg.limits.clone();
        Ok(Shared {
            profile,
            deps: deps.clone(),
            tokens: Tokens::new(None, Duration::from_secs(l.token_idle_secs), l.token_lifetime_secs),
            runs: RunManager::new(deps, Duration::from_secs(l.approval_timeout_secs)),
            tiles: super::tiles::TileHost::new(super::tiles::builtin()),
            allowed_tools,
            fingerprint: String::new(),
            addr_limit: Limiter::new(600),
            login_limit: Limiter::new(l.logins_per_minute),
            user_limit: Limiter::new(l.requests_per_minute),
            switch_limit: Limiter::new(10),
            tls: None,
            open: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            cfg,
        })
    }
}

impl Service {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
    pub fn fingerprint(&self) -> &str {
        &self.shared.fingerprint
    }
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Stop accepting, wait up to `grace` for requests in flight, stop the runs. Safe to call twice.
    pub fn shutdown(&self, grace: Duration) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.accept.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join(); // the listener is dropped when the loop ends: nothing new connects
        }
        let t = Instant::now();
        while self.shared.open.load(Ordering::SeqCst) > 0 && t.elapsed() < grace {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.shared.runs.stop_all();
        log::info!("service: stopped ({} connections still open)", self.shared.open.load(Ordering::SeqCst));
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if shared.open.load(Ordering::SeqCst) >= shared.cfg.limits.max_connections {
                    continue; // refusing is cheaper than serving
                }
                shared.open.fetch_add(1, Ordering::SeqCst);
                let s = shared.clone();
                let spawned = std::thread::Builder::new().name("iyakki-svc-conn".into()).spawn(move || {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(stream, peer, &s)));
                    if r.is_err() {
                        log::error!("service: a connection handler crashed");
                    }
                    s.open.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    shared.open.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn error_response(status: u16, code: &str, message: &str) -> Response {
    Response::json(status, &json!({"error": {"code": code, "message": message}}))
}

/// Read until the request head is complete (or too long, or the peer is slow or gone). The bytes read so far are returned, body bytes
/// that came with the head included.
fn read_head(conn: &mut Conn) -> Result<Vec<u8>, Option<Response>> {
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
    let started = Instant::now();
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > crate::httpmini::MAX_HEAD {
            return Err(Some(error_response(431, "invalid_input", "request headers too large")));
        }
        if started.elapsed() > FIRST_BYTES_TIMEOUT {
            return Err(None);
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    Ok(buf)
}

fn read_request(conn: &mut Conn, max_body: usize, mut buf: Vec<u8>) -> Result<Request, Option<Response>> {
    let mut chunk = [0u8; 4096];
    let started = Instant::now();
    loop {
        match parse_limited(&buf, max_body) {
            Parse::Done(r) => return Ok(r),
            Parse::Bad(code, msg) => return Err(Some(error_response(code, if code == 413 { "too_large" } else { "invalid_input" }, msg))),
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
    let Some(tls) = shared.tls.clone() else { return };
    let Ok(c) = rustls::ServerConnection::new(tls) else { return };
    let mut conn = Conn::Tls(Box::new(rustls::StreamOwned::new(c, stream)));
    match read_head(&mut conn) {
        Err(None) => return,
        Err(Some(r)) => {
            let _ = conn.write_all(&r.header("Strict-Transport-Security", "max-age=31536000").to_bytes());
            let _ = conn.flush();
        }
        // the live event stream: a WebSocket that signs in by its first message (events.rs)
        Ok(head) if super::events::is_events_head(&head) && super::events::serve(shared, &mut conn, &head, peer.ip()) => {}
        // binary uploads and downloads are streamed, never held whole in memory (files.rs); anything it does not take goes the ordinary way
        Ok(head) if super::files::is_stream_head(&head) && super::files::serve_stream(shared, &mut conn, &head, peer.ip()) => {}
        Ok(head) => {
            let resp = match read_request(&mut conn, shared.cfg.limits.max_body_bytes, head) {
                Ok(req) => handle(shared, &req, peer.ip()),
                Err(Some(r)) => r,
                Err(None) => return,
            };
            let _ = conn.write_all(&resp.header("Strict-Transport-Security", "max-age=31536000").to_bytes());
            let _ = conn.flush();
        }
    }
    let Conn::Tls(t) = &mut conn;
    t.conn.send_close_notify();
    let _ = t.flush();
}

impl super::events::WsIo for Conn {
    fn set_read_timeout(&mut self, d: Duration) {
        let Conn::Tls(s) = self;
        let _ = s.sock.set_read_timeout(Some(d));
    }
}

/// May this request's `Origin` open a socket (the same rule as every request)?
pub(super) fn origin_allowed(sh: &Shared, req: &Request) -> bool {
    origin_ok(sh, req).is_ok()
}

/// Sign-in for a route that does not go through [`handle`] (the streamed file routes): the same checks in the same order, origin,
/// address limit, token, account, role, per-person limit. `Err` is the finished answer. `Ok` carries the person and the origin to echo.
pub(super) fn authenticate_user(sh: &Shared, req: &Request, peer: IpAddr, route: &Route) -> Result<(Principal, Option<String>), Response> {
    let allow_origin = origin_ok(sh, req).map_err(|()| error_response(403, "denied", "origin not allowed"))?;
    if !sh.addr_limit.allow(&peer.to_string()).0 {
        return Err(decorate(error_response(429, "rate_limited", "too many requests from this address").header("Retry-After", "30"), &allow_origin));
    }
    let tok = bearer(req).ok_or_else(|| unauthorized(&allow_origin))?;
    let user = sh.tokens.find(tok).ok_or_else(|| unauthorized(&allow_origin))?;
    let Some(p) = sh.deps.auth.lookup(&user) else {
        sh.tokens.revoke_user(&user);
        return Err(unauthorized(&allow_origin));
    };
    if route.access == Access::Admin && !p.admin {
        return Err(decorate(error_response(403, "denied", "administrators only"), &allow_origin));
    }
    if !need_ok(&p, route.needs) {
        return Err(decorate(error_response(403, "denied", "your role does not allow this"), &allow_origin));
    }
    let (ok, wait) = sh.user_limit.allow(&p.user);
    if !ok {
        return Err(decorate(error_response(429, "rate_limited", "too many requests").header("Retry-After", &wait.to_string()), &allow_origin));
    }
    Ok((p, allow_origin))
}

/// The same finishing touch [`handle`] gives every answer.
pub(super) fn with_origin(r: Response, origin: &Option<String>) -> Response {
    decorate(r, origin)
}

pub(super) fn error_answer(status: u16, code: &str, message: &str) -> Response {
    error_response(status, code, message)
}

pub(super) fn bearer(req: &Request) -> Option<&str> {
    let v = req.header("authorization")?;
    let (scheme, tok) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(tok.trim())
}

/// `Origin` is allowed when listed by the administrator or when it is the service's own address.
fn origin_ok(sh: &Shared, req: &Request) -> Result<Option<String>, ()> {
    let Some(o) = req.header("origin") else { return Ok(None) };
    if sh.cfg.cors_origins.iter().any(|x| x == o) {
        return Ok(Some(o.to_string()));
    }
    match req.header("host") {
        Some(h) if o.strip_prefix("https://") == Some(h) => Ok(None),
        _ => Err(()),
    }
}

fn need_ok(p: &Principal, n: Need) -> bool {
    use iyakki_sec::policy::App;
    match n {
        Need::Nothing => true,
        Need::Chat => p.policy.agent && p.policy.apps.contains(&App::Chat),
        Need::Flows => p.policy.apps.contains(&App::Flows),
        Need::Data => p.policy.apps.contains(&App::Data),
    }
}

fn audit_clean(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(120).collect()
}

/// The whole pipeline for one request.
pub fn handle(sh: &Shared, req: &Request, peer: IpAddr) -> Response {
    let t0 = Instant::now();
    let ip = peer.to_string();
    let allow_origin = match origin_ok(sh, req) {
        Ok(o) => o,
        Err(()) => return error_response(403, "denied", "origin not allowed"),
    };
    if req.method == "OPTIONS" {
        // only for a listed origin; everything else about OPTIONS does not exist
        return match allow_origin {
            Some(o) if req.header("access-control-request-method").is_some() => Response::new(204, "text/plain", Vec::new())
                .header("Access-Control-Allow-Origin", &o)
                .header("Access-Control-Allow-Methods", "GET, POST")
                .header("Access-Control-Allow-Headers", "Authorization, Content-Type")
                .header("Access-Control-Max-Age", "600")
                .header("Vary", "Origin"),
            _ => error_response(404, "not_found", "not found"),
        };
    }
    if !sh.addr_limit.allow(&ip).0 {
        return error_response(429, "rate_limited", "too many requests from this address").header("Retry-After", "30");
    }
    if super::hooks::is_hook_path(&req.path) {
        // webhooks: no bearer token, the secret address and the optional signature are the sender's proof (hooks.rs)
        return super::hooks::serve(sh, req, peer);
    }
    if super::ui::is_ui_path(&req.path) {
        // the web UI's static files: no sign-in, no API route (see `ui.rs`)
        return super::ui::serve(req).unwrap_or_else(|| error_response(404, "not_found", "not found"));
    }
    let (route, params): (&Route, Vec<String>) = match routes::find(sh.profile, &req.method, &req.path) {
        Found::Route(r, p) => (r, p),
        Found::WrongMethod => return decorate(error_response(405, "invalid_input", "method not allowed"), &allow_origin),
        Found::Nothing => return decorate(error_response(404, "not_found", "not found"), &allow_origin),
    };
    // sign-in
    let mut principal: Option<Principal> = None;
    let mut token = None;
    if route.access != Access::Public {
        let Some(tok) = bearer(req) else { return unauthorized(&allow_origin) };
        let Some(user) = sh.tokens.find(tok) else { return unauthorized(&allow_origin) };
        let Some(p) = sh.deps.auth.lookup(&user) else {
            sh.tokens.revoke_user(&user); // the account is gone: every token of it ends
            return unauthorized(&allow_origin);
        };
        if route.access == Access::Admin && !p.admin {
            return decorate(error_response(403, "denied", "administrators only"), &allow_origin);
        }
        if !need_ok(&p, route.needs) {
            return decorate(error_response(403, "denied", "your role does not allow this"), &allow_origin);
        }
        // the tile events route has its own, higher limit (tiles.rs): a pointer-driven page sends many small requests
        let (ok, wait) = if route.id == "tile_events" { (true, 0) } else { sh.user_limit.allow(&p.user) };
        if !ok {
            return decorate(error_response(429, "rate_limited", "too many requests").header("Retry-After", &wait.to_string()), &allow_origin);
        }
        token = Some(tok.to_string());
        principal = Some(p);
    } else if route.id == "login" {
        let (ok, wait) = sh.login_limit.allow(&ip);
        if !ok {
            return decorate(error_response(429, "rate_limited", "too many sign-in tries").header("Retry-After", &wait.to_string()), &allow_origin);
        }
    }
    if !req.body.is_empty() && route.id != "files_upload" && req.header("content-type").is_none_or(|c| !c.to_ascii_lowercase().starts_with("application/json")) {
        return decorate(error_response(400, "invalid_input", "send application/json"), &allow_origin);
    }
    let ctx = handlers::Ctx { sh, req, params: &params, principal: principal.as_ref(), token: token.as_deref(), ip: &ip };
    let (resp, detail) = match handlers::dispatch(route.id, &ctx) {
        Ok((status, v, detail)) => (Response::json(status, &v), detail),
        Err(f) => (error_response(f.status, f.code, &f.message), format!("refused:{}", f.code)),
    };
    let status = resp.status;
    if route.audit && route.id != "login" {
        let user = principal.as_ref().map(|p| p.user.as_str()).unwrap_or("-");
        sh.deps.audit.record(user, &format!("api.{}", route.id), &audit_clean(&format!("{detail} status={status} addr={ip}")));
    }
    log::info!("service: {} {} {} {}ms user={}", req.method, route.id, status, t0.elapsed().as_millis(), principal.as_ref().map(|p| p.user.as_str()).unwrap_or("-"));
    decorate(resp, &allow_origin)
}

fn unauthorized(origin: &Option<String>) -> Response {
    decorate(error_response(401, "unauthorized", "sign in first (POST /api/v1/login) and send the token as a Bearer header").header("WWW-Authenticate", "Bearer"), origin)
}

fn decorate(r: Response, origin: &Option<String>) -> Response {
    match origin {
        Some(o) => r.header("Access-Control-Allow-Origin", o).header("Vary", "Origin"),
        None => r,
    }
}

pub(super) fn json_body(req: &Request) -> Result<Value, Fail> {
    serde_json::from_slice(&req.body).map_err(|_| Fail::new(400, "invalid_input", "the body is not valid JSON"))
}

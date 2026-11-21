//! The machine's end of the transport, as a reference: it serves a contract implementation (the fake, or the real core later) to a client
//! on a stream. It decides who the caller is: the [`CallContext`] is fixed per listener (the transport kind and the scopes the pairing
//! granted), never read from the request. It paces the connection with the contract's rate limit.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::*;
use iyakki_core_api::contract::{RateLimit, RateLimiter};

pub struct Served {
    pub hello: Hello,
    /// A domain that is `None` is not served: its calls are answered `unsupported`.
    pub settings: Option<Arc<dyn Settings>>,
    pub vault: Option<Arc<dyn Vault>>,
    pub flows: Option<Arc<dyn Flows>>,
    /// The embedded assistant and camera views of the core (`facets`): the person is `ctx`'s, the scopes are `assistant:use` and `cameras:view`.
    pub assistant: Option<Arc<dyn iyakki_core_api::Assistant>>,
    pub cameras: Option<Arc<dyn iyakki_core_api::Cameras>>,
    pub appevents: Option<Arc<dyn AppEventFeed>>,
    /// The object calls of the UI program (`rpc.call`, `rpc.release`, F-SPLIT-03): `None` answers them `unsupported`.
    pub rpc: Option<Arc<dyn RpcServe>>,
    /// Who every caller on this listener is.
    pub ctx: CallContext,
}

/// The machine's end of the object calls (`rpc.call`, `rpc.release`): the crate `iyakki-ui-rpc` implements it over the agent's facets, so this
/// crate stays free of the agent. `obj` 0 is the root object; the others were handed out by an earlier answer of the same connection.
pub trait RpcServe: Send + Sync {
    fn call(&self, ctx: &CallContext, conn: u64, obj: u64, iface: &str, method: &str, args: Value) -> CallResult<Value>;
    /// The UI dropped its end of an object; the connection is gone when `obj` is `None` (drop everything of it).
    fn release(&self, conn: u64, obj: Option<u64>);
}

static NEXT_CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn invalid(why: &str) -> CallError {
    CallError::new(ErrorCode::InvalidInput, "transport.bad_request", why)
}

fn dispatch(s: &Served, req: &Request, conn: u64) -> CallResult<Value> {
    let key = || req.args.get("key").and_then(Value::as_str).ok_or_else(|| invalid("`key` is a text"));
    let to = |v: &dyn erased::Ser| v.to_value();
    match req.call.as_str() {
        "hello" => to(&s.hello),
        "settings.schema" => to(&settings(s)?.schema(&s.ctx)?),
        "settings.get" => to(&settings(s)?.get(&s.ctx, key()?)?),
        "settings.set" => {
            let value: SettingValue = serde_json::from_value(req.args.get("value").cloned().unwrap_or(Value::Null)).map_err(|e| invalid(&format!("`value` is not a setting value: {e}")))?;
            let idem = match req.args.get("idempotency_key") {
                None | Some(Value::Null) => None,
                Some(Value::String(k)) => Some(IdempotencyKey::new(k)?),
                Some(_) => return Err(invalid("`idempotency_key` is a text")),
            };
            to(&settings(s)?.set(&s.ctx, key()?, value, idem.as_ref())?)
        }
        "settings.watch" => {
            let resume: Resume = serde_json::from_value(req.args.get("resume").cloned().unwrap_or(json!("latest"))).map_err(|e| invalid(&format!("`resume` is wrong: {e}")))?;
            let limit = req.args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
            let k = req.args.get("key").and_then(Value::as_str);
            to(&settings(s)?.watch(&s.ctx, k, resume, limit)?)
        }
        "vault.status" => to(&vault(s)?.status(&s.ctx)?),
        "vault.lock" => to(&vault(s)?.lock(&s.ctx)?),
        "vault.use" => {
            let action = req.args.get("action").and_then(Value::as_str).ok_or_else(|| invalid("`action` is a text"))?;
            to(&vault(s)?.use_secret(&s.ctx, &NamedAction(action.to_string()), idem_of(&req.args)?.as_ref())?)
        }
        "flows.list" => to(&flows(s)?.list(&s.ctx)?),
        "flows.start" => {
            let flow = req.args.get("flow").and_then(Value::as_str).ok_or_else(|| invalid("`flow` is a text"))?;
            let idem = idem_of(&req.args)?.ok_or_else(|| invalid("`idempotency_key` is needed to start a flow"))?;
            to(&flows(s)?.start(&s.ctx, flow, &idem)?)
        }
        "flows.stop" => to(&flows(s)?.stop(&s.ctx, &run_of(&req.args)?)?),
        "flows.status" => to(&flows(s)?.status(&s.ctx, &run_of(&req.args)?)?),
        "flows.events" => {
            let resume: Resume = serde_json::from_value(req.args.get("resume").cloned().unwrap_or(json!("latest"))).map_err(|e| invalid(&format!("`resume` is wrong: {e}")))?;
            let limit = req.args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
            let run = match req.args.get("run") {
                None | Some(Value::Null) => None,
                Some(_) => Some(run_of(&req.args)?),
            };
            to(&flows(s)?.events(&s.ctx, run.as_ref(), resume, limit)?)
        }
        "appevents.register" => {
            let event = req.args.get("event").and_then(Value::as_str).ok_or_else(|| invalid("`event` is a text"))?;
            let filter = req.args.get("filter").and_then(Value::as_str).unwrap_or("");
            appevents(s)?.register(&s.ctx, event, filter)?;
            to(&json!({}))
        }
        "appevents.events" => {
            let resume: Resume = serde_json::from_value(req.args.get("resume").cloned().unwrap_or(json!("latest"))).map_err(|e| invalid(&format!("`resume` is wrong: {e}")))?;
            let limit = req.args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
            to(&appevents(s)?.events(&s.ctx, resume, limit)?)
        }
        "flows.approve" => {
            let id = req.args.get("approval_id").and_then(Value::as_str).ok_or_else(|| invalid("`approval_id` is a text"))?;
            let allow = req.args.get("allow").and_then(Value::as_bool).ok_or_else(|| invalid("`allow` is true or false"))?;
            to(&flows(s)?.approve(&s.ctx, &run_of(&req.args)?, id, allow)?)
        }
        "rpc.call" | "rpc.release" => rpc_dispatch(s, req, conn),
        other => Err(CallError::new(ErrorCode::Unsupported, "transport.unknown_call", &format!("This machine does not know the call {other}.")).hint("Read `hello` for the calls it supports.")),
    }
}

fn rpc_dispatch(s: &Served, req: &Request, conn: u64) -> CallResult<Value> {
    let r = s.rpc.as_ref().ok_or_else(|| missing("rpc"))?;
    let obj = req.args.get("obj").and_then(Value::as_u64).ok_or_else(|| invalid("`obj` is a number"))?;
    if req.call == "rpc.release" {
        r.release(conn, Some(obj));
        return Ok(Value::Null);
    }
    let text = |k: &str| req.args.get(k).and_then(Value::as_str).ok_or_else(|| invalid(&format!("`{k}` is a text")));
    r.call(&s.ctx, conn, obj, text("iface")?, text("method")?, req.args.get("args").cloned().unwrap_or(Value::Null))
}

fn missing(domain: &str) -> CallError {
    CallError::new(ErrorCode::Unsupported, "transport.no_domain", &format!("This machine does not serve {domain}.")).hint("Read `hello` for the domains it serves.")
}
fn settings(s: &Served) -> CallResult<&Arc<dyn Settings>> {
    s.settings.as_ref().ok_or_else(|| missing("settings"))
}
fn vault(s: &Served) -> CallResult<&Arc<dyn Vault>> {
    s.vault.as_ref().ok_or_else(|| missing("vault"))
}
fn flows(s: &Served) -> CallResult<&Arc<dyn Flows>> {
    s.flows.as_ref().ok_or_else(|| missing("flows"))
}
fn appevents(s: &Served) -> CallResult<&Arc<dyn AppEventFeed>> {
    s.appevents.as_ref().ok_or_else(|| missing("appevents"))
}
fn idem_of(a: &Value) -> CallResult<Option<IdempotencyKey>> {
    match a.get("idempotency_key") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(k)) => IdempotencyKey::new(k).map(Some),
        Some(_) => Err(invalid("`idempotency_key` is a text")),
    }
}
fn run_of(a: &Value) -> CallResult<RunId> {
    a.get("run").and_then(Value::as_str).map(|r| RunId(r.to_string())).ok_or_else(|| invalid("`run` is a text"))
}

mod erased {
    use serde::Serialize;
    use serde_json::Value;
    pub trait Ser {
        fn to_value(&self) -> Result<Value, crate::CallError>;
    }
    impl<T: Serialize> Ser for T {
        fn to_value(&self) -> Result<Value, crate::CallError> {
            serde_json::to_value(self).map_err(|e| crate::CallError::new(crate::ErrorCode::Internal, "transport.encode", &e.to_string()))
        }
    }
}

/// Serve one connection until it closes, a line is too long, or too many calls were made.
pub fn serve_connection(s: &Served, input: impl Read, out: impl Write + Send) {
    serve_connection_x(s, input, out, None);
}

/// Where the bytes of a bulk block come from (`bulk.get`, F-SPLIT-07): a display list, an audio block, a camera frame. A caller of the unix
/// socket gets them as a sealed memory file handed over beside the answer line, never inside it.
pub trait BulkSource: Send + Sync {
    fn get(&self, ctx: &CallContext, name: &str) -> CallResult<Vec<u8>>;
}

/// [`serve_connection`] that can also answer `bulk.get` by handing a memory file over `stream` (the same unix socket as `input` and `out`).
#[cfg(unix)]
pub fn serve_connection_bulk(s: &Served, input: impl Read, out: impl Write + Send, bulk: &dyn BulkSource, stream: &std::os::unix::net::UnixStream) {
    serve_connection_x(s, input, out, Some((bulk, stream)));
}

#[cfg(unix)]
type BulkPath<'a> = Option<(&'a dyn BulkSource, &'a std::os::unix::net::UnixStream)>;
#[cfg(not(unix))]
type BulkPath<'a> = Option<(&'a dyn BulkSource, &'a ())>;

fn serve_connection_x<W: Write + Send>(s: &Served, input: impl Read, out: W, bulk: BulkPath<'_>) {
    // the writer is shared with the push threads of the connection (`facets`): one line at a time, under the lock; the threads live in this
    // scope, so they end with the connection and need no 'static writer
    let out = std::sync::Mutex::new(out);
    let mut fc = crate::facets::Conn::new();
    let stop = AtomicBool::new(false);
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let _gone = Gone(s.rpc.clone(), conn);
    std::thread::scope(|scope| {
        let mut input = BufReader::new(input);
        let start = Instant::now();
        let mut limiter = RateLimiter::new(s.hello.rate_limit);
        let write = |r: &Response| -> bool {
            let mut o = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            // one write for the line and its newline: an unbuffered socket would otherwise send them apart, and a client that reads with a single
            // `recvmsg` (`bulk.get`) could get the line without its newline and meet the newline as the start of the next answer
            let mut text = serde_json::to_string(r).unwrap_or_default();
            text.push('\n');
            o.write_all(text.as_bytes()).and_then(|_| o.flush()).is_ok()
        };
        for _ in 0..MAX_CALLS_PER_CONNECTION {
            let mut line = String::new();
            match (&mut input).take(MAX_LINE as u64 + 1).read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let resp = if line.len() > MAX_LINE {
                let _ = write(&Response::err(0, invalid("that line is too long")));
                // the rest of the line is read and dropped (up to a cap) so the close is a clean one: a close with unread bytes is a reset
                // that can eat the answer on its way to the client
                let mut rest = Vec::new();
                let _ = (&mut input).take(4 * 1024 * 1024).read_until(b'\n', &mut rest);
                break; // the rest of the line cannot be told from the next request
            } else {
                match serde_json::from_str::<Request>(line.trim_end()) {
                    Err(e) => Response::err(0, invalid(&format!("that is not a request: {e}"))),
                    Ok(req) => match limiter.check(start.elapsed().as_millis() as u64) {
                        Err(e) => Response::err(req.id, e),
                        Ok(()) if req.call == "bulk.get" => {
                            // the block's head line goes out on the stream itself: no push line may be cut into it
                            let _w = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            match bulk_answer(s, &req, bulk) {
                                Ok(None) => continue, // answered with the descriptor
                                Ok(Some(r)) => r,
                                Err(e) => Response::err(req.id, e),
                            }
                        }
                        Ok(()) => {
                            let r = crate::facets::dispatch(s, &mut fc, &req).unwrap_or_else(|| dispatch(s, &req, conn));
                            for job in fc.jobs.drain(..) {
                                let (out, stop) = (&out, &stop);
                                scope.spawn(move || crate::facets::push_loop(job, out, stop));
                            }
                            match r {
                                Ok(v) => Response::ok(req.id, v),
                                Err(e) => Response::err(req.id, e),
                            }
                        }
                    },
                }
            };
            if !write(&resp) {
                break;
            }
        }
        fc.finish(&stop);
    });
}

/// Drops every object of a connection when the connection ends.
struct Gone(Option<Arc<dyn RpcServe>>, u64);

impl Drop for Gone {
    fn drop(&mut self) {
        if let Some(r) = &self.0 {
            r.release(self.1, None);
        }
    }
}

#[cfg(unix)]
fn bulk_answer(s: &Served, req: &Request, bulk: BulkPath<'_>) -> CallResult<Option<Response>> {
    let Some((src, stream)) = bulk else {
        return Err(CallError::new(ErrorCode::Unsupported, "transport.no_bulk", "This connection cannot hand over a block (only the local unix socket can).").hint("Use the unix socket."));
    };
    let name = req.args.get("name").and_then(Value::as_str).ok_or_else(|| invalid("`name` is a text"))?;
    let bytes = src.get(&s.ctx, name)?;
    let fd = crate::ipc::make_bulk(name, &bytes).map_err(|e| CallError::new(ErrorCode::Internal, "transport.bulk", &e.to_string()))?;
    let head = serde_json::to_string(&Response::ok(req.id, json!({"bulk": name, "len": bytes.len()}))).unwrap_or_default();
    use std::os::fd::AsRawFd;
    crate::ipc::send_with_fd(stream, &format!("{head}\n"), fd.as_raw_fd()).map_err(|e| CallError::new(ErrorCode::Internal, "transport.bulk", &e.to_string()))?;
    Ok(None)
}

#[cfg(not(unix))]
fn bulk_answer(_s: &Served, _req: &Request, _bulk: BulkPath<'_>) -> CallResult<Option<Response>> {
    Err(CallError::new(ErrorCode::Unsupported, "transport.no_bulk", "This connection cannot hand over a block."))
}

/// The most connections served at the same time (one more is closed at once).
pub const MAX_CONNECTIONS: usize = 32;

/// Serve every connection of `listener` (loopback only), each on its own thread, until `stop` is set.
pub fn serve(listener: TcpListener, s: Arc<Served>, stop: Arc<AtomicBool>) -> std::io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the reference server only listens on the loopback address"));
    }
    listener.set_nonblocking(true)?;
    let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((c, _)) => {
                let _ = c.set_nonblocking(false);
                // a connection that says nothing for a minute is closed
                let _ = c.set_read_timeout(Some(Duration::from_secs(60)));
                if open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue;
                }
                open.fetch_add(1, Ordering::Relaxed);
                let (served, open) = (s.clone(), open.clone());
                std::thread::spawn(move || {
                    if let Ok(r) = c.try_clone() {
                        serve_connection(&served, r, c);
                    }
                    open.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Serve every connection of a unix socket listener, each on its own thread, until `stop` is set. The socket is the machine's own: who may
/// connect is the file mode of the socket (the caller makes it 0600), and who the caller IS comes from `Served::ctx`, fixed per listener.
#[cfg(unix)]
pub fn serve_unix(listener: std::os::unix::net::UnixListener, s: Arc<Served>, stop: Arc<AtomicBool>) -> std::io::Result<()> {
    serve_unix_for(listener, s, stop, None)
}

/// Like [`serve_unix`], and with `Some(uid)` every connection's peer is read from the kernel (`SO_PEERCRED`) and a peer that is neither root nor `uid`
/// is closed at once (the UI program's door, F-SPLIT-03: the file mode is the first lock, this is the second).
#[cfg(unix)]
pub fn serve_unix_for(listener: std::os::unix::net::UnixListener, s: Arc<Served>, stop: Arc<AtomicBool>, ui_uid: Option<u32>) -> std::io::Result<()> {
    serve_unix_bulk(listener, s, stop, ui_uid, None)
}

/// Like [`serve_unix_for`], and with a [`BulkSource`] a connection may ask for a block with `bulk.get`: it comes back as a memory file passed
/// over the socket beside the answer line.
#[cfg(unix)]
pub fn serve_unix_bulk(listener: std::os::unix::net::UnixListener, s: Arc<Served>, stop: Arc<AtomicBool>, ui_uid: Option<u32>, bulk: Option<Arc<dyn BulkSource>>) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((c, _)) => {
                if let Some(uid) = ui_uid {
                    match crate::ipc::peer_of(&c) {
                        Ok(p) if crate::ipc::peer_allowed(&p, uid) => {}
                        _ => continue, // dropped: the connection closes
                    }
                }
                let _ = c.set_nonblocking(false);
                let _ = c.set_read_timeout(Some(Duration::from_secs(60)));
                if open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue;
                }
                open.fetch_add(1, Ordering::Relaxed);
                let (served, open, bulk) = (s.clone(), open.clone(), bulk.clone());
                std::thread::spawn(move || {
                    if let Ok(r) = c.try_clone() {
                        match &bulk {
                            Some(b) => serve_connection_bulk(&served, r, &c, b.as_ref(), &c),
                            None => serve_connection(&served, r, &c),
                        }
                    }
                    open.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// What a machine that serves only settings says in `hello`.
pub fn settings_hello(machine: &str, rate: RateLimit) -> Hello {
    use iyakki_core_api::contract::DomainSupport;
    Hello {
        contract: CONTRACT_VERSION,
        oldest_minor: 0,
        machine: machine.into(),
        build: "reference".into(),
        domains: vec![DomainSupport { domain: "settings".into(), since_minor: 0, calls: ["schema", "get", "set", "watch"].map(String::from).to_vec(), events: vec!["changed".into()] }],
        capabilities: Default::default(),
        rate_limit: rate,
        deprecations: vec![],
    }
}

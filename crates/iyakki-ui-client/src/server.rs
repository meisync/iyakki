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
    /// Who every caller on this listener is.
    pub ctx: CallContext,
}

fn invalid(why: &str) -> CallError {
    CallError::new(ErrorCode::InvalidInput, "transport.bad_request", why)
}

fn dispatch(s: &Served, req: &Request) -> CallResult<Value> {
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
        "flows.approve" => {
            let id = req.args.get("approval_id").and_then(Value::as_str).ok_or_else(|| invalid("`approval_id` is a text"))?;
            let allow = req.args.get("allow").and_then(Value::as_bool).ok_or_else(|| invalid("`allow` is true or false"))?;
            to(&flows(s)?.approve(&s.ctx, &run_of(&req.args)?, id, allow)?)
        }
        other => Err(CallError::new(ErrorCode::Unsupported, "transport.unknown_call", &format!("This machine does not know the call {other}.")).hint("Read `hello` for the calls it supports.")),
    }
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
pub fn serve_connection(s: &Served, input: impl Read, mut out: impl Write) {
    let mut input = BufReader::new(input);
    let start = Instant::now();
    let mut limiter = RateLimiter::new(s.hello.rate_limit);
    for _ in 0..MAX_CALLS_PER_CONNECTION {
        let mut line = String::new();
        match (&mut input).take(MAX_LINE as u64 + 1).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let resp = if line.len() > MAX_LINE {
            let r = Response::err(0, invalid("that line is too long"));
            let _ = writeln!(out, "{}", serde_json::to_string(&r).unwrap_or_default());
            return; // the rest of the line cannot be told from the next request
        } else {
            match serde_json::from_str::<Request>(line.trim_end()) {
                Err(e) => Response::err(0, invalid(&format!("that is not a request: {e}"))),
                Ok(req) => match limiter.check(start.elapsed().as_millis() as u64) {
                    Err(e) => Response::err(req.id, e),
                    Ok(()) => match dispatch(s, &req) {
                        Ok(v) => Response::ok(req.id, v),
                        Err(e) => Response::err(req.id, e),
                    },
                },
            }
        };
        if writeln!(out, "{}", serde_json::to_string(&resp).unwrap_or_default()).and_then(|_| out.flush()).is_err() {
            return;
        }
    }
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
    listener.set_nonblocking(true)?;
    let open = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((c, _)) => {
                let _ = c.set_nonblocking(false);
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

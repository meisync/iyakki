//! The client end: [`Connection`] sends one request at a time over a stream and reads the answer; [`RemoteSettings`] is the `Settings`
//! trait of the contract over it, so a screen written against the trait does not know whether the machine is in the same process.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::*;

fn transport_error(why: &str) -> CallError {
    CallError::new(ErrorCode::Offline, "client.transport", &format!("The machine did not answer: {why}.")).hint("Check that the machine is on and the address is right, then try again.")
}

pub struct Connection {
    out: Box<dyn Write + Send>,
    input: BufReader<Box<dyn Read + Send>>,
    next: u64,
}

impl Connection {
    /// Over any pair of streams (a test passes in-memory ones).
    pub fn new(input: Box<dyn Read + Send>, out: Box<dyn Write + Send>) -> Connection {
        Connection { out, input: BufReader::new(input), next: 1 }
    }

    /// Over TCP; every read and write gives up after `timeout`.
    pub fn tcp(addr: impl ToSocketAddrs, timeout: Duration) -> CallResult<Connection> {
        let a = addr.to_socket_addrs().map_err(|e| transport_error(&e.to_string()))?.next().ok_or_else(|| transport_error("no address"))?;
        let s = TcpStream::connect_timeout(&a, timeout).map_err(|e| transport_error(&e.to_string()))?;
        s.set_read_timeout(Some(timeout)).and_then(|_| s.set_write_timeout(Some(timeout))).map_err(|e| transport_error(&e.to_string()))?;
        let r = s.try_clone().map_err(|e| transport_error(&e.to_string()))?;
        Ok(Connection::new(Box::new(r), Box::new(s)))
    }

    /// Over a unix socket the machine listens on; every read and write gives up after `timeout`.
    #[cfg(unix)]
    pub fn unix(path: impl AsRef<std::path::Path>, timeout: Duration) -> CallResult<Connection> {
        let s = std::os::unix::net::UnixStream::connect(path).map_err(|e| transport_error(&e.to_string()))?;
        s.set_read_timeout(Some(timeout)).and_then(|_| s.set_write_timeout(Some(timeout))).map_err(|e| transport_error(&e.to_string()))?;
        let r = s.try_clone().map_err(|e| transport_error(&e.to_string()))?;
        Ok(Connection::new(Box::new(r), Box::new(s)))
    }

    /// One call: the typed answer of the machine, or its `CallError` as it came.
    pub fn call(&mut self, call: &str, args: Value) -> CallResult<Value> {
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_string(&Request { id, call: call.into(), args }).map_err(|e| CallError::new(ErrorCode::Internal, "client.encode", &e.to_string()))?;
        if line.len() >= MAX_LINE {
            return Err(CallError::new(ErrorCode::InvalidInput, "client.too_big", "That request is too big to send."));
        }
        line.push('\n');
        self.out.write_all(line.as_bytes()).and_then(|_| self.out.flush()).map_err(|e| transport_error(&e.to_string()))?;
        let mut answer = String::new();
        let n = (&mut self.input).take(MAX_LINE as u64 + 1).read_line(&mut answer).map_err(|e| transport_error(&e.to_string()))?;
        if n == 0 {
            return Err(transport_error("the connection was closed"));
        }
        if answer.len() > MAX_LINE {
            return Err(CallError::new(ErrorCode::Internal, "client.too_big_answer", "The machine sent an answer that is too big."));
        }
        let r: Response = serde_json::from_str(answer.trim_end()).map_err(|e| CallError::new(ErrorCode::Internal, "client.bad_answer", &format!("The machine sent something that is not an answer: {e}.")))?;
        if r.id != id {
            return Err(CallError::new(ErrorCode::Internal, "client.out_of_step", "The machine answered a different request."));
        }
        match (r.ok, r.err) {
            (Some(v), None) => Ok(v),
            (None, Some(e)) => Err(e),
            _ => Err(CallError::new(ErrorCode::Internal, "client.bad_answer", "The answer has neither a result nor an error, or both.")),
        }
    }

    pub fn hello(&mut self) -> CallResult<Hello> {
        let v = self.call("hello", Value::Null)?;
        serde_json::from_value(v).map_err(|e| CallError::new(ErrorCode::Internal, "client.bad_hello", &e.to_string()))
    }

    /// Say what this UI would like to use and learn what it may: the screens whose domain is `missing` are hidden, not failed.
    pub fn negotiate(&mut self, wants: &[&str]) -> CallResult<(Hello, Negotiated)> {
        let hello = self.hello()?;
        let mine = ClientHello { contract: CONTRACT_VERSION, wants: wants.iter().map(|s| s.to_string()).collect() };
        let n = negotiate(&hello, &mine);
        if n.version.is_none() {
            let why = n.say(&hello, &mine).unwrap_or_else(|| "no common contract version".into());
            return Err(CallError::new(ErrorCode::VersionMismatch, "client.version", &why));
        }
        Ok((hello, n))
    }
}

/// `Settings` over a [`Connection`]. The `CallContext` argument is ignored on purpose: who the caller is, is decided by the machine's end of
/// the transport (its scopes, its transport kind), never by what a client says.
pub struct RemoteSettings {
    conn: Mutex<Connection>,
}

impl RemoteSettings {
    pub fn new(conn: Connection) -> RemoteSettings {
        RemoteSettings { conn: Mutex::new(conn) }
    }

    fn call<T: serde::de::DeserializeOwned>(&self, call: &str, args: Value) -> CallResult<T> {
        let v = self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner).call(call, args)?;
        serde_json::from_value(v).map_err(|e| CallError::new(ErrorCode::Internal, "client.bad_answer", &format!("The answer to {call} has the wrong shape: {e}.")))
    }
}

impl Settings for RemoteSettings {
    fn schema(&self, _: &CallContext) -> CallResult<Vec<SettingSpec>> {
        self.call("settings.schema", Value::Null)
    }
    fn get(&self, _: &CallContext, key: &str) -> CallResult<SettingValue> {
        self.call("settings.get", json!({"key": key}))
    }
    fn set(&self, _: &CallContext, key: &str, value: SettingValue, idem: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
        self.call("settings.set", json!({"key": key, "value": value, "idempotency_key": idem.map(IdempotencyKey::as_str)}))
    }
    fn watch(&self, _: &CallContext, key: Option<&str>, resume: Resume, limit: usize) -> CallResult<EventBatch<SettingEvent>> {
        self.call("settings.watch", json!({"key": key, "resume": resume, "limit": limit}))
    }
}

/// `Vault` over a [`Connection`]. There is no `unlock` here: it needs the machine's own seat, which no transport carries.
pub struct RemoteVault {
    conn: std::sync::Arc<Mutex<Connection>>,
}

/// `Flows` over a [`Connection`].
pub struct RemoteFlows {
    conn: std::sync::Arc<Mutex<Connection>>,
}

/// One connection shared by the domains of a UI (each call still goes one at a time).
pub struct Remote {
    conn: std::sync::Arc<Mutex<Connection>>,
}

fn typed<T: serde::de::DeserializeOwned>(conn: &Mutex<Connection>, call: &str, args: Value) -> CallResult<T> {
    let v = conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner).call(call, args)?;
    serde_json::from_value(v).map_err(|e| CallError::new(ErrorCode::Internal, "client.bad_answer", &format!("The answer to {call} has the wrong shape: {e}.")))
}

impl Remote {
    pub fn new(conn: Connection) -> Remote {
        Remote { conn: std::sync::Arc::new(Mutex::new(conn)) }
    }
    pub fn vault(&self) -> RemoteVault {
        RemoteVault { conn: self.conn.clone() }
    }
    pub fn flows(&self) -> RemoteFlows {
        RemoteFlows { conn: self.conn.clone() }
    }
    pub fn settings(&self) -> RemoteSharedSettings {
        RemoteSharedSettings { conn: self.conn.clone() }
    }
}

/// `Settings` over the shared connection of a [`Remote`].
pub struct RemoteSharedSettings {
    conn: std::sync::Arc<Mutex<Connection>>,
}

impl Settings for RemoteSharedSettings {
    fn schema(&self, _: &CallContext) -> CallResult<Vec<SettingSpec>> {
        typed(&self.conn, "settings.schema", Value::Null)
    }
    fn get(&self, _: &CallContext, key: &str) -> CallResult<SettingValue> {
        typed(&self.conn, "settings.get", json!({"key": key}))
    }
    fn set(&self, _: &CallContext, key: &str, value: SettingValue, idem: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
        typed(&self.conn, "settings.set", json!({"key": key, "value": value, "idempotency_key": idem.map(IdempotencyKey::as_str)}))
    }
    fn watch(&self, _: &CallContext, key: Option<&str>, resume: Resume, limit: usize) -> CallResult<EventBatch<SettingEvent>> {
        typed(&self.conn, "settings.watch", json!({"key": key, "resume": resume, "limit": limit}))
    }
}

impl RemoteVault {
    pub fn status(&self) -> CallResult<VaultStatus> {
        typed(&self.conn, "vault.status", Value::Null)
    }
    pub fn lock(&self) -> CallResult<VaultStatus> {
        typed(&self.conn, "vault.lock", Value::Null)
    }
    pub fn use_secret(&self, action: &str, idem: Option<&IdempotencyKey>) -> CallResult<ActionOutcome> {
        typed(&self.conn, "vault.use", json!({"action": action, "idempotency_key": idem.map(IdempotencyKey::as_str)}))
    }
}

impl RemoteFlows {
    pub fn list(&self) -> CallResult<Vec<FlowInfo>> {
        typed(&self.conn, "flows.list", Value::Null)
    }
    pub fn start(&self, flow: &str, idem: &IdempotencyKey) -> CallResult<RunId> {
        typed(&self.conn, "flows.start", json!({"flow": flow, "idempotency_key": idem.as_str()}))
    }
    pub fn stop(&self, run: &RunId) -> CallResult<RunStatus> {
        typed(&self.conn, "flows.stop", json!({"run": run}))
    }
    pub fn status(&self, run: &RunId) -> CallResult<RunStatus> {
        typed(&self.conn, "flows.status", json!({"run": run}))
    }
    pub fn events(&self, run: Option<&RunId>, resume: Resume, limit: usize) -> CallResult<EventBatch<FlowEvent>> {
        typed(&self.conn, "flows.events", json!({"run": run, "resume": resume, "limit": limit}))
    }
    pub fn approve(&self, run: &RunId, approval_id: &str, allow: bool) -> CallResult<RunStatus> {
        typed(&self.conn, "flows.approve", json!({"run": run, "approval_id": approval_id, "allow": allow}))
    }
}

//! The facets of the agent as remote objects over the core's UI socket (F-SPLIT-03, docs/development/UI-RPC-DESIGN.md).
//!
//! One call is one line of the contract transport: `rpc.call` with `{obj, iface, method, args}` (args are a JSON array in the order of the
//! parameters), answered with the JSON of the result. A method that returns a handle (`Arc<dyn Trait>`) is answered with a number, the
//! object id, and the UI holds a `Remote<Trait>` that calls on that id and sends `rpc.release` when it is dropped. Object 0 is a root:
//! the core registers one object per trait it serves ([`Host::with_root`]).
//!
//! The code in `gen_remote.rs` and `gen_serve.rs` is generated; this file is the part they share.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use iyakki_core_api::contract::{CallContext, CallError, CallResult, ErrorCode};
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::RpcServe;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

mod gen_remote;
mod handle;
pub use handle::RemoteServicesHandle;
mod gen_serve;
pub use gen_remote::*;
pub use gen_serve::Obj;

/// The most objects one connection may hold at the same time (a UI that leaks handles is told so, not served until memory is gone).
pub const MAX_OBJECTS: usize = 4096;

/// What a remote call answers when the core cannot (the connection is gone, the call is not served): every return type has one. A result
/// is `Err(why)`, an option `None`, a list empty, a number 0, a flag false; nothing panics, so a lost core never takes the UI down.
pub trait Lost {
    fn lost(why: &str) -> Self;
}

/// An error type a [`Lost`] result can carry.
pub trait LostErr {
    fn lost(why: &str) -> Self;
}

impl LostErr for String {
    fn lost(why: &str) -> Self {
        why.to_string()
    }
}

impl<T, E: LostErr> Lost for Result<T, E> {
    fn lost(why: &str) -> Self {
        Err(E::lost(why))
    }
}

macro_rules! lost_default {
    ($($t:ty),*) => { $(impl Lost for $t { fn lost(_: &str) -> Self { Default::default() } })* };
}
lost_default!((), bool, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64, String, std::path::PathBuf);

// the domain types a lost call has to answer with: the quiet value of each
impl Lost for iyakki_shared::media::QueueState {
    fn lost(_: &str) -> Self {
        Default::default()
    }
}
impl Lost for iyakki_shared::media::Check {
    fn lost(_: &str) -> Self {
        iyakki_shared::media::Check::Idle
    }
}

lost_default!(
    std::time::Duration,
    iyakki_shared::inbox::Counts,
    iyakki_shared::devtools::Summary,
    iyakki_shared::devtools::Snapshot,
    iyakki_shared::usagelog::Totals,
    iyakki_shared::browserconsent::ControlView
);
impl Lost for iyakki_shared::inbox::Level {
    fn lost(_: &str) -> Self {
        iyakki_shared::inbox::Level::Show
    }
}
impl<K, V> Lost for std::collections::BTreeMap<K, V> {
    fn lost(_: &str) -> Self {
        Self::new()
    }
}
impl LostErr for iyakki_shared::vault::SecretError {
    fn lost(why: &str) -> Self {
        iyakki_shared::vault::SecretError::Backend(why.to_string())
    }
}

impl<T> Lost for Option<T> {
    fn lost(_: &str) -> Self {
        None
    }
}
impl<T> Lost for Vec<T> {
    fn lost(_: &str) -> Self {
        Vec::new()
    }
}
impl<K, V, S: Default> Lost for HashMap<K, V, S> {
    fn lost(_: &str) -> Self {
        HashMap::default()
    }
}
impl<A: Lost, B: Lost> Lost for (A, B) {
    fn lost(w: &str) -> Self {
        (A::lost(w), B::lost(w))
    }
}
impl<A: Lost, B: Lost, C: Lost> Lost for (A, B, C) {
    fn lost(w: &str) -> Self {
        (A::lost(w), B::lost(w), C::lost(w))
    }
}
impl<T: Lost> Lost for Arc<T> {
    fn lost(w: &str) -> Self {
        Arc::new(T::lost(w))
    }
}

fn why(e: &CallError) -> String {
    e.message.clone()
}

/// The UI's end: one connection shared by every remote object of the program (a call is one request and one answer, one at a time).
#[derive(Clone)]
pub struct Rpc {
    conn: Arc<Mutex<Connection>>,
}

impl Rpc {
    pub fn new(conn: Connection) -> Rpc {
        Rpc { conn: Arc::new(Mutex::new(conn)) }
    }

    /// One call on object `obj` of the trait `iface`.
    pub fn call<T: DeserializeOwned>(&self, iface: &str, obj: u64, method: &str, args: Value) -> Result<T, String> {
        let v = self.conn.lock().unwrap_or_else(PoisonError::into_inner).call("rpc.call", json!({"obj": obj, "iface": iface, "method": method, "args": args})).map_err(|e| why(&e))?;
        serde_json::from_value(v).map_err(|e| format!("the answer to {iface}::{method} has the wrong shape: {e}"))
    }

    /// Tell the core the UI dropped its end of `obj` (best effort: a lost connection frees everything anyway).
    pub fn release(&self, obj: u64) {
        if obj != 0 {
            let _ = self.conn.lock().unwrap_or_else(PoisonError::into_inner).call("rpc.release", json!({"obj": obj}));
        }
    }

    /// The root object of the trait `iface`, or `None` when this build does not know the trait.
    pub fn root(&self, iface: &str) -> Option<RemoteAny> {
        remote_any(iface, self, 0)
    }
}

pub(crate) fn de<T: DeserializeOwned>(a: Value) -> Result<T, String> {
    serde_json::from_value(a).map_err(|e| format!("the arguments are wrong: {e}"))
}

pub(crate) fn ser<T: Serialize + ?Sized>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| format!("the answer cannot be encoded: {e}"))
}

/// The objects of one connection: what a call may hand out ([`Registry::add`]).
pub struct Registry<'a> {
    host: &'a Host,
    conn: u64,
}

impl Registry<'_> {
    /// Keep a handle for the UI and return its id (0 when the connection holds too many; the UI's call then fails as a lost object).
    pub(crate) fn add(&self, o: Obj) -> u64 {
        let mut all = self.host.conns.lock().unwrap_or_else(PoisonError::into_inner);
        let mine = all.entry(self.conn).or_default();
        if mine.len() >= MAX_OBJECTS {
            return 0;
        }
        let id = self.host.next.fetch_add(1, Ordering::Relaxed);
        mine.insert(id, o);
        id
    }
}

/// The core's end: the root objects it serves and, per connection, the handles it handed out. Implements the transport's `RpcServe`.
#[derive(Default)]
pub struct Host {
    roots: Mutex<HashMap<&'static str, Obj>>,
    conns: Mutex<HashMap<u64, HashMap<u64, Obj>>>,
    next: AtomicU64,
}

impl Host {
    pub fn new() -> Host {
        Host { next: AtomicU64::new(1), ..Default::default() }
    }

    /// Serve `o` as the root object of its trait (object 0 of `iface`).
    pub fn with_root(self, o: Obj) -> Host {
        self.roots.lock().unwrap_or_else(PoisonError::into_inner).insert(o.iface(), o);
        self
    }

    /// The number of handles `conn` holds (for tests and the audit line).
    pub fn held(&self, conn: u64) -> usize {
        self.conns.lock().unwrap_or_else(PoisonError::into_inner).get(&conn).map_or(0, HashMap::len)
    }

    /// The number of handles held for all connections together.
    pub fn held_total(&self) -> usize {
        self.conns.lock().unwrap_or_else(PoisonError::into_inner).values().map(HashMap::len).sum()
    }

    fn find(&self, conn: u64, obj: u64, iface: &str) -> Option<Obj> {
        if obj == 0 {
            return self.roots.lock().unwrap_or_else(PoisonError::into_inner).get(iface).cloned();
        }
        self.conns.lock().unwrap_or_else(PoisonError::into_inner).get(&conn).and_then(|m| m.get(&obj)).filter(|o| o.iface() == iface).cloned()
    }
}

impl RpcServe for Host {
    fn call(&self, _ctx: &CallContext, conn: u64, obj: u64, iface: &str, method: &str, args: Value) -> CallResult<Value> {
        let o = self.find(conn, obj, iface).ok_or_else(|| CallError::new(ErrorCode::NotFound, "rpc.no_object", &format!("There is no {iface} object {obj} on this connection.")))?;
        o.serve(&Registry { host: self, conn }, method, args).map_err(|m| CallError::new(ErrorCode::InvalidInput, "rpc.call_failed", &m))
    }

    fn release(&self, conn: u64, obj: Option<u64>) {
        let mut all = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        match obj {
            None => {
                all.remove(&conn);
            }
            Some(id) => {
                if let Some(m) = all.get_mut(&conn) {
                    m.remove(&id);
                }
            }
        }
    }
}

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

mod agent_api;
mod gen_remote;
mod handle;
mod gen_services;
mod unserved;
pub mod served;
mod served_hosts;
#[allow(non_snake_case)]
mod served_apps;
mod served_sound;
mod served_state;
#[allow(non_snake_case)]
mod served_jobs;
mod served_store;
mod served_tiles;
mod gen_flows_jobs;
mod served_flows;
mod served_policy;
mod served_auth;
mod served_login;
pub use served_login::{TicketClock, TICKET_TTL_MS};
#[cfg(test)]
mod served_auth_audit;
pub use served_tiles::{RemoteTileRt, RemoteWasmTile};
pub use served_state::{remote_dev_state, serve_dev_state, RemoteDevState, BEAT, PUBLISH};
pub use served_sound::{remote_core_sound, serve_core_sound, RemoteCoreSound};
mod served_person;
mod served_git;
mod gen_unserved;
pub use agent_api::RemoteAgentApi;
pub use handle::{CoreEnv, RemoteServicesHandle, ServicesRoot};
pub use served_hosts::serve_uploads;
pub use served_apps::{remote_chat_backend, remote_frame_source, serve_frame_source, FramePath, RemoteFrameSource, remote_host, remote_picture_backend, remote_vision_backend, serve_chat_backend, serve_host, serve_picture_backend, serve_vision_backend, RemoteChatBackend, RemoteHost, RemotePictureBackend, RemoteVisionBackend};
pub use served::{remote_files, SecretStoreOf, Served, ServedParts};
pub use served_person::{remote_core_terminal, remote_terminal, serve_core_terminal, serve_terminal, RemoteCodeVault, RemoteCoreTerminal, HIDDEN};
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

/// A lost call that asked where a chain breaks: the end of the numbers, so no entry is believed intact.
impl LostErr for usize {
    fn lost(_: &str) -> Self {
        usize::MAX
    }
}

impl LostErr for String {
    fn lost(why: &str) -> Self {
        why.to_string()
    }
}

impl LostErr for iyakki_memx::kb::KbError {
    fn lost(why: &str) -> Self {
        iyakki_memx::kb::KbError::Storage(why.to_string())
    }
}

/// An account call that was lost: a storage error naming why, so no screen believes it signed anybody in or changed an account.
impl LostErr for iyakki_accounts::AccountError {
    fn lost(why: &str) -> Self {
        iyakki_accounts::AccountError::Storage(why.to_string())
    }
}

impl LostErr for iyakki_shared::settings::SettingsError {
    fn lost(why: &str) -> Self {
        iyakki_shared::settings::SettingsError::Storage(why.to_string())
    }
}

/// Settings of a lost call: the defaults, read-only with the reason, so no screen believes it changed them.
impl Lost for iyakki_agent_api::SettingsSnap {
    fn lost(why: &str) -> Self {
        iyakki_agent_api::SettingsSnap { generation: 0, settings: Default::default(), read_only: Some(why.to_string()) }
    }
}

impl<T, E: LostErr> Lost for Result<T, E> {
    fn lost(why: &str) -> Self {
        Err(E::lost(why))
    }
}

macro_rules! lost_default {
    ($($t:ty),* $(,)?) => { $(impl Lost for $t { fn lost(_: &str) -> Self { Default::default() } })* };
}
lost_default!((), bool, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, isize, f32, f64, String, std::path::PathBuf, serde_json::Value);
include!("lost_default.rs");
include!("lost_manual.rs");


// the domain types a lost call has to answer with: the quiet value of each
/// A usage series of a lost call: nothing used.
impl Lost for iyakki_shared::usagelog::Series {
    fn lost(_: &str) -> Self {
        iyakki_shared::usagelog::Series { models: Vec::new(), buckets: Vec::new(), totals: Vec::new(), input: 0, output: 0, calls: 0 }
    }
}
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
    iyakki_shared::browserconsent::ControlView,
    iyakki_shared::extagent::AgentsCfg,
    iyakki_shared::context::ContextInfo,
    iyakki_shared::audioout::SoundSettings,
    iyakki_shared::syssounds::SystemSounds,
    iyakki_shared::voice::VoiceSettings,
    iyakki_shared::providers::ImageTrace,
    iyakki_shared::k3sdev::Health,
    iyakki_shared::wsexec::Out,
    iyakki_core_api::FileRoots
);
impl Lost for iyakki_shared::cloudmode::Setting {
    fn lost(_: &str) -> Self {
        Default::default()
    }
}
/// A sign-in the core did not answer: both vaults stay locked and say why (a failed call never opens anything).
impl Lost for iyakki_shared::services::LoginReport {
    fn lost(why: &str) -> Self {
        iyakki_shared::services::LoginReport { user_vault: iyakki_shared::services::VaultState::Locked(why.to_string()), system_vault: iyakki_shared::services::VaultState::Locked(why.to_string()), browser_data: iyakki_shared::profilestore::SignIn::Off }
    }
}
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
impl LostErr for iyakki_shared::visionmodel::VisionError {
    fn lost(why: &str) -> Self {
        iyakki_shared::visionmodel::VisionError::NoneFound(why.to_string())
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
impl LostErr for iyakki_shared::skills::SkillError {
    fn lost(why: &str) -> Self {
        iyakki_shared::skills::SkillError::Storage(why.to_string())
    }
}
impl LostErr for iyakki_shared::vault::VaultError {
    fn lost(why: &str) -> Self {
        iyakki_shared::vault::VaultError::Storage(why.to_string())
    }
}
impl LostErr for iyakki_shared::pkg::PkgError {
    fn lost(why: &str) -> Self {
        iyakki_shared::pkg::PkgError::Storage(why.to_string())
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

    /// One call on object `obj` of the trait `iface`. A value bigger than a line goes by the `Upload` and `Download` objects without the caller
    /// noticing (D-SPLIT-03-STORE: the store's catalogs are bigger than a line): arguments over [`INLINE_MAX`] are uploaded and named in the call
    /// (`{"$up": id}`), an answer over it is a download the UI reads (`{"$big": id}`).
    pub fn call<T: DeserializeOwned>(&self, iface: &str, obj: u64, method: &str, args: Value) -> Result<T, String> {
        let args = self.with_ticket(iface, method, args)?;
        let spooled = !spool_free(iface, method);
        let (args, _guard) = if spooled { self.spool_args(args)? } else { (args, None) };
        let v = self.conn.lock().unwrap_or_else(PoisonError::into_inner).call("rpc.call", json!({"obj": obj, "iface": iface, "method": method, "args": args})).map_err(|e| why(&e))?;
        let v = match v.as_object().filter(|o| o.len() == 1).and_then(|o| o.get(BIG)).and_then(Value::as_u64) {
            Some(id) if spooled => serde_json::from_slice(&served_hosts::read_all(self, id)?).map_err(|e| format!("the big answer to {iface}::{method} has the wrong shape: {e}"))?,
            _ => v,
        };
        serde_json::from_value(v).map_err(|e| format!("the answer to {iface}::{method} has the wrong shape: {e}"))
    }

    /// A call that carries an approval the person gave on the card asks the core for the ticket first and sends it as one more argument
    /// (served_login.rs). A call that is not approved, or takes no approval, is sent as it is.
    fn with_ticket(&self, iface: &str, method: &str, mut args: Value) -> Result<Value, String> {
        if let Some(tk) = served_login::ticketed(iface, method) {
            if args.get(tk.approved).and_then(Value::as_bool) == Some(true) && args.as_array().map(Vec::len) == Some(tk.wire_args) {
                let tok: String = self.call("Approval", 0, "issue", json!([iface, method, args.clone()]))?;
                if let Some(a) = args.as_array_mut() {
                    a.push(Value::String(tok));
                }
            }
        }
        Ok(args)
    }

    fn spool_args(&self, args: Value) -> Result<(Value, Option<served_hosts::Sent>), String> {
        let text = args.to_string();
        if text.len() <= INLINE_MAX {
            return Ok((args, None));
        }
        let sent = served_hosts::send(self, text.as_bytes())?;
        Ok((json!({ UP: sent.id }), Some(sent)))
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

/// The most a call or an answer carries inline: the size of one chunk of an upload (a line is at most 64 KiB).
pub(crate) const INLINE_MAX: usize = served_hosts::CHUNK;
const UP: &str = "$up";
const BIG: &str = "$big";

/// The calls that carry the bytes of the others never spool themselves.
fn spool_free(iface: &str, method: &str) -> bool {
    matches!(iface, "Upload" | "Download") || (iface == "ServicesHandle" && method == "upload_new")
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
    /// The core's own services, when the host was given them ([`Host::with_services`]).
    pub(crate) fn services(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        self.host.side.lock().unwrap_or_else(PoisonError::into_inner).services.clone()
    }

    pub(crate) fn side(&self) -> CoreSide {
        self.host.side.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// An object this connection holds, by id and trait (for a call that names an upload it made before).
    pub(crate) fn find(&self, id: u64, iface: &str) -> Option<Obj> {
        self.host.find(self.conn, id, iface)
    }

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
    /// What the core supplies in place of what the UI used to pass (D-SPLIT-03-APPS2, D-SPLIT-03-STORE).
    side: Mutex<CoreSide>,
    /// Who signed in on each connection and the approval tickets (served_login.rs).
    login: Mutex<served_login::Login>,
}

/// The core's own objects that stand in for the services, the web fetch, the audit callback and the person's account a UI would have passed. A
/// call that needs one the host was not given refuses by name.
#[derive(Clone, Default)]
pub struct CoreSide {
    /// The core's own services, type-erased (`PlatformApi::private_*`).
    pub services: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// The core's own way to fetch an address (the store's catalogs and checks).
    pub fetch: Option<Arc<dyn iyakki_shared::store_catalog::Fetch>>,
    /// The audit line of the core (the UI never writes the audit log through a callback of its own).
    pub audit: Option<iyakki_shared::gitkeys::AuditFn>,
    /// The machine's accounts: who is asking and what the policy of that account allows is read here, never taken from the UI.
    pub accounts: Option<Arc<dyn iyakki_agent_api::AccountsApi>>,
    /// The machine's file rules and data folder: a folder the UI names is judged by them before the core opens it.
    pub file_scope: Option<Arc<dyn iyakki_core_api::FileScope>>,
    pub data_dir: Option<std::path::PathBuf>,
    /// The core's own way to fetch for a running tile app's page (D-SPLIT-03-TILES): the web object of the UI is never used.
    pub web: Option<Arc<dyn iyakki_web::Fetch>>,
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

    /// The core's own services for the calls that need them as an `Arc<dyn Any>` (`PlatformApi::private_store_state`, `private_finish_local`,
    /// `private_start_job`). The UI never names them.
    pub fn with_services(self, any: Arc<dyn std::any::Any + Send + Sync>) -> Host {
        self.side.lock().unwrap_or_else(PoisonError::into_inner).services = Some(any);
        self
    }

    /// The core's own fetch, audit and accounts for the store calls (D-SPLIT-03-STORE).
    pub fn with_store_side(self, fetch: Arc<dyn iyakki_shared::store_catalog::Fetch>, audit: iyakki_shared::gitkeys::AuditFn, accounts: Arc<dyn iyakki_agent_api::AccountsApi>, file_scope: Option<Arc<dyn iyakki_core_api::FileScope>>, data_dir: std::path::PathBuf) -> Host {
        {
            let mut g = self.side.lock().unwrap_or_else(PoisonError::into_inner);
            g.fetch = Some(fetch);
            g.audit = Some(audit);
            g.accounts = Some(accounts);
            g.file_scope = file_scope;
            g.data_dir = Some(data_dir);
        }
        self
    }

    /// The core's own web fetch for the pages of the tile apps it runs for the UI (D-SPLIT-03-TILES). Without it a tile app of the core opens with no network.
    pub fn with_web_fetch(self, web: Arc<dyn iyakki_web::Fetch>) -> Host {
        self.side.lock().unwrap_or_else(PoisonError::into_inner).web = Some(web);
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
    fn call(&self, ctx: &CallContext, conn: u64, obj: u64, iface: &str, method: &str, args: Value) -> CallResult<Value> {
        if iface == "Approval" && method == "issue" {
            let reg = Registry { host: self, conn };
            return served_login::issue(&reg, args).map_err(|m| CallError::new(ErrorCode::InvalidInput, "rpc.call_failed", &m));
        }
        let o = self.find(conn, obj, iface).ok_or_else(|| CallError::new(ErrorCode::NotFound, "rpc.no_object", &format!("There is no {iface} object {obj} on this connection.")))?;
        let reg = Registry { host: self, conn };
        // the FLOWS calls name the scope they need (D-S5-02); a socket without it is refused before anything is asked of the core
        if let Some(scope) = served_flows::flows_scope(iface, method) {
            if !ctx.allows(scope) {
                return Err(CallError::new(ErrorCode::Denied, "rpc.no_scope", &served_flows::refused(iface, method, scope)));
            }
        }
        let failed = |m: String| CallError::new(ErrorCode::InvalidInput, "rpc.call_failed", &m);
        let spooled = !spool_free(iface, method);
        let args = match args.as_object().filter(|o| o.len() == 1).and_then(|o| o.get(UP)).and_then(Value::as_u64) {
            Some(id) if spooled => serde_json::from_slice(&served_hosts::bytes_in(&reg, "", id).map_err(failed)?).map_err(|e| failed(format!("the uploaded arguments are wrong: {e}")))?,
            _ => args,
        };
        // every claim of a right in the arguments is replaced by the core's own reading, or the call is refused (served_auth.rs)
        let mut args = args;
        served_login::gate(&reg, iface, method, &mut args).map_err(failed)?;
        served_auth::vouch(&reg, iface, method, &mut args).map_err(failed)?;
        let sent = args.clone();
        let v = o.serve(&reg, method, args).map_err(failed)?;
        served_login::settle(&reg, iface, method, &sent, &v);
        // an answer bigger than a line is a download the UI reads
        if spooled {
            let text = v.to_string();
            if text.len() > INLINE_MAX {
                return Ok(json!({ BIG: served_hosts::down_bytes(&reg, text.into_bytes()) }));
            }
        }
        Ok(v)
    }

    fn release(&self, conn: u64, obj: Option<u64>) {
        let mut all = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        match obj {
            None => {
                all.remove(&conn);
                drop(all);
                self.forget_login(conn);
                return;
            }
            Some(id) => {
                if let Some(m) = all.get_mut(&conn) {
                    m.remove(&id);
                }
            }
        }
    }
}

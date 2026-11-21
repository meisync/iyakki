//! The UI's view of the core's services over the socket (F-SPLIT-03 piece 6). It is the set of typed accessors of the member stores the shell
//! already holds as trait objects (`member_stores`: inbox, calendar, study, device watch, secret requests, usage, screen, developer tools, the
//! secrets without values, accounts, audit), each a remote object of one core root. It does not implement `ServicesHandle` itself: that trait has
//! about 200 methods and the census (UI-RPC-CENSUS.md) puts 163 of the 1,661 methods of the agent traits in classes E, X and A (closures, I/O objects
//! of the UI, downcasts) that need a decision per method, and the browser state and the external-agent status are still concrete types in the shell
//! (`consent` and `jobs` are fields, `refresh` takes `Arc<Self>`).
//! Part 1 of the finish (D-SPLIT-03-HANDLE): `gen_services.rs` (dist/split/gen_services.py) implements `ServicesHandle` for it. The plain methods
//! and the handle accessors of generated traits go over the socket to the core's `ServicesRoot`; what cannot cross answers the quiet value
//! ("not served over the UI socket"), and `k_*` below are the methods that keep their answer or change a value by reading and writing it whole.
//! `is_real` is true only while the core answers a call, so a lost core is visible and a fake is never taken for it (KI-1010).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use iyakki_agent_api::{AccountsApi, AuditApi, BrowserJobsApi, BrowserStateApi, CalendarApi, ChatRuns, ConsentApi, DevToolsApi, DevWatchApi, ExtStatusApi, InboxApi, KnowledgeBaseApi, McpApi, ScreenCaptureApi, SecretRequestsApi, SecretsApi, ServicesHandle, SettingsApi, StudyApi, UsageLogApi};
use iyakki_shared::audioout::SoundSettings;
use iyakki_shared::gitkeys::AuditFn;
use iyakki_shared::syssounds::SystemSounds;
use serde_json::{json, Value};

use crate::{RemoteAny, RemoteMcpApi, Rpc};

/// The core's end of the whole `ServicesHandle` (the root object `ServicesHandle`): the real services and the audit log the core passes for every
/// call that carries an audit callback in the trait (the callback never crosses the socket: the UI does not write the machine's audit log).
pub struct ServicesRoot {
    pub svc: Arc<dyn ServicesHandle>,
    pub audit: AuditFn,
    /// The core's own objects that stand in for what the UI used to pass in (D-SPLIT-03-SERVED).
    pub parts: crate::served::ServedParts,
    env: Option<CoreEnv>,
}

/// What the core owns itself and passes in place of the objects and closures a UI program would have supplied (D-SPLIT-03-ENV): its own
/// operating-system handle (the fleet gate and the router gate come from it) and its own accounts (the SFTP login check). A method that needs
/// one of these and finds none answers "not served over the UI socket".
#[derive(Clone)]
pub struct CoreEnv {
    pub os: Arc<dyn iyakki_core_api::Os>,
    pub users: iyakki_accounts::UsersRef,
}

impl CoreEnv {
    pub fn new(os: Arc<dyn iyakki_core_api::Os>, users: iyakki_accounts::UsersRef) -> CoreEnv {
        CoreEnv { os, users }
    }

    /// The SFTP password check over the core's own accounts (what the shell's `sftp_for` built from its user store).
    pub fn sftp_login(&self) -> iyakki_agent_api::SftpLogin {
        let u = self.users.clone();
        Arc::new(move |n, p| u.lock().unwrap_or_else(|e| e.into_inner()).authenticate(n, p).map(|s| s.policy).map_err(|e| e.to_string()))
    }

    /// The policy of an account by name, for the key check.
    pub fn sftp_lookup(&self) -> iyakki_agent_api::SftpLookup {
        let u = self.users.clone();
        Arc::new(move |n| {
            let mut s = u.lock().unwrap_or_else(|e| e.into_inner());
            s.refresh();
            s.get(n).map(|r| r.policy())
        })
    }
}

impl ServicesRoot {
    pub fn new(svc: Arc<dyn ServicesHandle>, audit: AuditFn) -> ServicesRoot {
        ServicesRoot { svc, audit, parts: Default::default(), env: None }
    }

    /// The same, with the core's accounts, file rules and host for the accessors that took them from the UI (D-SPLIT-03-SERVED).
    pub fn with_parts(mut self, parts: crate::served::ServedParts) -> ServicesRoot {
        self.parts = parts;
        self
    }

    /// Give the root the core's own objects (see [`CoreEnv`]).
    pub fn with_env(mut self, env: CoreEnv) -> ServicesRoot {
        self.env = Some(env);
        self
    }

    pub(crate) fn env(&self) -> Result<&CoreEnv, String> {
        self.env.as_ref().ok_or_else(|| "not served over the UI socket".to_string())
    }
}

/// What the remote handle keeps of the core's borrowed answers, read once (a `&Path` or `&'static str` has to live as long as the handle).
#[derive(Default)]
struct Kept {
    dir: OnceLock<PathBuf>,
    cloud_rule: OnceLock<&'static str>,
    fleet_rule: OnceLock<&'static str>,
}

/// One string per distinct text, kept for the life of the program: a `&'static str` that came over the socket (the sets are small and fixed).
pub(crate) fn intern(s: String) -> &'static str {
    static SET: OnceLock<Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let mut set = SET.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(k) = set.get(s.as_str()) {
        return k;
    }
    let k: &'static str = Box::leak(s.into_boxed_str());
    set.insert(k);
    k
}

struct Offline;
impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("not served over the UI socket".into())
    }
}

/// The services of the core as one UI program sees them.
#[derive(Clone)]
pub struct RemoteServicesHandle {
    pub(crate) rpc: Rpc,
    kept: Arc<Kept>,
    /// The secrets without values: empty and locked, the plain value never crosses (D-SPLIT-03-SECRETS; `secrets_api()` is the served form).
    secrets: Arc<iyakki_shared::vault::Secrets>,
    mcp: Arc<RemoteMcpApi>,
}

macro_rules! root {
    ($(#[$m:meta])* $name:ident, $variant:ident, $tr:ty) => {
        $(#[$m])*
        pub fn $name(&self) -> Arc<$tr> {
            match self.rpc.root(stringify!($variant)) {
                Some(RemoteAny::$variant(r)) => r,
                _ => unreachable!(concat!(stringify!($variant), " is generated")),
            }
        }
    };
}

impl RemoteServicesHandle {
    pub fn new(rpc: Rpc) -> RemoteServicesHandle {
        let mcp = Arc::new(RemoteMcpApi { rpc: rpc.clone(), obj: 0 });
        RemoteServicesHandle { rpc, kept: Arc::default(), secrets: Arc::new(iyakki_shared::vault::Secrets::new(None, Arc::new(Offline))), mcp }
    }

    /// The connection every remote object shares.
    pub fn rpc(&self) -> &Rpc {
        &self.rpc
    }

    /// True only while the core answers: one cheap read on a root it must serve. A closed socket, a core without the `rpc` switch or a
    /// core of another build gives false, and nothing else in this handle makes it true.
    pub fn is_real(&self) -> bool {
        self.rpc.call::<Value>("InboxApi", 0, "list", json!([""])).is_ok()
    }

    root!(/// The core's notification inbox.
        inbox, InboxApi, dyn InboxApi);
    root!(/// The core's usage log.
        usage, UsageLogApi, dyn UsageLogApi);
    root!(/// Each person's events and alarms.
        calendar, CalendarApi, dyn CalendarApi);
    root!(/// Study plans and lessons.
        study, StudyApi, dyn StudyApi);
    root!(/// The secrets as the interface that never hands out a value (D-SPLIT-03-SECRETS).
        secrets_api, SecretsApi, dyn SecretsApi);
    root!(/// The device watch.
        devwatch, DevWatchApi, dyn DevWatchApi);
    root!(/// The questions that ask the person for a secret.
        secret_requests, SecretRequestsApi, dyn SecretRequestsApi);
    root!(/// The assistant's screen capture.
        screen, ScreenCaptureApi, dyn ScreenCaptureApi);
    root!(/// The developer tools buffers.
        devtools, DevToolsApi, dyn DevToolsApi);
    root!(/// The people of this machine.
        accounts, AccountsApi, dyn AccountsApi);
    root!(/// The audit log (when the core serves it).
        audit, AuditApi, dyn AuditApi);
    root!(/// Chat runs: start, poll events, answer approvals.
        chat_runs, ChatRuns, dyn ChatRuns);
    root!(/// The system settings as the interface that crosses the socket.
        settings_api, SettingsApi, dyn SettingsApi);
    root!(/// The knowledge base.
        kb, KnowledgeBaseApi, dyn KnowledgeBaseApi);
    root!(/// The browser state.
        browser, BrowserStateApi, dyn BrowserStateApi);
    root!(/// The assistant's questions about the person's browser.
        browser_consent, ConsentApi, dyn ConsentApi);
    root!(/// The queue between the assistant's browser tools and the page engine.
        browser_jobs, BrowserJobsApi, dyn BrowserJobsApi);
    root!(/// The external agents' status.
        ext_status, ExtStatusApi, dyn ExtStatusApi);

    // ---- what `ServicesHandle` asks that is not a plain call (called by gen_services.rs)
    pub(crate) fn k_is_real(&self) -> bool {
        RemoteServicesHandle::is_real(self)
    }

    /// The data folder: read once, kept for the life of the handle (a lost core answers an empty path and asks again next time).
    pub(crate) fn k_dir(&self) -> &Path {
        if let Some(p) = self.kept.dir.get() {
            return p;
        }
        match self.rpc.call::<PathBuf>("ServicesHandle", 0, "dir", json!([])) {
            Ok(p) => self.kept.dir.get_or_init(|| p),
            Err(_) => Path::new(""),
        }
    }

    fn rule(&self, cell: &OnceLock<&'static str>, name: &str) -> &'static str {
        if let Some(r) = cell.get() {
            return r;
        }
        match self.rpc.call::<String>("ServicesHandle", 0, name, json!([])) {
            Ok(s) => cell.get_or_init(|| intern(s)),
            Err(_) => "",
        }
    }

    pub(crate) fn k_cloud_stream_prompt_rule(&self) -> &'static str {
        self.rule(&self.kept.cloud_rule, "cloud_stream_prompt_rule")
    }

    pub(crate) fn k_fleet_prompt_rule(&self) -> &'static str {
        self.rule(&self.kept.fleet_rule, "fleet_prompt_rule")
    }

    pub(crate) fn k_mcp(&self) -> &dyn McpApi {
        self.mcp.as_ref()
    }

    pub(crate) fn k_secrets(&self) -> &iyakki_shared::vault::Secrets {
        &self.secrets
    }

    /// A closure that changes a value becomes: read the whole value, change the copy, write the whole value back (the core runs the same closure form).
    pub(crate) fn k_update_sound_with(&self, f: &mut dyn FnMut(&mut SoundSettings)) -> Result<SoundSettings, String> {
        let mut v = ServicesHandle::sound_settings(self);
        f(&mut v);
        self.rpc.call::<Result<SoundSettings, String>>("ServicesHandle", 0, "set_sound_settings", json!([v]))?
    }

    pub(crate) fn k_update_system_sounds_with(&self, f: &mut dyn FnMut(&mut SystemSounds)) -> Result<SystemSounds, String> {
        let mut v = ServicesHandle::system_sounds(self);
        f(&mut v);
        self.rpc.call::<Result<SystemSounds, String>>("ServicesHandle", 0, "set_system_sounds", json!([v]))?
    }

    /// The rows of the router plan: the one answer that carries a `&'static str` (the kind of the row), interned on this side.
    pub(crate) fn k_router_plan_rows(&self, user: &str) -> Vec<(String, String, Vec<String>, &'static str, Option<String>)> {
        type Row = (String, String, Vec<String>, String, Option<String>);
        let rows = self.rpc.call::<Vec<Row>>("ServicesHandle", 0, "router_plan_rows", json!([user])).unwrap_or_default();
        rows.into_iter().map(|(a, b, c, d, e)| (a, b, c, intern(d), e)).collect()
    }
}

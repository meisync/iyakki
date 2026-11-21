//! The real core serves the contract transport of `iyakki-ui-client` (F-SPLIT-07) to a UI program outside the process, over a unix
//! socket on the machine. **Off by default**: nothing listens until an administrator switches it on on the device (the chat tools
//! `ui_socket_status` / `ui_socket_switch`, which ask every time) and the switch is a file in the device's data folder.
//!
//! * Who the caller is belongs to the LISTENER, never to the request: the context is `Transport::LocalSocket` with the account and the
//!   contract scopes the administrator granted for the socket. A request cannot name scopes, cannot be local, and `vault.unlock` and the
//!   answer to a consent question are not on the transport at all (the machine's own seat only).
//! * The socket is a file with mode 0600 in a folder with mode 0700 (the file mode is who may connect); a stale socket file left by a
//!   crash is replaced, a path that is not a socket is refused. No TCP port is opened by this module.
//! * `hello` comes from the real catalog (`hello_for`), so a UI negotiates against what this machine really serves.
//! * Every call goes through the same adapters as the developer-mode channel (`Core::settings`, `vault`, flows of the account), so the
//!   guards, the idempotency keys and the audit lines ("who via uisocket: call [risk] outcome", never a value) are the same ones.
//! * The door has two locks: the file mode, and the peer the kernel reports for every connection (`SO_PEERCRED`): only root and the owner
//!   of the socket file are let in. Binary blocks (`bulk.get`, [`CoreBulk`]) and pushed settings changes (`settings.subscribe`) are served on
//!   the same connection; the caller of both is the listener's context, never the request's.
//! * The switch is read again every two seconds by [`watch`]: switching it off closes the socket and removes the file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_core_api::contract::catalog::hello_for;
use iyakki_core_api::contract::*;
use iyakki_core_api::Capabilities;
use iyakki_sec::policy::{Policy, Role};
use iyakki_ui_client::server::{serve_unix_bulk, BulkSource, RpcServe, Served};
use serde::{Deserialize, Serialize};

use super::{Core, CoreFlows};

/// The file in the device's data folder that holds the switch and the grants.
pub const CONFIG_FILE: &str = "ui-socket.json";

/// The scopes an administrator can grant the socket: the grantable contract scopes of developer mode, minus the consent one (no
/// consent calls are on this transport).
pub fn grantable() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = crate::devmode::contract::GRANTABLE.iter().copied().filter(|s| *s != Scope::CONSENT_READ).collect();
    v.extend(FACET_SCOPES);
    v
}

/// The scopes of the assistant and camera facets on this socket (F-SDK-85, F-SDK-166): use, and the person's yes to a card. They are not scopes
/// of the paired-computer contract (`GRANTABLE`), which does not serve these domains.
pub const FACET_SCOPES: [&str; 7] = [iyakki_ui_client::facets::ASSISTANT_USE, iyakki_ui_client::facets::ASSISTANT_APPROVE, iyakki_ui_client::facets::CAMERAS_VIEW, iyakki_ui_client::facets::CAMERAS_CONSENT, iyakki_ui_client::facets::DISPLAY_VIEW, iyakki_ui_client::facets::FLOWS_EDIT, iyakki_ui_client::facets::FLOWS_HOOKS];

/// The facets of the core this socket may carry besides settings, the vault and the flows: the embedded assistant and the camera views, which
/// the daemon builds over its own services. Without them the domains are not served.
#[derive(Default, Clone)]
pub struct Facets {
    pub assistant: Option<Arc<dyn iyakki_core_api::Assistant>>,
    pub cameras: Option<Arc<dyn iyakki_core_api::Cameras>>,
    /// The root objects of the shell's services (F-SPLIT-03), served only when the config's `rpc` switch is on.
    pub rpc: Option<RpcFactory>,
    /// More blocks for `bulk.get` (a display list or a camera frame producer, F-SDK-29 and F-SPLIT-07): asked after the core's own blocks.
    pub bulk: Option<Arc<dyn BulkSource>>,
}

/// The blocks the real core hands over beside an answer (`bulk.get`, F-SPLIT-07). Who may ask is the listener's context, as for every call:
/// * `settings.all`: every setting the listener's scopes may read, as one JSON object `{key: value}`. It goes through the same adapter
///   as `settings.get` (guards, scopes), so a key the caller may not read is simply not in the block.
///
/// Any other name goes to the extra source of [`Facets::bulk`], and without one is `not_found`. A display-list or frame producer is not
/// here: that needs the display-list protocol of F-SDK-29, which waits for an owner decision.
pub struct CoreBulk {
    core: Arc<Core>,
    extra: Option<Arc<dyn BulkSource>>,
}

impl CoreBulk {
    pub fn new(core: Arc<Core>, extra: Option<Arc<dyn BulkSource>>) -> CoreBulk {
        CoreBulk { core, extra }
    }
}

impl BulkSource for CoreBulk {
    fn get(&self, ctx: &CallContext, name: &str) -> CallResult<Vec<u8>> {
        match name {
            "settings.all" => {
                let mut all = serde_json::Map::new();
                for spec in self.core.settings.schema(ctx)? {
                    // a key this caller may not read is left out; any other refusal is the caller's to see
                    match self.core.settings.get(ctx, &spec.key) {
                        Ok(v) => {
                            all.insert(spec.key, serde_json::to_value(&v).unwrap_or(serde_json::Value::Null));
                        }
                        Err(e) if e.code == ErrorCode::Denied => {}
                        Err(e) => return Err(e),
                    }
                }
                serde_json::to_vec(&all).map_err(|e| CallError::new(ErrorCode::Internal, "uisocket.bulk_encode", &e.to_string()))
            }
            other => match &self.extra {
                Some(x) => x.get(ctx, other),
                None => Err(CallError::new(ErrorCode::NotFound, "uisocket.no_such_block", "This core has no block of that name.")),
            },
        }
    }
}

/// The pace of the first-party UI program on the object-call socket. The catalog's 120 calls a minute is for a program somebody else wrote; the shell
/// of this device asks for several hundred values a second (every frame reads the settings, the accounts, the audit log), so a socket that serves it the
/// objects (`rpc`) is told this much instead (D-SPLIT-07-RATE). The peer is still checked on every connection (root and the socket's owner only).
pub const FIRST_PARTY_RATE: RateLimit = RateLimit { calls: 200_000, per_ms: 1000 };

/// How the composition root gives the core its root objects: one host for the core's own services (`iyakki-compose::rpc::RpcRoots`).
pub type RpcFactory = Arc<dyn Fn(&Arc<Core>) -> Arc<dyn RpcServe> + Send + Sync>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct UiSocketConfig {
    /// Off by default.
    pub enabled: bool,
    /// The contract scopes of every caller on this socket.
    pub scopes: Vec<String>,
    /// The account the UI program acts as (its flows and its audit name).
    pub account: String,
    /// Use the role of that account's template (`user` unless the administrator says `admin`).
    pub admin: bool,
    /// Also serve the shell's services as objects (`rpc.call`, F-SPLIT-03): for the first-party UI program only. Off by default; it needs the
    /// composition root to have given a factory ([`RpcFactory`]).
    pub rpc: bool,
}

impl Default for UiSocketConfig {
    fn default() -> Self {
        UiSocketConfig { enabled: false, scopes: vec![], account: "ui".into(), admin: false, rpc: false }
    }
}

pub fn load(dir: &Path) -> UiSocketConfig {
    std::fs::read(dir.join(CONFIG_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// What the first-party UI program (`iyakki-ui --socket`, supervised by the core) holds on the socket: read the settings, see the vault's state, read the
/// flows and the app events, use the assistant and the camera views and say yes to their cards. It does not hold `settings:write`, `vault:use`,
/// `vault:lock` or the flow rights: those changes go through the objects, where the core does its own checks and its own audit.
pub const PROGRAM_SCOPES: [&str; 11] = [
    Scope::SETTINGS_READ,
    Scope::VAULT_STATUS,
    Scope::FLOWS_READ,
    Scope::FLOWS_RUN,
    iyakki_ui_client::facets::FLOWS_EDIT,
    Scope::EVENTS_READ,
    iyakki_ui_client::facets::ASSISTANT_USE,
    iyakki_ui_client::facets::ASSISTANT_APPROVE,
    iyakki_ui_client::facets::CAMERAS_VIEW,
    iyakki_ui_client::facets::CAMERAS_CONSENT,
    iyakki_ui_client::facets::DISPLAY_VIEW,
];

/// The core is about to start the UI program (the image or an administrator asked for `iyakki.ui=process`, `uiprocess::mode`): make sure the socket the
/// program connects to is on, serves the objects (`rpc`) and holds the scopes of [`PROGRAM_SCOPES`] (D-SPLIT-03-SOCKET). What an administrator set
/// already stays: a wider set of scopes is kept, the account name and the template are kept, a socket that was off or without `rpc` is turned on.
/// `Ok(true)` when the file was written, `Ok(false)` when it already said enough. The watcher of [`watch_with`] opens the socket within two seconds, and
/// the program waits for it.
pub fn hand_over(dir: &Path) -> Result<bool, String> {
    let mut c = load(dir);
    let missing: Vec<&str> = PROGRAM_SCOPES.iter().copied().filter(|s| !c.scopes.iter().any(|h| h == s)).collect();
    if c.enabled && c.rpc && missing.is_empty() {
        return Ok(false);
    }
    c.enabled = true;
    c.rpc = true;
    c.scopes.extend(missing.into_iter().map(String::from));
    c.scopes.sort();
    c.scopes.dedup();
    save(dir, &c)?;
    Ok(true)
}

/// Keep the config (administrators: the caller checked). Refuses a scope that cannot be granted.
pub fn save(dir: &Path, cfg: &UiSocketConfig) -> Result<(), String> {
    let ok = grantable();
    if let Some(bad) = cfg.scopes.iter().find(|s| !ok.contains(&s.as_str())) {
        return Err(format!("{bad} cannot be granted to the UI socket; the scopes that can are: {}", ok.join(", ")));
    }
    if cfg.account.is_empty() || cfg.account.len() > 32 || !cfg.account.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err("the account name is letters, digits, - and _ (1 to 32)".into());
    }
    let p = dir.join(CONFIG_FILE);
    let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

/// The status as plain text (the chat tool and the Settings page say this).
pub fn status_text(dir: &Path, listening: bool) -> String {
    let c = load(dir);
    format!(
        "The UI socket is {}{}. Scopes: {}. Account: {}{}{}. It is a unix socket on this machine only; no network port. Scopes that can be granted: {}.",
        if c.enabled { "on" } else { "off (the default)" },
        if c.enabled && !listening { " (waiting to start)" } else { "" },
        if c.scopes.is_empty() { "none".to_string() } else { c.scopes.join(", ") },
        c.account,
        if c.admin { " (administrator template)" } else { "" },
        if c.rpc { ", and serves the shell's services as objects" } else { "" },
        grantable().join(", ")
    )
}

// thin forwarding wrappers so the transport can own trait objects while the Core stays shared
struct SettingsRef(Arc<Core>);
impl Settings for SettingsRef {
    fn schema(&self, c: &CallContext) -> CallResult<Vec<SettingSpec>> {
        self.0.settings.schema(c)
    }
    fn get(&self, c: &CallContext, k: &str) -> CallResult<SettingValue> {
        self.0.settings.get(c, k)
    }
    fn set(&self, c: &CallContext, k: &str, v: SettingValue, i: Option<&IdempotencyKey>) -> CallResult<SettingChange> {
        self.0.settings.set(c, k, v, i)
    }
    fn watch(&self, c: &CallContext, k: Option<&str>, r: Resume, l: usize) -> CallResult<EventBatch<SettingEvent>> {
        self.0.settings.watch(c, k, r, l)
    }
}
struct VaultRef(Arc<Core>);
impl Vault for VaultRef {
    fn status(&self, c: &CallContext) -> CallResult<VaultStatus> {
        self.0.vault.status(c)
    }
    fn unlock(&self, s: &LocalSeat, p: &str) -> CallResult<VaultStatus> {
        self.0.vault.unlock(s, p)
    }
    fn lock(&self, c: &CallContext) -> CallResult<VaultStatus> {
        self.0.vault.lock(c)
    }
    fn use_secret(&self, c: &CallContext, a: &NamedAction, i: Option<&IdempotencyKey>) -> CallResult<ActionOutcome> {
        self.0.vault.use_secret(c, a, i)
    }
}
/// The flows of the configured account, opened when a call needs them (an account that may not use FLOWS gets `denied`).
struct FlowsRef {
    core: Arc<Core>,
    account: String,
    policy: Policy,
}
impl FlowsRef {
    fn flows(&self) -> CallResult<Arc<CoreFlows>> {
        self.core.flows_for(&self.account, &self.policy)
    }
}
impl Flows for FlowsRef {
    fn list(&self, c: &CallContext) -> CallResult<Vec<FlowInfo>> {
        self.flows()?.list(c)
    }
    fn start(&self, c: &CallContext, f: &str, i: &IdempotencyKey) -> CallResult<RunId> {
        self.flows()?.start(c, f, i)
    }
    fn stop(&self, c: &CallContext, r: &RunId) -> CallResult<RunStatus> {
        self.flows()?.stop(c, r)
    }
    fn status(&self, c: &CallContext, r: &RunId) -> CallResult<RunStatus> {
        self.flows()?.status(c, r)
    }
    fn events(&self, c: &CallContext, r: Option<&RunId>, re: Resume, l: usize) -> CallResult<EventBatch<FlowEvent>> {
        self.flows()?.events(c, r, re, l)
    }
    fn approve(&self, c: &CallContext, r: &RunId, a: &str, al: bool) -> CallResult<RunStatus> {
        self.flows()?.approve(c, r, a, al)
    }
}

/// The app events of the configured account's client devices.
struct AppEventsRef {
    core: Arc<Core>,
    account: String,
}
impl AppEventFeed for AppEventsRef {
    fn register(&self, c: &CallContext, e: &str, f: &str) -> CallResult<()> {
        self.core.appevents.for_person(&self.account).register(c, e, f)
    }
    fn events(&self, c: &CallContext, r: Resume, l: usize) -> CallResult<EventBatch<AppEventBody>> {
        self.core.appevents.for_person(&self.account).events(c, r, l)
    }
}

/// What this listener serves: the real adapters, the real catalog hello, the identity fixed from the config.
pub fn served(core: &Arc<Core>, cfg: &UiSocketConfig, hostname: &str) -> Served {
    served_with(core, cfg, hostname, &Facets::default())
}

/// [`served`] with the assistant and camera facets (each domain is in `hello` only when it is served).
pub fn served_with(core: &Arc<Core>, cfg: &UiSocketConfig, hostname: &str, facets: &Facets) -> Served {
    let caps = Capabilities { display: core.svc.screen.present(), ..Capabilities::none() };
    let scopes: Vec<&str> = cfg.scopes.iter().map(String::as_str).collect();
    let policy = if cfg.admin { Role::Admin.template() } else { Role::User.template() };
    let mut hello = hello_for(hostname, env!("CARGO_PKG_VERSION"), caps);
    if cfg.rpc && facets.rpc.is_some() {
        hello.rate_limit = FIRST_PARTY_RATE;
    }
    let domain = |name: &str, calls: &[&str], events: &[&str]| DomainSupport { domain: name.into(), since_minor: 0, calls: calls.iter().map(|c| c.to_string()).collect(), events: events.iter().map(|c| c.to_string()).collect() };
    // pushed settings changes (`settings.subscribe`, a facet of the transport, not of the catalog): advertised where it is served
    // the same for the events of flow runs and of app events (D-SPLIT-07-PUSH, additive: the catalog does not change)
    for name in ["settings", "flows", "appevents"] {
        if let Some(d) = hello.domains.iter_mut().find(|d| d.domain == name) {
            d.calls.push("subscribe".into());
        }
    }
    if facets.assistant.is_some() {
        hello.domains.push(domain("assistant", &["models", "roles", "open", "set_context", "choose", "send", "poll", "subscribe", "decide", "answer", "cancel", "history", "close"], &["assistant"]));
    }
    if facets.cameras.is_some() {
        hello.domains.push(domain("cameras", &["list", "open", "decide", "state", "set_visible", "frame", "overlay", "snapshot", "ptz", "events", "close"], &[]));
    }
    Served {
        hello,
        settings: Some(Arc::new(SettingsRef(core.clone()))),
        vault: Some(Arc::new(VaultRef(core.clone()))),
        flows: Some(Arc::new(FlowsRef { core: core.clone(), account: cfg.account.clone(), policy })),
        assistant: facets.assistant.clone(),
        cameras: facets.cameras.clone(),
        appevents: Some(Arc::new(AppEventsRef { core: core.clone(), account: cfg.account.clone() })),
        rpc: facets.rpc.as_ref().filter(|_| cfg.rpc).map(|f| f(core)),
        ctx: CallContext::remote(&cfg.account, Transport::LocalSocket, &scopes),
    }
}

/// A listening socket and the thread that serves it. Dropping it closes the socket and removes the file.
#[derive(Debug)]
pub struct UiServer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
}

impl UiServer {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for UiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Listen on `path` (a unix socket, mode 0600; its folder is made with mode 0700). Refuses a path that exists and is not a socket.
pub fn start(core: &Arc<Core>, cfg: &UiSocketConfig, hostname: &str, path: &Path) -> Result<UiServer, String> {
    start_with(core, cfg, hostname, path, &Facets::default())
}

/// [`start`] with the assistant and camera facets.
pub fn start_with(core: &Arc<Core>, cfg: &UiSocketConfig, hostname: &str, path: &Path, facets: &Facets) -> Result<UiServer, String> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("the socket folder: {e}"))?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            let _ = std::fs::remove_file(path); // a stale socket of a crashed run
        }
        Ok(_) => return Err(format!("{} exists and is not a socket", path.display())),
        Err(_) => {}
    }
    let listener = std::os::unix::net::UnixListener::bind(path).map_err(|e| format!("the socket: {e}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("the socket mode: {e}"))?;
    let s = Arc::new(served_with(core, cfg, hostname, facets));
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    // the second lock after the file mode: the kernel names the peer of every connection (`SO_PEERCRED`) and only root and the owner of the socket
    // (the core's own user, who made it) are let in; a connection of any other user is closed before a byte is read
    let owner = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).map_err(|e| format!("the socket owner: {e}"))?.uid()
    };
    let bulk: Arc<dyn BulkSource> = Arc::new(CoreBulk::new(core.clone(), facets.bulk.clone()));
    let thread = std::thread::spawn(move || {
        if let Err(e) = serve_unix_bulk(listener, s, st, Some(owner), Some(bulk)) {
            log::error!("ui socket: {e}");
        }
    });
    log::info!("ui socket: listening on {} for account {} with {} scope(s)", path.display(), cfg.account, cfg.scopes.len());
    Ok(UiServer { stop, thread: Some(thread), path: path.to_path_buf() })
}

/// Follow the switch: while the config says on, listen on `path`; when it says off (or the scopes change), close it. Returns the thread.
pub fn watch(core: Arc<Core>, hostname: String, path: PathBuf) -> std::thread::JoinHandle<()> {
    watch_with(core, hostname, path, Facets::default())
}

/// [`watch`] with the assistant and camera facets.
pub fn watch_with(core: Arc<Core>, hostname: String, path: PathBuf, facets: Facets) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let dir = core.svc.dir().to_path_buf();
        let mut running: Option<(UiSocketConfig, UiServer)> = None;
        loop {
            let cfg = load(&dir);
            let same = running.as_ref().is_some_and(|(c, _)| *c == cfg);
            if !cfg.enabled || !same {
                running = None; // closes and removes the socket
            }
            if cfg.enabled && running.is_none() {
                match start_with(&core, &cfg, &hostname, &path, &facets) {
                    Ok(s) => running = Some((cfg, s)),
                    Err(e) => log::error!("ui socket: {e}"),
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_ui_client::client::Connection;
    use serde_json::json;
    use std::time::{Duration, Instant};

    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn core(tag: &str) -> Arc<Core> {
        let d = crate::testdir::unique("iyakki-uisock", tag);
        Arc::new(Core::new(crate::services::Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap()))
    }

    /// A short folder for the socket (a unix socket path over about 100 bytes fails on Linux).
    fn short(tag: &str) -> PathBuf {
        let p = std::path::PathBuf::from("/tmp").join(format!("iyk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// The core starts the UI program: the socket is switched on with the program's scopes and the objects, once; an administrator's wider grant and
    /// account stay; a second call writes nothing.
    #[test]
    fn handing_the_socket_over_turns_it_on_with_the_programs_scopes_and_keeps_what_an_administrator_set() {
        let d = crate::testdir::unique("iyakki-uisock", "handover");
        assert!(!load(&d).enabled, "off by default");
        assert_eq!(hand_over(&d), Ok(true));
        let c = load(&d);
        assert!(c.enabled && c.rpc, "on, and serving the objects");
        for s in PROGRAM_SCOPES {
            assert!(c.scopes.iter().any(|h| h == s), "{s} is held");
        }
        assert!(!c.scopes.iter().any(|s| s == Scope::SETTINGS_WRITE || s == Scope::VAULT_USE), "the program holds no write or secret scope");
        assert_eq!(hand_over(&d), Ok(false), "already enough: nothing is written");
        // an administrator's choices stay
        let mut wide = load(&d);
        wide.scopes.push(Scope::FLOWS_RUN.into());
        wide.account = "kiosk".into();
        wide.rpc = false;
        save(&d, &wide).unwrap();
        assert_eq!(hand_over(&d), Ok(true), "a socket without the objects is switched back on");
        let c = load(&d);
        assert!(c.rpc && c.scopes.iter().any(|s| s == Scope::FLOWS_RUN) && c.account == "kiosk");
        // every scope of the program can really be granted (`save` refuses the others)
        assert!(PROGRAM_SCOPES.iter().all(|s| grantable().contains(s)));
    }

    struct NoObjects;
    impl RpcServe for NoObjects {
        fn call(&self, _: &CallContext, _: u64, _: u64, _: &str, _: &str, _: serde_json::Value) -> CallResult<serde_json::Value> {
            Err(CallError::new(ErrorCode::NotFound, "test.none", "none"))
        }
        fn release(&self, _: u64, _: Option<u64>) {}
    }

    /// The pace of the socket: an outside client keeps the catalog's limit; the first-party program that is served the objects (`rpc` on and a factory
    /// given) is told the larger one, because the shell reads hundreds of values a second.
    #[test]
    fn the_socket_that_serves_the_objects_is_not_paced_like_an_outside_client() {
        let core = core("rate");
        let factory: RpcFactory = Arc::new(|_| Arc::new(NoObjects) as Arc<dyn RpcServe>);
        let with = Facets { rpc: Some(factory), ..Facets::default() };
        let catalog = hello_for("m", env!("CARGO_PKG_VERSION"), Capabilities::none()).rate_limit;
        assert_eq!(served(&core, &cfg(&["settings:read"]), "m").hello.rate_limit, catalog, "no objects: the catalog's limit");
        let mut on = cfg(&["settings:read"]);
        on.rpc = true;
        assert_eq!(served_with(&core, &on, "m", &Facets::default()).hello.rate_limit, catalog, "rpc on but no factory: nothing is served, nothing is raised");
        assert_eq!(served_with(&core, &cfg(&["settings:read"]), "m", &with).hello.rate_limit, catalog, "a factory but the switch off: the catalog's limit");
        assert_eq!(served_with(&core, &on, "m", &with).hello.rate_limit, FIRST_PARTY_RATE);
    }

    fn cfg(scopes: &[&str]) -> UiSocketConfig {
        UiSocketConfig { enabled: true, scopes: scopes.iter().map(|s| s.to_string()).collect(), account: "ui".into(), admin: false, rpc: false }
    }

    fn wait(mut f: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn it_is_off_by_default_and_refuses_scopes_that_cannot_be_granted() {
        let d = crate::testdir::unique("iyakki-uisock", "cfg");
        assert!(!load(&d).enabled && load(&d).scopes.is_empty());
        assert!(status_text(&d, false).contains("off (the default)"));
        let mut c = cfg(&["consent:read"]);
        assert!(save(&d, &c).unwrap_err().contains("cannot be granted"));
        c.scopes = vec!["local".into()];
        assert!(save(&d, &c).is_err());
        c.scopes = vec!["settings:read".into()];
        c.account = "bad name!".into();
        assert!(save(&d, &c).is_err());
        c.account = "ui".into();
        save(&d, &c).unwrap();
        assert_eq!(load(&d), c);
    }

    #[test]
    fn the_real_core_answers_over_a_unix_socket_with_the_identity_of_the_listener() {
        use std::os::unix::fs::PermissionsExt;
        let core = core("serve");
        let dir = short("a");
        let path = dir.join("ui.sock");
        let server = start(&core, &cfg(&["settings:read"]), "iyakki-test", &path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        // hello is the real catalog's: the first domains with their calls
        let (hello, n) = c.negotiate(&["settings", "flows"]).unwrap();
        assert_eq!(hello.machine, "iyakki-test");
        assert!(hello.domains.iter().any(|d| d.domain == "settings" && d.calls.contains(&"get".to_string())));
        assert!(n.usable.iter().any(|d| d == "settings"), "{n:?}");
        // a read the listener's scopes allow comes from the real settings store
        let schema = c.call("settings.schema", json!({})).unwrap();
        let key = schema[0]["key"].as_str().expect("the real core has settings").to_string();
        assert!(c.call("settings.get", json!({"key": key})).is_ok());
        // a change needs a scope the listener does not hold: refused, and a request cannot grant itself one
        let e = c.call("settings.set", json!({"key": key, "value": {"type": "bool", "value": true}, "scopes": ["settings:write"], "transport": "in_process"})).unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied, "{e:?}");
        // the vault is not unlockable here and a vault read needs its scope
        assert_eq!(c.call("vault.unlock", json!({"password": "x"})).unwrap_err().code, ErrorCode::Unsupported);
        assert_eq!(c.call("vault.status", json!({})).unwrap_err().code, ErrorCode::Denied);
        // the audit of the real adapters names the transport and never a value
        let lines = core.settings.audit_handle().all();
        assert!(lines.iter().any(|l| l.contains("via uisocket") && l.contains("settings.set") && l.contains("refused")), "{lines:?}");
        drop(c);
        drop(server);
        assert!(!path.exists(), "the socket file is removed when the server stops");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_core_hands_a_block_of_settings_over_as_a_memory_file_only_for_what_the_listener_may_read() {
        let core = core("bulk");
        let dir = short("k");
        let path = dir.join("ui.sock");
        let server = start(&core, &cfg(&["settings:read"]), "iyakki-test", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        let block = c.bulk_get("settings.all").unwrap();
        let all: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&block).unwrap();
        let schema = c.call("settings.schema", json!({})).unwrap();
        let key = schema[0]["key"].as_str().unwrap().to_string();
        assert!(all.contains_key(&key), "a key the listener may read is in the block");
        assert_eq!(all[&key], c.call("settings.get", json!({"key": key})).unwrap(), "the block says what settings.get says");
        assert_eq!(c.bulk_get("no.such.block").unwrap_err().code, ErrorCode::NotFound);
        // ordinary calls go on after a block
        let after = c.call("settings.get", json!({"key": key}));
        assert!(after.is_ok(), "{after:?}");
        drop(c);
        drop(server);
        // a listener without the read scope gets no keys at all
        let server = start(&core, &cfg(&[]), "iyakki-test", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        match c.bulk_get("settings.all") {
            Ok(b) => assert_eq!(serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&b).unwrap().len(), 0),
            Err(e) => assert_eq!(e.code, ErrorCode::Denied),
        }
        drop(c);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_change_is_pushed_to_a_subscribed_connection_and_a_listener_without_the_scope_cannot_subscribe() {
        let core = core("push");
        let dir = short("p");
        let path = dir.join("ui.sock");
        let server = start(&core, &cfg(&["settings:read", "settings:write"]), "h", &path).unwrap();
        let mut watcher = Connection::unix(&path, Duration::from_secs(10)).unwrap();
        let (hello, _) = watcher.negotiate(&["settings"]).unwrap();
        assert!(hello.domains.iter().any(|d| d.domain == "settings" && d.calls.contains(&"subscribe".to_string())));
        // the pushed flow and app events are advertised the same way (D-SPLIT-07-PUSH: additive, only where this core serves them)
        for d in ["flows", "appevents"] {
            assert!(hello.domains.iter().any(|x| x.domain == d && x.calls.contains(&"subscribe".to_string())), "{d}");
        }
        watcher.call("settings.subscribe", json!({})).unwrap();
        let mut writer = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        let schema = writer.call("settings.schema", json!({})).unwrap();
        let spec = schema.as_array().unwrap().iter().find(|s| s["default"]["type"] == "bool" && s["write_scope"] == "settings:write" && s["write_risk"] == "change").expect("a plain boolean setting");
        let key = spec["key"].as_str().unwrap().to_string();
        let before = writer.call("settings.get", json!({"key": key})).unwrap();
        let flipped = json!({"type": "bool", "value": !before["value"].as_bool().unwrap_or(false)});
        writer.call("settings.set", json!({"key": key, "value": flipped})).unwrap();
        // the push arrives on its own (the read waits for it; no sleep, no poll by the client)
        let mut seen = None;
        while seen.is_none() {
            watcher.read_push().unwrap();
            for line in watcher.take_pushed() {
                if line["push"] == "settings" {
                    seen = line["events"].as_array().and_then(|e| e.iter().find(|e| e["body"]["key"] == key.as_str() || e["key"] == key.as_str()).cloned());
                }
            }
        }
        assert!(seen.is_some());
        drop(watcher);
        drop(writer);
        drop(server);
        let server = start(&core, &cfg(&[]), "h", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        assert_eq!(c.call("settings.subscribe", json!({})).unwrap_err().code, ErrorCode::Denied);
        drop(c);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_app_event_of_the_real_core_is_pushed_to_the_ui_program_which_holds_its_own_event_scopes_and_a_listener_without_the_scope_is_refused() {
        use iyakki_core_api::appevents::{payload, Val};
        use iyakki_core_api::AppEvents;
        let core = core("appev");
        let dir = short("e");
        let path = dir.join("ui.sock");
        let server = start(&core, &cfg(&["events:read"]), "h", &path).unwrap();
        let mut watcher = Connection::unix(&path, Duration::from_secs(10)).unwrap();
        // the app of this connection is `client.ui`: the table of built-in apps gives it the `events.*` scopes the shell needs, so the register is
        // accepted by the real hub (it is refused for an app the table does not know) and a core event reaches the program without polling
        watcher.call("appevents.register", json!({"event": "session.changed", "filter": ""})).unwrap();
        let refused = watcher.call("appevents.register", json!({"event": "no.such.event", "filter": ""})).unwrap_err();
        assert_eq!(refused.code, ErrorCode::Denied, "{refused:?}");
        watcher.call("appevents.subscribe", json!({})).unwrap();
        core.svc.app_events.emit("session.changed", Some("ui"), payload(&[("state", Val::Text("signed_out".into()))])).unwrap();
        let mut seen = None;
        while seen.is_none() {
            watcher.read_push().unwrap();
            for line in watcher.take_pushed() {
                if line["push"] == "appevents" {
                    seen = line["events"].as_array().and_then(|e| e.iter().find(|e| e["body"]["name"] == "session.changed").cloned());
                }
            }
        }
        assert_eq!(seen.unwrap()["body"]["payload"]["state"], "signed_out", "the payload carries the state word");
        drop(watcher);
        drop(server);
        // a listener without `events:read` cannot register or subscribe
        let server = start(&core, &cfg(&["settings:read"]), "h", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        assert_eq!(c.call("appevents.subscribe", json!({})).unwrap_err().code, ErrorCode::Denied);
        assert_eq!(c.call("appevents.register", json!({"event": "session.changed", "filter": ""})).unwrap_err().code, ErrorCode::Denied);
        drop(c);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flow_run_of_the_real_engine_is_pushed_to_the_ui_program_without_polling_and_the_scopes_decide() {
        let core = core("flowpush");
        let dir = short("f");
        let path = dir.join("ui.sock");
        let server = start(&core, &cfg(&["flows:read", "flows:run"]), "h", &path).unwrap();
        let mut starter = Connection::unix(&path, Duration::from_secs(20)).unwrap();
        // the first call opens the account's engine; then a flow can be put into its store
        assert!(starter.call("flows.list", json!({})).unwrap().as_array().unwrap().is_empty());
        let env = crate::flows::FlowEnv::existing(&core.svc, "ui").unwrap();
        env.create_draft(crate::flows::build_flow(&json!({"name": "Backup", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}]})).unwrap(), "user:ui").unwrap();
        let mut watcher = Connection::unix(&path, Duration::from_secs(20)).unwrap();
        watcher.call("flows.subscribe", json!({})).unwrap();
        let list = starter.call("flows.list", json!({})).unwrap();
        let id = list[0]["id"].as_str().expect("the draft is listed").to_string();
        env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:ui", label: "", changelog: "x", approved_by: None }).unwrap();
        let run = starter.call("flows.start", json!({"flow": id, "idempotency_key": "push-key-real-1"})).unwrap();
        assert!(run.is_string() || run.is_object(), "{run:?}");
        // the events of this run arrive on the watcher's own connection: the read waits for the push, the client never asks again
        let mut kinds: Vec<String> = Vec::new();
        while !kinds.iter().any(|k| k == "finished") {
            watcher.read_push().unwrap();
            for line in watcher.take_pushed() {
                if line["push"] == "flows" {
                    kinds.extend(line["events"].as_array().unwrap().iter().filter_map(|e| e["body"]["type"].as_str().or_else(|| e["type"].as_str()).map(String::from)));
                }
            }
        }
        drop(watcher);
        drop(starter);
        drop(server);
        // without `flows:read` nothing can subscribe; a listener with read but not run cannot start
        let server = start(&core, &cfg(&["flows:read"]), "h", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        assert_eq!(c.call("flows.start", json!({"flow": id, "idempotency_key": "push-key-real-2"})).unwrap_err().code, ErrorCode::Denied);
        drop(c);
        drop(server);
        let server = start(&core, &cfg(&[]), "h", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        assert_eq!(c.call("flows.subscribe", json!({})).unwrap_err().code, ErrorCode::Denied);
        drop(c);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_granted_write_scope_changes_the_real_setting_and_a_stale_or_foreign_path_is_handled() {
        let core = core("write");
        let dir = short("b");
        let path = dir.join("ui.sock");
        // a path that is not a socket is refused and left alone
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "x").unwrap();
        assert!(start(&core, &cfg(&["settings:read"]), "h", &path).unwrap_err().contains("not a socket"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
        std::fs::remove_file(&path).unwrap();
        // a stale socket file of a crashed run is replaced
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let server = start(&core, &cfg(&["settings:read", "settings:write"]), "h", &path).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        let schema = c.call("settings.schema", json!({})).unwrap();
        let spec = schema.as_array().unwrap().iter().find(|s| s["default"]["type"] == "bool" && s["write_scope"] == "settings:write" && s["write_risk"] == "change");
        let spec = spec.expect("the real core has a plain boolean setting a write scope may change");
        {
            let key = spec["key"].as_str().unwrap();
            let before = c.call("settings.get", json!({"key": key})).unwrap();
            let flipped = json!({"type": "bool", "value": !before["value"].as_bool().unwrap_or(false)});
            c.call("settings.set", json!({"key": key, "value": flipped})).unwrap();
            assert_eq!(c.call("settings.get", json!({"key": key})).unwrap()["value"], flipped["value"]);
        }
        drop(c);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ui_core_handle_reads_the_machines_settings_over_the_socket_like_the_local_one_and_is_refused_without_the_scope() {
        use iyakki_core_api::Core as _;
        let core = core("socketcore");
        let local = crate::corehandle::AgentCore::new(core.svc.clone());
        let dir = short("d");
        let path = dir.join("ui.sock");
        let server = start(&core, &cfg(&["settings:read"]), "iyakki-test", &path).unwrap();
        let remote = iyakki_ui_client::socketcore::SocketCore::new(Connection::unix(&path, Duration::from_secs(5)).unwrap());
        // the same keys and the same values as the local handle gives
        let keys = local.settings_view().keys();
        assert!(!keys.is_empty(), "the real core has settings");
        assert_eq!(remote.settings_view().keys(), keys);
        for k in keys.iter().take(8) {
            assert_eq!(remote.settings_view().get(k), local.settings_view().get(k), "{k}");
        }
        assert!(remote.settings_view().get("no.such.setting").is_err());
        // the second domain: the vault status (the scope is granted below; here none, so it is refused while settings answer)
        assert!(remote.vault_view().status("ui").is_err(), "vault.status needs its own scope");
        // the facets that are not served over the socket say so and never pretend
        assert!(remote.mixer().play_wav(iyakki_core_api::SoundKind::Notification, "x", &[]).unwrap_err().contains("not served"));
        assert!(remote.app_status().all("alice").is_empty());
        assert_eq!(remote.monitors().list().len(), 0);
        drop(remote);
        drop(server);
        // the third domain: the flows of the served account, the same list from both handles
        let server = start(&core, &cfg(&["settings:read", "flows:read"]), "iyakki-test", &path).unwrap();
        let remote = iyakki_ui_client::socketcore::SocketCore::new(Connection::unix(&path, Duration::from_secs(5)).unwrap());
        let over_socket = remote.flows_view().list("ignored", true).unwrap();
        assert!(over_socket.is_empty(), "no flow yet: {over_socket:?}");
        assert_eq!(local.flows_view().list("ui", true), Ok(over_socket.clone()), "the served account's engine is the one the socket opened");
        assert!(local.flows_view().list("ui", false).is_err(), "a policy that forbids FLOWS is refused before anything opens");
        assert!(local.flows_view().list("nobody-signed-in", true).unwrap_err().contains("no flows engine"));
        let env = crate::flows::FlowEnv::existing(&core.svc, "ui").unwrap();
        env.create_draft(crate::flows::build_flow(&json!({"name": "Backup", "nodes": [{"id": "n", "kind": "notify", "config": {"title": "x"}}]})).unwrap(), "user:ui").unwrap();
        assert_eq!(remote.flows_view().list("x", true).unwrap().iter().map(|f| f.1.clone()).collect::<Vec<_>>(), vec!["Backup".to_string()]);
        assert_eq!(local.flows_view().list("ui", true).unwrap().len(), 1);
        drop(remote);
        drop(server);
        let server = start(&core, &cfg(&["settings:read", "vault:status"]), "iyakki-test", &path).unwrap();
        let remote = iyakki_ui_client::socketcore::SocketCore::new(Connection::unix(&path, Duration::from_secs(5)).unwrap());
        assert_eq!(remote.vault_view().status("ignored"), local.vault_view().status("ui"), "the same answer as the local handle gives for the served account");
        assert_eq!(remote.vault_view().status("x").map(|s| s.1), Ok(0), "nothing leaks but a count");
        drop(remote);
        drop(server);
        // a listener that grants no settings scope: a read is refused and nothing leaks
        let server = start(&core, &cfg(&[]), "iyakki-test", &path).unwrap();
        let remote = iyakki_ui_client::socketcore::SocketCore::new(Connection::unix(&path, Duration::from_secs(5)).unwrap());
        assert!(remote.settings_view().keys().is_empty());
        assert!(remote.settings_view().get(&keys[0]).is_err());
        drop(remote);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_watcher_follows_the_switch_in_the_data_folder() {
        let core = core("watch");
        let dir = short("c");
        let path = dir.join("ui.sock");
        let data = core.svc.dir().to_path_buf();
        // off: nothing listens
        let _t = watch(core.clone(), "h".into(), path.clone());
        std::thread::sleep(Duration::from_millis(300));
        assert!(!path.exists());
        save(&data, &cfg(&["settings:read"])).unwrap();
        assert!(wait(|| Connection::unix(&path, Duration::from_secs(2)).is_ok_and(|mut c| c.hello().is_ok())), "the socket opens when the switch goes on");
        let mut off = cfg(&["settings:read"]);
        off.enabled = false;
        save(&data, &off).unwrap();
        assert!(wait(|| !path.exists()), "the socket closes when the switch goes off");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_assistant_and_camera_domains_are_served_only_when_given_and_only_with_their_scopes() {
        use iyakki_core_api::assistant::SessionSpec;
        use iyakki_core_api::Core as _;
        let d = crate::testdir::unique("iyakki-uisock", "facets");
        assert!(save(&d, &cfg(&["assistant:use", "assistant:approve", "cameras:view", "cameras:consent"])).is_ok(), "the facet scopes can be granted");
        assert!(grantable().contains(&"cameras:consent"));
        let core = core("facets");
        let agent = crate::corehandle::AgentCore::new(core.svc.clone());
        let fake = Arc::new(iyakki_core_fake::FakeAssistant::new());
        let dir = short("g");
        let path = dir.join("ui.sock");
        // the agent's own core has no assistant configured here: the domain is served and says so in the core's words
        let facets = Facets { assistant: Some(agent.shared_assistant()), cameras: Some(agent.shared_cameras()), rpc: None, bulk: None };
        let server = start_with(&core, &cfg(&["assistant:use", "cameras:view"]), "iyakki-test", &path, &facets).unwrap();
        let mut c = Connection::unix(&path, Duration::from_secs(5)).unwrap();
        let hello = c.hello().unwrap();
        assert!(hello.domains.iter().any(|d| d.domain == "assistant") && hello.domains.iter().any(|d| d.domain == "cameras"));
        let remote = iyakki_ui_client::socketcore::SocketCore::new(c);
        let spec = SessionSpec { conversation: "main".into(), ..Default::default() };
        assert!(remote.assistant().open("u", "sample", &spec).unwrap_err().contains("no embedded assistant"));
        assert!(remote.cameras().list("u", "sample").is_empty());
        drop(remote);
        drop(server);
        // the same socket with a working assistant: a session opens and a message goes in; without the scope it is refused
        let facets = Facets { assistant: Some(fake), cameras: None, rpc: None, bulk: None };
        let server = start_with(&core, &cfg(&["assistant:use"]), "iyakki-test", &path, &facets).unwrap();
        let remote = iyakki_ui_client::socketcore::SocketCore::new(Connection::unix(&path, Duration::from_secs(5)).unwrap());
        let sid = remote.assistant().open("u", "sample", &spec).unwrap().id;
        remote.assistant().send("u", &sid, "hello").unwrap();
        assert!(remote.cameras().open("u", "sample", &iyakki_core_api::cameras::ViewSpec::default()).unwrap_err().contains("does not serve cameras"));
        drop(remote);
        drop(server);
        let none = start_with(&core, &cfg(&["settings:read"]), "iyakki-test", &path, &facets).unwrap();
        let remote = iyakki_ui_client::socketcore::SocketCore::new(Connection::unix(&path, Duration::from_secs(5)).unwrap());
        assert!(remote.assistant().open("u", "sample", &spec).unwrap_err().contains("assistant:use"));
        drop(remote);
        drop(none);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

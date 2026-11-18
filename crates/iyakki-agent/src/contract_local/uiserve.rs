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
//! * The switch is read again every two seconds by [`watch`]: switching it off closes the socket and removes the file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iyakki_core_api::contract::catalog::hello_for;
use iyakki_core_api::contract::*;
use iyakki_core_api::Capabilities;
use iyakki_sec::policy::{Policy, Role};
use iyakki_ui_client::server::{serve_unix, Served};
use serde::{Deserialize, Serialize};

use super::{Core, CoreFlows};

/// The file in the device's data folder that holds the switch and the grants.
pub const CONFIG_FILE: &str = "ui-socket.json";

/// The scopes an administrator can grant the socket: the grantable contract scopes of developer mode, minus the consent one (no
/// consent calls are on this transport).
pub fn grantable() -> Vec<&'static str> {
    crate::devmode::contract::GRANTABLE.iter().copied().filter(|s| *s != Scope::CONSENT_READ).collect()
}

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
}

impl Default for UiSocketConfig {
    fn default() -> Self {
        UiSocketConfig { enabled: false, scopes: vec![], account: "ui".into(), admin: false }
    }
}

pub fn load(dir: &Path) -> UiSocketConfig {
    std::fs::read(dir.join(CONFIG_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
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
        "The UI socket is {}{}. Scopes: {}. Account: {}{}. It is a unix socket on this machine only; no network port. Scopes that can be granted: {}.",
        if c.enabled { "on" } else { "off (the default)" },
        if c.enabled && !listening { " (waiting to start)" } else { "" },
        if c.scopes.is_empty() { "none".to_string() } else { c.scopes.join(", ") },
        c.account,
        if c.admin { " (administrator template)" } else { "" },
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

/// What this listener serves: the real adapters, the real catalog hello, the identity fixed from the config.
pub fn served(core: &Arc<Core>, cfg: &UiSocketConfig, hostname: &str) -> Served {
    let caps = Capabilities { display: core.svc.screen.present(), ..Capabilities::none() };
    let scopes: Vec<&str> = cfg.scopes.iter().map(String::as_str).collect();
    let policy = if cfg.admin { Role::Admin.template() } else { Role::User.template() };
    Served {
        hello: hello_for(hostname, env!("CARGO_PKG_VERSION"), caps),
        settings: Some(Arc::new(SettingsRef(core.clone()))),
        vault: Some(Arc::new(VaultRef(core.clone()))),
        flows: Some(Arc::new(FlowsRef { core: core.clone(), account: cfg.account.clone(), policy })),
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
    let s = Arc::new(served(core, cfg, hostname));
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let thread = std::thread::spawn(move || {
        if let Err(e) = serve_unix(listener, s, st) {
            log::error!("ui socket: {e}");
        }
    });
    log::info!("ui socket: listening on {} for account {} with {} scope(s)", path.display(), cfg.account, cfg.scopes.len());
    Ok(UiServer { stop, thread: Some(thread), path: path.to_path_buf() })
}

/// Follow the switch: while the config says on, listen on `path`; when it says off (or the scopes change), close it. Returns the thread.
pub fn watch(core: Arc<Core>, hostname: String, path: PathBuf) -> std::thread::JoinHandle<()> {
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
                match start(&core, &cfg, &hostname, &path) {
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

    fn cfg(scopes: &[&str]) -> UiSocketConfig {
        UiSocketConfig { enabled: true, scopes: scopes.iter().map(|s| s.to_string()).collect(), account: "ui".into(), admin: false }
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
}

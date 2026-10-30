//! The real firewall of this process: kernel backend, system clock and resolver, the settings file on the data volume, a
//! background thread for the timers, and the blocked-connection log. The shell's Firewall page and boot both use [`engine`].

use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::blocked::BlockLog;
use crate::engine::{AuditFn, Clock, Engine, Resolver, Saved, Store};
use crate::ruleset::Ctx;

/// Where the settings live (machine settings; only administrators change them).
pub const CONF_PATH: &str = "/var/lib/iyakki/firewall.json";

pub struct SysClock;

impl Clock for SysClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
    }
}

pub struct SysResolver;

impl Resolver for SysResolver {
    fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String> {
        (name, 0u16).to_socket_addrs().map(|i| i.map(|a| a.ip()).collect()).map_err(|e| e.to_string())
    }
}

pub struct FileStore {
    pub path: PathBuf,
}

impl Store for FileStore {
    fn load(&self) -> Result<Option<Saved>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(t) => match serde_json::from_str::<Saved>(&t) {
                Ok(s) if s.config.version == crate::model::CONFIG_VERSION => Ok(Some(s)),
                Ok(s) => {
                    iyakki_schema::report(format!("firewall {}: settings version {} is not this release's ({}); the file is left as it is, the safe defaults are used and nothing is saved over it", self.path.display(), s.config.version, crate::model::CONFIG_VERSION));
                    Err(format!("settings version {}", s.config.version))
                }
                Err(e) => {
                    // kept for inspection: `.bad`, or a numbered name when an earlier damaged file is still there
                    let bad = self.path.with_extension("json.bad");
                    let to = if bad.exists() { iyakki_schema::sibling(&self.path, &format!("corrupt-{}", iyakki_schema::now_secs())) } else { bad };
                    let moved = std::fs::rename(&self.path, &to).is_ok();
                    iyakki_schema::report(format!("firewall {}: damaged ({e}); {} and started with the safe defaults", self.path.display(), if moved { format!("moved to {}", to.display()) } else { "could not be moved aside".into() }));
                    Err(e.to_string())
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn save(&self, s: &Saved) -> Result<(), String> {
        use std::io::Write;
        // a file written by a release with another settings version is never replaced
        if let Some(n) = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).and_then(|v| v["config"]["version"].as_u64()).filter(|n| *n > crate::model::CONFIG_VERSION as u64) {
            return Err(format!("The firewall settings were written by a newer IYAKKI (version {n}; this one reads version {}). They are kept as they are; update IYAKKI first.", crate::model::CONFIG_VERSION));
        }
        let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// The emergency command leaves this note for the running firewall, which then marks itself off.
pub const OFF_NOTE: &str = "/run/iyakki-firewall-off";

static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
static LOG: OnceLock<Arc<BlockLog>> = OnceLock::new();

fn make() -> Mutex<Engine> {
    #[cfg(target_os = "linux")]
    let backend: Box<dyn crate::backend::Backend> = Box::new(crate::backend::KernelBackend::new());
    #[cfg(not(target_os = "linux"))]
    let backend: Box<dyn crate::backend::Backend> = Box::new(crate::backend::FakeBackend::default());
    let mut e = Engine::new(backend, Box::new(SysClock), Box::new(SysResolver), Box::new(FileStore { path: CONF_PATH.into() }));
    e.sync_dns = false;
    Mutex::new(e)
}

/// The engine of this process.
pub fn engine() -> MutexGuard<'static, Engine> {
    ENGINE.get_or_init(make).lock().unwrap_or_else(|e| e.into_inner())
}

/// Like [`engine`] but never waits: for drawing a frame (`None` while the timers thread holds it).
pub fn try_engine() -> Option<MutexGuard<'static, Engine>> {
    match ENGINE.get_or_init(make).try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(_) => None,
    }
}

/// True when the firewall is wanted but not in force (the top bar shows a warning). Never waits and never starts anything.
pub fn alert() -> bool {
    ENGINE.get().and_then(|m| m.try_lock().ok()).is_some_and(|e| e.managed() && matches!(e.status(), crate::engine::Status::NotProtected(_) | crate::engine::Status::Unavailable(_)))
}

/// Which engine a page talks to: the process-wide one, or (in tests) its own with fakes.
#[derive(Clone)]
pub enum Handle {
    Global,
    Own(Arc<Mutex<Engine>>),
}

impl Handle {
    pub fn lock(&self) -> MutexGuard<'_, Engine> {
        match self {
            Handle::Global => engine(),
            Handle::Own(m) => m.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

pub fn block_log() -> Arc<BlockLog> {
    LOG.get_or_init(BlockLog::new).clone()
}

/// The direct links (one cable to one other computer) that are up, and the administrator's "allow more" switch. The rules are
/// applied again only when something changed. Called by the link manager of `iyakki_agent::directlink`.
pub fn set_direct_links(ifaces: Vec<String>, allow_more: bool) {
    let mut e = engine();
    if e.ctx().direct_ifaces == ifaces && e.ctx().direct_allow_more == allow_more {
        return;
    }
    let mut c = e.ctx().clone();
    c.direct_ifaces = ifaces;
    c.direct_allow_more = allow_more;
    e.set_ctx(c);
    e.network_changed();
}

/// Where audit entries go (the shell's audit log). Entries made before this is called are delivered at once.
pub fn set_audit(f: AuditFn) {
    engine().set_audit(f);
}

/// The kernel command line value of `key`, `iyakki.x=value`.
pub fn param<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline.split_whitespace().find_map(|t| t.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// The port in an `iyakki.web=` value (`8443`, `host:8443`, `http:9000`); none for the default.
pub fn web_port(v: &str) -> Option<u16> {
    let v = v.trim().strip_prefix("http:").unwrap_or(v.trim());
    let last = v.rsplit(':').next().unwrap_or(v);
    last.parse::<u16>().ok().filter(|p| *p > 1)
}

pub fn is_cloud(cmdline: &str, release: &str) -> bool {
    param(cmdline, "iyakki.mode") == Some("cloud") || release.lines().any(|l| l.trim_start().starts_with("VARIANT=") && l.contains("cloud"))
}

/// The facts about this machine for the generator, from the kernel command line and `/proc`.
pub fn ctx_from(cmdline: &str, release: &str, ipv6: bool) -> Ctx {
    Ctx { now: 0, ssh_boot: param(cmdline, "iyakki.ssh") == Some("1"), ipv6, cloud: is_cloud(cmdline, release), remote_port: param(cmdline, "iyakki.web").and_then(web_port), direct_ifaces: Vec::new(), direct_allow_more: false }
}

fn ipv6_on() -> bool {
    Path::new("/proc/net/if_inet6").exists() && std::fs::read_to_string("/proc/sys/net/ipv6/conf/all/disable_ipv6").map_or(true, |v| v.trim() == "0")
}

/// Boot: apply the saved settings (before the network services listen), start the timers and the blocked log. Never panics;
/// a failure is in the status and the audit log.
pub fn boot(cmdline: &str) {
    let release = std::fs::read_to_string("/etc/iyakki-release").unwrap_or_default();
    {
        let mut e = engine();
        e.set_ctx(ctx_from(cmdline, &release, ipv6_on()));
        e.boot(param(cmdline, "iyakki.firewall") == Some("off"));
    }
    start_log();
    start_ticker();
}

/// `iyakki firewall off`: works from a console or an SSH session even when the shell is stuck. Takes our table out of the
/// kernel, saves "off" and leaves a note so the running firewall agrees. Touches nothing else.
pub fn emergency_off(conf: &Path, note: &Path) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    let mut be = crate::backend::KernelBackend::new();
    #[cfg(not(target_os = "linux"))]
    let mut be = crate::backend::FakeBackend::default();
    emergency_off_with(conf, note, &mut be)
}

/// [`emergency_off`] over any backend (the tests use a fake).
pub fn emergency_off_with(conf: &Path, note: &Path, be: &mut dyn crate::backend::Backend) -> Result<String, String> {
    let store = FileStore { path: conf.into() };
    let mut saved = store.load().ok().flatten().unwrap_or_default();
    saved.config.enabled = false;
    let removed = be.remove().map_err(|e| e.to_string());
    let saved_ok = store.save(&saved);
    let _ = std::fs::write(note, "off\n");
    match (removed, saved_ok) {
        (Ok(()), Ok(())) => Ok("The firewall is off: its rules are removed and it stays off after a restart. Switch it on again in Settings -> Firewall.".into()),
        (Ok(()), Err(e)) => Ok(format!("The firewall rules are removed, but \"off\" could not be saved ({e}); it will come back at the next restart.")),
        (Err(e), _) => Err(format!("Could not remove the firewall rules: {e}")),
    }
}

/// `iyakki firewall status`, from the saved settings and the kernel.
pub fn status_text(conf: &Path) -> String {
    let saved = FileStore { path: conf.into() }.load();
    let mut out = String::new();
    match saved {
        Ok(Some(s)) => out.push_str(&format!("settings: {} rules, {} presets, firewall {}\n", s.config.rules.len(), s.config.presets.len(), if s.config.enabled { "enabled" } else { "disabled" })),
        Ok(None) => out.push_str("settings: none saved (defaults: enabled, all incoming connections blocked)\n"),
        Err(e) => out.push_str(&format!("settings: unreadable ({e}); defaults are used\n")),
    }
    #[cfg(target_os = "linux")]
    {
        match crate::nl::NfSocket::open(3).and_then(|mut s| s.counters()) {
            Ok((p, b)) => out.push_str(&format!("kernel: table inet {} is in force; blocked so far: {p} packets, {b} bytes\n", crate::ruleset::TABLE)),
            Err(e) => out.push_str(&format!("kernel: no table of ours in force ({e})\n")),
        }
    }
    out
}

fn check_off_note() {
    if Path::new(OFF_NOTE).exists() {
        let _ = std::fs::remove_file(OFF_NOTE);
        let mut e = engine();
        let _ = e.disable_now("emergency command (iyakki firewall off)");
    }
}

fn start_log() {
    #[cfg(target_os = "linux")]
    {
        static STARTED: OnceLock<()> = OnceLock::new();
        if !engine().status().protected() || STARTED.set(()).is_err() {
            return;
        }
        if let Err(e) = crate::blocked::start(block_log(), crate::ruleset::LOG_GROUP, || SysClock.now()) {
            log::warn!("firewall: the blocked-connection log could not start ({e}); blocking still works");
        }
    }
}

/// The network came up (boot) or changed.
pub fn network_changed() {
    engine().network_changed();
    start_log();
}

fn net_signature() -> u64 {
    let mut h: u64 = 1469598103934665603;
    for f in ["/proc/net/route", "/proc/net/if_inet6"] {
        for b in std::fs::read(f).unwrap_or_default() {
            h = (h ^ u64::from(b)).wrapping_mul(1099511628211);
        }
    }
    h
}

fn start_ticker() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("iyakki-fw-timers".into()).spawn(|| {
        let mut sig = net_signature();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            check_off_note();
            let due = {
                let mut e = engine();
                let now = net_signature();
                if now != sig {
                    sig = now;
                    e.network_changed();
                }
                e.tick();
                e.dns_due()
            };
            if due.is_empty() {
                continue;
            }
            // lookups can take seconds: never while holding the lock the page needs
            let answers: Vec<_> = due.into_iter().map(|n| (n.clone(), SysResolver.resolve(&n))).collect();
            engine().finish_dns(answers);
        }
    });
}

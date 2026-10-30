//! The running side of SSH tunnels: starts the OpenSSH client for every enabled tunnel, watches it, restarts it with a back-off, and
//! answers the page (Settings → Network → SSH tunnels) and the Browser. The model, the command line and the error words are in
//! [`crate::sshtunnel`]; see `docs/SSH-TUNNELS.md`.
//!
//! * **The program** is the OpenSSH client of the images (`/usr/libexec/iyakki/git-ssh`, [`crate::gitjobs::find_ssh`]). The
//!   [`Runner`] / [`Proc`] pair is the only place a process is started, so the tests use [`FakeRunner`] and no network or server is needed.
//! * **The key** never leaves memory: the private half is read from the vault ([`GitKeys::pair`]) and served to the client by a
//!   [`GitAgent`] that lives exactly as long as that client process; the client gets only `SSH_AUTH_SOCK`.
//! * **Enabled** tunnels are started by [`Manager::tick`] (the supervisor calls it every few seconds) and restarted after
//!   a drop with a back-off of 2, 4, 8 … 300 seconds. A **hard** fault (host not trusted, host key changed, key refused, port taken,
//!   key unavailable) is not retried by itself: the person fixes it and the next Enable or Retry starts again.
//! * **Disabled** tunnels are kept and never started.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::gitagent::{AgentKey, GitAgent};
use crate::gitjobs::Probe;
use crate::gitkeys::{host_check, AuditFn, GitKeys, HostCheck};
use crate::ssh::{HostKey, KnownHosts};
use crate::sshkey::KeyPair;
use crate::sshtunnel::{self as tun, Fault, Ports, Route, SiteRouter, Store, Tunnel};

/// Seconds a client may take to listen before the run counts as timed out.
pub const CONNECT_LIMIT: u64 = 30;
pub const BACKOFF_MAX: u64 = 300;

// ------------------------------------------------------------------ processes

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcState {
    /// Alive; `listening` once the local port is open (after login, with `ExitOnForwardFailure`).
    Running { listening: bool },
    /// Ended, with what it wrote to its error output.
    Exited { stderr: String },
}

/// One client process.
pub trait Proc: Send {
    fn poll(&mut self) -> ProcState;
    fn kill(&mut self);
}

/// What a run needs.
pub struct Launch {
    pub id: String,
    pub args: Vec<String>,
    pub local_port: u16,
    /// The key for the in-memory agent. The fake ignores it.
    pub key: KeyPair,
    pub key_name: String,
}

pub trait Runner: Send + Sync {
    fn start(&self, l: &Launch) -> Result<Box<dyn Proc>, String>;
}

// ------------------------------------------------------------------ state

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Stopped,
    Connecting,
    /// Since this Unix time.
    Connected { since: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Status {
    pub state: State,
    /// The last problem in plain words; kept until the next success.
    pub error: Option<String>,
    pub fault: Option<Fault>,
    pub attempts: u32,
    /// The Unix time of the next automatic try.
    pub retry_at: Option<u64>,
    /// Waits for the person (a hard fault).
    pub needs_you: bool,
}

impl Status {
    /// One line for the list.
    pub fn line(&self, enabled: bool, now: u64) -> String {
        match self.state {
            State::Connected { since } => format!("connected, {} ago", tun::ago(now.saturating_sub(since))),
            State::Connecting => "connecting…".into(),
            State::Stopped if !enabled => "disabled".into(),
            State::Stopped => match (&self.error, self.retry_at) {
                (Some(_), Some(at)) => format!("retrying in {} s", at.saturating_sub(now)),
                (Some(_), None) => "stopped: needs you".into(),
                (None, _) => "starting…".into(),
            },
        }
    }
}

struct Rt {
    proc: Option<Box<dyn Proc>>,
    started: u64,
    status: Status,
}

impl Rt {
    fn new() -> Rt {
        Rt { proc: None, started: 0, status: Status::default() }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub tunnel: Tunnel,
    pub status: Status,
}

/// What the check of one host of the route found.
#[derive(Clone, Debug)]
pub struct HostReport {
    pub host: String,
    pub port: u16,
    pub check: Result<HostCheck, String>,
}

impl HostReport {
    pub fn text(&self) -> String {
        let p = crate::ssh::pattern(&self.host, self.port);
        match &self.check {
            Ok(HostCheck::Trusted) => format!("{p}: its key is trusted."),
            Ok(HostCheck::New(keys)) => format!("{p} is not trusted yet. It presents {}. Compare with the fingerprint its owner gave you, then trust it.", crate::ssh::describe_keys(keys).join(", ")),
            Ok(HostCheck::Changed { recorded, presented }) => crate::gitkeys::changed_text(&self.host, self.port, recorded, presented),
            Err(e) => format!("{p}: {e}"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TestReport {
    pub ok: bool,
    pub lines: Vec<String>,
    /// Hosts to trust before it can work, with the keys they present.
    pub untrusted: Vec<(String, u16, Vec<HostKey>)>,
}

// ------------------------------------------------------------------ the manager

pub struct Manager {
    store: Store,
    keys: Arc<GitKeys>,
    probe: Probe,
    runner: Arc<dyn Runner>,
    ports: Arc<dyn Ports>,
    audit: AuditFn,
    /// Waits between the polls of a test (milliseconds); tests do not wait.
    pause: Arc<dyn Fn(u64) + Send + Sync>,
    state: Mutex<HashMap<String, Rt>>,
    /// True for the manager of a signed-in person ([`Manager::for_services`]): [`Manager::ensure_supervised`] may start its thread.
    auto: bool,
    sup: Mutex<Option<Supervisor>>,
}

fn backoff(attempts: u32) -> u64 {
    (2u64 << attempts.min(8)).min(BACKOFF_MAX)
}

fn slug(name: &str) -> String {
    let s: String = name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.trim_matches('-').chars().take(24).collect()
}

impl Manager {
    /// `dir` is the person's SSH folder (`<data>/ssh/<user>`): `tunnels.json` lives there, the keys and known hosts are `keys`'.
    pub fn new(dir: &Path, keys: Arc<GitKeys>, probe: Probe, runner: Arc<dyn Runner>, ports: Arc<dyn Ports>, audit: AuditFn) -> Manager {
        Manager { store: Store::new(&dir.join("tunnels.json")), keys, probe, runner, ports, audit, pause: Arc::new(|ms| std::thread::sleep(std::time::Duration::from_millis(ms))), state: Mutex::new(HashMap::new()), auto: false, sup: Mutex::new(None) }
    }

    pub fn with_pause(mut self, pause: Arc<dyn Fn(u64) + Send + Sync>) -> Manager {
        self.pause = pause;
        self
    }

    /// The real one for a signed-in person. One per person for the life of the process: [`Manager::shared`].
    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Manager {
        let env = crate::ssh::Env::for_services(svc, user);
        let keys = Arc::new(GitKeys::new(&env.dir, Arc::new(crate::gitkeys::ServicesStore::new(svc, user)), audit.clone(), crate::gitkeys::shared_session()));
        let search = svc.clis.search_path();
        let ports: Arc<dyn Ports> = Arc::new(tun::OsPorts);
        let runner = Arc::new(OpenSshRunner { search: search.clone(), dir: env.dir.clone(), ports: ports.clone(), audit: audit.clone() });
        Manager { auto: true, ..Manager::new(&env.dir, keys, crate::gitjobs::real_probe(search), runner, ports, audit) }
    }

    fn registry() -> &'static Mutex<HashMap<String, Arc<Manager>>> {
        static ALL: std::sync::OnceLock<Mutex<HashMap<String, Arc<Manager>>>> = std::sync::OnceLock::new();
        ALL.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn shared(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Arc<Manager> {
        Self::registry().lock().unwrap().entry(format!("{}|{user}", svc.dir().display())).or_insert_with(|| Arc::new(Manager::for_services(svc, user, audit))).clone()
    }

    /// Starts the supervisor thread of a signed-in person's manager (once). Tests' managers (from [`Manager::new`]) never start one.
    pub fn ensure_supervised(self: &Arc<Manager>) {
        if self.auto {
            let mut sup = self.sup.lock().unwrap();
            if sup.is_none() {
                *sup = Some(self.supervise());
            }
        }
    }

    /// Stops the supervisor thread and every client (this person signs out).
    pub fn end_supervision(&self) {
        let sup = self.sup.lock().unwrap().take();
        drop(sup);
        self.stop_all();
    }

    fn say(&self, action: &str, detail: &str) {
        (self.audit)(&format!("sshtunnel.{action}"), detail);
    }

    fn kh(&self) -> KnownHosts {
        self.keys.known_hosts()
    }

    pub fn keys(&self) -> &GitKeys {
        &self.keys
    }

    // ---------------------------------------------------------------- the list

    pub fn tunnels(&self) -> Vec<Tunnel> {
        self.store.list()
    }

    pub fn get(&self, id: &str) -> Option<Tunnel> {
        self.store.get(id)
    }

    pub fn rows(&self) -> Vec<Row> {
        let st = self.state.lock().unwrap();
        self.store.list().into_iter().map(|t| Row { status: st.get(&t.id).map(|r| r.status.clone()).unwrap_or_default(), tunnel: t }).collect()
    }

    pub fn status(&self, id: &str) -> Status {
        self.state.lock().unwrap().get(id).map(|r| r.status.clone()).unwrap_or_default()
    }

    fn is_live(&self, id: &str) -> bool {
        self.state.lock().unwrap().get(id).is_some_and(|r| r.proc.is_some())
    }

    fn stop_locked(rt: &mut Rt) {
        if let Some(mut p) = rt.proc.take() {
            p.kill();
        }
        rt.status.state = State::Stopped;
    }

    fn stop(&self, id: &str) {
        if let Some(rt) = self.state.lock().unwrap().get_mut(id) {
            Self::stop_locked(rt);
        }
    }

    // ---------------------------------------------------------------- add, edit, enable, delete

    /// Adds (empty `id`) or changes a tunnel. `local_port` 0 picks a free one. With `new_key` a key is made for this tunnel
    /// (`tunnel-<name>`), kept in the vault, and deleted with the tunnel; its public half is in `keys().key(id)`. A changed tunnel that
    /// was running stops and, if enabled, starts again with the next [`Manager::tick`].
    pub fn save(&self, mut t: Tunnel, new_key: bool) -> Result<Tunnel, String> {
        let old = if t.id.is_empty() { None } else { Some(self.store.get(&t.id).ok_or("that tunnel is gone")?) };
        let made = if new_key {
            t.key = "pending".into();
            t.validate_fields()?;
            let existing: Vec<String> = self.keys.keys().into_iter().map(|k| k.id).collect();
            let base = format!("tunnel-{}", slug(&t.name));
            let id = (1u32..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|c| !existing.contains(c)).unwrap_or(base);
            // the key is made last, once everything else is known to be fine
            t.key = id.clone();
            t.own_key = true;
            Some(id)
        } else {
            if old.as_ref().is_some_and(|o| o.own_key && o.key == t.key) {
                t.own_key = true;
            } else {
                t.own_key = false;
            }
            None
        };
        t.validate_fields()?;
        if made.is_none() && !self.keys.keys().iter().any(|k| k.id == t.key) {
            return Err(format!("there is no key called {} (Settings > Git and SSH keys)", t.key));
        }
        let unchanged_port = old.as_ref().is_some_and(|o| o.local_port == t.local_port && t.local_port != 0);
        if t.local_port == 0 {
            let others: Vec<u16> = self.store.list().iter().filter(|x| x.id != t.id).map(|x| x.local_port).collect();
            let mut picked = 0;
            for _ in 0..20 {
                let p = self.ports.pick()?;
                if !others.contains(&p) {
                    picked = p;
                    break;
                }
            }
            if picked == 0 {
                return Err("no free local port could be found".into());
            }
            t.local_port = picked;
        } else if let Some(o) = self.store.list().iter().find(|x| x.id != t.id && x.local_port == t.local_port) {
            return Err(format!("port {} is already used by the tunnel {}", t.local_port, o.name));
        } else if !(unchanged_port && self.is_live(&t.id)) && !self.ports.is_free(t.local_port) {
            return Err(format!("port {} is already used by another program on this device; leave the port empty to get a free one", t.local_port));
        }
        if let Some(id) = &made {
            self.keys.generate(id, &format!("{}@iyakki-tunnel", slug(&t.name)))?;
        }
        let saved = match self.store.put(t) {
            Ok(s) => s,
            Err(e) => {
                if let Some(id) = &made {
                    let _ = self.keys.delete_key(id);
                }
                return Err(e);
            }
        };
        if let Some(o) = old {
            if o != saved {
                self.stop(&saved.id);
                self.state.lock().unwrap().remove(&saved.id);
            }
            if o.own_key && o.key != saved.key {
                let _ = self.keys.delete_key(&o.key);
            }
        }
        self.say("save", &format!("{} {}", saved.name, saved.route_text()));
        Ok(saved)
    }

    /// Enables or disables. Disabling stops the client now; enabling clears an old failure so the next tick starts it.
    pub fn set_enabled(&self, id: &str, on: bool) -> Result<Tunnel, String> {
        let mut t = self.store.get(id).ok_or("that tunnel is gone")?;
        t.enabled = on;
        let t = self.store.put(t)?;
        let mut st = self.state.lock().unwrap();
        let rt = st.entry(id.to_string()).or_insert_with(Rt::new);
        Self::stop_locked(rt);
        rt.status = Status::default();
        drop(st);
        self.say(if on { "enable" } else { "disable" }, &t.name);
        Ok(t)
    }

    /// Clears a failure so the next tick tries again.
    pub fn retry(&self, id: &str) {
        if let Some(rt) = self.state.lock().unwrap().get_mut(id) {
            Self::stop_locked(rt);
            rt.status = Status::default();
        }
    }

    /// Deletes the tunnel: stops it, removes the key made for it from the vault, and forgets its hosts in the known hosts unless
    /// another tunnel uses them. Returns what was removed, in words.
    pub fn remove(&self, id: &str) -> Result<Vec<String>, String> {
        self.stop(id);
        let t = self.store.remove(id)?;
        self.state.lock().unwrap().remove(id);
        let mut done = vec![format!("the tunnel {}", t.name)];
        if t.own_key && self.keys.delete_key(&t.key).is_ok() {
            done.push(format!("its key {} from your vault", t.key));
        }
        let rest = self.store.list();
        for (h, p) in t.hops() {
            if rest.iter().any(|o| o.hops().contains(&(h.clone(), p))) {
                continue;
            }
            if self.kh().forget(&h, p).unwrap_or(0) > 0 {
                done.push(format!("the trusted key of {}", crate::ssh::pattern(&h, p)));
            }
        }
        self.say("delete", &t.name);
        Ok(done)
    }

    // ---------------------------------------------------------------- host keys

    /// What every host of the route presents against what is recorded. Connects to each host (no login).
    pub fn check_hosts(&self, t: &Tunnel) -> Vec<HostReport> {
        t.hops().into_iter().map(|(host, port)| HostReport { check: (self.probe)(&host, port).map(|presented| host_check(&self.kh(), &host, port, presented)), host, port }).collect()
    }

    /// Trusts the key of `host` that has `fingerprint` (the one the person compared). A host with a recorded key that is not the one
    /// presented is refused: the old entry has to be forgotten first, on purpose.
    pub fn trust(&self, host: &str, port: u16, fingerprint: &str) -> Result<String, String> {
        tun::valid_host(host)?;
        let presented = (self.probe)(host, port)?;
        match host_check(&self.kh(), host, port, presented.clone()) {
            HostCheck::Changed { recorded, presented } => Err(crate::gitkeys::changed_text(host, port, &recorded, &presented)),
            _ => {
                let hit: Vec<HostKey> = presented.into_iter().filter(|k| k.fingerprint() == fingerprint).collect();
                if hit.is_empty() {
                    return Err(format!("{} does not present a key with the fingerprint {fingerprint}; nothing was trusted", crate::ssh::pattern(host, port)));
                }
                self.kh().trust(host, port, &hit)?;
                self.say("trust", &format!("{} {fingerprint}", crate::ssh::pattern(host, port)));
                Ok(format!("trusted {} for {}", crate::ssh::describe_keys(&hit).join(", "), crate::ssh::pattern(host, port)))
            }
        }
    }

    pub fn forget_host(&self, host: &str, port: u16) -> Result<usize, String> {
        self.kh().forget(host, port)
    }

    // ---------------------------------------------------------------- running

    fn fault(rt: &mut Rt, f: Fault, now: u64) {
        let hard = !f.retryable();
        rt.status.fault = Some(f);
        rt.status.error = Some(f.words().to_string());
        rt.status.needs_you = hard;
        rt.status.retry_at = if hard { None } else { Some(now + backoff(rt.status.attempts)) };
        rt.status.attempts = rt.status.attempts.saturating_add(1);
        rt.status.state = State::Stopped;
    }

    /// Checks everything that can be checked without a network and starts the client.
    fn launch(&self, t: &Tunnel) -> Result<Box<dyn Proc>, (Fault, Option<String>)> {
        let kh_path = self.keys.known_hosts_path();
        let args = tun::build_args(t, &kh_path).map_err(|e| (Fault::Other, Some(e)))?;
        if kh_path.parent().is_some_and(|d| d.join("config").exists()) {
            return Err((Fault::Other, Some("a .ssh/config file exists in IYAKKI's SSH home; tunnels refuse to run with it (remove it)".into())));
        }
        if t.hops().iter().any(|(h, p)| self.kh().for_host(h, *p).is_empty()) {
            return Err((Fault::NotTrusted, None));
        }
        let key = self.keys.pair(&t.key).map_err(|e| (Fault::KeyUnavailable, Some(format!("{} ({})", Fault::KeyUnavailable.words(), e))))?;
        self.runner.start(&Launch { id: t.id.clone(), args, local_port: t.local_port, key, key_name: t.key.clone() }).map_err(|e| (Fault::Other, Some(e)))
    }

    /// One step of the supervisor: starts what should run, notices what ended, stops what is disabled. `now` is Unix seconds.
    pub fn tick(&self, now: u64) {
        let all = self.store.list();
        let mut st = self.state.lock().unwrap();
        st.retain(|id, rt| {
            let keep = all.iter().any(|t| &t.id == id);
            if !keep {
                Self::stop_locked(rt);
            }
            keep
        });
        for t in &all {
            let rt = st.entry(t.id.clone()).or_insert_with(Rt::new);
            if !t.enabled {
                if rt.proc.is_some() || rt.status.state != State::Stopped {
                    Self::stop_locked(rt);
                }
                continue;
            }
            if let Some(p) = rt.proc.as_mut() {
                match p.poll() {
                    ProcState::Running { listening: true } => {
                        if !matches!(rt.status.state, State::Connected { .. }) {
                            rt.status = Status { state: State::Connected { since: now }, ..Status::default() };
                            self.say("connected", &t.name);
                        }
                    }
                    ProcState::Running { listening: false } => {
                        if now.saturating_sub(rt.started) > CONNECT_LIMIT {
                            Self::stop_locked(rt);
                            Self::fault(rt, Fault::Timeout, now);
                        }
                    }
                    ProcState::Exited { stderr } => {
                        rt.proc = None;
                        let f = tun::classify(&stderr);
                        let was_up = matches!(rt.status.state, State::Connected { .. });
                        if was_up {
                            rt.status.attempts = 0;
                        }
                        Self::fault(rt, if was_up && f == Fault::Other { Fault::Dropped } else { f }, now);
                        self.say("stopped", &format!("{} {:?}", t.name, rt.status.fault));
                    }
                }
                continue;
            }
            if rt.status.needs_you || rt.status.retry_at.is_some_and(|at| now < at) {
                continue;
            }
            match self.launch(t) {
                Ok(p) => {
                    rt.proc = Some(p);
                    rt.started = now;
                    rt.status.state = State::Connecting;
                    self.say("start", &t.name);
                }
                Err((f, text)) => {
                    Self::fault(rt, f, now);
                    if let Some(x) = text {
                        rt.status.error = Some(x);
                    }
                    self.say("failed", &format!("{} {:?}", t.name, f));
                }
            }
        }
    }

    /// Stops every client (the device is shutting down, or the person signed out).
    pub fn stop_all(&self) {
        for rt in self.state.lock().unwrap().values_mut() {
            Self::stop_locked(rt);
        }
    }

    /// Checks a tunnel the way it will run, without enabling it: the hosts' keys, the key, the port, then a real run that waits for
    /// the local port to open and stops again.
    pub fn test(&self, id: &str, now: u64) -> TestReport {
        let mut r = TestReport::default();
        let Some(t) = self.store.get(id) else {
            r.lines.push("That tunnel is gone.".into());
            return r;
        };
        if self.is_live(id) {
            let s = self.status(id);
            r.ok = matches!(s.state, State::Connected { .. });
            r.lines.push(format!("{} is {}.", t.name, s.line(true, now)));
            return r;
        }
        let mut blocked = false;
        for h in self.check_hosts(&t) {
            r.lines.push(h.text());
            match h.check {
                Ok(HostCheck::Trusted) => {}
                Ok(HostCheck::New(keys)) => {
                    blocked = true;
                    r.untrusted.push((h.host, h.port, keys));
                }
                _ => blocked = true,
            }
        }
        if blocked {
            return r;
        }
        if !self.ports.is_free(t.local_port) {
            r.lines.push(Fault::AddressInUse.words().to_string());
            return r;
        }
        let proc = self.launch(&t);
        let mut p = match proc {
            Ok(p) => p,
            Err((f, text)) => {
                r.lines.push(text.unwrap_or_else(|| f.words().to_string()));
                return r;
            }
        };
        for _ in 0..(CONNECT_LIMIT * 2) {
            match p.poll() {
                ProcState::Running { listening: true } => {
                    p.kill();
                    r.ok = true;
                    r.lines.push(format!("Connected. The tunnel works; open {} in the Browser once it is enabled.", t.address()));
                    return r;
                }
                ProcState::Running { listening: false } => (self.pause)(500),
                ProcState::Exited { stderr } => {
                    r.lines.push(tun::classify(&stderr).words().to_string());
                    return r;
                }
            }
        }
        p.kill();
        r.lines.push(Fault::Timeout.words().to_string());
        r
    }

    /// **For the Browser.** The tunnel `router` names for `url`, when it exists, is enabled and is connected now (a page is never
    /// sent to a dead port).
    pub fn route(&self, router: &dyn SiteRouter, url: &str) -> Option<Route> {
        let id = router.tunnel_for(url)?;
        let t = self.store.get(&id)?;
        if !t.enabled || !matches!(self.status(&id).state, State::Connected { .. }) {
            return None;
        }
        Some(Route { id, name: t.name.clone(), kind: t.kind, address: t.address() })
    }

    /// Runs [`Manager::tick`] every two seconds on a thread until the returned handle is dropped. The first tick starts every
    /// enabled tunnel.
    pub fn supervise(self: &Arc<Manager>) -> Supervisor {
        let stop = Arc::new(AtomicBool::new(false));
        let (m, s) = (self.clone(), stop.clone());
        let h = std::thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                m.tick(now);
                for _ in 0..20 {
                    if s.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            m.stop_all();
        });
        Supervisor { stop, handle: Some(h) }
    }
}

pub struct Supervisor {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// **Sign-in hook.** Starts the tunnels the person left enabled: their keys are in the vault that was just opened. Nothing is started
/// (not even a thread) when no tunnel is enabled.
pub fn start_for_login(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) {
    let m = Manager::shared(svc, user, audit);
    if m.tunnels().iter().any(|t| t.enabled) {
        m.ensure_supervised();
    }
}

/// **Sign-out hook.** Stops this person's tunnels (the vault closes with the session, so the keys go too).
pub fn on_logout(svc: &Arc<crate::services::Services>, user: &str) {
    let m = Manager::registry().lock().unwrap().get(&format!("{}|{user}", svc.dir().display())).cloned();
    if let Some(m) = m {
        m.end_supervision();
    }
}

// ------------------------------------------------------------------ the real client

/// Runs OpenSSH with the arguments of [`tun::build_args`], a cut-down environment and the in-memory agent.
pub struct OpenSshRunner {
    pub search: String,
    /// The person's SSH folder: `home/` is `HOME`, `run/<id>/` holds the agent socket while the client runs.
    pub dir: PathBuf,
    pub ports: Arc<dyn Ports>,
    pub audit: AuditFn,
}

struct RealProc {
    child: std::process::Child,
    err: Arc<Mutex<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
    port: u16,
    ports: Arc<dyn Ports>,
    up: bool,
    // last: the agent is closed after the client is gone
    _agent: GitAgent,
}

impl Proc for RealProc {
    fn poll(&mut self) -> ProcState {
        match self.child.try_wait() {
            Ok(None) => {
                // the port is ours when binding it fails (nothing else could have taken it: it was free when the run began)
                self.up = self.up || !self.ports.is_free(self.port);
                ProcState::Running { listening: self.up }
            }
            _ => {
                if let Some(h) = self.reader.take() {
                    let _ = h.join();
                }
                ProcState::Exited { stderr: self.err.lock().unwrap().clone() }
            }
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RealProc {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Runner for OpenSshRunner {
    fn start(&self, l: &Launch) -> Result<Box<dyn Proc>, String> {
        use std::io::Read;
        if !tun::valid_id(&l.id) {
            return Err("bad tunnel id".into());
        }
        let ssh = crate::gitjobs::find_ssh(&self.search)?;
        if !self.ports.is_free(l.local_port) {
            return Err(Fault::AddressInUse.words().into());
        }
        let home = self.dir.join("home");
        std::fs::create_dir_all(home.join(".ssh")).map_err(|e| format!("could not create {}: {e}", home.display()))?;
        let audit = self.audit.clone();
        let name = l.key_name.clone();
        let agent = GitAgent::start(&self.dir.join("run").join(&l.id), vec![AgentKey { id: l.key_name.clone(), pair: l.key.clone() }], vec![], Arc::new(move |_, fp| audit("sshtunnel.sign", &format!("{name} {fp}"))), Arc::new(|_| {}))?;
        let mut cmd = std::process::Command::new(&ssh);
        cmd.args(&l.args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()))
            .env("HOME", &home)
            .env("LC_ALL", "C")
            .env("SSH_AUTH_SOCK", agent.ssh_sock())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("could not run {}: {e}", ssh.display()))?;
        let err = Arc::new(Mutex::new(String::new()));
        let mut pipe = child.stderr.take();
        let e2 = err.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            while let Some(Ok(n)) = pipe.as_mut().map(|p| p.read(&mut buf)) {
                if n == 0 {
                    break;
                }
                let mut s = e2.lock().unwrap();
                if s.len() < 4096 {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        });
        Ok(Box::new(RealProc { child, err, reader: Some(reader), port: l.local_port, ports: self.ports.clone(), up: false, _agent: agent }))
    }
}

// ------------------------------------------------------------------ fakes for the tests

/// What a fake client does.
#[derive(Clone, Debug)]
pub enum Plan {
    /// Listens at once.
    Connect,
    /// Listens after this many polls.
    ConnectAfter(u32),
    /// Ends at once with this error text (an OpenSSH message).
    Fail(String),
    /// The program cannot be started.
    SpawnError(String),
    /// Runs but never listens.
    Hang,
}

pub struct FakeShared {
    pub plan: Plan,
    pub polls: u32,
    pub killed: bool,
    /// Set by [`FakeRunner::drop_all`].
    pub exit: Option<String>,
}

pub struct FakeProc(pub Arc<Mutex<FakeShared>>);

impl Proc for FakeProc {
    fn poll(&mut self) -> ProcState {
        let mut s = self.0.lock().unwrap();
        s.polls += 1;
        if let Some(e) = &s.exit {
            return ProcState::Exited { stderr: e.clone() };
        }
        match s.plan.clone() {
            Plan::Connect => ProcState::Running { listening: true },
            Plan::ConnectAfter(n) => ProcState::Running { listening: s.polls > n },
            Plan::Fail(e) => ProcState::Exited { stderr: e },
            Plan::Hang | Plan::SpawnError(_) => ProcState::Running { listening: false },
        }
    }
    fn kill(&mut self) {
        self.0.lock().unwrap().killed = true;
    }
}

/// A runner with no process: runs follow `plans` (then `Connect`), and every start is recorded.
#[derive(Default)]
pub struct FakeRunner {
    pub plans: Mutex<std::collections::VecDeque<Plan>>,
    /// (tunnel id, arguments, local port, key name) of every start.
    pub launches: Mutex<Vec<(String, Vec<String>, u16, String)>>,
    pub procs: Mutex<Vec<Arc<Mutex<FakeShared>>>>,
}

impl FakeRunner {
    pub fn plan(&self, p: Plan) {
        self.plans.lock().unwrap().push_back(p);
    }
    /// The processes still running (not killed, not ended).
    pub fn alive(&self) -> usize {
        self.procs.lock().unwrap().iter().filter(|p| {
            let s = p.lock().unwrap();
            !s.killed && s.exit.is_none() && !matches!(s.plan, Plan::Fail(_))
        }).count()
    }
    /// Every running client ends with this error text (the network dropped).
    pub fn drop_all(&self, stderr: &str) {
        for p in self.procs.lock().unwrap().iter() {
            let mut s = p.lock().unwrap();
            if !s.killed {
                s.exit = Some(stderr.into());
            }
        }
    }
}

impl Runner for FakeRunner {
    fn start(&self, l: &Launch) -> Result<Box<dyn Proc>, String> {
        let plan = self.plans.lock().unwrap().pop_front().unwrap_or(Plan::Connect);
        if let Plan::SpawnError(e) = &plan {
            return Err(e.clone());
        }
        self.launches.lock().unwrap().push((l.id.clone(), l.args.clone(), l.local_port, l.key_name.clone()));
        let sh = Arc::new(Mutex::new(FakeShared { plan, polls: 0, killed: false, exit: None }));
        self.procs.lock().unwrap().push(sh.clone());
        Ok(Box::new(FakeProc(sh)))
    }
}

/// Ports without a machine: `taken` are used by "another program"; `pick` counts up from 40000.
#[derive(Default)]
pub struct FakePorts {
    pub taken: Mutex<std::collections::HashSet<u16>>,
    next: Mutex<u16>,
}

impl Ports for FakePorts {
    fn is_free(&self, port: u16) -> bool {
        !self.taken.lock().unwrap().contains(&port)
    }
    fn pick(&self) -> Result<u16, String> {
        let mut n = self.next.lock().unwrap();
        loop {
            *n = if *n < 40000 { 40000 } else { *n + 1 };
            if self.is_free(*n) {
                return Ok(*n);
            }
        }
    }
}

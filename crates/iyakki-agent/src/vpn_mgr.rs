//! The VPN manager: keeps the person's imported profiles, brings a WireGuard profile up and down, applies FULL or HYBRID routing,
//! installs the fail-closed guard when the person switched it on, and removes everything again on stop. Design, threat list and
//! owner decisions: `docs/development/VPN-CONNECTOR-PLAN.md`. The rules (importers, route and guard plans, the DNS state machine)
//! are in the crate `iyakki-vpn`; the machine is reached only through [`crate::vpn_sys::Sys`], so every test here runs against
//! [`crate::vpn_sys::FakeSys`].
//!
//! * **Secrets** (WireGuard private and preshared keys, OpenVPN secret blocks) live in the signed-in person's vault under
//!   `vpn.<id>.*`; the profile file holds only public data. Keys are read when a connection is made and handed to the kernel by
//!   netlink; they never reach argv, a file, the audit line, the status or a tool result.
//! * **Order of a connect** (fail closed): guard first, then the interface, the keys, the addresses, the routes, then DNS. Any step
//!   that fails undoes the ones before it; the guard stays when the person asked for it (that is what fail closed means).
//! * **One tunnel at a time.** Enabling a profile disables the others. The interface has a fixed name, so a crash leaves
//!   something [`Manager::recover`] can find and remove without any journal.
//! * **Enabled** profiles connect after sign-in (the vault must be open) and reconnect with a back-off. A hard fault (a refused
//!   route, a server on the cluster network, an unsupported kind) is not retried by itself.
//! * **OpenVPN** profiles connect only when the person has downloaded the OpenVPN program (never shipped); [`crate::vpn_openvpn`] runs it
//!   with a fixed argument list and no scripts, and what the server pushes goes through the same route and DNS checks as WireGuard.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_vpn::dnsroute::{Action, Config as DnsConfig, Event as DnsEvent, Forwarder};
use iyakki_vpn::net::{check_dns, check_endpoint, default_cluster_cidrs, Cidr, Context, EndpointVerdict};
use iyakki_vpn::plan::{self, FailClosedInput, GuardBase, HybridRouteInput, KillSwitchInput, KillSwitchSpec, Proto, Route, RouteInput};
use iyakki_vpn::profile::{Body, Mode, Profile, Settings, WgConfig};
use iyakki_vpn::secret::{ProfileSecrets, Secret};
use iyakki_vpn::wgnl::{DeviceSet, PeerSet};
use iyakki_vpn::{b64, ovpn, wgconf};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::gitkeys::{AuditFn, SecretStore};
use crate::vpn_sys::Sys;
pub use iyakki_shared::vpn_mgr::{State};

/// The one interface name. Fixed, so cleanup after a crash needs no journal.
pub const IFNAME: &str = "iyvpn0";
pub const MAX_PROFILES: usize = 16;
pub const BACKOFF_MAX: u64 = 300;
/// Seconds without a handshake after which a connection counts as stalled, and the age after which a handshake no longer counts.
pub const HANDSHAKE_WAIT: u64 = 30;
pub const HANDSHAKE_STALE: u64 = 180;
/// A heartbeat that comes later than this after the one before it means the computer slept: the tunnel is built again.
pub const RESUME_GAP: u64 = 90;
/// The loopback port of the DNS forwarder (HYBRID with domain rules).
pub const DNS_PORT: u16 = 15353;

// ------------------------------------------------------------------ state


pub use iyakki_shared::vpn_mgr::{ago, describe, guard_line_of, Imported, Light, Row, Status, Summary};

/// What a connect has put in place; [`Manager::teardown`] undoes exactly this.
#[derive(Default)]
struct Applied {
    link: bool,
    routes: Vec<Route>,
    resolv: bool,
    capture: bool,
    guard: bool,
    /// HYBRID with accounts chosen: the policy routing that is in place (taken out first when the tunnel goes).
    policy: Option<iyakki_vpn::policyroute::PolicyPlan>,
    /// HYBRID: the forwarder state machine, the host routes it has made, and (when real sockets are on) the thread that serves it.
    fwd: Option<Arc<Mutex<Forwarder>>>,
    host_routes: Arc<Mutex<BTreeSet<IpAddr>>>,
    dns: Option<crate::vpn_dns::Runner>,
    /// The running OpenVPN program of an OpenVPN profile.
    ovpn: Mutex<Option<Box<dyn crate::vpn_openvpn::OvpnProc>>>,
}

struct Rt {
    status: Status,
    applied: Option<Applied>,
    started: u64,
    fwd: Option<Arc<Mutex<Forwarder>>>,
    host_routes: Arc<Mutex<BTreeSet<IpAddr>>>,
    /// The socket loop of the forwarder; dropping it stops and joins the thread.
    dns: Option<crate::vpn_dns::Runner>,
    /// The network the tunnel was built on (the default route's interface and gateway) and when the heartbeat last looked: a change of the
    /// first is roaming, a long gap in the second is a resume from sleep (F-VPN-05).
    net_sig: Option<(String, Option<std::net::Ipv4Addr>)>,
    last_watch: u64,
}

impl Rt {
    fn new() -> Rt {
        Rt { status: Status::default(), applied: None, started: 0, fwd: None, host_routes: Arc::default(), dns: None, net_sig: None, last_watch: 0 }
    }

    /// One event for the forwarder, if this profile has one.
    fn fwd_event(&self, ev: DnsEvent) -> Vec<Action> {
        self.fwd.as_ref().map(|f| f.lock().unwrap_or_else(std::sync::PoisonError::into_inner).handle(ev)).unwrap_or_default()
    }
}

/// The forwarder state machine plus what applies its route changes, as the socket loop sees it (no manager lock is taken, so
/// stopping the loop from a teardown cannot deadlock).
struct Live {
    fwd: Arc<Mutex<Forwarder>>,
    sys: Arc<dyn Sys>,
    routes: Arc<Mutex<BTreeSet<IpAddr>>>,
}

impl crate::vpn_dns::EventSink for Live {
    fn event(&self, ev: DnsEvent, now: u64) -> Vec<Action> {
        let acts = self.fwd.lock().unwrap_or_else(std::sync::PoisonError::into_inner).handle(ev);
        apply_route_actions(&*self.sys, &self.routes, acts, now)
    }
}

/// Applies the route and set changes of the DNS forwarder; returns the packets the socket loop must send or answer. A route is
/// added **before** the answer that names it is returned, and the guard's set learns the address with the same lifetime.
fn apply_route_actions(sys: &dyn Sys, host_routes: &Mutex<BTreeSet<IpAddr>>, actions: Vec<Action>, now: u64) -> Vec<Action> {
    let mut io = Vec::new();
    for a in actions {
        match a {
            Action::RouteAdd { ip, expires } => {
                let r = Route { dest: host_cidr(ip), via: None, dev: IFNAME.into() };
                if sys.ip(&r.ip_args(true)).is_ok() {
                    let _ = sys.guard_set(ip, Some(expires.saturating_sub(now).max(1)), true);
                    host_routes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(ip);
                }
            }
            Action::RouteDel { ip } => {
                let r = Route { dest: host_cidr(ip), via: None, dev: IFNAME.into() };
                let _ = sys.ip(&r.ip_args(false));
                let _ = sys.guard_set(ip, None, false);
                host_routes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&ip);
            }
            Action::Note(_) => {}
            other => io.push(other),
        }
    }
    io
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    profile: Profile,
    #[serde(default)]
    enabled: bool,
}

struct Fail {
    hard: bool,
    msg: String,
}

fn soft(m: impl Into<String>) -> Fail {
    Fail { hard: false, msg: m.into() }
}
fn hard(m: impl Into<String>) -> Fail {
    Fail { hard: true, msg: m.into() }
}

// ------------------------------------------------------------------ the store

struct Store {
    path: PathBuf,
}

impl Store {
    fn list(&self) -> Vec<Stored> {
        let mut v: Vec<Stored> = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        v.retain(|s| valid_id(&s.profile.id));
        v.sort_by_key(|s| s.profile.name.to_ascii_lowercase());
        v
    }

    fn write(&self, all: &[Stored]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("could not create {}: {e}", d.display()))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("could not write {}: {e}", self.path.display()))?;
        f.write_all(&serde_json::to_vec_pretty(all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() >= 2 && id.len() <= 5 && id.starts_with('v') && id[1..].chars().all(|c| c.is_ascii_digit())
}

fn backoff(attempts: u32) -> u64 {
    (2u64 << attempts.min(8)).min(BACKOFF_MAX)
}

fn secret_names(id: &str, p: &Profile) -> Vec<String> {
    match &p.body {
        Body::Wireguard(w) => {
            let mut v = vec![format!("vpn.{id}.key")];
            v.extend(w.peers.iter().enumerate().filter(|(_, p)| p.has_preshared).map(|(i, _)| format!("vpn.{id}.psk.{i}")));
            v
        }
        Body::Openvpn(o) => o.secret_blocks.iter().map(|b| format!("vpn.{id}.blk.{b}")).chain(std::iter::once(format!("vpn.{id}.auth"))).collect(),
    }
}

// ------------------------------------------------------------------ the manager

pub struct Manager {
    store: Store,
    vault: Arc<dyn SecretStore>,
    sys: Arc<dyn Sys>,
    audit: AuditFn,
    /// One connect, teardown or settings change at a time.
    op: Mutex<()>,
    state: Mutex<HashMap<String, Rt>>,
    auto: bool,
    sup: Mutex<Option<Supervisor>>,
    /// Real forwarder sockets: the loopback port and the socket mark. `None` in tests that run against the fake system layer.
    dns_io: Option<(u16, Option<u32>)>,
    /// Starts the downloaded OpenVPN program; `None`: OpenVPN profiles are kept but cannot be connected.
    ovpn: Option<Arc<dyn crate::vpn_openvpn::OvpnLauncher>>,
}

impl Manager {
    pub fn new(dir: &Path, vault: Arc<dyn SecretStore>, sys: Arc<dyn Sys>, audit: AuditFn) -> Manager {
        Manager { store: Store { path: dir.join("profiles.json") }, vault, sys, audit, op: Mutex::new(()), state: Mutex::new(HashMap::new()), auto: false, sup: Mutex::new(None), dns_io: None, ovpn: None }
    }

    /// Serve the DNS forwarder on real loopback sockets at `port` (the redirect rule points there); `mark` goes on its upstream
    /// sockets. Tests pass `None` for the mark (setting it needs administrator rights).
    pub fn with_dns_sockets(mut self, port: u16, mark: Option<u32>) -> Manager {
        self.dns_io = Some((port, mark));
        self
    }

    /// The folder of this person's VPN data (profiles, `helper.json`).
    fn data_dir(&self) -> PathBuf {
        self.store.path.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    /// Whether the privileged helper is switched on (the `helper.json` setting; off unless turned on).
    pub fn helper_enabled(&self) -> bool {
        crate::vpn_helper::helper_enabled(&self.data_dir())
    }

    /// Switch the privileged helper on or off: init starts or stops it, and the manager uses it from the next sign-in. Audited.
    pub fn set_helper_enabled(&self, on: bool) -> Result<(), String> {
        let dir = self.data_dir();
        let mut s = crate::vpn_helper::read_setting(&dir);
        s.enabled = on;
        crate::vpn_helper::write_setting(&dir, &s)?;
        self.say("helper", if on { "on" } else { "off" });
        Ok(())
    }

    /// Let OpenVPN profiles connect, through this launcher (the real one runs the program the person downloaded).
    pub fn with_openvpn(mut self, l: Arc<dyn crate::vpn_openvpn::OvpnLauncher>) -> Manager {
        self.ovpn = Some(l);
        self
    }

    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Manager {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        let dir = svc.dir().join("vpn").join(safe);
        let vault = Arc::new(crate::gitkeys::ServicesStore::new(svc, user));
        // The helper (a separate privileged process behind a Unix socket, `iyakki vpn-helper`) is used only when its setting is on;
        // it is off unless someone turned it on, and nothing starts the helper by itself yet (docs/development/VPN-CONNECTOR-PLAN.md).
        let hs = crate::vpn_helper::read_setting(&dir);
        let sys: Arc<dyn Sys> = if hs.enabled { Arc::new(crate::vpn_helper::RemoteSys::new(hs.socket)) } else { Arc::new(crate::vpn_sys::OsSys::default()) };
        // the config file of an OpenVPN profile holds secrets: it goes to the memory-backed folder when there is one
        let run_dir = if Path::new("/run").is_dir() { PathBuf::from("/run").join(format!("iyakki-vpn-{}", dir.file_name().and_then(|n| n.to_str()).unwrap_or("x"))) } else { dir.join("run") };
        let launcher = crate::vpn_openvpn::RealLauncher { binary: svc.clis.bin_dir().join("openvpn"), run_dir };
        Manager { auto: true, ..Manager::new(&dir, vault, sys, audit).with_dns_sockets(DNS_PORT, Some(plan::DNS_MARK)).with_openvpn(Arc::new(launcher)) }
    }

    fn registry() -> &'static Mutex<HashMap<String, Arc<Manager>>> {
        static ALL: std::sync::OnceLock<Mutex<HashMap<String, Arc<Manager>>>> = std::sync::OnceLock::new();
        ALL.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// The manager of this account if one was already started (reads only; never starts a VPN or a supervisor).
    pub fn peek(dir: &std::path::Path, user: &str) -> Option<Arc<Manager>> {
        Self::registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&format!("{}|{user}", dir.display())).cloned()
    }

    /// The manager of this person if one exists already (a reader that must not create one with its own audit sink).
    pub fn existing(svc: &Arc<crate::services::Services>, user: &str) -> Option<Arc<Manager>> {
        Self::registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&format!("{}|{user}", svc.dir().display())).cloned()
    }

    pub fn shared(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> Arc<Manager> {
        Self::registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(format!("{}|{user}", svc.dir().display())).or_insert_with(|| Arc::new(Manager::for_services(svc, user, audit))).clone()
    }

    pub fn ensure_supervised(self: &Arc<Manager>) {
        if self.auto {
            let mut sup = self.sup.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if sup.is_none() {
                *sup = Some(self.supervise());
            }
        }
    }

    pub fn end_supervision(&self) {
        let sup = self.sup.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        drop(sup);
        self.stop_all();
    }

    fn say(&self, action: &str, detail: &str) {
        (self.audit)(&format!("vpn.{action}"), detail);
    }

    // ---------------------------------------------------------------- the list

    pub fn profiles(&self) -> Vec<Profile> {
        self.store.list().into_iter().map(|s| s.profile).collect()
    }

    pub fn get(&self, id: &str) -> Option<Profile> {
        self.store.list().into_iter().find(|s| s.profile.id == id).map(|s| s.profile)
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.store.list().into_iter().any(|s| s.profile.id == id && s.enabled)
    }

    pub fn rows(&self) -> Vec<Row> {
        let st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.store.list().into_iter().map(|s| Row { status: st.get(&s.profile.id).map(|r| r.status.clone()).unwrap_or_default(), enabled: s.enabled, profile: s.profile }).collect()
    }

    /// The one-line summary of the enabled profile for the top-bar lock and the Devices line (the shell asks this on its tick; it
    /// only reads the manager's state and never touches the machine).
    pub fn summary(&self) -> Option<Summary> {
        let s = self.store.list().into_iter().find(|s| s.enabled)?;
        let st = self.status(&s.profile.id);
        let mode = if s.profile.settings.mode == Mode::Full { "FULL" } else { "HYBRID" };
        let (light, line) = match st.state {
            State::Connected { .. } => (Light::Secure, format!("VPN connected: {} ({mode})", s.profile.name)),
            State::Connecting => (Light::Working, format!("VPN connecting: {} ({mode})", s.profile.name)),
            State::Stopped if st.guard => (Light::Blocked, format!("VPN down: {}; the guard is blocking the traffic that must not go direct", s.profile.name)),
            State::Stopped if st.needs_you => (Light::Open, format!("VPN stopped: {} needs you (open Settings, Network, VPN)", s.profile.name)),
            State::Stopped => (Light::Open, format!("VPN not connected: {}; traffic goes direct", s.profile.name)),
        };
        Some(Summary { light, line })
    }

    /// The line the Firewall page shows while a VPN guard is in the kernel right now: a FULL profile with its kill switch, or a HYBRID
    /// profile that fails closed. `None` when nothing is blocking (no enabled profile, no guard, or a guard the profile does not ask for).
    /// It reads the manager's state only and never touches the machine.
    pub fn guard_line(&self) -> Option<String> {
        guard_line_of(&self.rows())
    }

    /// What the policy routing of F-VPN-08 is doing right now, in a sentence (`None` when none is in place): how many accounts, chosen and
    /// excluded ranges it covers. Reads the manager's state only.
    pub fn policy_line(&self) -> Option<String> {
        let st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        st.values().filter_map(|rt| rt.applied.as_ref()?.policy.as_ref()).map(|p| {
            format!("Policy routing: {} account(s) go through the tunnel for {} chosen range(s); {} range(s) are always left out.", p.uids().len(), p.include().len(), p.exclude().len())
        }).next()
    }

    /// Tell the startup registry how the VPN is: nothing enabled is "disabled" (nothing to wait for), connected is ready, coming up is starting,
    /// and a profile that is enabled but not connected is a failure with the same one line the top bar shows (it never names a key or a server).
    pub fn report_startup(&self, reg: &crate::startup::Registry) {
        match self.summary() {
            None => {
                reg.disabled("vpn");
            }
            Some(Summary { light: Light::Secure, .. }) => {
                reg.ready("vpn");
            }
            Some(Summary { light: Light::Working, line }) => {
                reg.starting("vpn", line.trim_start_matches("VPN "));
            }
            Some(Summary { line, .. }) => {
                reg.failed("vpn", &line);
            }
        }
    }

    pub fn status(&self, id: &str) -> Status {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(id).map(|r| r.status.clone()).unwrap_or_default()
    }

    // ---------------------------------------------------------------- import

    /// Parses a pasted or read file, keeps the secrets in the vault and the rest in the profile list. The profile starts
    /// **disabled** and in FULL mode with every option off. The error words never contain a value from the file.
    pub fn import(&self, name: &str, input: &[u8]) -> Result<Imported, String> {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut all = self.store.list();
        if all.len() >= MAX_PROFILES {
            return Err(format!("at most {MAX_PROFILES} VPN profiles"));
        }
        let ctx = self.import_context();
        let text = String::from_utf8_lossy(input);
        let imported = if text.contains("[Interface]") { wgconf::parse(name, input, &ctx) } else { ovpn::parse(name, input, &ctx) }.map_err(|e| e.to_string())?;
        if all.iter().any(|s| s.profile.name.eq_ignore_ascii_case(&imported.name)) {
            return Err(format!("there is already a VPN profile called {}", imported.name));
        }
        let id = (1u32..).map(|n| format!("v{n}")).find(|id| !all.iter().any(|s| &s.profile.id == id)).unwrap_or_default();
        let (profile, secrets, warnings) = imported.into_profile(&id);
        let written = self.put_secrets(&id, &profile, &secrets)?;
        all.push(Stored { profile: profile.clone(), enabled: false });
        if let Err(e) = self.store.write(&all) {
            for n in written {
                let _ = self.vault.delete(&n);
            }
            return Err(e);
        }
        self.say("import", &format!("{} ({})", profile.name, profile.kind().as_str()));
        Ok(Imported { profile, warnings })
    }

    fn import_context(&self) -> Context {
        let lan = self.sys.net_info().map(|n| n.lan).unwrap_or_default();
        Context { cluster: default_cluster_cidrs(), lan, tunnel: vec![] }
    }

    fn put_secrets(&self, id: &str, p: &Profile, s: &ProfileSecrets) -> Result<Vec<String>, String> {
        let mut written: Vec<String> = Vec::new();
        let mut put = |name: String, value: &str| -> Result<(), String> {
            if let Err(e) = self.vault.set(&name, value) {
                for n in &written {
                    let _ = self.vault.delete(n);
                }
                return Err(e);
            }
            written.push(name);
            Ok(())
        };
        if let Some(k) = &s.private_key {
            let raw = k.as_key().ok_or("the private key is not 32 bytes")?;
            put(format!("vpn.{id}.key"), &Zeroizing::new(b64::encode(raw)))?;
        }
        for (i, k) in &s.preshared {
            let raw = k.as_key().ok_or("a preshared key is not 32 bytes")?;
            put(format!("vpn.{id}.psk.{i}"), &Zeroizing::new(b64::encode(raw)))?;
        }
        for (n, b) in &s.blocks {
            let text = Zeroizing::new(String::from_utf8_lossy(b.expose()).into_owned());
            put(format!("vpn.{id}.blk.{n}"), &text)?;
        }
        let _ = p;
        Ok(written)
    }

    fn load_secrets(&self, id: &str, p: &Profile) -> Result<ProfileSecrets, String> {
        let mut s = ProfileSecrets::default();
        match &p.body {
            Body::Wireguard(w) => {
                let k = self.vault.get(&format!("vpn.{id}.key"))?;
                s.private_key = Some(Secret::from_key_b64(&k).ok_or("the stored private key is damaged; delete the profile and import it again")?);
                for (i, peer) in w.peers.iter().enumerate() {
                    if peer.has_preshared {
                        let k = self.vault.get(&format!("vpn.{id}.psk.{i}"))?;
                        s.preshared.push((i, Secret::from_key_b64(&k).ok_or("a stored preshared key is damaged")?));
                    }
                }
            }
            Body::Openvpn(o) => {
                for b in &o.secret_blocks {
                    s.blocks.push((b.clone(), Secret::new(self.vault.get(&format!("vpn.{id}.blk.{b}"))?.as_bytes().to_vec())));
                }
            }
        }
        Ok(s)
    }

    /// The user name and password of an OpenVPN profile that asks for them (`auth-user-pass` without a file): typed by the person on the
    /// page, kept only in the vault, never in an argument, a file of ours, the status or a tool result. The program gets them in its
    /// private config file for the moment it starts.
    pub fn set_credentials(&self, id: &str, user: &str, pass: &str) -> Result<(), String> {
        let p = self.get(id).ok_or("that VPN profile is gone")?;
        match &p.body {
            Body::Openvpn(o) if o.needs_credentials => {}
            _ => return Err("this profile does not ask for a user name and password".into()),
        }
        if user.is_empty() || pass.is_empty() || user.len() > 256 || pass.len() > 1024 || user.contains(['\n', '\r', '\0']) || pass.contains(['\n', '\r', '\0']) {
            return Err("the user name and password must be one line each, not empty and not very long".into());
        }
        self.vault.set(&format!("vpn.{id}.auth"), &format!("{user}\n{pass}"))?;
        self.say("credentials", &p.name);
        Ok(())
    }

    /// Does the vault hold the user name and password of this profile?
    pub fn has_credentials(&self, id: &str) -> bool {
        self.vault.get(&format!("vpn.{id}.auth")).is_ok()
    }

    // ---------------------------------------------------------------- settings, enable, delete

    /// Changes the mode, filters and options of a profile (validated). A live connection is rebuilt with the new settings.
    pub fn set_settings(&self, id: &str, settings: Settings, now: u64) -> Result<Profile, String> {
        settings.validate().map_err(|e| e.to_string())?;
        let live = {
            let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut all = self.store.list();
            let s = all.iter_mut().find(|s| s.profile.id == id).ok_or("that VPN profile is gone")?;
            let ctx = self.import_context();
            settings.filters.check_routes(&ctx).map_err(|e| e.to_string())?;
            s.profile.settings = settings;
            let p = s.profile.clone();
            let enabled = s.enabled;
            self.store.write(&all)?;
            self.say("settings", &p.name);
            (p, enabled)
        };
        if live.1 {
            self.restart(id, now);
        }
        self.get(id).ok_or_else(|| "that VPN profile is gone".into())
    }

    /// Enables (connects now, and again after every sign-in) or disables (disconnects and removes everything). Enabling a profile
    /// disables the others. Returns the profile.
    pub fn set_enabled(&self, id: &str, on: bool, now: u64) -> Result<Profile, String> {
        let p = {
            let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut all = self.store.list();
            let p = all.iter().find(|s| s.profile.id == id).map(|s| s.profile.clone()).ok_or("that VPN profile is gone")?;
            if on && p.kind() != iyakki_vpn::Kind::Wireguard {
                match &self.ovpn {
                    Some(l) => l.available()?,
                    None => return Err("OpenVPN profiles are kept, but connecting needs the OpenVPN program, which you download yourself (Settings -> Downloads). It is not available here; use a WireGuard profile.".into()),
                }
            }
            let others: Vec<String> = if on { all.iter().filter(|s| s.enabled && s.profile.id != id).map(|s| s.profile.id.clone()).collect() } else { vec![] };
            for s in &mut all {
                if s.profile.id == id {
                    s.enabled = on;
                } else if on {
                    s.enabled = false;
                }
            }
            self.store.write(&all)?;
            for o in others {
                self.stop_locked(&o, true);
            }
            if !on {
                self.stop_locked(id, true);
            }
            p
        };
        self.say(if on { "enable" } else { "disable" }, &p.name);
        if on {
            self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), Rt::new());
            self.tick(now);
        }
        Ok(p)
    }

    /// Deletes the profile: disconnects, removes its secrets from the vault and everything it put on the system.
    pub fn remove(&self, id: &str) -> Result<Vec<String>, String> {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut all = self.store.list();
        let p = all.iter().find(|s| s.profile.id == id).map(|s| s.profile.clone()).ok_or("that VPN profile is gone")?;
        self.stop_locked(id, true);
        all.retain(|s| s.profile.id != id);
        self.store.write(&all)?;
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(id);
        let mut done = vec![format!("the VPN profile {}", p.name)];
        let names = secret_names(id, &p);
        let mut gone = 0;
        for n in &names {
            if self.vault.delete(n).is_ok() {
                gone += 1;
            }
        }
        if gone > 0 {
            done.push(format!("{gone} secret{} from your vault", if gone == 1 { "" } else { "s" }));
        }
        self.say("delete", &p.name);
        Ok(done)
    }

    /// Clears a failure so the next tick tries again.
    pub fn retry(&self, id: &str, now: u64) {
        {
            let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            self.stop_locked(id, false);
            self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), Rt::new());
        }
        self.tick(now);
    }

    fn restart(&self, id: &str, now: u64) {
        {
            let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            self.stop_locked(id, true);
            self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), Rt::new());
        }
        self.tick(now);
    }

    // ---------------------------------------------------------------- teardown

    /// Stops one profile. With `drop_guard` the guard table goes too (disable, delete); without it the guard stays if the
    /// profile asks for one (a retry, a lost connection).
    fn stop_locked(&self, id: &str, drop_guard: bool) {
        let rt = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(id);
        let keep = !drop_guard && self.get(id).is_some_and(|p| wants_guard(&p.settings));
        if let Some(mut rt) = rt {
            self.teardown(&mut rt, !keep);
        } else if drop_guard {
            self.sys.remove_guard();
        }
        if drop_guard {
            self.sys.remove_guard();
        }
        // a record that says "stopped" so the page shows it
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), Rt::new());
    }

    fn teardown(&self, rt: &mut Rt, drop_guard: bool) {
        // the socket loop first (it joins), so no route is added behind our back
        rt.dns = None;
        for ip in std::mem::take(&mut *rt.host_routes.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            let _ = self.sys.guard_set(ip, None, false);
            let r = Route { dest: host_cidr(ip), via: None, dev: IFNAME.into() };
            let _ = self.sys.ip(&r.ip_args(false));
        }
        rt.fwd = None;
        if let Some(a) = rt.applied.take() {
            self.undo(&a, drop_guard);
        }
        self.sys.tunnel_networks(&[]);
        rt.status.state = State::Stopped;
        rt.status.guard = self.sys.guard_present();
    }

    fn undo(&self, a: &Applied, drop_guard: bool) {
        if let Some(plan) = &a.policy {
            self.sys.remove_policy(plan);
        }
        // the OpenVPN program first, so it cannot put anything back while the rest is taken away
        if let Some(mut p) = a.ovpn.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            p.stop();
        }
        for r in a.routes.iter().rev() {
            let _ = self.sys.ip(&r.ip_args(false));
        }
        if a.capture {
            self.sys.remove_dns_capture();
        }
        if a.resolv {
            self.restore_resolv();
        }
        if a.link {
            if let Ok(args) = plan::link_del_args(IFNAME) {
                let _ = self.sys.ip(&args);
            }
        }
        if a.guard && drop_guard {
            self.sys.remove_guard();
        }
    }

    fn restore_resolv(&self) {
        if let Some(cur) = self.sys.read_resolv() {
            if let Some(text) = plan::resolv_restore(&cur, self.sys.read_backup().as_deref()) {
                let _ = self.sys.write_resolv(&text);
            }
        }
        self.sys.drop_backup();
    }

    pub fn stop_all(&self) {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let ids: Vec<String> = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).keys().cloned().collect();
        for id in ids {
            self.stop_locked(&id, true);
        }
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    /// After a crash or a restart: removes the interface, the DNS capture and a leftover DNS file; keeps the guard only when an
    /// enabled profile asks for one. Nothing is read from a journal: names are fixed.
    pub fn recover(&self) {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let live = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).values().any(|r| r.applied.is_some());
        if live {
            return;
        }
        if self.sys.link_exists(IFNAME) {
            if let Ok(a) = plan::link_del_args(IFNAME) {
                let _ = self.sys.ip(&a);
            }
        }
        self.sys.remove_dns_capture();
        self.restore_resolv();
        let wanted = self.store.list().iter().any(|s| s.enabled && wants_guard(&s.profile.settings));
        if !wanted {
            self.sys.remove_guard();
        }
    }

    // ---------------------------------------------------------------- connect

    fn build_context(&self, net: &crate::vpn_sys::NetInfo, w: &WgConfig) -> Context {
        Context { cluster: default_cluster_cidrs(), lan: net.lan.clone(), tunnel: w.addresses.iter().map(|a| a.network()).collect() }
    }

    /// Everything a connect needs, in order. `a` records what was done so the caller can undo it on an error.
    fn bring_up(&self, id: &str, p: &Profile, a: &mut Applied, rt_warnings: &mut Vec<String>) -> Result<(), Fail> {
        if let Body::Openvpn(o) = &p.body {
            return self.bring_up_openvpn(id, p, o, a, rt_warnings);
        }
        let Body::Wireguard(w) = &p.body else { return Err(hard("only WireGuard profiles can be connected")) };
        p.settings.validate().map_err(|e| hard(e.to_string()))?;
        let secrets = self.load_secrets(id, p).map_err(soft)?;
        let key = secrets.private_key.as_ref().ok_or_else(|| hard("the profile has no private key"))?;
        let net = self.sys.net_info().map_err(soft)?;
        let ctx = self.build_context(&net, w);

        // the server: resolved once, checked, pinned
        let mut pins: Vec<IpAddr> = Vec::new();
        let mut eps: Vec<(IpAddr, u16, Proto)> = Vec::new();
        let mut peer_addr: Vec<Option<SocketAddr>> = Vec::new();
        for peer in &w.peers {
            let Some(e) = &peer.endpoint else {
                peer_addr.push(None);
                continue;
            };
            let ips = self.sys.resolve(&e.host).map_err(soft)?;
            let mut chosen = None;
            for ip in &ips {
                match check_endpoint(*ip, &ctx) {
                    EndpointVerdict::Ok => {}
                    EndpointVerdict::NeedsConfirm(m) => rt_warnings.push(format!("{m}; make sure that is really your VPN server")),
                    EndpointVerdict::Refuse(m) => return Err(hard(format!("the VPN server is refused: {m}"))),
                }
                if !pins.contains(ip) {
                    pins.push(*ip);
                }
                eps.push((*ip, e.port, Proto::Udp));
                chosen.get_or_insert(SocketAddr::new(*ip, e.port));
            }
            peer_addr.push(chosen);
        }
        let allowed: Vec<Cidr> = w.peers.iter().flat_map(|p| p.allowed_ips.iter().copied()).collect();

        // the plans (all checks happen here, before anything changes)
        let full = p.settings.mode == Mode::Full;
        let routes = if full {
            plan::plan_routes(&RouteInput { ifname: IFNAME, physical_if: &net.physical_if, endpoints: &pins, allowed_ips: &allowed, gateway4: net.gateway4, gateway6: net.gateway6, ctx: &ctx })
        } else {
            let nets = p.settings.filters.nets();
            let w2 = p.settings.filters.check_routes(&ctx).map_err(|e| hard(e.to_string()))?;
            rt_warnings.extend(w2);
            plan::plan_hybrid_routes(&HybridRouteInput { ifname: IFNAME, physical_if: &net.physical_if, endpoints: &pins, allowed_ips: &allowed, gateway4: net.gateway4, gateway6: net.gateway6, ctx: &ctx, nets: &nets })
        }
        .map_err(|e| hard(e.to_string()))?;
        rt_warnings.extend(routes.warnings.iter().cloned());

        let dns = if full {
            let (d, wn) = check_dns(&w.dns.iter().map(|i| i.to_string()).collect::<Vec<_>>(), &ctx).map_err(|e| hard(e.to_string()))?;
            rt_warnings.extend(wn);
            if let Some(m) = plan::dns_leak_warning(routes.full_tunnel_v4 || routes.full_tunnel_v6, &d) {
                rt_warnings.push(m);
            }
            d
        } else {
            vec![]
        };

        let guard_spec = self.guard_spec(p, &net, &ctx, &eps).map_err(|e| hard(e.to_string()))?;
        let dns_rules = !full && p.settings.filters.len() > p.settings.filters.nets().len();
        let dns_port = self.dns_io.map_or(DNS_PORT, |d| d.0);
        let mut capture = if dns_rules { Some(plan::dns_capture(dns_port).map_err(|e| hard(e.to_string()))?) } else { None };
        if !full {
            a.fwd = Some(Arc::new(Mutex::new(self.forwarder(p, &ctx, &pins))));
        }

        // apply: the guard first
        if let Some(spec) = &guard_spec {
            self.sys.install_guard(spec).map_err(|e| hard(format!("not connected, because the guard could not be put in place first: {e}")))?;
            a.guard = true;
        }
        // an interface left by a crash would make the add fail
        if self.sys.link_exists(IFNAME) {
            let _ = self.sys.ip(&plan::link_del_args(IFNAME).map_err(|e| hard(e.to_string()))?);
        }
        self.sys.ip(&plan::link_add_args(IFNAME).map_err(|e| hard(e.to_string()))?).map_err(|e| soft(format!("could not create the VPN interface ({e}); is the WireGuard module available?")))?;
        a.link = true;
        let mut peers: Vec<PeerSet> = Vec::new();
        for (i, peer) in w.peers.iter().enumerate() {
            let mut ps = PeerSet::new(&peer.public_key, &peer.allowed_ips);
            ps.preshared = secrets.preshared.iter().find(|(n, _)| *n == i).map(|(_, s)| s);
            ps.endpoint = peer_addr.get(i).copied().flatten();
            ps.keepalive = peer.keepalive;
            ps.replace_allowed_ips = true;
            peers.push(ps);
        }
        let dev = DeviceSet { ifname: IFNAME, private_key: Some(key), listen_port: w.listen_port, fwmark: None, replace_peers: true, peers };
        self.sys.wg_configure(&dev).map_err(|e| soft(format!("could not configure WireGuard: {e}")))?;
        drop(dev);
        for addr in &w.addresses {
            self.sys.ip(&plan::addr_add_args(IFNAME, addr).map_err(|e| hard(e.to_string()))?).map_err(|e| soft(format!("could not set the tunnel address: {e}")))?;
        }
        self.sys.ip(&plan::link_up_args(IFNAME, w.mtu).map_err(|e| hard(e.to_string()))?).map_err(|e| soft(format!("could not bring the interface up: {e}")))?;
        for r in &routes.add {
            self.sys.ip(&r.ip_args(true)).map_err(|e| soft(format!("could not add the route {}: {e}", r.dest)))?;
            a.routes.push(r.clone());
        }
        if !full && !p.settings.policy_uids.is_empty() {
            // F-VPN-08: the plan is applied only now that the tunnel is up; the LAN, the cluster and the servers are always left out
            let include: Vec<Cidr> = p.settings.filters.nets().iter().filter(|(_, inc)| *inc).map(|(c, _)| *c).collect();
            let mut exclude: Vec<Cidr> = net.lan.clone();
            exclude.extend(ctx.cluster.iter().copied());
            exclude.extend(pins.iter().map(|ip| host_cidr(*ip)));
            let planned = iyakki_vpn::policyroute::plan_policy_routes(&iyakki_vpn::policyroute::PolicyInput { ifname: IFNAME, table: iyakki_vpn::policyroute::DEFAULT_TABLE, mark: iyakki_vpn::policyroute::DEFAULT_MARK, priority: iyakki_vpn::policyroute::DEFAULT_PRIORITY, include: &include, exclude: &exclude, uids: &p.settings.policy_uids });
            match planned.map_err(|e| e.to_string()).and_then(|plan| self.sys.install_policy(&plan).map(|()| plan)) {
                Ok(plan) => a.policy = Some(plan),
                Err(e) if p.settings.fail_closed => return Err(hard(format!("the chosen accounts need policy routing: {e}"))),
                Err(e) => rt_warnings.push(format!("the chosen accounts are not sent through the VPN: {e}")),
            }
        }
        if !dns.is_empty() {
            let cur = self.sys.read_resolv().unwrap_or_default();
            // never save our own file as "the original" (a crash left it behind)
            if cur.lines().next() != Some(plan::DNS_MARKER) {
                self.sys.write_backup(&cur).map_err(soft)?;
            }
            a.resolv = true;
            self.sys.write_resolv(&plan::resolv_conf(&dns)).map_err(|e| soft(format!("could not set the VPN's DNS: {e}")))?;
        }
        // the forwarder must be listening and answering before the redirect rule sends the machine's DNS to it
        if let (Some(_), Some((port, mark))) = (&capture, self.dns_io) {
            if let Err(e) = self.start_dns_loop(w, &ctx, a, port, mark) {
                if p.settings.fail_closed {
                    return Err(hard(format!("domain rules need the DNS forwarder: {e}")));
                }
                rt_warnings.push(format!("domain rules will not work: {e}"));
                capture = None;
            }
        }
        if let Some(c) = &capture {
            match self.sys.install_dns_capture(c) {
                Ok(()) => a.capture = true,
                Err(e) if p.settings.fail_closed => return Err(hard(format!("domain rules need the DNS capture rule: {e}"))),
                Err(e) => rt_warnings.push(format!("domain rules will not work: {e}")),
            }
        }
        // the firewall may count the tunnel's own network as the local network (its setting "Networks of a connected VPN")
        let nets: Vec<String> = w.addresses.iter().map(|c| c.network()).map(|n| format!("{}/{}", n.addr, n.prefix)).collect();
        self.sys.tunnel_networks(&nets);
        Ok(())
    }

    /// An OpenVPN profile: the same order as WireGuard (checks, guard, then the changes), with the program creating the interface and
    /// the server's pushed settings (not the program) deciding the addresses, routes and DNS that IYAKKI applies after checking them.
    fn bring_up_openvpn(&self, id: &str, p: &Profile, o: &iyakki_vpn::profile::OvpnConfig, a: &mut Applied, rt_warnings: &mut Vec<String>) -> Result<(), Fail> {
        let launcher = self.ovpn.clone().ok_or_else(|| hard("OpenVPN profiles need the OpenVPN program, which you download yourself (Settings -> Downloads)"))?;
        launcher.available().map_err(hard)?;
        p.settings.validate().map_err(|e| hard(e.to_string()))?;
        let secrets = self.load_secrets(id, p).map_err(soft)?;
        let mut text = Zeroizing::new(ovpn::assemble(o, &secrets).map_err(|e| hard(e.to_string()))?);
        if o.needs_credentials {
            let cred = self.vault.get(&format!("vpn.{id}.auth")).map_err(|_| hard("this profile needs a user name and password: enter them on the VPN page first"))?;
            let (user, pass) = cred.split_once('\n').ok_or_else(|| hard("the saved user name and password are damaged; save them again"))?;
            text.push_str(&format!("\n<auth-user-pass>\n{user}\n{pass}\n</auth-user-pass>\n"));
        }
        let net = self.sys.net_info().map_err(soft)?;
        let ctx = Context { cluster: default_cluster_cidrs(), lan: net.lan.clone(), tunnel: vec![] };
        // the servers: resolved once and checked like a WireGuard endpoint (the program resolves them again itself)
        let mut pins: Vec<IpAddr> = Vec::new();
        let mut eps: Vec<(IpAddr, u16, Proto)> = Vec::new();
        for r in &o.remotes {
            let ips = self.sys.resolve(&r.host).map_err(soft)?;
            let proto = if r.proto.as_deref().is_some_and(|p| p.starts_with("tcp")) { Proto::Tcp } else { Proto::Udp };
            for ip in ips {
                match check_endpoint(ip, &ctx) {
                    EndpointVerdict::Ok => {}
                    EndpointVerdict::NeedsConfirm(m) => rt_warnings.push(format!("{m}; make sure that is really your VPN server")),
                    EndpointVerdict::Refuse(m) => return Err(hard(format!("the VPN server is refused: {m}"))),
                }
                if !pins.contains(&ip) {
                    pins.push(ip);
                }
                eps.push((ip, r.port, proto));
            }
        }
        let full = p.settings.mode == Mode::Full;
        if !full {
            rt_warnings.push("domain rules are not used for OpenVPN profiles; address rules are".into());
        }
        let guard_spec = self.guard_spec(p, &net, &ctx, &eps).map_err(|e| hard(e.to_string()))?;
        if let Some(spec) = &guard_spec {
            self.sys.install_guard(spec).map_err(|e| hard(format!("not connected, because the guard could not be put in place first: {e}")))?;
            a.guard = true;
        }
        if self.sys.link_exists(IFNAME) {
            let _ = self.sys.ip(&plan::link_del_args(IFNAME).map_err(|e| hard(e.to_string()))?);
        }
        let mut proc = launcher.launch(&text, IFNAME).map_err(soft)?;
        drop(text);
        let up = crate::vpn_openvpn::await_up(&mut *proc, &o.routes, o.redirect_gateway, full, &ctx);
        let checked = match up {
            Ok(c) => c,
            Err(e) => {
                proc.stop();
                return Err(Fail { hard: e.hard, msg: e.msg });
            }
        };
        *a.ovpn.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(proc);
        a.link = true;
        rt_warnings.extend(checked.warnings.iter().cloned());
        // the pushed addresses count as the tunnel's own network in the route checks from here on
        let ctx = Context { tunnel: checked.addresses.iter().map(|c| c.network()).collect(), ..ctx };
        let routes = if full {
            plan::plan_routes(&RouteInput { ifname: IFNAME, physical_if: &net.physical_if, endpoints: &pins, allowed_ips: &checked.allowed, gateway4: net.gateway4, gateway6: net.gateway6, ctx: &ctx })
        } else {
            let nets = p.settings.filters.nets();
            let w2 = p.settings.filters.check_routes(&ctx).map_err(|e| hard(e.to_string()))?;
            rt_warnings.extend(w2);
            plan::plan_hybrid_routes(&HybridRouteInput { ifname: IFNAME, physical_if: &net.physical_if, endpoints: &pins, allowed_ips: &checked.allowed, gateway4: net.gateway4, gateway6: net.gateway6, ctx: &ctx, nets: &nets })
        }
        .map_err(|e| hard(e.to_string()))?;
        rt_warnings.extend(routes.warnings.iter().cloned());
        for addr in &checked.addresses {
            self.sys.ip(&plan::addr_add_args(IFNAME, addr).map_err(|e| hard(e.to_string()))?).map_err(|e| soft(format!("could not set the tunnel address: {e}")))?;
        }
        self.sys.ip(&plan::link_up_args(IFNAME, checked.mtu).map_err(|e| hard(e.to_string()))?).map_err(|e| soft(format!("could not bring the interface up: {e}")))?;
        for r in &routes.add {
            self.sys.ip(&r.ip_args(true)).map_err(|e| soft(format!("could not add the route {}: {e}", r.dest)))?;
            a.routes.push(r.clone());
        }
        if !checked.dns.is_empty() {
            let cur = self.sys.read_resolv().unwrap_or_default();
            if cur.lines().next() != Some(plan::DNS_MARKER) {
                self.sys.write_backup(&cur).map_err(soft)?;
            }
            a.resolv = true;
            self.sys.write_resolv(&plan::resolv_conf(&checked.dns)).map_err(|e| soft(format!("could not set the VPN's DNS: {e}")))?;
        }
        let nets: Vec<String> = checked.addresses.iter().map(|c| c.network()).map(|n| format!("{}/{}", n.addr, n.prefix)).collect();
        self.sys.tunnel_networks(&nets);
        Ok(())
    }

    /// Binds the forwarder's loopback sockets and starts serving them. Needs a DNS server in the profile that is reachable through
    /// the tunnel (a host route is added when its address is not inside the tunnel's own network) and at least one system resolver.
    fn start_dns_loop(&self, w: &WgConfig, ctx: &Context, a: &mut Applied, port: u16, mark: Option<u32>) -> Result<(), String> {
        let (servers, _) = check_dns(&w.dns.iter().map(|i| i.to_string()).collect::<Vec<_>>(), ctx).map_err(|e| e.to_string())?;
        let vpn = *servers.first().ok_or("the profile lists no DNS server for the tunnel names (add DNS = ... to the profile)")?;
        let in_tunnel_net = w.addresses.iter().any(|c| c.network().contains_ip(vpn));
        let allowed = w.peers.iter().any(|p| p.allowed_ips.iter().any(|c| c.contains_ip(vpn)));
        if !in_tunnel_net && !allowed {
            return Err(format!("the VPN's DNS server {vpn} is not inside the peer's AllowedIPs, so the tunnel would drop the questions"));
        }
        let system = crate::vpn_dns::parse_nameservers(&self.sys.read_resolv().unwrap_or_default());
        if system.is_empty() {
            return Err("no system DNS server was found in resolv.conf for the names that go direct".into());
        }
        let bound = crate::vpn_dns::Bound::bind(port)?;
        if !in_tunnel_net {
            let r = Route { dest: host_cidr(vpn), via: None, dev: IFNAME.into() };
            self.sys.ip(&r.ip_args(true)).map_err(|e| format!("could not route the VPN's DNS server through the tunnel ({e})"))?;
            a.routes.push(r);
        }
        let fwd = a.fwd.clone().ok_or("no forwarder")?;
        let sink = Arc::new(Live { fwd, sys: self.sys.clone(), routes: a.host_routes.clone() });
        let ups = crate::vpn_dns::Upstreams { system, vpn: vec![SocketAddr::new(vpn, 53)] };
        a.dns = Some(crate::vpn_dns::start(bound, ups, mark, sink)?);
        Ok(())
    }

    fn guard_spec(&self, p: &Profile, net: &crate::vpn_sys::NetInfo, ctx: &Context, eps: &[(IpAddr, u16, Proto)]) -> Result<Option<KillSwitchSpec>, iyakki_vpn::VpnError> {
        let base = GuardBase { ifname: IFNAME, physical_ifs: &net.physical_ifs, endpoints: eps, lan: &net.lan, cluster: &ctx.cluster, cluster_if_prefixes: &["cni", "flannel", "veth"], dev_port: plan::DEV_PORT };
        match p.settings.mode {
            Mode::Full if p.settings.kill_switch => plan::kill_switch(&KillSwitchInput { base, block_dns_outside: false }).map(Some),
            Mode::Hybrid if p.settings.fail_closed => {
                let nets = p.settings.filters.nets();
                plan::fail_closed_guard(&FailClosedInput { base, nets: &nets }).map(Some)
            }
            _ => Ok(None),
        }
    }

    fn connect(&self, id: &str, now: u64) {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(p) = self.get(id) else { return };
        if !self.is_enabled(id) {
            return;
        }
        let mut applied = Applied::default();
        let mut warnings = Vec::new();
        let r = self.bring_up(id, &p, &mut applied, &mut warnings);
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let rt = st.entry(id.to_string()).or_insert_with(Rt::new);
        rt.status.attempts += 1;
        match r {
            Ok(()) => {
                rt.started = now;
                rt.last_watch = now;
                rt.net_sig = self.sys.net_info().ok().map(|n| (n.physical_if, n.gateway4));
                rt.status.state = State::Connecting;
                rt.status.error = None;
                rt.status.retry_at = None;
                rt.status.needs_you = false;
                rt.status.warnings = warnings;
                rt.status.guard = applied.guard;
                if p.settings.mode == Mode::Hybrid {
                    rt.fwd = applied.fwd.take();
                    rt.host_routes = applied.host_routes.clone();
                    rt.dns = applied.dns.take();
                }
                rt.applied = Some(applied);
                drop(st);
                self.say("connect", &p.name);
            }
            Err(f) => {
                drop(st);
                let keep = wants_guard(&p.settings) && applied.guard;
                applied.dns = None; // stop the socket loop (joins) before anything it uses is removed
                for ip in std::mem::take(&mut *applied.host_routes.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
                    let _ = self.sys.guard_set(ip, None, false);
                    let _ = self.sys.ip(&Route { dest: host_cidr(ip), via: None, dev: IFNAME.into() }.ip_args(false));
                }
                self.undo(&applied, !keep);
                let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let rt = st.entry(id.to_string()).or_insert_with(Rt::new);
                rt.status.state = State::Stopped;
                rt.status.guard = self.sys.guard_present();
                rt.status.error = Some(if keep { format!("{} The guard stays in place while this profile is enabled; local network access is not affected. Disable the profile to remove it.", f.msg) } else { f.msg.clone() });
                rt.status.needs_you = f.hard;
                rt.status.retry_at = (!f.hard).then(|| now + backoff(rt.status.attempts));
                rt.status.warnings = warnings;
                drop(st);
                self.say("fail", &p.name);
            }
        }
    }

    fn forwarder(&self, p: &Profile, ctx: &Context, endpoints: &[IpAddr]) -> Forwarder {
        let mut f = Forwarder::new(DnsConfig::new(p.settings.fail_closed, endpoints.to_vec(), ctx.clone()), p.settings.filters.clone());
        let _ = f.handle(DnsEvent::TunnelState(false));
        f
    }

    // ---------------------------------------------------------------- the heartbeat

    /// Connects enabled profiles that are not up, watches the ones that are, and lets the DNS forwarder expire its routes.
    pub fn tick(&self, now: u64) {
        for s in self.store.list() {
            if !s.enabled {
                continue;
            }
            let id = s.profile.id.clone();
            let (live, retry_at, needs_you) = {
                let st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                match st.get(&id) {
                    Some(r) => (r.applied.is_some(), r.status.retry_at, r.status.needs_you),
                    None => (false, None, false),
                }
            };
            if live {
                if let Some(why) = self.moved_or_resumed(&id, now) {
                    self.say(why, &s.profile.name);
                    self.restart(&id, now);
                    continue;
                }
                self.watch(&id, now);
            } else if !needs_you && retry_at.is_none_or(|at| now >= at) {
                self.connect(&id, now);
            }
        }
        self.report_startup(crate::startup::global());
    }

    /// Roaming and resume (F-VPN-05): `Some("roam")` when the default route moved to another interface or gateway since the tunnel was built,
    /// `Some("resume")` when the heartbeat was away longer than [`RESUME_GAP`]. The caller builds the tunnel again, so the server address is
    /// resolved and pinned afresh, the host route and the guard follow the new network and a new handshake is made at once.
    fn moved_or_resumed(&self, id: &str, now: u64) -> Option<&'static str> {
        let sig = self.sys.net_info().ok().map(|n| (n.physical_if, n.gateway4));
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let rt = st.get_mut(id)?;
        let gap = rt.last_watch != 0 && now.saturating_sub(rt.last_watch) > RESUME_GAP;
        rt.last_watch = now;
        if gap {
            return Some("resume");
        }
        match (&rt.net_sig, sig) {
            (Some(old), Some(new)) if *old != new && !new.0.is_empty() => Some("roam"),
            (None, Some(new)) => {
                rt.net_sig = Some(new);
                None
            }
            _ => None,
        }
    }

    fn watch(&self, id: &str, now: u64) {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let p = self.get(id);
        let gone = !self.sys.link_exists(IFNAME);
        if p.as_ref().is_some_and(|p| p.kind() == iyakki_vpn::Kind::Openvpn) {
            self.watch_openvpn(id, now, gone);
            return;
        }
        let status = if gone { Err("the VPN interface disappeared".to_string()) } else { self.sys.wg_status(IFNAME) };
        let mut actions = Vec::new();
        let mut lost: Option<String> = None;
        {
            let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(rt) = st.get_mut(id) else { return };
            match status {
                Err(e) => lost = Some(e),
                Ok(d) => {
                    rt.status.rx = d.peers.iter().map(|p| p.rx_bytes).sum();
                    rt.status.tx = d.peers.iter().map(|p| p.tx_bytes).sum();
                    let hs = d.peers.iter().filter_map(|p| p.last_handshake).max();
                    rt.status.last_handshake = hs;
                    let fresh = hs.is_some_and(|h| (now as i64 - h).max(0) as u64 <= HANDSHAKE_STALE);
                    let was_up = matches!(rt.status.state, State::Connected { .. });
                    if fresh {
                        if !was_up {
                            rt.status.state = State::Connected { since: now };
                            rt.status.error = None;
                            rt.status.attempts = 0;
                            actions.extend(rt.fwd_event(DnsEvent::TunnelState(true)));
                        }
                    } else {
                        if was_up {
                            rt.status.state = State::Connecting;
                            rt.status.error = Some("the last handshake is old; waiting for the server".into());
                            actions.extend(rt.fwd_event(DnsEvent::TunnelState(false)));
                        } else if now.saturating_sub(rt.started) > HANDSHAKE_WAIT {
                            rt.status.error = Some("no answer from the server yet: check the server address, the keys and that UDP is not blocked".into());
                        }
                    }
                    actions.extend(rt.fwd_event(DnsEvent::Tick { now }));
                }
            }
        }
        if let Some(e) = lost {
            // tunnel parts go, the guard stays when the profile asks for it
            let keep = p.as_ref().is_some_and(|p| wants_guard(&p.settings));
            let old = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(id);
            if let Some(mut rt) = old {
                self.teardown(&mut rt, !keep);
                let mut n = Rt::new();
                n.status.attempts = rt.status.attempts + 1;
                n.status.error = Some(e);
                n.status.retry_at = Some(now + backoff(n.status.attempts));
                n.status.guard = self.sys.guard_present();
                self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), n);
            }
            return;
        }
        let routes = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(id).map(|r| r.host_routes.clone());
        if let Some(routes) = routes {
            let _ = apply_route_actions(&*self.sys, &routes, actions, now);
        }
    }

    /// An OpenVPN profile is "connected" while its program runs and its interface is there (the program reported the tunnel up before
    /// anything was applied); when either ends the pieces are taken away and the manager retries with a back-off.
    fn watch_openvpn(&self, id: &str, now: u64, link_gone: bool) {
        let p = self.get(id);
        let alive = {
            let st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(rt) = st.get(id) else { return };
            rt.applied.as_ref().is_some_and(|a| a.ovpn.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut().is_some_and(|pr| pr.alive()))
        };
        if alive && !link_gone {
            let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(rt) = st.get_mut(id) {
                if !matches!(rt.status.state, State::Connected { .. }) {
                    rt.status.state = State::Connected { since: now };
                    rt.status.error = None;
                    rt.status.attempts = 0;
                }
            }
            return;
        }
        let why = if alive { "the VPN interface disappeared" } else { "the OpenVPN program ended" };
        let keep = p.as_ref().is_some_and(|p| wants_guard(&p.settings));
        let old = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(id);
        if let Some(mut rt) = old {
            self.teardown(&mut rt, !keep);
            let mut n = Rt::new();
            n.status.attempts = rt.status.attempts + 1;
            n.status.error = Some(why.into());
            n.status.retry_at = Some(now + backoff(n.status.attempts));
            n.status.guard = self.sys.guard_present();
            self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.to_string(), n);
        }
    }

    /// One event for the DNS forwarder of the connected HYBRID profile (the socket loop of [`crate::vpn_dns`] feeds the same state
    /// machine without this lock; this entry is for tests and tools). Route changes are applied; the packets to send are returned.
    pub fn dns_event(&self, ev: DnsEvent, now: u64) -> Vec<Action> {
        let _op = self.op.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).values().find_map(|rt| rt.fwd.clone().map(|f| (f, rt.host_routes.clone())));
        match found {
            Some((f, routes)) => {
                let acts = f.lock().unwrap_or_else(std::sync::PoisonError::into_inner).handle(ev);
                apply_route_actions(&*self.sys, &routes, acts, now)
            }
            None => vec![],
        }
    }

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

fn wants_guard(s: &Settings) -> bool {
    match s.mode {
        Mode::Full => s.kill_switch,
        Mode::Hybrid => s.fail_closed,
    }
}

fn host_cidr(ip: IpAddr) -> Cidr {
    Cidr::new(ip, if ip.is_ipv4() { 32 } else { 128 }).expect("a host prefix is always valid")
}

/// **Sign-in hook.** Cleans up after a crash and starts the profiles the person left enabled: their keys are in the vault that
/// was just opened. Nothing is started (not even a thread) when no profile is enabled and nothing is left over.
pub fn start_for_login(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) {
    let m = Manager::shared(svc, user, audit);
    if m.store.list().iter().any(|s| s.enabled) {
        m.recover();
        m.ensure_supervised();
    }
}

/// **Sign-out hook.** Stops the tunnel and removes everything it made (the vault closes with the session).
pub fn on_logout(svc: &Arc<crate::services::Services>, user: &str) {
    let m = Manager::registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&format!("{}|{user}", svc.dir().display())).cloned();
    if let Some(m) = m {
        m.end_supervision();
    }
}

impl Manager {
    /// What an import would find, without keeping anything (the assistant's card and the page's preview).
    pub fn preview(&self, name: &str, input: &[u8]) -> Result<Imported, String> {
        let ctx = self.import_context();
        let text = String::from_utf8_lossy(input);
        let imported = if text.contains("[Interface]") { wgconf::parse(name, input, &ctx) } else { ovpn::parse(name, input, &ctx) }.map_err(|e| e.to_string())?;
        if self.store.list().iter().any(|s| s.profile.name.eq_ignore_ascii_case(&imported.name)) {
            return Err(format!("there is already a VPN profile called {}", imported.name));
        }
        let (profile, _secrets, warnings) = imported.into_profile("v0");
        Ok(Imported { profile, warnings })
    }
}


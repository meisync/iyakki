//! The fleet hub (F-FLEET-01, 02, 07): the state of this system's pair groups, the channel to the other members and the heartbeats
//! (docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md sections 3.1 to 3.3 and 3.5). The permission asks (`fleet.*` capabilities, cards) are
//! made by the caller before it uses a method here (`fleettools.rs`); everything that arrives from another member is checked here.
//!
//! The channel is one short TCP exchange per message: a length-prefixed JSON envelope in, a length-prefixed JSON reply out.
//!
//! * A member message (heartbeat, assignment, answer, leave notice) is **signed** by the sender's key (`DOMAIN_MSG`) and its body is
//!   **sealed** with the group key of the current generation, bound to the fleet, the kind and the generation. The receiver checks that the
//!   sender is in the record it holds, the signature, the time (within five minutes), a nonce never seen before and the generation. A member
//!   that was removed is not in the record, so its next message is refused ("not a member"); after a removal its old key opens nothing.
//! * A **record push** (a new signed version of the group record) is signed by the sender and carries the record, which is itself signed by
//!   an administrator and accepted only by [`Record::accept_over`].
//! * A **join** (not yet a member) is signed by the joiner's own key and carries the one-time code. The code works once, for ten minutes, and
//!   five wrong codes lock joining for ten minutes. An administrator on the offering side approves the request (`approve_join`), and the person
//!   on the joining side accepts the record they are offered (`join_accept`), each seeing the other's fingerprint.
//! * Assignments carry only plain fields (no approvals, no capabilities, no secrets); the receiver's own setting decides whether it queues,
//!   accepts or refuses them. Nothing here runs a task: `fleet_work.rs` does, through the core's task manager, and the messages that come
//!   back (status, hand-back as a Git bundle, shared context) are in `fleet_hub/work.rs`.

pub use iyakki_shared::fleetview::{Accept, FleetView, InboxItem, MemberView, OfferView, OutItem, PendingView, RequestView, Status};
pub use iyakki_shared::fleetview::{AssignBody};
pub use iyakki_shared::fleetview::{looks_secret};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cloudkeys::{hex, random, MemberKeys, PublicKeys, ScopeKey};
mod relay;
mod tls;
mod work;
pub use relay::{mailbox_of as relay_mailbox_of, valid_mailbox as relay_valid_mailbox, Post as RelayPost};
pub use tls::{colon as colon_fp, Target};
pub use work::{ContextHandler, ModelsHandler, Returned, RowOwner, CHUNK, MAX_BUNDLE};

use crate::fleet::{clean, code_hash, fingerprint, new_code, normalise_code, same_hash, short_id, Heartbeat, HeartbeatSource, Member, Record, CODE_LIFE, DOMAIN_JOIN, DOMAIN_MSG};

const MAX_FRAME: usize = 256 * 1024;
const SKEW: u64 = 300;
const MAX_CONNS: usize = 32;
/// Five wrong codes in ten minutes lock joining.
const MAX_BAD_CODES: usize = 5;
const LOCK_WINDOW: u64 = 600;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if let Some(d) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d).map_err(|e| format!("could not make the folder: {e}"))?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("could not save: {e}"))?;
    f.write_all(text.as_bytes()).map_err(|e| format!("could not save: {e}"))?;
    f.sync_all().map_err(|e| format!("could not save: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save: {e}"))
}

// ---------------------------------------------------------------------------------------------- wire

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Env {
    v: u32,
    fleet: String,
    from: String,
    kind: String,
    ts: u64,
    nonce: String,
    generation: u64,
    body: String,
    sig: String,
}

impl Env {
    fn text(&self) -> String {
        format!("{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}", self.v, self.fleet, self.from, self.kind, self.ts, self.nonce, self.generation, self.body)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Reply {
    ok: bool,
    kind: String,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    error: String,
}

impl Reply {
    fn ok(kind: &str, body: Value) -> Reply {
        Reply { ok: true, kind: kind.into(), body, error: String::new() }
    }
    fn err(e: &str) -> Reply {
        Reply { ok: false, kind: "error".into(), body: Value::Null, error: e.into() }
    }
}

fn write_frame(s: &mut impl Write, v: &impl Serialize) -> Result<(), String> {
    let b = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    if b.len() > MAX_FRAME {
        return Err("message too large".into());
    }
    s.write_all(&(b.len() as u32).to_be_bytes()).and_then(|_| s.write_all(&b)).map_err(|e| e.to_string())
}

fn read_frame<T: for<'de> Deserialize<'de>>(s: &mut impl Read) -> Result<T, String> {
    let mut l = [0u8; 4];
    s.read_exact(&mut l).map_err(|e| e.to_string())?;
    let n = u32::from_be_bytes(l) as usize;
    if n > MAX_FRAME {
        return Err("message too large".into());
    }
    let mut b = vec![0u8; n];
    s.read_exact(&mut b).map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map_err(|e| format!("not a fleet message: {e}"))
}

/// One message to a member: on the LAN channel for `host:port`, over pinned TLS for `tls://host:port#fingerprint` (F-FLEET-08).
fn exchange(addr: &str, env: &Env) -> Result<Reply, String> {
    exchange_via(None, addr, env, &|_, _, _| Err("an answer through the relay cannot be opened here".into()))
}

/// The same, with the relay of this hub for a `relay://` address; `open` opens a sealed answer that came through it.
fn exchange_via(link: Option<&Arc<relay::RelayLink>>, addr: &str, env: &Env, open: &dyn Fn(&str, u64, &str) -> Result<Vec<u8>, String>) -> Result<Reply, String> {
    match Target::parse(addr)? {
        Target::Relay(mailbox) => relay::exchange_relay(link, &mailbox, env, open),
        Target::Plain(a) => exchange_plain(&a, env),
        Target::Tls { addr, fingerprint } => {
            let mut s = tls::connect(&addr, &fingerprint)?;
            write_frame(&mut s, env).map_err(|e| tls::explain(&e))?;
            let _ = s.flush();
            let r = read_frame(&mut s).map_err(|e| tls::explain(&e));
            s.conn.send_close_notify();
            let _ = s.flush();
            r
        }
    }
}

fn exchange_plain(addr: &str, env: &Env) -> Result<Reply, String> {
    let mut s = tls::connect_tcp(addr)?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
    write_frame(&mut s, env)?;
    read_frame(&mut s)
}

// ---------------------------------------------------------------------------------------------- what the hub keeps



#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    accept: Accept,
    #[serde(default)]
    trusted: BTreeSet<String>,
    /// The port the channel listens on when it is started without one (0: a free one).
    #[serde(default)]
    port: u16,
    /// Fleets whose project Context this computer shares (off until the person says so).
    #[serde(default)]
    context_sync: BTreeSet<String>,
    /// The channel listens over TLS (with the developer-mode certificate) and members reach this system at `tls://` addresses (F-FLEET-08).
    #[serde(default)]
    tls: bool,
    /// The person allowed the tasks of a silent member to go to another member by themselves (off until the person says so; F-FLEET-12).
    #[serde(default)]
    auto_requeue: bool,
    /// Seconds between heartbeats the person chose (10 to 30; 0: the usual 20).
    #[serde(default)]
    interval_secs: u64,
}

struct Offer {
    fleet: String,
    code: String,
    hash: [u8; 32],
    expires: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReqState {
    Pending,
    Approved,
    Declined,
}

struct JoinReq {
    id: String,
    fleet: String,
    member: Member,
    addr: String,
    state: ReqState,
    at: u64,
}

#[derive(Clone, Debug)]
pub enum PendingState {
    Waiting,
    /// An administrator approved: this record is what the person is offered.
    Offered(Box<Record>),
    Declined,
    Failed(String),
}

struct PendingJoin {
    addr: String,
    state: PendingState,
}

struct Peer {
    addr: Option<String>,
    last_seen: u64,
    hb: Option<Heartbeat>,
    refused: Option<String>,
    /// The member's heartbeat says it no longer listens over TLS while the address kept for it is a `tls://` one (the person sets a new one).
    tls_left: bool,
}

struct FleetState {
    rec: Record,
    key: Arc<ScopeKey>,
    peers: BTreeMap<String, Peer>,
}






#[derive(Clone, Debug, Serialize, Deserialize)]
struct HbBody {
    hb: Heartbeat,
    port: u16,
    known: BTreeMap<String, String>,
    /// The fingerprint of the certificate this member's channel shows (empty: the channel is plain). A heartbeat is signed and sealed, so
    /// a member that hears it can pin the connection back to this one (F-FLEET-08).
    #[serde(default)]
    tls: String,
    /// The mailbox this member is reached at through the relay, when the relay is on (F-FLEET-10).
    #[serde(default)]
    relay: String,
}

struct Inner {
    keys: MemberKeys,
    fleets: BTreeMap<String, FleetState>,
    offers: Vec<Offer>,
    requests: Vec<JoinReq>,
    pending: BTreeMap<String, PendingJoin>,
    seen: VecDeque<(u64, String)>,
    bad_codes: Vec<u64>,
    config: Config,
    inbox: Vec<InboxItem>,
    outbox: Vec<OutItem>,
    port: u16,
    logged: BTreeMap<String, u64>,
    assembly: BTreeMap<String, work::Assembly>,
}

struct Node {
    stop: Arc<AtomicBool>,
    addr: SocketAddr,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// One person-less system's pair groups. One hub per data folder.
pub struct Hub {
    dir: PathBuf,
    inner: Mutex<Inner>,
    clock: Mutex<Arc<dyn Fn() -> u64 + Send + Sync>>,
    audit: Mutex<Option<Arc<dyn Fn(&str, &str) + Send + Sync>>>,
    node: Mutex<Option<Node>>,
    interval: Mutex<Duration>,
    source: Mutex<Option<Arc<dyn HeartbeatSource>>>,
    pushers: Mutex<Vec<std::thread::JoinHandle<()>>>,
    conns: AtomicUsize,
    bind: Mutex<String>,
    work_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    lost_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    ctx_handler: Mutex<Option<ContextHandler>>,
    relay: Mutex<Option<Arc<relay::RelayLink>>>,
    relay_thread: Mutex<Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>>,
    models_handler: Mutex<Option<ModelsHandler>>,
    attached: Mutex<Option<Arc<crate::fleet_work::Attached>>>,
    /// The certificate of this system's channel when it listens over TLS: the settings and the fingerprint (F-FLEET-08).
    tls: Mutex<Option<(Arc<rustls::ServerConfig>, String)>>,
}

// ---------------------------------------------------------------------------------------------- views for screens and the chat







fn state_dir_name(id: &str) -> String {
    id.chars().filter(char::is_ascii_hexdigit).collect()
}

impl Hub {
    /// The hub of the data folder `dir` (made on first use; the member key is made then too).
    pub fn open(dir: &Path) -> Result<Arc<Hub>, String> {
        use std::sync::OnceLock;
        static HUBS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Hub>>>> = OnceLock::new();
        let reg = HUBS.get_or_init(|| Mutex::new(BTreeMap::new()));
        let mut g = lock(reg);
        if let Some(h) = g.get(dir) {
            return Ok(h.clone());
        }
        let h = Arc::new(Hub::load(dir)?);
        g.insert(dir.to_path_buf(), h.clone());
        Ok(h)
    }

    /// A hub that is not shared (tests: two systems in one process).
    pub fn load(dir: &Path) -> Result<Hub, String> {
        let fd = dir.join("fleet");
        let key_path = fd.join("identity.key");
        let keys = match std::fs::read_to_string(&key_path).ok().and_then(|t| MemberKeys::from_hex(t.trim()).ok()) {
            Some(k) => k,
            None => {
                let k = MemberKeys::generate()?;
                write_private(&key_path, &k.seed_hex())?;
                k
            }
        };
        let config: Config = std::fs::read_to_string(fd.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let inbox: Vec<InboxItem> = std::fs::read_to_string(fd.join("inbox.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let outbox: Vec<OutItem> = std::fs::read_to_string(fd.join("outbox.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let addrs: BTreeMap<String, String> = std::fs::read_to_string(fd.join("peers.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut fleets = BTreeMap::new();
        for e in std::fs::read_dir(fd.join("records")).into_iter().flatten().flatten() {
            let Some(rec) = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Record>(&t).ok()) else { continue };
            // a record that does not verify, or that does not open with our key, is left alone (not loaded)
            if rec.verify_self().is_err() {
                continue;
            }
            let Ok(key) = rec.open_key(&keys) else { continue };
            let mut peers = BTreeMap::new();
            for m in &rec.members {
                peers.insert(m.id.clone(), Peer { addr: addrs.get(&format!("{}/{}", rec.fleet, m.id)).cloned(), last_seen: 0, hb: None, refused: None, tls_left: false });
            }
            fleets.insert(rec.fleet.clone(), FleetState { rec, key: Arc::new(key), peers });
        }
        let interval_secs = config.interval_secs;
        Ok(Hub {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner { keys, fleets, offers: vec![], requests: vec![], pending: BTreeMap::new(), seen: VecDeque::new(), bad_codes: vec![], config, inbox, outbox, port: 0, logged: BTreeMap::new(), assembly: BTreeMap::new() }),
            clock: Mutex::new(Arc::new(now_secs)),
            audit: Mutex::new(None),
            node: Mutex::new(None),
            interval: Mutex::new(Duration::from_secs(if (10..=30).contains(&interval_secs) { interval_secs } else { 20 })),
            source: Mutex::new(None),
            pushers: Mutex::new(vec![]),
            conns: AtomicUsize::new(0),
            bind: Mutex::new("0.0.0.0:7671".into()),
            work_hook: Mutex::new(None),
            lost_hook: Mutex::new(None),
            ctx_handler: Mutex::new(None),
            relay: Mutex::new(None),
            relay_thread: Mutex::new(None),
            models_handler: Mutex::new(None),
            attached: Mutex::new(None),
            tls: Mutex::new(None),
        })
    }

    /// The address the channel listens on when it is started without one (tests use a loopback port).
    /// Make the channel listen over TLS with this certificate (the one of the developer-mode channel, `webservice::tls::Identity`): a
    /// node started after this call speaks TLS only, and tells its members the fingerprint in every heartbeat so they pin it (F-FLEET-08).
    pub fn set_tls(&self, config: Arc<rustls::ServerConfig>, fingerprint: &str) {
        *lock(&self.tls) = Some((config, fingerprint.to_string()));
    }

    /// Whether the person allowed the automatic requeue of a silent member's tasks.
    pub fn auto_requeue(&self) -> bool {
        lock(&self.inner).config.auto_requeue
    }

    pub fn set_auto_requeue(&self, on: bool) {
        let mut i = lock(&self.inner);
        i.config.auto_requeue = on;
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.requeue.auto", if on { "on" } else { "off" });
    }

    /// Whether the person turned the TLS channel on (kept across restarts; the certificate is loaded by the caller).
    pub fn tls_wanted(&self) -> bool {
        lock(&self.inner).config.tls
    }

    /// Turn the TLS channel on or off for the next start of the node (`config.tls`); off also forgets the certificate settings.
    pub fn set_tls_wanted(&self, on: bool) {
        let mut i = lock(&self.inner);
        i.config.tls = on;
        self.save_misc(&i);
        drop(i);
        if !on {
            *lock(&self.tls) = None;
        }
        self.audit("fleet.tls.set", if on { "on" } else { "off" });
    }

    /// The fingerprint the channel shows when it listens over TLS.
    pub fn tls_fingerprint(&self) -> Option<String> {
        lock(&self.tls).as_ref().map(|t| t.1.clone())
    }

    /// Tell this system where a member is: `host:port` or `tls://host:port#fingerprint`. This is the explicit address of a member the
    /// LAN discovery cannot find. Administrator-level: the caller has asked the core (`fleet.manage`).
    pub fn set_member_address(&self, fleet: &str, member: &str, addr: &str) -> Result<String, String> {
        let t = Target::parse(addr)?;
        if let Target::Plain(a) = &t {
            a.to_socket_addrs().map_err(|_| "that address is not valid (use host:port or tls://host:port#fingerprint)".to_string())?;
        }
        let text = t.text();
        let mut i = lock(&self.inner);
        let me = i.keys.public().sign;
        let f = i.fleets.get_mut(fleet).ok_or("that fleet is not known here")?;
        let id = f.rec.members.iter().find(|m| !member.is_empty() && (m.id.starts_with(member) || fingerprint(&m.id) == member)).map(|m| m.id.clone()).ok_or("that is not a member of the fleet")?;
        if id == me {
            return Err("that is this computer".into());
        }
        let p = f.peers.entry(id.clone()).or_insert(Peer { addr: None, last_seen: 0, hb: None, refused: None, tls_left: false });
        p.addr = Some(text.clone());
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.member.address", &format!("fleet={} member={} transport={}", short_id(fleet), fingerprint(&id), match t { Target::Tls { .. } => "tls", Target::Relay(_) => "relay", Target::Plain(_) => "plain" }));
        Ok(text)
    }

    pub fn set_bind(&self, bind: &str) {
        *lock(&self.bind) = bind.to_string();
    }

    pub fn bind(&self) -> String {
        lock(&self.bind).clone()
    }

    /// Tests: a clock of their own.
    pub fn set_clock(&self, f: Arc<dyn Fn() -> u64 + Send + Sync>) {
        *lock(&self.clock) = f;
    }

    /// The person chooses how often this computer shares its figures: 10 to 30 seconds (kept across restarts).
    pub fn set_person_interval(&self, secs: u64) -> Result<(), String> {
        if !(10..=30).contains(&secs) {
            return Err("the interval is 10 to 30 seconds".into());
        }
        let mut i = lock(&self.inner);
        i.config.interval_secs = secs;
        self.save_misc(&i);
        drop(i);
        *lock(&self.interval) = Duration::from_secs(secs);
        self.audit("fleet.interval.set", &format!("secs={secs}"));
        Ok(())
    }

    /// The seconds between heartbeats now.
    pub fn interval_secs(&self) -> u64 {
        lock(&self.interval).as_secs()
    }

    /// Tests: a heartbeat interval shorter than the 10 seconds a person can set.
    pub fn set_interval(&self, d: Duration) {
        *lock(&self.interval) = d;
    }

    /// Where the audit lines go (what, detail: ids and numbers, never content). The last one set is used.
    pub fn set_audit(&self, f: Arc<dyn Fn(&str, &str) + Send + Sync>) {
        *lock(&self.audit) = Some(f);
    }

    /// What a restart does to a bundle being received: the count in memory is gone, the part file and its note stay (tests).
    #[cfg(test)]
    pub fn forget_assemblies(&self) {
        lock(&self.inner).assembly.clear();
    }

    fn now(&self) -> u64 {
        (lock(&self.clock))()
    }

    fn audit(&self, what: &str, detail: &str) {
        if let Some(f) = lock(&self.audit).clone() {
            f(what, detail);
        }
    }

    /// An audit line that repeats at most once a minute for the same `key` (a refusal from a member that keeps trying).
    fn audit_once(&self, key: &str, what: &str, detail: &str) {
        let now = self.now();
        {
            let mut i = lock(&self.inner);
            if i.logged.get(key).is_some_and(|t| now.saturating_sub(*t) < 60) {
                return;
            }
            i.logged.insert(key.to_string(), now);
            if i.logged.len() > 512 {
                i.logged.retain(|_, t| now.saturating_sub(*t) < 60);
            }
        }
        self.audit(what, detail);
    }

    fn fd(&self) -> PathBuf {
        self.dir.join("fleet")
    }

    fn save_record(&self, rec: &Record) -> Result<(), String> {
        write_private(&self.fd().join("records").join(format!("{}.json", state_dir_name(&rec.fleet))), &serde_json::to_string(rec).map_err(|e| e.to_string())?)
    }

    fn save_misc(&self, i: &Inner) {
        let fd = self.fd();
        let _ = write_private(&fd.join("config.json"), &serde_json::to_string(&i.config).unwrap_or_default());
        let _ = write_private(&fd.join("inbox.json"), &serde_json::to_string(&i.inbox).unwrap_or_default());
        let _ = write_private(&fd.join("outbox.json"), &serde_json::to_string(&i.outbox).unwrap_or_default());
        let mut addrs = BTreeMap::new();
        for (f, s) in &i.fleets {
            for (id, p) in &s.peers {
                if let Some(a) = &p.addr {
                    addrs.insert(format!("{f}/{id}"), a.clone());
                }
            }
        }
        let _ = write_private(&fd.join("peers.json"), &serde_json::to_string(&addrs).unwrap_or_default());
    }

    // ------------------------------------------------------------------------------------------ status

    pub fn my_id(&self) -> String {
        lock(&self.inner).keys.public().sign
    }

    pub fn status(&self) -> Status {
        let now = self.now();
        let i = lock(&self.inner);
        let me = i.keys.public().sign;
        let interval = lock(&self.interval).as_secs().max(1);
        let lost_after = (interval * 3).max(3);
        let fleets = i
            .fleets
            .values()
            .map(|f| FleetView {
                id: f.rec.fleet.clone(),
                short: short_id(&f.rec.fleet),
                name: f.rec.name.clone(),
                version: f.rec.version,
                generation: f.rec.generation,
                i_am_admin: f.rec.member(&me).is_some_and(|m| m.admin),
                members: f
                    .rec
                    .members
                    .iter()
                    .map(|m| {
                        let p = f.peers.get(&m.id);
                        let seen = p.filter(|p| p.last_seen > 0).map(|p| now.saturating_sub(p.last_seen));
                        let state = if m.id == me {
                            "this computer"
                        } else if p.is_some_and(|p| p.refused.is_some()) {
                            "refused"
                        } else {
                            match seen {
                                None => "unknown",
                                Some(s) if s > lost_after => "lost",
                                Some(_) => "up",
                            }
                        };
                        MemberView { id: m.id.clone(), fingerprint: fingerprint(&m.id), name: m.name.clone(), admin: m.admin, me: m.id == me, trusted: i.config.trusted.contains(&m.id), state: state.into(), seen_secs_ago: seen, address_known: p.is_some_and(|p| p.addr.is_some()), transport: if p.is_some_and(|p| p.tls_left) { "tls-left" } else { p.and_then(|p| p.addr.as_deref()).map(|a| if a.starts_with(tls::SCHEME) { "tls" } else if a.starts_with(relay::SCHEME) { "relay" } else { "lan" }).unwrap_or("") }.to_string(), heartbeat: p.and_then(|p| p.hb.clone()) }
                    })
                    .collect(),
            })
            .collect();
        let offers = i.offers.iter().filter(|o| o.expires > now).map(|o| OfferView { fleet: o.fleet.clone(), short: short_id(&o.fleet), code: crate::fleet::show_code(&o.code), expires_in: o.expires - now }).collect();
        let requests = i
            .requests
            .iter()
            .filter(|r| r.state == ReqState::Pending && now.saturating_sub(r.at) < CODE_LIFE * 3)
            .map(|r| RequestView { id: r.id.clone(), fleet: r.fleet.clone(), short: short_id(&r.fleet), name: r.member.name.clone(), fingerprint: fingerprint(&r.member.id) })
            .collect();
        let pending = i
            .pending
            .iter()
            .map(|(t, p)| {
                let (state, offer) = match &p.state {
                    PendingState::Waiting => ("waiting".to_string(), None),
                    PendingState::Offered(r) => ("offered".to_string(), Some(offer_text(r))),
                    PendingState::Declined => ("declined".to_string(), None),
                    PendingState::Failed(e) => (format!("failed: {e}"), None),
                };
                PendingView { ticket: t.clone(), addr: p.addr.clone(), state, offer }
            })
            .collect();
        Status { me: me.clone(), my_fingerprint: fingerprint(&me), listening: (i.port != 0).then_some(i.port), accept: i.config.accept, fleets, offers, requests, pending, inbox: i.inbox.clone(), outbox: i.outbox.clone(), tls: i.config.tls, relay: self.relay_address().unwrap_or_default() }
    }

    /// Pick a fleet by id, a prefix of at least 4 digits, the short form or its name (when only one has that name).
    fn pick<'a>(i: &'a Inner, what: &str) -> Result<&'a FleetState, String> {
        let w = what.trim().to_lowercase().replace('-', "");
        if w.is_empty() {
            return match i.fleets.len() {
                1 => Ok(i.fleets.values().next().unwrap_or_else(|| unreachable!())),
                0 => Err("this computer is in no fleet yet".into()),
                _ => Err("this computer is in more than one fleet: say which".into()),
            };
        }
        let hits: Vec<&FleetState> = i.fleets.values().filter(|f| f.rec.fleet == w || (w.len() >= 4 && f.rec.fleet.starts_with(&w)) || f.rec.name.to_lowercase() == what.trim().to_lowercase()).collect();
        match hits.len() {
            1 => Ok(hits[0]),
            0 => Err(format!("no fleet matches {what}")),
            _ => Err(format!("more than one fleet matches {what}: use its id")),
        }
    }

    pub fn fleet_id(&self, what: &str) -> Result<String, String> {
        let i = lock(&self.inner);
        Self::pick(&i, what).map(|f| f.rec.fleet.clone())
    }

    // ------------------------------------------------------------------------------------------ create, offer, join

    pub fn create_fleet(&self, fleet_name: &str, my_name: &str) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let (rec, key) = Record::create(&i.keys, my_name, fleet_name)?;
        self.save_record(&rec)?;
        let id = rec.fleet.clone();
        let me = i.keys.public().sign;
        let peers = BTreeMap::from([(me, Peer { addr: None, last_seen: 0, hb: None, refused: None, tls_left: false })]);
        i.fleets.insert(id.clone(), FleetState { rec, key: Arc::new(key), peers });
        drop(i);
        self.audit("fleet.create", &format!("fleet={}", short_id(&id)));
        Ok(id)
    }

    /// A one-time join code for a fleet this system administers. Returns the code and how long it lives.
    pub fn offer_code(&self, fleet: &str) -> Result<(String, u64), String> {
        let now = self.now();
        let mut i = lock(&self.inner);
        let f = Self::pick(&i, fleet)?;
        let me = i.keys.public().sign;
        if !f.rec.member(&me).is_some_and(|m| m.admin) {
            return Err("only an administrator can offer a join code".into());
        }
        let id = f.rec.fleet.clone();
        i.offers.retain(|o| o.expires > now && o.fleet != id);
        let code = new_code()?;
        i.offers.push(Offer { fleet: id.clone(), hash: code_hash(&id, &code), code: code.clone(), expires: now + CODE_LIFE });
        drop(i);
        self.audit("fleet.offer", &format!("fleet={} life={CODE_LIFE}s", short_id(&id)));
        Ok((crate::fleet::show_code(&code), CODE_LIFE))
    }

    fn join_env(&self, kind: &str, body: Value) -> Env {
        let i = lock(&self.inner);
        let mut e = Env { v: 1, fleet: String::new(), from: i.keys.public().sign, kind: kind.into(), ts: self.now(), nonce: hex(&random::<16>().unwrap_or([0; 16])), generation: 0, body: body.to_string(), sig: String::new() };
        e.sig = i.keys.sign_text(DOMAIN_JOIN, e.text().as_bytes());
        e
    }

    /// Ask the member at `addr` to let this system join with the one-time `code`. Returns a ticket; the answer comes when an administrator there
    /// approves (see [`Hub::poll_pending`]).
    pub fn join_start(&self, addr: &str, code: &str, my_name: &str) -> Result<String, String> {
        let name = clean(my_name, 40);
        if name.is_empty() {
            return Err("give this computer a name".into());
        }
        let t = Target::parse(addr)?;
        if matches!(t, Target::Relay(_)) {
            return Err("a join is made directly (its one-time code is for two people at the same place), not through the relay".into());
        }
        let addr = &t.text();
        let (wrap, port) = {
            let i = lock(&self.inner);
            (i.keys.public().wrap, i.port)
        };
        let tls = self.tls_fingerprint().unwrap_or_default();
        let env = self.join_env("join_request", json!({"wrap": wrap, "name": name, "port": port, "tls": tls, "code": normalise_code(code)}));
        let reply = exchange(addr, &env)?;
        if !reply.ok {
            return Err(reply.error);
        }
        let ticket = reply.body["ticket"].as_str().ok_or("the other computer gave no ticket")?.to_string();
        let mut i = lock(&self.inner);
        i.pending.insert(ticket.clone(), PendingJoin { addr: addr.to_string(), state: PendingState::Waiting });
        drop(i);
        self.audit("fleet.join.start", &format!("ticket={}", &ticket[..8.min(ticket.len())]));
        Ok(ticket)
    }

    /// Ask every waiting join whether it was approved. Cheap; called by the heartbeat thread and by a person asking.
    pub fn poll_pending(&self) {
        let waiting: Vec<(String, String)> = lock(&self.inner).pending.iter().filter(|(_, p)| matches!(p.state, PendingState::Waiting)).map(|(t, p)| (t.clone(), p.addr.clone())).collect();
        for (t, addr) in waiting {
            let wrap = lock(&self.inner).keys.public().wrap;
            let env = self.join_env("join_poll", json!({"ticket": t, "wrap": wrap}));
            let st = match exchange(&addr, &env) {
                Err(_) => continue, // try again at the next poll
                Ok(r) if !r.ok => PendingState::Failed(r.error),
                Ok(r) => match r.kind.as_str() {
                    "pending" => continue,
                    "declined" => PendingState::Declined,
                    "approved" => match serde_json::from_value::<Record>(r.body["record"].clone()) {
                        Ok(rec) => {
                            let me = self.my_id();
                            match rec.verify_self() {
                                Ok(()) if rec.member(&me).is_some() => PendingState::Offered(Box::new(rec)),
                                Ok(()) => PendingState::Failed("the record does not list this computer".into()),
                                Err(e) => PendingState::Failed(e),
                            }
                        }
                        Err(_) => PendingState::Failed("the record could not be read".into()),
                    },
                    _ => PendingState::Failed("an answer this version does not know".into()),
                },
            };
            if let Some(p) = lock(&self.inner).pending.get_mut(&t) {
                p.state = st;
            }
        }
    }

    /// The text for the card that asks the person to accept an offer (fleet name, who signed it, who is in it).
    pub fn offer_card(&self, ticket: &str) -> Result<String, String> {
        let i = lock(&self.inner);
        match i.pending.get(ticket).map(|p| &p.state) {
            Some(PendingState::Offered(r)) => Ok(offer_text(r)),
            Some(_) => Err("there is no offer to accept for this ticket yet".into()),
            None => Err("no such ticket".into()),
        }
    }

    /// The person accepts the record they were offered: it is kept and this system is a member.
    pub fn join_accept(&self, ticket: &str) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let p = i.pending.get(ticket).ok_or("no such ticket")?;
        let PendingState::Offered(rec) = p.state.clone() else { return Err("there is no offer to accept for this ticket yet".into()) };
        let addr = p.addr.clone();
        if i.fleets.contains_key(&rec.fleet) {
            return Err("this computer is already in that fleet".into());
        }
        let key = rec.open_key(&i.keys)?;
        self.save_record(&rec)?;
        let mut peers: BTreeMap<String, Peer> = rec.members.iter().map(|m| (m.id.clone(), Peer { addr: None, last_seen: 0, hb: None, refused: None, tls_left: false })).collect();
        if let Some(p) = peers.get_mut(&rec.signer) {
            p.addr = Some(addr);
        }
        let id = rec.fleet.clone();
        i.fleets.insert(id.clone(), FleetState { rec: *rec, key: Arc::new(key), peers });
        i.pending.remove(ticket);
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.join.accept", &format!("fleet={}", short_id(&id)));
        Ok(id)
    }

    pub fn join_decline(&self, ticket: &str) -> Result<(), String> {
        let mut i = lock(&self.inner);
        i.pending.remove(ticket).map(|_| ()).ok_or_else(|| "no such ticket".to_string())?;
        drop(i);
        self.audit("fleet.join.decline", "");
        Ok(())
    }

    /// An administrator approves a join request: a new version of the record with the new member, pushed to the members.
    pub fn approve_join(self: &Arc<Self>, request: &str) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let idx = i.requests.iter().position(|r| r.id == request && r.state == ReqState::Pending).ok_or("no such request")?;
        let (fleet, member, addr) = {
            let r = &i.requests[idx];
            (r.fleet.clone(), r.member.clone(), r.addr.clone())
        };
        let keys = i.keys.clone();
        let f = i.fleets.get_mut(&fleet).ok_or("that fleet is gone")?;
        let rec = f.rec.with_member(&keys, &f.key, member.clone())?;
        self.save_record(&rec)?;
        f.peers.insert(member.id.clone(), Peer { addr: Some(addr), last_seen: 0, hb: None, refused: None, tls_left: false });
        f.rec = rec;
        i.requests[idx].state = ReqState::Approved;
        self.save_misc(&i);
        let jobs = self.push_jobs(&i, &fleet, None);
        drop(i);
        self.spawn_send(jobs);
        self.audit("fleet.join.approve", &format!("fleet={} who={}", short_id(&fleet), fingerprint(&member.id)));
        Ok(fleet)
    }

    pub fn decline_join(&self, request: &str) -> Result<(), String> {
        let mut i = lock(&self.inner);
        let r = i.requests.iter_mut().find(|r| r.id == request && r.state == ReqState::Pending).ok_or("no such request")?;
        r.state = ReqState::Declined;
        let who = fingerprint(&r.member.id);
        drop(i);
        self.audit("fleet.join.decline", &format!("who={who}"));
        Ok(())
    }

    // ------------------------------------------------------------------------------------------ members

    /// Envelopes carrying the current record to every other member (and to `also`, a member that was just removed).
    fn push_jobs(&self, i: &Inner, fleet: &str, also: Option<(&str, &str)>) -> Vec<(String, Env)> {
        let Some(f) = i.fleets.get(fleet) else { return vec![] };
        let me = i.keys.public().sign;
        let body = serde_json::to_string(&f.rec).unwrap_or_default();
        let mut jobs = vec![];
        let mut targets: Vec<String> = vec![];
        for m in &f.rec.members {
            if m.id != me {
                if let Some(a) = f.peers.get(&m.id).and_then(|p| p.addr.clone()) {
                    targets.push(a);
                }
            }
        }
        if let Some((_, addr)) = also {
            targets.push(addr.to_string());
        }
        for a in targets {
            jobs.push((a, self.signed(i, fleet, "record_push", f.rec.generation, body.clone())));
        }
        jobs
    }

    /// One message to an address of any kind (the LAN, pinned TLS, the relay).
    fn send_to(&self, addr: &str, env: &Env) -> Result<Reply, String> {
        exchange_via(self.relay_link().as_ref(), addr, env, &|f, g, s| self.open_relayed(f, g, s))
    }

    fn signed(&self, i: &Inner, fleet: &str, kind: &str, generation: u64, body: String) -> Env {
        let mut e = Env { v: 1, fleet: fleet.into(), from: i.keys.public().sign, kind: kind.into(), ts: self.now(), nonce: hex(&random::<16>().unwrap_or([0; 16])), generation, body, sig: String::new() };
        e.sig = i.keys.sign_text(DOMAIN_MSG, e.text().as_bytes());
        e
    }

    /// A sealed member message for `fleet` (signed by this system).
    fn sealed(&self, i: &Inner, fleet: &str, kind: &str, plain: &Value) -> Result<Env, String> {
        let f = i.fleets.get(fleet).ok_or("that fleet is gone")?;
        let body = f.key.seal(fleet, kind, f.rec.generation, plain.to_string().as_bytes())?;
        Ok(self.signed(i, fleet, kind, f.rec.generation, body))
    }

    fn spawn_send(self: &Arc<Self>, jobs: Vec<(String, Env)>) {
        if jobs.is_empty() {
            return;
        }
        let me = self.clone();
        let h = std::thread::spawn(move || {
            for (addr, env) in jobs {
                let _ = me.send_to(&addr, &env);
            }
        });
        let mut p = lock(&self.pushers);
        p.retain(|h| !h.is_finished());
        p.push(h);
    }

    /// Wait for the messages sent in the background (tests, and a screen that wants to be sure).
    pub fn settle(&self) {
        let hs: Vec<_> = lock(&self.pushers).drain(..).collect();
        for h in hs {
            let _ = h.join();
        }
    }

    pub fn remove_member(self: &Arc<Self>, fleet: &str, who: &str) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let fid = Self::pick(&i, fleet)?.rec.fleet.clone();
        let keys = i.keys.clone();
        let f = i.fleets.get_mut(&fid).ok_or("that fleet is gone")?;
        let target = f.rec.find(who)?.clone();
        if target.id == keys.public().sign {
            return Err("that is this computer: use leave".into());
        }
        let (rec, key) = f.rec.without_member(&keys, &target.id)?;
        let removed_addr = f.peers.get(&target.id).and_then(|p| p.addr.clone());
        // the removed member is told with a push signed under the OLD record's rules (it still holds the old record and accepts it)
        self.save_record(&rec)?;
        f.rec = rec;
        f.key = Arc::new(key);
        f.peers.remove(&target.id);
        i.config.trusted.remove(&target.id);
        self.save_misc(&i);
        let also = removed_addr.as_deref().map(|a| (target.id.as_str(), a));
        let jobs = self.push_jobs(&i, &fid, also);
        drop(i);
        self.spawn_send(jobs);
        self.audit("fleet.member.removed", &format!("fleet={} who={}", short_id(&fid), fingerprint(&target.id)));
        Ok(target.name)
    }

    pub fn set_admin(self: &Arc<Self>, fleet: &str, who: &str, admin: bool) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let fid = Self::pick(&i, fleet)?.rec.fleet.clone();
        let keys = i.keys.clone();
        let f = i.fleets.get_mut(&fid).ok_or("that fleet is gone")?;
        let target = f.rec.find(who)?.clone();
        let rec = f.rec.with_admin(&keys, &target.id, admin)?;
        self.save_record(&rec)?;
        f.rec = rec;
        let jobs = self.push_jobs(&i, &fid, None);
        drop(i);
        self.spawn_send(jobs);
        self.audit("fleet.member.admin", &format!("fleet={} who={} admin={admin}", short_id(&fid), fingerprint(&target.id)));
        Ok(target.name)
    }

    /// Leave a fleet: the members are told and this system forgets the fleet. The last administrator cannot leave while other members remain.
    pub fn leave(self: &Arc<Self>, fleet: &str) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let fid = Self::pick(&i, fleet)?.rec.fleet.clone();
        let me = i.keys.public().sign;
        let f = i.fleets.get(&fid).ok_or("that fleet is gone")?;
        let others = f.rec.members.len() > 1;
        if others && f.rec.member(&me).is_some_and(|m| m.admin) && f.rec.admins().len() == 1 {
            return Err("this computer is the only administrator: make another member an administrator first".into());
        }
        let name = f.rec.name.clone();
        let mut jobs = vec![];
        for m in &f.rec.members {
            if m.id != me {
                if let (Some(a), Ok(e)) = (f.peers.get(&m.id).and_then(|p| p.addr.clone()), self.sealed(&i, &fid, "leave_notice", &json!({}))) {
                    jobs.push((a, e));
                }
            }
        }
        i.fleets.remove(&fid);
        i.inbox.retain(|x| x.fleet != fid);
        let _ = std::fs::remove_file(self.fd().join("records").join(format!("{}.json", state_dir_name(&fid))));
        self.save_misc(&i);
        drop(i);
        self.spawn_send(jobs);
        self.audit("fleet.leave", &format!("fleet={}", short_id(&fid)));
        Ok(name)
    }

    // ------------------------------------------------------------------------------------------ work

    pub fn set_accept(&self, a: Accept) {
        let mut i = lock(&self.inner);
        i.config.accept = a;
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.accept.set", a.label());
    }

    pub fn trust(&self, fleet: &str, who: &str, on: bool) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let f = Self::pick(&i, fleet)?;
        let m = f.rec.find(who)?.clone();
        if on {
            i.config.trusted.insert(m.id.clone());
        } else {
            i.config.trusted.remove(&m.id);
        }
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.trust.set", &format!("who={} on={on}", fingerprint(&m.id)));
        Ok(m.name)
    }

    /// Send an assignment to a member. Returns what the other side answered (`waiting`, `accepted` ...).
    pub fn assign(&self, fleet: &str, to: &str, body: &AssignBody) -> Result<String, String> {
        body.check()?;
        let (addr, env, fid, target) = {
            let i = lock(&self.inner);
            let f = Self::pick(&i, fleet)?;
            let m = f.rec.find(to)?.clone();
            if m.id == i.keys.public().sign {
                return Err("that is this computer".into());
            }
            let addr = f.peers.get(&m.id).and_then(|p| p.addr.clone()).ok_or("this computer does not know where that member is yet (wait for its first heartbeat)")?;
            let env = self.sealed(&i, &f.rec.fleet, "assign", &serde_json::to_value(body).map_err(|e| e.to_string())?)?;
            (addr, env, f.rec.fleet.clone(), m)
        };
        let reply = self.send_to(&addr, &env);
        let state = match &reply {
            Ok(r) if r.ok => r.body["state"].as_str().unwrap_or("sent").to_string(),
            Ok(r) => format!("failed: {}", r.error),
            Err(e) => format!("failed: {e}"),
        };
        let mut i = lock(&self.inner);
        i.outbox.push(OutItem { id: body.id.clone(), fleet: fid.clone(), to: target.id.clone(), to_name: target.name.clone(), title: clean(&body.title, 120), state: state.clone(), at: self.now(), rows: body.rows.clone(), body: Some(body.clone()), detail: String::new(), head: String::new(), tests: String::new() });
        if i.outbox.len() > 200 {
            i.outbox.remove(0);
        }
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.assign", &format!("fleet={} to={} task={} state={}", short_id(&fid), fingerprint(&target.id), &body.id[..8], state.split(':').next().unwrap_or("")));
        match reply {
            Ok(r) if r.ok => Ok(state),
            Ok(r) => Err(r.error),
            Err(e) => Err(e),
        }
    }

    /// The person on the worker decides on a waiting assignment; the assigner is told.
    pub fn inbox_decide(self: &Arc<Self>, id: &str, accept: bool) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let idx = i.inbox.iter().position(|x| x.id == id || (id.len() >= 6 && x.id.starts_with(id))).ok_or("no such task in the inbox")?;
        if i.inbox[idx].state != "waiting" {
            return Err("that task was already answered".into());
        }
        let state = if accept { "accepted" } else { "declined" };
        i.inbox[idx].state = state.into();
        let item = i.inbox[idx].clone();
        let job = i.fleets.get(&item.fleet).and_then(|f| f.peers.get(&item.from).and_then(|p| p.addr.clone())).and_then(|a| self.sealed(&i, &item.fleet, "assign_reply", &json!({"id": item.id, "state": state})).ok().map(|e| (a, e)));
        self.save_misc(&i);
        drop(i);
        self.spawn_send(job.into_iter().collect());
        self.audit("fleet.inbox.decide", &format!("task={} state={state}", &item.id[..8]));
        self.kick();
        Ok(item.body.title)
    }

    // ------------------------------------------------------------------------------------------ heartbeats

    /// Send this system's numbers to every member it knows an address for (once per interval).
    pub fn heartbeat_tick(&self, source: &dyn HeartbeatSource) {
        let mut hb = source.read();
        // The tasks an attached IDE workspace runs are counted here, so the source needs no knowledge of the IDE.
        if let Some(a) = self.attached() {
            hb.tasks_running = a.worker.running();
        }
        let hb = hb.sanitised();
        let tls_fp = self.tls_fingerprint().unwrap_or_default();
        let relay_box = self.relay_link().map(|l| l.mailbox_text()).unwrap_or_default();
        let (jobs, me) = {
            let i = lock(&self.inner);
            let me = i.keys.public().sign;
            let mut jobs = vec![];
            for (fid, f) in &i.fleets {
                let known: BTreeMap<String, String> = f.peers.iter().filter_map(|(id, p)| p.addr.clone().map(|a| (id.clone(), a))).collect();
                for m in &f.rec.members {
                    if m.id == me {
                        continue;
                    }
                    if let Some(a) = f.peers.get(&m.id).and_then(|p| p.addr.clone()) {
                        if let Ok(e) = self.sealed(&i, fid, "heartbeat", &json!(HbBody { hb: hb.clone(), port: i.port, known: known.clone(), tls: tls_fp.clone(), relay: relay_box.clone() })) {
                            jobs.push((fid.clone(), m.id.clone(), a, e));
                        }
                    }
                }
            }
            (jobs, me)
        };
        {
            // this system's own numbers are shown too
            let mut i = lock(&self.inner);
            let now = self.now();
            for f in i.fleets.values_mut() {
                if let Some(p) = f.peers.get_mut(&me) {
                    p.hb = Some(hb.clone());
                    p.last_seen = now;
                }
            }
        }
        for (fid, mid, addr, env) in jobs {
            match self.send_to(&addr, &env) {
                Ok(r) if r.ok => {
                    if let Some(p) = lock(&self.inner).fleets.get_mut(&fid).and_then(|f| f.peers.get_mut(&mid)) {
                        p.refused = None;
                    }
                }
                Ok(r) => {
                    let why = r.error.clone();
                    if let Some(p) = lock(&self.inner).fleets.get_mut(&fid).and_then(|f| f.peers.get_mut(&mid)) {
                        if why.contains("not a member") {
                            p.refused = Some(why);
                        }
                    }
                }
                Err(_) => {}
            }
        }
        self.poll_pending();
    }

    // ------------------------------------------------------------------------------------------ the node

    /// Start listening on `bind` (like `0.0.0.0:7671` or `127.0.0.1:0`) and sending heartbeats. Returns the address it listens on.
    pub fn start_node(self: &Arc<Self>, bind: &str, source: Arc<dyn HeartbeatSource>) -> Result<SocketAddr, String> {
        let mut n = lock(&self.node);
        if let Some(node) = n.as_ref() {
            *lock(&self.source) = Some(source);
            return Ok(node.addr);
        }
        let tls_cfg = lock(&self.tls).as_ref().map(|t| t.0.clone());
        let l = TcpListener::bind(bind).map_err(|e| format!("could not listen on {bind}: {e}"))?;
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let addr = l.local_addr().map_err(|e| e.to_string())?;
        lock(&self.inner).port = addr.port();
        *lock(&self.source) = Some(source);
        let stop = Arc::new(AtomicBool::new(false));
        let (h1, s1) = (self.clone(), stop.clone());
        let accept = std::thread::spawn(move || {
            while !s1.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((s, peer)) => {
                        if h1.conns.load(Ordering::SeqCst) >= MAX_CONNS {
                            continue;
                        }
                        h1.conns.fetch_add(1, Ordering::SeqCst);
                        let h2 = h1.clone();
                        let cfg = tls_cfg.clone();
                        std::thread::spawn(move || {
                            h2.serve(s, peer, cfg);
                            h2.conns.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        });
        let (h3, s3) = (self.clone(), stop.clone());
        let beat = std::thread::spawn(move || {
            let mut waited = Duration::ZERO;
            let step = Duration::from_millis(20);
            while !s3.load(Ordering::SeqCst) {
                std::thread::sleep(step);
                waited += step;
                let iv = *lock(&h3.interval);
                if waited >= iv {
                    waited = Duration::ZERO;
                    if let Some(src) = lock(&h3.source).clone() {
                        h3.heartbeat_tick(src.as_ref());
                        h3.kick();
                        h3.kick_lost();
                    }
                }
            }
        });
        *n = Some(Node { stop, addr, threads: vec![accept, beat] });
        drop(n);
        self.audit("fleet.node.start", &format!("port={} transport={}", addr.port(), if lock(&self.tls).is_some() { "tls" } else { "plain" }));
        Ok(addr)
    }

    pub fn stop_node(&self) {
        let node = lock(&self.node).take();
        if let Some(mut n) = node {
            n.stop.store(true, Ordering::SeqCst);
            for t in n.threads.drain(..) {
                let _ = t.join();
            }
            lock(&self.inner).port = 0;
            self.audit("fleet.node.stop", "");
        }
    }

    pub fn listening(&self) -> Option<SocketAddr> {
        lock(&self.node).as_ref().map(|n| n.addr)
    }

    fn serve(&self, s: TcpStream, peer: SocketAddr, tls: Option<Arc<rustls::ServerConfig>>) {
        let _ = s.set_nonblocking(false);
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        match tls {
            None => {
                let mut s = s;
                self.serve_stream(&mut s, peer);
            }
            Some(cfg) => {
                // a peer that does not speak TLS (or shows no good handshake) gets nothing back and is dropped
                let Ok(conn) = rustls::ServerConnection::new(cfg) else { return };
                let mut s = rustls::StreamOwned::new(conn, s);
                self.serve_stream(&mut s, peer);
                s.conn.send_close_notify();
                let _ = s.flush();
            }
        }
    }

    fn serve_stream<S: Read + Write>(&self, s: &mut S, peer: SocketAddr) {
        let reply = match read_frame::<Env>(s) {
            Ok(env) => self.handle(env, peer),
            Err(e) => Reply::err(&e),
        };
        let _ = write_frame(s, &reply);
        let _ = s.flush();
    }

    // ------------------------------------------------------------------------------------------ receiving

    fn check_time_and_nonce(&self, i: &mut Inner, env: &Env) -> Result<(), String> {
        let now = self.now();
        if env.v != 1 {
            return Err("a message version this system does not know".into());
        }
        if env.ts.abs_diff(now) > SKEW {
            return Err("the message is too old or too new (check the clocks)".into());
        }
        if env.nonce.len() < 16 || env.nonce.len() > 64 {
            return Err("bad nonce".into());
        }
        while i.seen.front().is_some_and(|(t, _)| now.saturating_sub(*t) > SKEW * 2) {
            i.seen.pop_front();
        }
        let key = format!("{}/{}", env.from, env.nonce);
        if i.seen.iter().any(|(_, k)| *k == key) {
            return Err("a message seen before".into());
        }
        i.seen.push_back((env.ts, key));
        if i.seen.len() > 4096 {
            i.seen.pop_front();
        }
        Ok(())
    }

    fn handle(&self, env: Env, peer: SocketAddr) -> Reply {
        match env.kind.as_str() {
            "join_request" | "join_poll" => self.handle_join(env, peer),
            "record_push" | "heartbeat" | "assign" | "assign_reply" | "leave_notice" | "task_status" | "task_answer" | "base_chunk" | "base_have" | "handback" | "handback_have" | "context" | "models_query" => self.handle_member(env, peer),
            _ => Reply::err("a message kind this system does not know"),
        }
    }

    fn handle_join(&self, env: Env, peer: SocketAddr) -> Reply {
        let body: Value = match serde_json::from_str(&env.body) {
            Ok(v) => v,
            Err(_) => return Reply::err("bad message"),
        };
        let wrap = body["wrap"].as_str().unwrap_or("").to_string();
        let keys = PublicKeys { wrap: wrap.clone(), sign: env.from.clone() };
        if keys.check().is_err() || !keys.verify_text(DOMAIN_JOIN, env.text().as_bytes(), &env.sig) {
            return Reply::err("the message is not signed by the key it names");
        }
        let now = self.now();
        let mut i = lock(&self.inner);
        if let Err(e) = self.check_time_and_nonce(&mut i, &env) {
            return Reply::err(&e);
        }
        if env.kind == "join_poll" {
            let t = body["ticket"].as_str().unwrap_or("");
            return match i.requests.iter().find(|r| r.id == t && r.member.id == env.from) {
                None => Reply::err("no such request"),
                Some(r) if now.saturating_sub(r.at) > CODE_LIFE * 3 => Reply::err("the request is too old: ask again"),
                Some(r) => match r.state {
                    ReqState::Pending => Reply::ok("pending", Value::Null),
                    ReqState::Declined => Reply::ok("declined", Value::Null),
                    ReqState::Approved => match i.fleets.get(&r.fleet) {
                        Some(f) => Reply::ok("approved", json!({"record": f.rec})),
                        None => Reply::err("that fleet is gone"),
                    },
                },
            };
        }
        // a join request
        i.bad_codes.retain(|t| now.saturating_sub(*t) < LOCK_WINDOW);
        if i.bad_codes.len() >= MAX_BAD_CODES {
            i.offers.clear();
            drop(i);
            self.audit_once("join-locked", "fleet.join.locked", "too many wrong codes");
            return Reply::err("too many wrong codes: joining is locked for ten minutes");
        }
        i.offers.retain(|o| o.expires > now);
        let given = normalise_code(body["code"].as_str().unwrap_or(""));
        let pos = i.offers.iter().position(|o| {
            let h = code_hash(&o.fleet, &given);
            same_hash(&h, &o.hash)
        });
        let Some(pos) = pos else {
            i.bad_codes.push(now);
            drop(i);
            self.audit("fleet.join.badcode", &format!("who={}", fingerprint(&env.from)));
            return Reply::err("that code is not valid (it works once, for ten minutes)");
        };
        let offer = i.offers.remove(pos); // one use
        let name = clean(body["name"].as_str().unwrap_or(""), 40);
        if name.is_empty() {
            return Reply::err("the joining computer gave no name");
        }
        let port = body["port"].as_u64().unwrap_or(0) as u16;
        let pin = crate::devmode::auth::norm_fp(body["tls"].as_str().unwrap_or(""));
        let addr = match (port, pin.len()) {
            (0, _) => String::new(),
            // the joining system listens over TLS: reach it back over TLS with the certificate it named
            (_, 64) => tls::advertised(peer.ip(), port, &pin),
            _ => format!("{}:{port}", peer.ip()),
        };
        let member = Member { id: env.from.clone(), wrap, name, admin: false };
        if i.fleets.get(&offer.fleet).is_some_and(|f| f.rec.member(&member.id).is_some()) {
            return Reply::err("that computer is already a member");
        }
        let id = hex(&random::<16>().unwrap_or([0; 16]));
        i.requests.push(JoinReq { id: id.clone(), fleet: offer.fleet.clone(), member, addr, state: ReqState::Pending, at: now });
        drop(i);
        self.audit("fleet.join.request", &format!("fleet={} who={}", short_id(&offer.fleet), fingerprint(&env.from)));
        Reply::ok("pending", json!({"ticket": id}))
    }

    fn handle_member(&self, env: Env, peer: SocketAddr) -> Reply {
        let mut i = lock(&self.inner);
        let Some(f) = i.fleets.get(&env.fleet) else {
            drop(i);
            self.audit_once(&format!("nf/{}", env.from), "fleet.refused", &format!("reason=unknown_fleet from={}", fingerprint(&env.from)));
            return Reply::err("not a member: this system does not know that fleet");
        };
        let Some(sender) = f.rec.member(&env.from).cloned() else {
            let fp = fingerprint(&env.from);
            drop(i);
            self.audit_once(&format!("nm/{}", env.from), "fleet.refused", &format!("reason=not_member from={fp}"));
            return Reply::err("not a member of this fleet (any more)");
        };
        if !sender.keys().verify_text(DOMAIN_MSG, env.text().as_bytes(), &env.sig) {
            let fp = fingerprint(&env.from);
            drop(i);
            self.audit_once(&format!("bs/{}", env.from), "fleet.refused", &format!("reason=bad_signature from={fp}"));
            return Reply::err("the signature is not valid");
        }
        if let Err(e) = self.check_time_and_nonce(&mut i, &env) {
            return Reply::err(&e);
        }
        let fid = env.fleet.clone();
        // a record push is public (the record is signed by an administrator and carries only wrapped keys)
        if env.kind == "record_push" {
            let new: Record = match serde_json::from_str(&env.body) {
                Ok(r) => r,
                Err(_) => return Reply::err("the record could not be read"),
            };
            let old = i.fleets[&fid].rec.clone();
            if let Err(e) = new.accept_over(&old) {
                drop(i);
                self.audit_once(&format!("rp/{}", env.from), "fleet.refused", &format!("reason=record from={} {e}", fingerprint(&env.from)));
                return Reply::err(&e);
            }
            let me = i.keys.public().sign;
            if new.member(&me).is_none() {
                // this system was removed: forget the fleet
                i.fleets.remove(&fid);
                i.inbox.retain(|x| x.fleet != fid);
                let _ = std::fs::remove_file(self.fd().join("records").join(format!("{}.json", state_dir_name(&fid))));
                self.save_misc(&i);
                drop(i);
                self.audit("fleet.removed", &format!("fleet={}", short_id(&fid)));
                return Reply::ok("removed", Value::Null);
            }
            let key = match new.open_key(&i.keys) {
                Ok(k) => Arc::new(k),
                Err(e) => return Reply::err(&e),
            };
            if self.save_record(&new).is_err() {
                return Reply::err("could not keep the record");
            }
            let f = i.fleets.get_mut(&fid).unwrap_or_else(|| unreachable!());
            f.peers.retain(|id, _| new.member(id).is_some());
            for m in &new.members {
                f.peers.entry(m.id.clone()).or_insert(Peer { addr: None, last_seen: 0, hb: None, refused: None, tls_left: false });
            }
            if let Some(p) = f.peers.get_mut(&env.from) {
                p.last_seen = self.now();
            }
            f.rec = new;
            f.key = key;
            let trusted: Vec<String> = i.config.trusted.iter().filter(|t| i.fleets[&fid].rec.member(t).is_none()).cloned().collect();
            for t in trusted {
                i.config.trusted.remove(&t);
            }
            self.save_misc(&i);
            drop(i);
            self.audit("fleet.record.accepted", &format!("fleet={} from={}", short_id(&fid), fingerprint(&env.from)));
            return Reply::ok("ok", Value::Null);
        }
        let f = &i.fleets[&fid];
        if env.generation != f.rec.generation {
            return Reply::err("the group key generation differs (the record is being updated: try again)");
        }
        let plain = match f.key.open(&fid, &env.kind, f.rec.generation, &env.body).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            Some(v) => v,
            None => return Reply::err("the message does not open with the group key"),
        };
        match env.kind.as_str() {
            "heartbeat" => {
                let Ok(hb) = serde_json::from_value::<HbBody>(plain) else { return Reply::err("bad heartbeat") };
                let now = self.now();
                let me = i.keys.public().sign;
                let f = i.fleets.get_mut(&fid).unwrap_or_else(|| unreachable!());
                let fresh = f.peers.get(&env.from).is_none_or(|p| p.last_seen == 0 || now.saturating_sub(p.last_seen) > 60);
                let ids: BTreeSet<String> = f.rec.members.iter().map(|m| m.id.clone()).collect();
                let p = f.peers.entry(env.from.clone()).or_insert(Peer { addr: None, last_seen: 0, hb: None, refused: None, tls_left: false });
                p.last_seen = now;
                p.hb = Some(hb.hb.sanitised());
                p.refused = None;
                p.tls_left = hb.tls.is_empty() && p.addr.as_deref().is_some_and(|a| a.starts_with(tls::SCHEME));
                if !hb.relay.is_empty() && relay::valid_mailbox(&hb.relay) && p.addr.is_none() {
                    // a member that can be reached only through the relay: its mailbox is its address until a direct one is given
                    p.addr = Some(format!("{}{}", relay::SCHEME, hb.relay));
                }
                if hb.port != 0 && !peer.ip().is_unspecified() {
                    let fp = crate::devmode::auth::norm_fp(&hb.tls);
                    let valid = hb.tls.len() <= 100 && fp.len() == 64;
                    let learned = if valid { tls::advertised(peer.ip(), hb.port, &fp) } else { format!("{}:{}", peer.ip(), hb.port) };
                    p.addr = Some(match p.addr.as_deref().map(Target::parse) {
                        // an address a person gave stays as given; only the pin follows the member's own signed word (a new certificate)
                        Some(Ok(Target::Tls { addr, .. })) if valid => Target::Tls { addr, fingerprint: fp }.text(),
                        Some(Ok(Target::Tls { .. })) => p.addr.clone().unwrap_or(learned),
                        _ => learned,
                    });
                }
                for (id, a) in hb.known {
                    if id != me && ids.contains(&id) && a.len() <= 100 {
                        let q = f.peers.entry(id).or_insert(Peer { addr: None, last_seen: 0, hb: None, refused: None, tls_left: false });
                        if q.addr.is_none() {
                            q.addr = Some(a);
                        }
                    }
                }
                drop(i);
                if fresh {
                    self.audit("fleet.heartbeat", &format!("fleet={} from={} (first after silence)", short_id(&fid), fingerprint(&env.from)));
                }
                Reply::ok("ok", Value::Null)
            }
            "assign" => {
                let body: AssignBody = match serde_json::from_value(plain) {
                    Ok(b) => b,
                    Err(e) => return Reply::err(&format!("the assignment has fields this system does not accept: {e}")),
                };
                if let Err(e) = body.check() {
                    drop(i);
                    self.audit("fleet.assign.refused", &format!("from={} {}", fingerprint(&env.from), e.split(';').next().unwrap_or("")));
                    return Reply::err(&e);
                }
                let mode = i.config.accept;
                let state = match mode {
                    Accept::Never => {
                        drop(i);
                        self.audit("fleet.assign.refused", &format!("from={} reason=accepts_no_work", fingerprint(&env.from)));
                        return Reply::err("this computer does not accept work");
                    }
                    Accept::Members if i.config.trusted.contains(&env.from) => "accepted",
                    _ => "waiting",
                };
                if i.inbox.iter().any(|x| x.id == body.id) {
                    return Reply::err("that task is already here");
                }
                if i.inbox.iter().filter(|x| x.state == "waiting").count() >= 50 {
                    return Reply::err("the inbox of this computer is full");
                }
                let item = InboxItem { id: body.id.clone(), fleet: fid.clone(), from: env.from.clone(), from_name: sender.name.clone(), body, state: state.into(), at: self.now(), task: String::new(), detail: String::new(), answer: String::new() };
                i.inbox.push(item.clone());
                self.save_misc(&i);
                drop(i);
                self.kick();
                self.audit("fleet.assign.in", &format!("fleet={} from={} task={} state={state}", short_id(&fid), fingerprint(&env.from), &item.id[..8]));
                Reply::ok("queued", json!({"state": state}))
            }
            "assign_reply" => {
                let id = plain["id"].as_str().unwrap_or("").to_string();
                let st = plain["state"].as_str().unwrap_or("").to_string();
                if !matches!(st.as_str(), "accepted" | "declined") {
                    return Reply::err("bad answer");
                }
                match i.outbox.iter_mut().find(|o| o.id == id && o.to == env.from) {
                    Some(o) => o.state = st.clone(),
                    None => return Reply::err("no such task was sent to you"),
                }
                self.save_misc(&i);
                drop(i);
                self.audit("fleet.assign.reply", &format!("from={} task={} state={st}", fingerprint(&env.from), &id[..8.min(id.len())]));
                Reply::ok("ok", Value::Null)
            }
            "models_query" => {
                drop(i);
                let Some(h) = lock(&self.models_handler).clone() else { return Reply::err("this computer shares no model list") };
                let list = h();
                self.audit_once(&format!("mq/{}", env.from), "fleet.models.out", &format!("fleet={} to={} models={}", short_id(&fid), fingerprint(&env.from), list.len()));
                Reply::ok("models", json!({"models": list}))
            }
            "task_status" | "task_answer" | "base_chunk" | "base_have" | "handback" | "handback_have" | "context" => self.handle_work(i, &env, &fid, &sender.name, plain),
            "leave_notice" => {
                // the member left: the administrator with the lowest id among those that remain makes the removal (so two do not both)
                let me = i.keys.public().sign;
                let keys = i.keys.clone();
                let f = i.fleets.get_mut(&fid).unwrap_or_else(|| unreachable!());
                let designated = f.rec.members.iter().filter(|m| m.admin && m.id != env.from).map(|m| m.id.clone()).min();
                if designated.as_deref() != Some(me.as_str()) {
                    // not ours to do: keep the member listed until the designated administrator's record arrives
                    return Reply::ok("ok", Value::Null);
                }
                let Ok((rec, key)) = f.rec.without_member(&keys, &env.from) else { return Reply::ok("ok", Value::Null) };
                if self.save_record(&rec).is_err() {
                    return Reply::err("could not keep the record");
                }
                f.rec = rec;
                f.key = Arc::new(key);
                f.peers.remove(&env.from);
                i.config.trusted.remove(&env.from);
                self.save_misc(&i);
                let jobs = self.push_jobs(&i, &fid, None);
                drop(i);
                // sent from this thread: the person who left is waiting for nothing, the others get the new record
                let link = self.relay_link();
                std::thread::spawn(move || {
                    for (addr, env) in jobs {
                        let _ = exchange_via(link.as_ref(), &addr, &env, &|_, _, _| Err("not read here".into()));
                    }
                });
                self.audit("fleet.member.left", &format!("fleet={} who={}", short_id(&fid), fingerprint(&env.from)));
                Reply::ok("ok", Value::Null)
            }
            _ => Reply::err("a message kind this system does not know"),
        }
    }
}

fn offer_text(r: &Record) -> String {
    let who: Vec<String> = r.members.iter().map(|m| format!("{} ({}{})", m.name, fingerprint(&m.id), if m.admin { ", administrator" } else { "" })).collect();
    let signer = r.member(&r.signer).map(|m| format!("{} {}", m.name, fingerprint(&m.id))).unwrap_or_default();
    format!("Join the fleet {} ({}), approved by {signer}. Members: {}. Compare the fingerprints with what the other computers show.", r.name, short_id(&r.fleet), who.join("; "))
}

#[cfg(test)]
#[path = "fleet_hub_tests.rs"]
mod tests;

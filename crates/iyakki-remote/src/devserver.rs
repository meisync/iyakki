//! The developer-mode listener (`iyakki dev ...` talks to it): HTTPS only, with the certificate of the web front end, on port 7979
//! (Settings → Developer mode can change it), open **only while developer mode is on**. [`watch`] starts it when an administrator
//! switches developer mode on and stops it when it goes off (by hand or by the automatic switch-off); every request checks again.
//!
//! Routes: `GET /v1/dev/hello` and `POST /v1/dev/pair` need no key (the pairing code is the proof); everything else must be signed
//! by a paired key (`iyakki_agent::devmode::auth`) and acts with the role of the account that key belongs to:
//! `GET whoami`, `POST install[?scope=shared]` (the package is the body), `POST uninstall?id=`, `GET list[?all=1]`,
//! `GET logs?id=&since=`, `POST open?id=`, `GET screenshot`. Each action goes to the developer-mode activity log and the audit log.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_agent::devmode::{api, auth, debug, ops, DevMode};
use iyakki_agent::store::install::Actor;
use iyakki_agent::store::Store;
use iyakki_shell::shell::ShellEnv;
use serde_json::json;

use crate::http::{Parse, Request, Response};
use crate::session::RateLimit;

const MAX_CONNECTIONS: usize = 16;
const FIRST_BYTES: Duration = Duration::from_secs(10);
const WHOLE_REQUEST: Duration = Duration::from_secs(120);
/// The user name the shell answers a developer screenshot for (not a valid account name, so it can never be a person's).
pub const SCREEN_USER: &str = iyakki_agent::devmode::SCREEN_ANY;

struct Shared {
    env: ShellEnv,
    dm: DevMode,
    fp: String,
    tls: Arc<rustls::ServerConfig>,
    nonces: auth::Nonces,
    /// Refused signatures per address (a stranger hammering the port is slowed down).
    fails: RateLimit,
    /// Pairing attempts per address.
    pairs: RateLimit,
    open: AtomicUsize,
    stop: AtomicBool,
    screenshot_wait: Duration,
    /// Where /proc, /sys and the logs are read from (a made-up machine in tests).
    paths: debug::Paths,
    /// A home folder to use for every account instead of the usual one (tests only).
    home: Option<PathBuf>,
    /// The firewall engine `config firewall` talks to (the process-wide one; tests give their own).
    cfg_opts: Mutex<crate::devconfig::Options>,
    /// The port this listener really has (a forward to it is refused).
    port: u16,
    update_opts: Mutex<UpdateOpts>,
}

/// What system updates talk to; the defaults are the real boot partition and `/etc/iyakki-release` (tests give their own).
#[derive(Clone, Default)]
pub struct UpdateOpts {
    pub boot: Option<Arc<dyn iyakki_agent::devmode::update::BootMount>>,
    pub release: Option<String>,
    pub data_free: Option<u64>,
}

/// A running listener; it stops when dropped.
pub struct DevServer {
    shared: Arc<Shared>,
    addr: SocketAddr,
}

impl DevServer {
    /// Listen on `addr` with the TLS identity `id` (its fingerprint is what computers pin).
    pub fn start(env: ShellEnv, addr: &str, id: &crate::tls::Identity) -> std::io::Result<DevServer> {
        Self::start_with_paths(env, addr, id, debug::Paths::real(), None)
    }

    /// [`DevServer::start`] reading `/proc`, `/sys` and the logs from `paths`, and using `home` as every account's home folder (tests).
    pub fn start_with_paths(env: ShellEnv, addr: &str, id: &crate::tls::Identity, paths: debug::Paths, home: Option<PathBuf>) -> std::io::Result<DevServer> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let dm = DevMode::open(env.services.dir());
        let shared = Arc::new(Shared {
            env,
            dm,
            fp: id.fingerprint.clone(),
            tls: id.config.clone(),
            nonces: auth::Nonces::default(),
            fails: RateLimit::new(60),
            pairs: RateLimit::new(10),
            open: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            screenshot_wait: Duration::from_secs(10),
            paths,
            home,
            cfg_opts: Mutex::new(Default::default()),
            port: addr.port(),
            update_opts: Mutex::new(UpdateOpts::default()),
        });
        let s = shared.clone();
        std::thread::Builder::new().name("iyakki-dev-accept".into()).spawn(move || accept_loop(listener, s))?;
        Ok(DevServer { shared, addr })
    }

    /// Use another boot partition, release text and data-partition size for system updates (tests).
    pub fn use_update_options(&self, o: UpdateOpts) {
        *self.shared.update_opts.lock().unwrap_or_else(|e| e.into_inner()) = o;
    }

    /// Use another firewall engine and host name file (tests).
    pub fn use_config_options(&self, o: crate::devconfig::Options) {
        *self.shared.cfg_opts.lock().unwrap_or_else(|e| e.into_inner()) = o;
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn fingerprint(&self) -> &str {
        &self.shared.fp
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: TcpListener, sh: Arc<Shared>) {
    while !sh.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if sh.open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue;
                }
                sh.open.fetch_add(1, Ordering::Relaxed);
                let s = sh.clone();
                let spawned = std::thread::Builder::new().name("iyakki-dev-conn".into()).stack_size(8 * 1024 * 1024).spawn(move || {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(stream, peer, &s))).is_err() {
                        log::error!("developer mode: a connection handler crashed");
                    }
                    s.open.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    sh.open.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn connection(stream: TcpStream, peer: SocketAddr, sh: &Shared) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(FIRST_BYTES));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let Ok(c) = rustls::ServerConnection::new(sh.tls.clone()) else { return };
    let mut conn = rustls::StreamOwned::new(c, stream);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let started = Instant::now();
    let resp = loop {
        match crate::http::parse_limited(&buf, ops::MAX_PACKAGE) {
            Parse::Done(r) => break route(&r, peer, sh),
            Parse::Bad(code, msg) => break Response::error(code, msg),
            Parse::Incomplete => {}
        }
        if started.elapsed() > WHOLE_REQUEST {
            return;
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let _ = conn.write_all(&resp.to_bytes());
    let _ = conn.flush();
    conn.conn.send_close_notify();
    let _ = conn.flush();
}

fn refused(sh: &Shared, peer: SocketAddr, what: &str, why: &str) {
    sh.dm.log("?", "", what, &format!("{why} (from {})", peer.ip()), false);
}

fn route(req: &Request, peer: SocketAddr, sh: &Shared) -> Response {
    // nothing at all while developer mode is off (the listener is also being stopped)
    if !sh.dm.is_on() {
        return Response::error(403, "developer mode is off on this device");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/dev/hello") => Response::json(200, &json!({ "name": sh.env.hostname, "version": env!("CARGO_PKG_VERSION"), "fingerprint": sh.fp, "pairing": sh.dm.pairing().is_some() })),
        ("POST", "/v1/dev/pair") => pair(req, peer, sh),
        (_, p) if p.starts_with("/v1/dev/") => signed(req, peer, sh),
        _ => Response::error(404, "not found"),
    }
}

fn pair(req: &Request, peer: SocketAddr, sh: &Shared) -> Response {
    if !sh.pairs.allow(&peer.ip().to_string()) {
        refused(sh, peer, "devmode.pair", "too many pairing attempts");
        return Response::error(429, "too many pairing attempts, wait a minute");
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&req.body) else { return Response::error(400, "bad request") };
    let (name, key, proof) = (v["name"].as_str().unwrap_or(""), v["key"].as_str().unwrap_or(""), v["proof"].as_str().unwrap_or(""));
    match sh.dm.complete_pairing(&sh.fp, name, key, proof) {
        Ok((k, answer)) => {
            sh.env.audit.record(&k.account, "devmode.pair", &format!("{} ({}) from {}", k.name, k.short(), peer.ip()));
            Response::json(200, &json!({ "ok": true, "device": sh.env.hostname, "account": k.account, "answer": answer }))
        }
        Err(e) => Response::error(403, &e),
    }
}

/// The account's actor (its role decides personal or shared installs, and whether it may use the STORE at all).
fn actor_of(sh: &Shared, account: &str) -> Result<(Actor, iyakki_sec::policy::Policy, String), String> {
    let store = sh.env.store.lock().map_err(|_| "busy".to_string())?;
    let u = store.get(account).ok_or_else(|| format!("the account {account} that this computer acts as no longer exists on the device"))?;
    let policy = u.policy();
    let actor = Actor::from_policy(account, &policy);
    if !actor.allowed {
        return Err(format!("the account {account} may not use the STORE"));
    }
    Ok((actor, policy, u.role.name().to_string()))
}

fn signed(req: &Request, peer: SocketAddr, sh: &Shared) -> Response {
    let ip = peer.ip().to_string();
    let query: Vec<(String, String)> = req.query.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let headers = |n: &str| req.header(n).map(str::to_string);
    let key = match auth::check(&headers, &req.method, &req.path, &query, &req.body, &sh.fp, sh.dm.now(), &sh.nonces, &|k: &str| sh.dm.find_key(k).is_some()) {
        Ok(k) => k,
        Err(e) => {
            if !sh.fails.allow(&ip) {
                return Response::error(429, "too many refused requests, wait a minute");
            }
            refused(sh, peer, "devmode.auth", &format!("{} {}: {e}", req.method, req.path));
            return Response::error(if e == auth::AuthError::UnknownKey { 403 } else { 401 }, &e.to_string());
        }
    };
    let Some(k) = sh.dm.find_key(&key) else { return Response::error(403, "not paired") };
    sh.dm.touch_key(&key);
    let (actor, policy, role) = match actor_of(sh, &k.account) {
        Ok(x) => x,
        Err(e) => {
            sh.dm.log(&k.name, &k.account, "devmode.refused", &e, false);
            return Response::error(403, &e);
        }
    };
    let who = ops::Who { computer: k.name.clone(), account: k.account.clone() };
    let svc = &sh.env.services;
    let store = Store::for_services(svc);
    let audit = |action: &str, detail: &str| sh.env.audit.record(&k.account, action, &format!("{} (developer computer): {detail}", k.name));
    let q = |n: &str| req.query.get(n).map(String::as_str).unwrap_or("");
    // the routes of the scopes beyond install (debug, files, input, config, shell, ...): the scope is checked first, in one place
    {
        let ext = crate::devconfig::ShellConfig::new(svc.dir().to_path_buf(), sh.env.hostname.clone(), sh.cfg_opts.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let power = |reboot: bool| {
            let host = sh.env.host.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                if reboot { host.reboot() } else { host.power_off() }
            });
        };
        let ub = sh.update_opts.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let boot: Option<Arc<dyn iyakki_agent::devmode::update::BootMount>> = Some(ub.boot.clone().unwrap_or_else(|| Arc::new(iyakki_agent::devmode::bootmount::SysBoot::real(svc.dir()))));
        let release = ub.release.clone().unwrap_or_else(|| std::fs::read_to_string(iyakki_agent::relinfo::RELEASE_FILE).unwrap_or_default());
        let audit_fn = |a: &str, d: &str| audit(a, d);
        let tail = |n: usize| { let mut v = sh.env.audit.recent(n); v.reverse(); v };
        let cx = api::Ctx { dm: &sh.dm, key: &k, policy: &policy, role: &role, hostname: &sh.env.hostname, paths: &sh.paths, audit_tail: &tail, audit: &audit_fn, firewall_file: svc.dir().join("firewall.json"), home: sh.home.clone(), svc: Some(svc.as_ref()), ext: Some(&ext), own_port: sh.port, power: &power, boot: boot.as_deref(), release: &release, volatile: sh.env.data_volatile, data_free: ub.data_free };
        let qs = |n: &str| q(n).to_string();
        if let Some(o) = api::handle(&cx, &req.method, &req.path, &qs, &req.body) {
            return Response::new(o.status, o.ctype, o.body);
        }
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/dev/whoami") => {
            sh.dm.log(&k.name, &k.account, "dev.whoami", "", true);
            Response::json(200, &json!({ "device": sh.env.hostname, "computer": k.name, "account": k.account, "role": role, "status": sh.dm.status_text(), "scopes": k.scopes }))
        }
        ("POST", "/v1/dev/install") => match ops::install(svc, &store, &sh.dm, &actor, &who, req.body.clone(), q("scope") == "shared") {
            Ok(r) => {
                audit("devmode.install", &format!("{}:{} {} ({})", r.kind, r.name, r.version, r.scope));
                Response::json(200, &serde_json::to_value(&r).unwrap_or_default())
            }
            Err(e) => {
                audit("devmode.install.refused", &e);
                Response::error(422, &e)
            }
        },
        ("POST", "/v1/dev/uninstall") => match ops::uninstall(svc, &store, &sh.dm, &actor, &who, q("id")) {
            Ok(r) => {
                audit("devmode.uninstall", &format!("{}:{} {}", r.kind, r.name, r.version));
                Response::json(200, &json!({ "kind": r.kind, "name": r.name, "version": r.version }))
            }
            Err(e) => Response::error(422, &e),
        },
        ("GET", "/v1/dev/list") => {
            let items: Vec<serde_json::Value> = ops::list(&store, &actor, q("all") == "1")
                .into_iter()
                .map(|r| {
                    let level = iyakki_agent::store::trust::Level::ALL.into_iter().find(|l| l.id() == r.level).map_or(r.level.clone(), |l| l.label().to_string());
                    json!({ "kind": r.kind, "name": r.name, "version": r.version, "trust": level, "scope": r.scope, "source": r.source, "disabled": r.disabled })
                })
                .collect();
            sh.dm.log(&k.name, &k.account, "dev.list", &format!("{} items", items.len()), true);
            Response::json(200, &json!({ "items": items }))
        }
        ("GET", "/v1/dev/logs") => {
            let id = q("id");
            // only the logs of what this account can see (installed, or once installed by developer mode)
            let visible = ops::list(&store, &actor, true).iter().any(|r| r.name == id) || sh.dm.activity(400).iter().any(|a| a.account == k.account && a.detail.contains(&format!(":{id} ")));
            if !visible {
                sh.dm.log(&k.name, &k.account, "dev.logs", &format!("refused {id}: not installed for this account"), false);
                return Response::error(404, &format!("{id} is not installed for {}", k.account));
            }
            let since: u64 = q("since").parse().unwrap_or(0);
            let (text, next) = sh.dm.read_app_log(id, since);
            if since == 0 {
                sh.dm.log(&k.name, &k.account, "dev.logs", id, true);
            }
            Response::json(200, &json!({ "text": text, "next": next, "more": text.len() >= 64 * 1024 }))
        }
        ("POST", "/v1/dev/open") => {
            let id = q("id");
            let tiles = iyakki_agent::store::tilehub::installed(svc, &policy, &k.account);
            if !tiles.iter().any(|t| t.name == id) {
                sh.dm.log(&k.name, &k.account, "dev.open", &format!("refused {id}: no such app for this account"), false);
                return Response::error(404, &format!("{id} is not an app installed for {} (only tile apps open on the screen)", k.account));
            }
            iyakki_agent::devmode::request_open(svc.dir(), id);
            sh.dm.log(&k.name, &k.account, "dev.open", id, true);
            audit("devmode.open", id);
            let msg = if svc.screen.present() { format!("Asked the device to open {id}: it opens on the screen of the person signed in (if they have it).") } else { format!("Queued {id}: no screen is drawing on the device right now; it opens when someone signs in.") };
            Response::json(200, &json!({ "queued": true, "message": msg }))
        }
        ("GET", "/v1/dev/screenshot") if !matches!(q("display"), "" | "0") => {
            sh.dm.log(&k.name, &k.account, "dev.screenshot", &format!("refused: display {}", q("display")), false);
            Response::error(422, "this device draws on one screen (display 0); other displays are not available yet")
        }
        ("GET", "/v1/dev/screenshot") => match svc.screen.request(SCREEN_USER, sh.screenshot_wait) {
            Ok(f) => {
                sh.dm.log(&k.name, &k.account, "dev.screenshot", &format!("{}x{}", f.w, f.h), true);
                audit("devmode.screenshot", &format!("{}x{}", f.w, f.h));
                Response::new(200, "image/png", iyakki_agent::browserjobs::png::encode_rgba(f.w, f.h, &f.rgba))
            }
            Err(e) => {
                sh.dm.log(&k.name, &k.account, "dev.screenshot", &e, false);
                Response::error(503, &e)
            }
        },
        _ => Response::error(404, "not found"),
    }
}

// -------------------------------------------------------------------------------------------- on and off with developer mode

/// Keep the listener in step with developer mode: started (on its port, all addresses) while it is on, stopped when it goes off.
/// Checks every two seconds; the certificate is the web front end's (`<data>/tls`), made the first time.
pub fn watch(env: ShellEnv, tls_dir: PathBuf) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("iyakki-dev-watch".into()).spawn(move || {
        let dm = DevMode::open(env.services.dir());
        let mut running: Option<(DevServer, u16)> = None;
        let mut last_error = String::new();
        loop {
            let on = dm.is_on();
            let port = dm.state().port();
            if running.as_ref().is_some_and(|(_, p)| !on || *p != port) {
                running = None;
                log::info!("developer mode: listener stopped");
            }
            if on && running.is_none() {
                match start_on(&env, &tls_dir, port) {
                    Ok(s) => {
                        log::info!("developer mode: listening on {} (certificate {})", s.local_addr(), s.fingerprint());
                        last_error.clear();
                        running = Some((s, port));
                    }
                    Err(e) if e != last_error => {
                        log::error!("developer mode: {e}");
                        dm.log("system", "", "devmode.listen", &e, false);
                        last_error = e;
                    }
                    Err(_) => {}
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    })
}

fn start_on(env: &ShellEnv, tls_dir: &Path, port: u16) -> Result<DevServer, String> {
    let host = std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_default();
    let names: Vec<String> = [host].into_iter().filter(|h| !h.is_empty()).collect();
    let id = crate::tls::identity(tls_dir, &names)?;
    DevServer::start(env.share(), &format!("0.0.0.0:{port}"), &id).map_err(|e| format!("cannot listen on port {port}: {e}"))
}

/// The fingerprint computers will see (for the pairing screen), making the certificate if there is none yet.
pub fn fingerprint(tls_dir: &Path) -> Result<String, String> {
    crate::tls::identity(tls_dir, &[]).map(|i| i.fingerprint)
}

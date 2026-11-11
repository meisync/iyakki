//! Domains: the saved file, discovery from `kubectl get svc` text, the listener as a managed service (a real listener on a high loopback
//! port, a fake firewall, a fake announcer, a vault in memory), certificates in the vault, expiry. No real k3s, no second computer.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::gitkeys::MemStore;

const NOW: i64 = 1_900_000_000;

struct FakeKube(Mutex<Result<String, String>>);
impl Kube for FakeKube {
    fn services(&self) -> Result<String, String> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct GateLog {
    made: Vec<(GateReach, i64)>,
    open: Vec<u16>,
    closed: Vec<u16>,
    fail: bool,
}
struct FakeGates(Arc<Mutex<GateLog>>);
struct FakeGate(Arc<Mutex<GateLog>>);
impl GateMaker for FakeGates {
    fn make(&self, reach: GateReach, until: i64) -> Result<Arc<dyn PortGate>, String> {
        let mut l = self.0.lock().unwrap();
        if l.fail {
            return Err("the firewall is off".into());
        }
        l.made.push((reach, until));
        Ok(Arc::new(FakeGate(self.0.clone())))
    }
}
impl PortGate for FakeGate {
    fn open(&self, port: u16) -> Result<(), String> {
        self.0.lock().unwrap().open.push(port);
        Ok(())
    }
    fn close(&self, port: u16) {
        self.0.lock().unwrap().closed.push(port);
    }
}

#[derive(Default)]
struct FakeNames(Mutex<Vec<Vec<String>>>);
impl NameSink for FakeNames {
    fn set(&self, names: &[String]) -> Result<(), String> {
        self.0.lock().unwrap().push(names.to_vec());
        Ok(())
    }
}
impl FakeNames {
    fn last(&self) -> Vec<String> {
        self.0.lock().unwrap().last().cloned().unwrap_or_default()
    }
}

pub(crate) struct Rig {
    pub mgr: Arc<Manager>,
    pub dir: PathBuf,
    gate: Arc<Mutex<GateLog>>,
    names: Arc<FakeNames>,
    pub notes: Arc<Mutex<Vec<String>>>,
    pub audit: Arc<Mutex<Vec<String>>>,
    pub store: Arc<MemStore>,
    pub clock: Arc<AtomicI64>,
    kube: Arc<FakeKube>,
}

pub(crate) fn tmp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = std::path::Path::new(&base).join(format!("dom-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn free_port() -> u16 {
    // below the range the system hands out for bind(0) and outgoing connections, so no other test can take it between two starts
    crate::testdir::free_port()
}

pub(crate) fn rig(tag: &str) -> Rig {
    let dir = tmp(tag);
    let gate = Arc::new(Mutex::new(GateLog::default()));
    let names = Arc::new(FakeNames::default());
    let notes = Arc::new(Mutex::new(Vec::new()));
    let audit = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(MemStore::default());
    let clock = Arc::new(AtomicI64::new(NOW));
    let kube = Arc::new(FakeKube(Mutex::new(Ok(KUBECTL.to_string()))));
    let (n2, a2, c2) = (notes.clone(), audit.clone(), clock.clone());
    let deps = Deps {
        kube: kube.clone(),
        secrets: store.clone(),
        gates: Arc::new(FakeGates(gate.clone())),
        names: names.clone(),
        notify: Arc::new(move |t, b| n2.lock().unwrap().push(format!("{t} | {b}"))),
        dialer: Arc::new(|_| Err("no kubectl in the test".into())),
        held: Arc::new(|| vec![7979]),
        web_port: Arc::new(|| Some(9999)),
        audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))),
        now: Arc::new(move || c2.load(Ordering::SeqCst)),
    };
    Rig { mgr: Arc::new(Manager::new(&dir, deps)), dir, gate, names, notes, audit, store, clock, kube }
}

/// What `kubectl get svc -A -o json` answers on the dev cluster: the private store with its NodePort, a ClusterIP app, a headless
/// Service, a UDP-only Service and the cluster's own.
const KUBECTL: &str = r#"{"items":[
 {"metadata":{"namespace":"iyakki-store","name":"iyakki-store"},"spec":{"type":"ClusterIP","clusterIP":"10.43.0.10","ports":[{"name":"http","port":8080,"protocol":"TCP"}]}},
 {"metadata":{"namespace":"iyakki-store","name":"iyakki-store-nodeport"},"spec":{"type":"NodePort","clusterIP":"10.43.0.11","ports":[{"name":"http","port":8080,"protocol":"TCP","nodePort":30580}]}},
 {"metadata":{"namespace":"shop","name":"web"},"spec":{"type":"ClusterIP","clusterIP":"10.43.0.12","ports":[{"port":3000,"protocol":"TCP"},{"port":5353,"protocol":"UDP"}]}},
 {"metadata":{"namespace":"shop","name":"db"},"spec":{"type":"ClusterIP","clusterIP":"None","ports":[{"port":5432}]}},
 {"metadata":{"namespace":"shop","name":"dns-only"},"spec":{"type":"ClusterIP","clusterIP":"10.43.0.13","ports":[{"port":53,"protocol":"UDP"}]}},
 {"metadata":{"namespace":"shop","name":"ext"},"spec":{"type":"ExternalName","ports":[{"port":80}]}},
 {"metadata":{"namespace":"kube-system","name":"kube-dns"},"spec":{"type":"ClusterIP","clusterIP":"10.43.0.10","ports":[{"port":53,"protocol":"TCP"}]}}
]}"#;

fn route(host: &str, port: u16) -> Route {
    Route { host: host.into(), path_prefix: "/".into(), target: Target::Local { port }, scheme: Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members, client_ca: None }
}

/// A tiny HTTP upstream: answers every request with `upstream saw host=<Host header>` and closes.
struct Upstream {
    port: u16,
    stop: Arc<AtomicBool>,
}
impl Upstream {
    fn start() -> Upstream {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((mut c, _)) => {
                        c.set_nonblocking(false).unwrap();
                        c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut buf = [0u8; 4096];
                        let n = c.read(&mut buf).unwrap_or(0);
                        let head = String::from_utf8_lossy(&buf[..n]).to_string();
                        let host = head.lines().find_map(|l| l.strip_prefix("Host: ").or_else(|| l.strip_prefix("host: "))).unwrap_or("?").to_string();
                        let body = format!("upstream saw host={host}");
                        let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Upstream { port, stop }
    }
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn get(port: u16, host: &str) -> String {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(c, "GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut s = String::new();
    let _ = c.read_to_string(&mut s);
    s
}

fn on(r: &Rig, http: u16, https: u16) {
    r.mgr.set_listener(ListenerChange { enabled: Some(true), http_port: Some(http), https_port: Some(https), ..Default::default() }).unwrap();
}

// ------------------------------------------------------------------------------------------------------------------ discovery

#[test]
fn services_found_on_the_cluster_are_offered_with_their_ports_and_the_right_target() {
    let found = parse_services(KUBECTL).unwrap();
    let names: Vec<String> = found.iter().map(|f| format!("{}/{}", f.namespace, f.name)).collect();
    assert_eq!(names, ["iyakki-store/iyakki-store", "iyakki-store/iyakki-store-nodeport", "shop/web", "kube-system/kube-dns"], "headless, UDP-only and ExternalName are left out; kube-system is last");
    let np = &found[1];
    assert_eq!(np.target(&np.ports[0]), Target::Local { port: 30580 }, "a NodePort is reached on this device");
    assert!(np.label(&np.ports[0]).contains("NodePort 30580"));
    let web = &found[2];
    assert_eq!(web.ports.len(), 1, "the UDP port is dropped");
    assert_eq!(web.target(&web.ports[0]), Target::Service { name: "web".into(), port: 3000 });
    assert!(parse_services("not json").is_err() && parse_services("{}").is_err());
    let own = own_services(Some(9999));
    assert_eq!(own[0].target, Target::Local { port: 30580 });
    assert_eq!(own[1].scheme, Scheme::Passthrough, "the HTTPS web desktop passes TLS through");
    assert_eq!(own_services(None).len(), 1);
}

#[test]
fn discovery_goes_through_the_kubectl_helper_and_says_why_when_it_cannot() {
    let r = rig("disc");
    assert_eq!(r.mgr.discover().unwrap().len(), 4);
    *r.kube.0.lock().unwrap() = Err("The cluster is not running. Start it in Store, Setup, k3s.".into());
    assert!(r.mgr.discover().unwrap_err().contains("not running"));
}

// ------------------------------------------------------------------------------------------------------------------ routes

#[test]
fn routes_are_checked_by_the_engine_saved_and_kept_across_a_restart() {
    let r = rig("routes");
    let e = r.mgr.save_route(None, route("not a host", 30580), None).unwrap_err();
    assert!(e.contains("is not a host name"), "{e}");
    let mut bad_path = route("store.local", 30580);
    bad_path.path_prefix = "x".into();
    assert!(r.mgr.save_route(None, bad_path, None).unwrap_err().contains("must start with /"));
    let https = Route { scheme: Scheme::Https, ..route("store.local", 30580) };
    assert!(r.mgr.save_route(None, https, None).unwrap_err().contains("https needs a certificate"));
    let mut missing = Route { scheme: Scheme::Https, cert: Some(CertRef::Vault("nope".into())), ..route("store.local", 30580) };
    assert!(r.mgr.save_route(None, missing.clone(), None).unwrap_err().contains("no certificate called \"nope\""));
    missing.cert = None;
    assert_eq!(r.mgr.routes().len(), 0, "nothing was saved");
    r.mgr.save_route(None, route("Store.local", 30580), None).unwrap();
    assert_eq!(r.mgr.routes()[0].host, "store.local", "host names are lower case");
    assert!(r.mgr.save_route(None, route("store.local", 1), None).unwrap_err().contains("already a route"));
    // edit: the same key replaced
    let mut edited = route("store.local", 30581);
    edited.websocket = false;
    r.mgr.save_route(Some(("store.local", "/")), edited, None).unwrap();
    assert_eq!(r.mgr.routes().len(), 1);
    assert_eq!(r.mgr.routes()[0].target, Target::Local { port: 30581 });
    // a cluster Service needs its namespace, and routes in two namespaces cannot be served together
    let svc = Route { target: Target::Service { name: "web".into(), port: 3000 }, ..route("web.local", 1) };
    assert!(r.mgr.save_route(None, svc.clone(), None).unwrap_err().contains("namespace"));
    r.mgr.save_route(None, svc, Some("shop")).unwrap();
    let svc2 = Route { target: Target::Service { name: "api".into(), port: 80 }, ..route("api.local", 1) };
    assert!(r.mgr.save_route(None, svc2, Some("other")).unwrap_err().contains("different namespaces"));
    assert_eq!(r.mgr.namespace_of(&r.mgr.route("web.local", "/").unwrap()).as_deref(), Some("shop"));
    // a fresh manager over the same folder sees the same table
    let again = load(&r.dir);
    assert_eq!(again.routes.len(), 2);
    assert!(r.mgr.remove_route("nope.local", "/").is_err());
    r.mgr.remove_route("web.local", "/").unwrap();
    assert!(load(&r.dir).service_ns.is_empty(), "the namespace goes with the last route that used it");
    assert!(r.audit.lock().unwrap().iter().any(|a| a.starts_with("domains.route.save store.local/")));
}

// ------------------------------------------------------------------------------------------------------------------ the listener

#[test]
fn host_based_routing_works_end_to_end_through_the_managed_listener_on_a_high_port() {
    let r = rig("e2e");
    let up = Upstream::start();
    r.mgr.save_route(None, route("store.local", up.port), None).unwrap();
    r.mgr.save_route(None, route("other.example.org", up.port), None).unwrap();
    assert!(!r.mgr.status().running, "saving a route does not start anything");
    let (http, https) = (free_port(), free_port());
    on(&r, http, https);
    let st = r.mgr.status();
    assert!(st.running && st.ports == [http, https] && st.bind == "127.0.0.1", "{st:?}");
    assert!(st.line().contains("this device only"));
    // the name, as another computer would send it: one Host header per route
    assert!(get(http, "store.local").contains("upstream saw host=store.local"));
    assert!(get(http, &format!("store.local:{http}")).contains("200 OK"), "a port in the Host header is ignored when routing");
    assert!(get(http, "other.example.org").contains("upstream saw host=other.example.org"));
    assert!(get(http, "nobody.local").starts_with("HTTP/1.1 404"), "an unknown name is a 404, not a guess");
    // the local name is announced while it runs, and only the .local one
    assert_eq!(r.names.last(), ["store.local"]);
    // a route added while it runs is served at once; removing it stops serving it
    r.mgr.save_route(None, route("new.local", up.port), None).unwrap();
    assert!(get(http, "new.local").contains("200 OK"));
    assert_eq!(r.names.last(), ["new.local", "store.local"]);
    r.mgr.remove_route("new.local", "/").unwrap();
    assert!(get(http, "new.local").starts_with("HTTP/1.1 404"));
    assert_eq!(r.names.last(), ["store.local"]);
    // the firewall was never touched
    assert!(r.gate.lock().unwrap().made.is_empty());
    // off: the port is closed and the name is withdrawn
    r.mgr.set_listener(ListenerChange { enabled: Some(false), ..Default::default() }).unwrap();
    assert!(crate::testdir::port_is_closed(http), "the listener is closed");
    assert!(r.names.last().is_empty() && !r.mgr.status().running);
    assert!(r.audit.lock().unwrap().iter().any(|a| a.starts_with("domains.listener.stop")));
}

#[test]
fn the_firewall_opening_is_its_own_step_with_local_scope_an_end_time_and_goes_with_disabling() {
    let r = rig("fw");
    let up = Upstream::start();
    r.mgr.save_route(None, route("store.local", up.port), None).unwrap();
    let (http, https) = (free_port(), free_port());
    // on and open in one call: refused, each step is its own approval
    let e = r.mgr.set_listener(ListenerChange { enabled: Some(true), http_port: Some(http), https_port: Some(https), firewall: Some(true), ..Default::default() }).unwrap_err();
    assert!(e.contains("two steps"), "{e}");
    assert!(!r.mgr.settings().enabled, "nothing was changed");
    // the firewall cannot be opened for a listener that is off
    assert!(r.mgr.set_listener(ListenerChange { firewall: Some(true), ..Default::default() }).is_err());
    on(&r, http, https);
    assert!(r.gate.lock().unwrap().made.is_empty() && r.mgr.status().bind == "127.0.0.1");
    // step two: open to the local network for 2 hours
    r.mgr.set_listener(ListenerChange { firewall: Some(true), firewall_hours: Some(2), ..Default::default() }).unwrap();
    let st = r.mgr.status();
    assert!(st.running && st.firewall && st.bind == "0.0.0.0", "{st:?}");
    assert_eq!(st.firewall_until, NOW + 7200);
    assert_eq!(r.gate.lock().unwrap().made, [(GateReach::LocalNetwork, NOW + 7200)], "local-network scope and the end time reach the firewall");
    assert_eq!(r.gate.lock().unwrap().open, [http], "only the port that has a route is opened");
    assert_eq!(st.firewall_ports, [http]);
    assert!(get(http, "store.local").contains("200 OK"), "it still serves after the restart");
    // the end time passes: the listener is off, the opening is removed, and the inbox is told
    r.clock.store(NOW + 7201, Ordering::SeqCst);
    let done = r.mgr.tick();
    assert!(done.iter().any(|d| d.contains("firewall opening ended")), "{done:?}");
    let st = r.mgr.status();
    assert!(!st.enabled && !st.running && !st.firewall && st.firewall_until == 0);
    assert!(r.gate.lock().unwrap().closed.contains(&http));
    assert!(r.notes.lock().unwrap().iter().any(|n| n.starts_with("Domain names: the firewall opening ended")));
    // turning it on again starts on this device only: the old approval does not come back
    // (new ports: the freed ones may have been taken by another test in this process since, and then the listener would not start at all)
    let (http, https) = (free_port(), free_port());
    on(&r, http, https);
    assert!(r.mgr.status().running && r.mgr.status().bind == "127.0.0.1" && !r.mgr.settings().firewall, "{:?}", r.mgr.status());
    // disabling by hand removes the opening too
    r.mgr.set_listener(ListenerChange { firewall: Some(true), ..Default::default() }).unwrap();
    let mid = r.mgr.status();
    assert!(mid.running && mid.firewall, "the second opening did not start: {mid:?} (ports {http}/{https})");
    let closed_before = r.gate.lock().unwrap().closed.len();
    r.mgr.set_listener(ListenerChange { enabled: Some(false), ..Default::default() }).unwrap();
    {
        let log = r.gate.lock().unwrap();
        assert!(log.closed.len() > closed_before && !r.mgr.settings().firewall, "closed {:?} (before: {closed_before}), made {:?}, open {:?}, status {:?}", log.closed, log.made, log.open, r.mgr.status());
    }
}

#[test]
fn a_firewall_that_is_not_there_keeps_the_listener_off_the_network() {
    let r = rig("fwfail");
    let up = Upstream::start();
    r.mgr.save_route(None, route("store.local", up.port), None).unwrap();
    on(&r, free_port(), free_port());
    r.gate.lock().unwrap().fail = true;
    r.mgr.set_listener(ListenerChange { firewall: Some(true), ..Default::default() }).unwrap();
    let st = r.mgr.status();
    assert!(!st.running, "{st:?}");
    assert!(st.last_errors.iter().any(|e| e.contains("the firewall is not available")), "{:?}", st.last_errors);
}

#[test]
fn ports_follow_the_rules_8080_8443_by_default_80_and_443_only_after_approval_nothing_else_low() {
    let r = rig("ports");
    assert_eq!((r.mgr.settings().http_port, r.mgr.settings().https_port), (8080, 8443));
    let c = |h, s, ok| ListenerChange { http_port: h, https_port: s, standard_ports_approved: ok, ..Default::default() };
    assert!(r.mgr.set_listener(c(Some(80), None, false)).unwrap_err().contains("needs your approval"));
    assert!(r.mgr.set_listener(c(Some(81), None, true)).unwrap_err().contains("below 1024"));
    assert!(r.mgr.set_listener(c(Some(443), None, true)).unwrap_err().contains("below 1024"), "443 is the https port, not the http one");
    assert!(r.mgr.set_listener(c(Some(9000), Some(9000), false)).unwrap_err().contains("must differ"));
    assert!(r.mgr.set_listener(c(Some(7979), None, false)).unwrap_err().contains("used by another service"));
    assert!(r.mgr.set_listener(c(None, Some(0), false)).is_err());
    r.mgr.set_listener(c(Some(80), Some(443), true)).unwrap();
    assert_eq!((r.mgr.settings().http_port, r.mgr.settings().https_port), (80, 443));
    // turning it on later with 80 still saved asks again
    assert!(r.mgr.set_listener(ListenerChange { enabled: Some(true), ..Default::default() }).unwrap_err().contains("needs your approval"));
}

#[test]
fn a_port_that_cannot_be_bound_is_reported_in_words_and_nothing_else_is_used() {
    let r = rig("busy");
    let up = Upstream::start();
    r.mgr.save_route(None, route("store.local", up.port), None).unwrap();
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = taken.local_addr().unwrap().port();
    on(&r, busy, free_port());
    let st = r.mgr.status();
    assert!(st.enabled && !st.running);
    assert!(st.last_errors.iter().any(|e| e.contains(&format!("cannot listen on port {busy}"))), "{:?}", st.last_errors);
    assert!(st.line().starts_with("On, but not listening"));
}

#[test]
fn resume_starts_a_listener_left_on_and_suspend_stops_it() {
    let r = rig("resume");
    let up = Upstream::start();
    r.mgr.save_route(None, route("store.local", up.port), None).unwrap();
    let (http, https) = (free_port(), free_port());
    on(&r, http, https);
    r.mgr.suspend();
    assert!(!r.mgr.status().running && r.mgr.settings().enabled, "signing out stops it but keeps the choice");
    r.mgr.resume();
    assert!(get(http, "store.local").contains("200 OK"));
    r.mgr.suspend();
}

#[test]
fn the_names_file_is_only_believed_by_the_process_that_wrote_it() {
    let dir = tmp("names");
    FileNames(dir.clone()).set(&["store.local".into()]).unwrap();
    assert_eq!(read_names(&dir), ["store.local"]);
    std::fs::write(dir.join(NAMES_FILE), r#"{"pid": 1, "names": ["old.local"]}"#).unwrap();
    assert!(read_names(&dir).is_empty(), "a name left from before a restart is not announced");
    assert_eq!(local_names(&[route("*.x.local", 1), route("a.local", 1), route("b.example.org", 1), route("A.local", 1)]), ["a.local"], "no wildcards, no other domains");
}

// ------------------------------------------------------------------------------------------------------------------ certificates

#[test]
fn certificates_live_in_the_vault_never_in_the_settings_file_and_a_trial_one_serves_a_route() {
    let r = rig("cert");
    let up = Upstream::start();
    let name = r.mgr.make_trial_cert("store.local").unwrap();
    assert_eq!(name, "store-local");
    assert!(r.store.map.lock().unwrap().contains_key("domain.store-local-chain") && r.store.map.lock().unwrap().contains_key("domain.store-local-key"));
    let https = Route { scheme: Scheme::Https, cert: Some(CertRef::Vault(name.clone())), ..route("store.local", up.port) };
    r.mgr.save_route(None, https, None).unwrap();
    let file = std::fs::read_to_string(r.dir.join(FILE)).unwrap();
    assert!(!file.contains("BEGIN") && !file.contains("PRIVATE") && file.contains("store-local"), "only the NAME is in the file: {file}");
    let rows = r.mgr.certs().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].names.contains(&"store.local".to_string()) && rows[0].used_by == ["store.local"] && rows[0].days > 365, "{rows:?}");
    assert!(rows[0].line().contains("(in use)"));
    // the listener serves it over TLS (the certificate is read from the vault into memory)
    let (http, tls) = (free_port(), free_port());
    on(&r, http, tls);
    let st = r.mgr.status();
    assert!(st.running && st.refused.is_empty(), "{st:?}");
    assert!(TcpStream::connect(("127.0.0.1", tls)).is_ok());
    // a certificate in use cannot be deleted; an unused one can
    assert!(r.mgr.delete_cert("store-local").unwrap_err().contains("used by the route store.local"));
    r.mgr.make_trial_cert("spare.local").unwrap();
    r.mgr.delete_cert("spare-local").unwrap();
    assert!(!r.store.map.lock().unwrap().keys().any(|k| k.contains("spare")));
    assert_eq!(cert_name_for("*.Apps.example.org"), "wild-apps-example-org");
}

#[test]
fn an_imported_certificate_goes_through_the_engines_check_first() {
    let r = rig("import");
    let (chain, key) = crate::cloudroute::self_signed("store.local").unwrap();
    let e = r.mgr.import_cert("mine", "other.local", &chain, &key).unwrap_err();
    assert!(e.contains("covers store.local and not other.local"), "{e}");
    let (_, other_key) = crate::cloudroute::self_signed("store.local").unwrap();
    assert!(r.mgr.import_cert("mine", "store.local", &chain, &other_key).unwrap_err().contains("does not belong"));
    assert!(r.mgr.import_cert("Bad Name", "store.local", &chain, &key).unwrap_err().contains("not a certificate name"));
    assert!(r.store.map.lock().unwrap().is_empty(), "a refused certificate leaves nothing in the vault");
    let line = r.mgr.import_cert("mine", "store.local", &chain, &key).unwrap();
    assert!(line.starts_with("valid for"), "{line}");
}

#[test]
fn a_certificate_close_to_its_end_or_past_it_warns_in_the_inbox_once_the_checks_run() {
    let r = rig("expiry");
    let name = r.mgr.make_trial_cert("store.local").unwrap();
    r.mgr.save_route(None, Route { scheme: Scheme::Https, cert: Some(CertRef::Vault(name)), ..route("store.local", 1) }, None).unwrap();
    assert!(r.mgr.check_certs().is_empty(), "far from the end: no warning");
    let end = {
        let rows = r.mgr.certs().unwrap();
        NOW + rows[0].days * 86400
    };
    r.clock.store(end - 10 * 86400, Ordering::SeqCst);
    let w = r.mgr.check_certs();
    assert!(w.len() == 1 && w[0].contains("runs out in"), "{w:?}");
    assert!(r.notes.lock().unwrap().iter().any(|n| n.starts_with("Certificate for store.local needs attention")));
    r.clock.store(end + 86400 * 2, Ordering::SeqCst);
    assert!(r.mgr.check_certs()[0].contains("expired"));
    // tick does it by itself, not more often than every six hours
    r.clock.store(end - 29 * 86400, Ordering::SeqCst);
    let n = r.notes.lock().unwrap().len();
    assert!(!r.mgr.tick().is_empty() && r.notes.lock().unwrap().len() == n + 1);
    assert!(r.mgr.tick().is_empty(), "the next check is hours away");
    // a missing vault entry is a problem, not a silent pass
    r.store.map.lock().unwrap().clear();
    r.clock.store(end - 29 * 86400 + 7 * 3600, Ordering::SeqCst);
    assert!(r.mgr.tick().iter().any(|d| d.contains("store.local")));
}

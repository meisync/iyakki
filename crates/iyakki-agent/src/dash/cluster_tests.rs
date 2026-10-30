//! Cluster addresses, with a fake kubectl and a fake upstream: no cluster, no network.

use super::*;
use crate::dash::connector::fake::*;
use crate::dash::connector::{Cancel, ErrKind, Http};
use crate::dash::registry;
use crate::dash::tools::Env;
use crate::wsexec::FakeRunner;
use iyakki_sec::policy::Role;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Scripted kubectl: the first rule whose text is in the joined arguments answers.
struct FakeKube {
    rules: Mutex<Vec<(String, Result<String, String>)>>,
    calls: AtomicUsize,
}

impl FakeKube {
    fn new(rules: Vec<(&str, Result<String, String>)>) -> Arc<FakeKube> {
        Arc::new(FakeKube { rules: Mutex::new(rules.into_iter().map(|(a, b)| (a.to_string(), b)).collect()), calls: AtomicUsize::new(0) })
    }
    fn set(&self, key: &str, v: Result<String, String>) {
        let mut r = self.rules.lock().unwrap();
        r.retain(|x| x.0 != key);
        r.insert(0, (key.into(), v));
    }
}

impl Kubectl for FakeKube {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let line = args.join(" ");
        self.rules.lock().unwrap().iter().find(|r| line.contains(&r.0)).map(|r| r.1.clone()).unwrap_or_else(|| Err(format!("Error from server (NotFound): not found: {line}")))
    }
}

fn svc_json(ns: &str, name: &str, ip: &str, ports: &[u16]) -> String {
    let ports: Vec<String> = ports.iter().map(|p| format!(r#"{{"name":"http","port":{p},"targetPort":{p}}}"#)).collect();
    format!(r#"{{"metadata":{{"name":"{name}","namespace":"{ns}","labels":{{"app":"{name}"}}}},"spec":{{"type":"ClusterIP","clusterIP":"{ip}","ports":[{}]}}}}"#, ports.join(","))
}

fn eps_json(ready: bool) -> String {
    if ready { r#"{"subsets":[{"addresses":[{"ip":"10.42.0.5"}]}]}"#.into() } else { r#"{"subsets":[{"notReadyAddresses":[{"ip":"10.42.0.5"}]}]}"#.into() }
}

fn es_kube(ip: &str, ready: bool) -> Arc<FakeKube> {
    FakeKube::new(vec![("get svc elasticsearch", Ok(svc_json("iyakki-data", "elasticsearch", ip, &[9200]))), ("get endpoints elasticsearch", Ok(eps_json(ready)))])
}

fn cluster(k: &Arc<FakeKube>, ttl: Duration) -> Arc<KubeCluster> {
    Arc::new(KubeCluster::new(k.clone(), ttl))
}

fn r(url: &str) -> ClusterRef {
    parse(url).unwrap().unwrap()
}

#[test]
fn both_spellings_parse_and_everything_else_is_left_alone() {
    let a = r("cluster://iyakki-data/elasticsearch:9200");
    assert_eq!((a.ns.as_str(), a.svc.as_str(), a.port, a.https), ("iyakki-data", "elasticsearch", Some(9200), false));
    assert_eq!(a.policy_host(), "elasticsearch.iyakki-data.svc");
    assert_eq!(r("http://elasticsearch.iyakki-data.svc:9200"), a);
    assert_eq!(r("http://elasticsearch.iyakki-data.svc.cluster.local:9200/"), ClusterRef { tail: "/".into(), ..a.clone() });
    assert!(r("https://prometheus.iyakki-monitoring.svc:9090").https && r("cluster+https://m/p:1").https);
    assert_eq!(r("cluster://iyakki-monitoring/prometheus:9090/prometheus").tail, "/prometheus");
    assert_eq!(r("cluster://ns/svc").port, None);
    for plain in ["http://localhost:9200", "http://10.43.1.2:9090", "https://es.example.com", "http://svc.example.svc.evil.com", "file:///x", "garbage"] {
        assert!(parse(plain).is_none(), "{plain}");
    }
    for bad in ["cluster://ns", "cluster://ns/svc:abc", "cluster://Ns/svc:1", "cluster://ns/-svc:1", "cluster://u@ns/svc:1", "http://a.b.c.svc:1", "http://only.svc:1", "cluster://ns/svc:0"] {
        assert!(matches!(parse(bad), Some(Err(_))), "{bad}");
    }
    // the form accepts the new spellings and still refuses odd ones
    let info = registry::kind("elasticsearch").unwrap();
    assert!(validate_url("cluster://iyakki-data/elasticsearch:9200", &info).is_empty());
    assert!(!validate_url("cluster://nope", &info).is_empty());
    assert!(!validate_url("ftp://x", &info).is_empty());
}

fn validate_url(url: &str, info: &crate::dash::connector::KindInfo) -> Vec<String> {
    crate::dash::connector::validate(&cfg("elasticsearch", url), info)
}

#[test]
fn a_service_is_resolved_to_its_current_cluster_ip_and_port() {
    let k = es_kube("10.43.7.9", true);
    let c = cluster(&k, Duration::ZERO);
    assert_eq!(resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9200")).unwrap(), "http://10.43.7.9:9200");
    assert_eq!(resolve(c.as_ref(), &r("http://elasticsearch.iyakki-data.svc:9200/x/")).unwrap(), "http://10.43.7.9:9200/x");
    // the port may be left out when the Service has one
    assert_eq!(resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch")).unwrap(), "http://10.43.7.9:9200");
    let e = resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9300")).unwrap_err();
    assert!(e.message.contains("no port 9300") && e.message.contains("9200"), "{e}");
}

#[test]
fn a_stale_cluster_ip_is_looked_up_again_and_a_short_memory_saves_calls() {
    let k = es_kube("10.43.7.9", true);
    let c = cluster(&k, Duration::ZERO);
    assert_eq!(resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9200")).unwrap(), "http://10.43.7.9:9200");
    k.set("get svc elasticsearch", Ok(svc_json("iyakki-data", "elasticsearch", "10.43.88.1", &[9200])));
    assert_eq!(resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9200")).unwrap(), "http://10.43.88.1:9200", "the Service was recreated: the new IP is used");
    // with a memory, the second call does not ask again, and a failed connect makes it forget
    let k = es_kube("10.43.7.9", true);
    let c = cluster(&k, Duration::from_secs(60));
    resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9200")).unwrap();
    let n = k.calls.load(Ordering::SeqCst);
    resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9200")).unwrap();
    assert_eq!(k.calls.load(Ordering::SeqCst), n);
    c.forget("iyakki-data", "elasticsearch");
    k.set("get svc elasticsearch", Ok(svc_json("iyakki-data", "elasticsearch", "10.43.5.5", &[9200])));
    assert_eq!(resolve(c.as_ref(), &r("cluster://iyakki-data/elasticsearch:9200")).unwrap(), "http://10.43.5.5:9200");
}

#[test]
fn no_cluster_missing_service_not_ready_and_unusable_addresses_say_what_is_wrong() {
    let q = r("cluster://iyakki-data/elasticsearch:9200");
    let e = resolve(&NoCluster, &q).unwrap_err();
    assert_eq!(e.kind, ErrKind::Network);
    assert!(e.message.contains("cluster is not available") && e.fix.unwrap().contains("Install it and start it"));
    let k = FakeKube::new(vec![("get svc", Err("k3s is stopped".into()))]);
    let e = resolve(cluster(&k, Duration::ZERO).as_ref(), &q).unwrap_err();
    assert!(e.message.contains("not available") && e.message.contains("k3s is stopped"), "{e}");
    let k = FakeKube::new(vec![("get svc", Err("Error from server (NotFound): services \"elasticsearch\" not found".into()))]);
    let e = resolve(cluster(&k, Duration::ZERO).as_ref(), &q).unwrap_err();
    assert_eq!(e.kind, ErrKind::Config);
    assert!(e.message.contains("no Service `elasticsearch` in namespace `iyakki-data`") && e.message.contains("kubectl says") && e.fix.unwrap().contains("Quick install"));
    let e = resolve(cluster(&es_kube("10.43.7.9", false), Duration::ZERO).as_ref(), &q).unwrap_err();
    assert!(e.message.contains("no ready pod") && e.fix.unwrap().contains("kubectl -n iyakki-data get pods"));
    // never a headless, loopback, link-local or external-name target
    for ip in ["None", "", "127.0.0.1", "0.0.0.0", "169.254.169.254", "224.0.0.1", "::1", "fe80::1", "not-an-ip"] {
        let e = resolve(cluster(&es_kube(ip, true), Duration::ZERO).as_ref(), &q).unwrap_err();
        assert!(e.message.contains("no usable cluster IP"), "{ip}: {e}");
    }
    let k = FakeKube::new(vec![("get svc elasticsearch", Ok(svc_json("iyakki-data", "elasticsearch", "10.43.1.1", &[9200]).replace("ClusterIP", "ExternalName")))]);
    assert!(resolve(cluster(&k, Duration::ZERO).as_ref(), &q).unwrap_err().message.contains("ExternalName"));
    // an IPv6 cluster IP is bracketed
    assert_eq!(resolve(cluster(&es_kube("fd00::10", true), Duration::ZERO).as_ref(), &q).unwrap(), "http://[fd00::10]:9200");
}

fn deps_with(up: &Arc<FakeUp>, k: &Arc<FakeKube>) -> crate::dash::connector::Deps {
    let mut d = deps(up, &[], Arc::new(FakeRunner::new(&[])));
    d.cluster = cluster(k, Duration::ZERO);
    d
}

#[test]
fn the_connector_talks_to_the_resolved_ip_only() {
    let up = FakeUp::new().on("/", 200, r#"{"cluster_name":"c","version":{"number":"8.14.0"}}"#);
    let k = es_kube("10.43.7.9", true);
    let d = deps_with(&up, &k);
    let s = registry::build(&cfg("elasticsearch", "cluster://iyakki-data/elasticsearch:9200"), &d).unwrap();
    s.test(&Cancel::new()).unwrap();
    assert!(up.urls().iter().all(|u| u.contains("http://10.43.7.9:9200/")), "{:?}", up.urls());
    // the Service moves: the next connector uses the new IP, the old one is not contacted
    k.set("get svc elasticsearch", Ok(svc_json("iyakki-data", "elasticsearch", "10.43.1.1", &[9200])));
    registry::build(&cfg("elasticsearch", "http://elasticsearch.iyakki-data.svc:9200"), &d).unwrap().test(&Cancel::new()).unwrap();
    assert!(up.urls().last().unwrap().contains("http://10.43.1.1:9200/"));
    // no cluster: a plain error, nothing was sent
    let up2 = FakeUp::new();
    let d2 = deps(&up2, &[], Arc::new(FakeRunner::new(&[])));
    let e = registry::build(&cfg("elasticsearch", "cluster://iyakki-data/elasticsearch:9200"), &d2).err().unwrap();
    assert!(e.message.contains("cluster is not available"), "{e}");
    assert!(up2.urls().is_empty());
}

fn env(tag: &str, up: &Arc<FakeUp>, k: &Arc<FakeKube>, hosts: &[&str]) -> Env {
    let dir = std::env::temp_dir().join(format!("iyakki-dashcluster-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut policy = Role::Admin.template();
    policy.net_hosts = hosts.iter().map(|h| h.to_string()).collect();
    Env { user: "amy".into(), policy, data_dir: dir.clone(), home: dir.join("home"), deps: deps_with(up, k), now_ms: Arc::new(|| 0), vault_names: Arc::new(Vec::new), vault_set: Arc::new(|_, _| Ok(())), audit: Arc::new(|_, _| {}), cache: crate::dash::multi::FetchCache::new(), people: Arc::new(crate::dash::tools::AnyPeople) }
}

#[test]
fn the_policy_sees_the_service_name_and_nothing_else_is_widened() {
    let up = FakeUp::new().on("/", 200, r#"{"cluster_name":"c","version":{"number":"8.14.0"}}"#);
    let k = es_kube("10.43.7.9", true);
    let save = |e: &Env, id: &str, url: &str| {
        let mut c = cfg("elasticsearch", url);
        c.id = id.into();
        c.name = id.into();
        e.sources().save(c).unwrap();
    };
    // allowed by the NAME
    let e = env("name", &up, &k, &["elasticsearch.iyakki-data.svc"]);
    save(&e, "cl", "cluster://iyakki-data/elasticsearch:9200");
    save(&e, "svc", "http://elasticsearch.iyakki-data.svc:9200");
    assert!(e.source("cl").is_ok() && e.source("svc").is_ok());
    // allowing only the resolved IP does not allow the Service, and the name does not allow the IP typed by hand
    let e = env("ip", &up, &k, &["10.43.7.9"]);
    save(&e, "cl", "cluster://iyakki-data/elasticsearch:9200");
    save(&e, "raw", "http://10.43.7.9:9200");
    save(&e, "other", "http://10.43.99.99:9200");
    assert!(e.source("cl").err().unwrap().message.contains("elasticsearch.iyakki-data.svc"));
    assert!(e.source("raw").is_ok());
    assert!(e.source("other").is_err(), "an arbitrary private IP is not newly allowed");
    let e = env("name2", &up, &k, &["elasticsearch.iyakki-data.svc"]);
    save(&e, "raw", "http://10.43.7.9:9200");
    save(&e, "loop", "http://127.0.0.1:9200");
    save(&e, "cl2", "cluster://other-ns/elasticsearch:9200");
    for id in ["raw", "loop", "cl2"] {
        assert!(e.source(id).is_err(), "{id}");
    }
    // a Service in a namespace the policy does not name is refused before kubectl is asked
    let before = k.calls.load(Ordering::SeqCst);
    let _ = e.source("cl2");
    assert_eq!(k.calls.load(Ordering::SeqCst), before);
    // nothing but the resolved IP is contacted, and a redirect answer is just an error (the client follows none)
    let e = env("wild", &up, &k, &["*"]);
    save(&e, "cl", "cluster://iyakki-data/elasticsearch:9200");
    let (_, s) = e.source("cl").unwrap();
    up.rules.lock().unwrap().insert(0, ("/".into(), 302, b"".to_vec()));
    assert!(s.test(&Cancel::new()).is_err());
    assert!(up.urls().iter().all(|u| u.contains("10.43.7.9") || u.contains("10.43.1.1")), "{:?}", up.urls());
}

fn net_err(url: &str, transport_error: &str, k: &Arc<FakeKube>) -> crate::dash::connector::DashError {
    let up = FakeUp::new();
    *up.fail.lock().unwrap() = Some(transport_error.into());
    let h = Http::from_config(&cfg("elasticsearch", url), &deps_with(&up, k), "Elasticsearch “x”").unwrap();
    h.send("GET", "/", &[], &[], &Cancel::new()).unwrap_err()
}

#[test]
fn every_failure_mode_is_named_in_plain_words_with_a_fix() {
    let none = FakeKube::new(vec![("get svc -A", Err("k3s is stopped".into()))]);
    // localhost with nothing listening: says where Quick install put it
    for (url, must) in [("http://localhost:9200", "cluster://iyakki-data/elasticsearch:9200"), ("http://127.0.0.1:9090", "cluster://iyakki-monitoring/prometheus:9090"), ("http://localhost:8123", "ClickHouse")] {
        let e = net_err(url, "Connection refused (os error 111)", &none);
        assert!(e.message.contains("nothing listens on this machine at port"), "{e}");
        let f = e.fix.clone().unwrap();
        assert!(f.contains(must) && f.contains("INSIDE the cluster") || must == "ClickHouse", "{f}");
    }
    assert!(net_err("http://localhost:9200", "Connection refused", &none).fix.unwrap().contains("'Cluster Elasticsearch'"));
    // with a cluster that has it, the real address is named
    let all = FakeKube::new(vec![("get svc -A", Ok(format!(r#"{{"items":[{}]}}"#, svc_json("team-a", "elasticsearch-master", "10.43.2.2", &[9200])))), ("get endpoints -A", Ok(r#"{"items":[]}"#.into()))]);
    let f = net_err("http://localhost:9200", "Connection refused", &all).fix.unwrap();
    assert!(f.contains("cluster://team-a/elasticsearch-master:9200"), "{f}");
    // a .svc name that reached the network stack (through a proxy, say) cannot be found: say how to write it
    let e = net_err("http://es.example.svc.evil.test:9200", "dns error: failed to lookup address information: Name or service not known", &none);
    assert!(e.fix.is_some());
    let e = net_err("http://elasticsearch:9200", "dns error: failed to lookup address information", &none);
    assert!(e.fix.unwrap().contains("Names ending in .svc only work inside the cluster") && e.message.contains("could not be found"));
    // an ordinary dead host, a timeout, TLS, refused on a cluster service
    assert!(net_err("http://db.example:9200", "Connection refused", &none).fix.unwrap().contains("Nothing is listening"));
    assert!(net_err("http://db.example:9200", "timed out reading response", &none).message.contains("did not answer in time"));
    assert!(net_err("https://db.example:9200", "invalid peer certificate: UnknownIssuer", &none).message.contains("TLS"));
    let k = es_kube("10.43.7.9", true);
    let e = net_err("cluster://iyakki-data/elasticsearch:9200", "Connection refused", &k);
    assert!(e.message.contains("connection refused") && e.fix.unwrap().contains("kubectl -n iyakki-data get pods"));
    // auth: a 401 from the server
    let up = FakeUp::new().on("/", 401, "{}");
    let h = Http::from_config(&cfg("elasticsearch", "cluster://iyakki-data/elasticsearch:9200"), &deps_with(&up, &k), "ES").unwrap();
    assert_eq!(h.send("GET", "/", &[], &[], &Cancel::new()).unwrap_err().kind, ErrKind::Auth);
}

fn fake_api(listening: &[u16], list: Result<Vec<SvcInfo>, DashError>) -> impl ClusterApi {
    struct A(Vec<u16>, Result<Vec<SvcInfo>, DashError>);
    impl ClusterApi for A {
        fn service(&self, _: &str, _: &str) -> Result<SvcInfo, DashError> {
            Err(no_cluster("fake"))
        }
        fn services(&self) -> Result<Vec<SvcInfo>, DashError> {
            self.1.clone()
        }
        fn host_listens(&self, p: u16) -> bool {
            self.0.contains(&p)
        }
    }
    A(listening.to_vec(), list)
}

fn svc(ns: &str, name: &str, port: u16, ready: Option<bool>) -> SvcInfo {
    SvcInfo { ns: ns.into(), name: name.into(), kind: "ClusterIP".into(), cluster_ip: "10.43.0.9".into(), ports: vec![("http".into(), port)], app: name.into(), ready }
}

#[test]
fn the_form_suggests_what_is_really_reachable_never_a_guess() {
    // in the cluster: the stable address, even though nothing listens on localhost
    let api = fake_api(&[], Ok(vec![svc("iyakki-data", "elasticsearch", 9200, Some(true)), svc("iyakki-monitoring", "prometheus", 9090, Some(true)), svc("kube-system", "kube-dns", 53, None)]));
    assert_eq!(suggest(&api, "elasticsearch").unwrap().url, "cluster://iyakki-data/elasticsearch:9200");
    assert_eq!(suggest(&api, "prometheus").unwrap().url, "cluster://iyakki-monitoring/prometheus:9090");
    // a process on this machine (and no Service): localhost
    let api = fake_api(&[8123], Ok(vec![]));
    let s = suggest(&api, "clickhouse").unwrap();
    assert_eq!(s.url, "http://localhost:8123");
    assert!(s.note.contains("listens on this machine"));
    // neither: nothing is filled in, and the note says why and what to do
    let s = suggest(&api, "elasticsearch").unwrap();
    assert_eq!(s.url, "");
    assert!(s.note.contains("no Elasticsearch found in the cluster") && s.note.contains("cluster://<namespace>/<service>:9200"), "{}", s.note);
    let s = suggest(&fake_api(&[], Err(no_cluster("k3s is stopped"))), "prometheus").unwrap();
    assert_eq!(s.url, "");
    assert!(s.note.contains("k3s is stopped"));
    // in the cluster but not ready yet: still offered, with the warning
    let api = fake_api(&[], Ok(vec![svc("iyakki-data", "elasticsearch", 9200, Some(false))]));
    let s = suggest(&api, "elasticsearch").unwrap();
    assert!(s.url.starts_with("cluster://") && s.note.contains("no pod is ready"));
    // the cluster wins over a stray listener, a ready one over a not-ready one, IYAKKI's namespace first
    let api = fake_api(&[9200], Ok(vec![svc("zzz", "elasticsearch", 9200, Some(true)), svc("iyakki-data", "elasticsearch", 9200, Some(true))]));
    assert_eq!(suggest(&api, "elasticsearch").unwrap().url, "cluster://iyakki-data/elasticsearch:9200");
    assert!(suggest(&api, "duckdb").is_none());
}

#[test]
fn candidates_are_found_by_name_label_and_port_and_exporters_are_not_mistaken() {
    let mut exp = svc("iyakki-monitoring", "prometheus-node-exporter", 9100, None);
    exp.ports.push(("m".into(), 9090));
    let mut ch = svc("db", "chi-analytics", 8123, Some(true));
    ch.app = "clickhouse".into();
    let list = vec![exp, svc("m", "alertmanager-prometheus", 9090, None), svc("a", "opensearch", 9200, Some(true)), ch, SvcInfo { cluster_ip: "None".into(), ..svc("h", "elasticsearch", 9200, None) }, svc("x", "web", 80, None)];
    let c = candidates(&list);
    assert_eq!(c.iter().map(|c| (c.kind, c.name.as_str())).collect::<Vec<_>>(), [("elasticsearch", "opensearch"), ("clickhouse", "chi-analytics")]);
}

#[test]
fn the_old_quick_install_source_with_a_baked_in_ip_is_migrated_once() {
    let dir = std::env::temp_dir().join(format!("iyakki-dashmig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dash")).unwrap();
    let st = crate::dash::store::Sources::open(&dir, "amy");
    let mut old = crate::quickinstall::prometheus_source();
    old.values.insert("url".into(), "http://10.43.7.9:9090".into());
    st.save(old).unwrap();
    let mut mine = cfg("prometheus", "http://10.43.7.9:9090");
    mine.id = "mine".into();
    st.save(mine).unwrap();
    assert!(migrate_sources(&st));
    assert_eq!(st.get("prometheus-cluster").unwrap().value("url"), "cluster://iyakki-monitoring/prometheus:9090");
    assert_eq!(st.get("mine").unwrap().value("url"), "http://10.43.7.9:9090", "other sources are never touched");
    assert!(!migrate_sources(&st), "idempotent");
    // the installer saves the same ids again without duplicating
    st.save(crate::quickinstall::prometheus_source()).unwrap();
    st.save(crate::quickinstall::elasticsearch_source()).unwrap();
    st.save(crate::quickinstall::elasticsearch_source()).unwrap();
    assert_eq!(st.list().len(), 3);
    assert_eq!(st.get("elasticsearch-cluster").unwrap().name, "Cluster Elasticsearch");
}

fn svc_list_kube() -> Arc<FakeKube> {
    FakeKube::new(vec![("get svc -A", Ok(format!(r#"{{"items":[{},{}]}}"#, svc_json("iyakki-monitoring", "prometheus", "10.43.4.4", &[9090]), svc_json("iyakki-data", "elasticsearch", "10.43.7.9", &[9200])))), ("get endpoints -A", Ok(r#"{"items":[]}"#.into()))])
}

fn local_prom(e: &Env, url: &str) {
    let mut c = cfg("prometheus", url);
    c.id = "prom".into();
    c.name = "Prometheus".into();
    c.values.insert("auth".into(), "bearer".into());
    c.values.insert("timeout_s".into(), "12".into());
    c.secrets.insert("token".into(), "uvault:svc.prom".into());
    e.sources().save(c).unwrap();
}

#[test]
fn use_cluster_address_rewrites_only_the_address_once_audited_and_only_for_an_administrator() {
    let up = FakeUp::new();
    let k = svc_list_kube();
    let log = Arc::new(Mutex::new(vec![]));
    let mut e = env("repair", &up, &k, &["*"]);
    let l2 = log.clone();
    e.audit = Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}")));
    local_prom(&e, "http://localhost:9090/prometheus");
    let before = e.sources().get("prom").unwrap();
    // nothing happens by itself: suggestion only reads
    assert_eq!(repair_url(&before, e.deps.cluster.as_ref()).unwrap(), "cluster://iyakki-monitoring/prometheus:9090/prometheus");
    assert_eq!(e.sources().get("prom").unwrap(), before);
    assert!(log.lock().unwrap().is_empty());
    // the click
    assert_eq!(e.use_cluster_address("prom").unwrap(), "cluster://iyakki-monitoring/prometheus:9090/prometheus");
    let after = e.sources().get("prom").unwrap();
    assert_eq!(after.value("url"), "cluster://iyakki-monitoring/prometheus:9090/prometheus");
    let mut expect = before.clone();
    expect.values.insert("url".into(), after.value("url").into());
    assert_eq!(after, expect, "name, sign-in, timeout and secrets are kept");
    assert_eq!(log.lock().unwrap().as_slice(), ["dash.source_repaired prom -> cluster://iyakki-monitoring/prometheus:9090/prometheus"]);
    // once: a source that no longer points at localhost is not touched again
    assert!(e.use_cluster_address("prom").unwrap_err().contains("does not point"));
    assert_eq!(log.lock().unwrap().len(), 1);
    // a non-administrator is refused and nothing changes
    let mut u = env("repair-user", &up, &k, &["*"]);
    u.policy = Role::User.template();
    local_prom(&u, "http://localhost:9090");
    assert!(u.use_cluster_address("prom").unwrap_err().contains("administrator"));
    assert_eq!(u.sources().get("prom").unwrap().value("url"), "http://localhost:9090");
    // a policy that does not name the Service refuses too; an unknown source and a kind with nothing in the cluster say so
    let mut p = env("repair-policy", &up, &k, &["other.example"]);
    p.policy.manage_settings = true;
    local_prom(&p, "http://127.0.0.1:9090");
    assert!(p.use_cluster_address("prom").unwrap_err().contains("prometheus.iyakki-monitoring.svc"));
    assert!(e.use_cluster_address("nope").unwrap_err().contains("no source"));
    let none = FakeKube::new(vec![("get svc -A", Ok(r#"{"items":[]}"#.into()))]);
    let e2 = env("repair-none", &up, &none, &["*"]);
    local_prom(&e2, "http://localhost:9090");
    assert!(e2.use_cluster_address("prom").unwrap_err().contains("no Prometheus Service was found"));
    assert_eq!(e2.sources().get("prom").unwrap().value("url"), "http://localhost:9090");
    // a remote server is never rewritten
    local_prom(&e2, "http://prom.example:9090");
    assert!(e2.use_cluster_address("prom").is_err());
    assert_eq!(local_target("http://[::1]:9200/x"), Some((9200, "/x".into())));
    assert_eq!(local_target("http://10.0.0.1:9200"), None);
}

const ADMIN_FILE: &str = r#"[
 {"id":"prometheus","kind":"prometheus","name":"Prometheus","values":{"auth":"none","url":"http://localhost:9090"},"secrets":{}},
 {"id":"elasticsearch","kind":"elasticsearch","name":"Elasticsearch","values":{"auth":"none","url":"http://localhost:9200"},"secrets":{}},
 {"id":"clickhouse","kind":"clickhouse","name":"ClickHouse","values":{"auth":"none","url":"http://localhost:8123"},"secrets":{}},
 {"id":"duckdb","kind":"duckdb","name":"DuckDB","values":{"files":"/home/admin/data.csv"},"secrets":{}}
]"#;
const SYSTEM_FILE: &str = r#"[
 {"id":"prometheus-cluster","kind":"prometheus","name":"Cluster Prometheus","values":{"url":"http://10.43.229.135:9090"},"secrets":{}},
 {"id":"graphdb-cluster","kind":"neo4j","name":"Cluster graph database","values":{"auth":"basic","url":"cluster://iyakki-data/graphdb:7474","username":"neo4j"},"secrets":{"password":"vault:svc.graphdb"}}
]"#;

fn mac_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("iyakki-dashmac-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dash")).unwrap();
    std::fs::write(dir.join("dash/sources-admin.json"), ADMIN_FILE).unwrap();
    std::fs::write(dir.join("dash/sources-system.json"), SYSTEM_FILE).unwrap();
    dir
}

#[test]
fn the_owners_files_are_migrated_the_shared_sources_are_seen_and_the_plans_are_right() {
    let dir = mac_dir("files");
    let admin = crate::dash::store::Sources::open(&dir, "admin");
    let ids: Vec<String> = admin.list().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, ["prometheus", "elasticsearch", "clickhouse", "duckdb", "prometheus-cluster", "graphdb-cluster"], "the account sees the shared sources too");
    assert!(admin.is_shared("prometheus-cluster") && !admin.is_shared("prometheus"));
    // opening DASH as the account migrates the shared file, not the account's own sources, and copies nothing into the account's file
    assert!(migrate_all(&dir, "admin"));
    let sys = crate::dash::store::Sources::open(&dir, "system");
    assert_eq!(sys.get("prometheus-cluster").unwrap().value("url"), "cluster://iyakki-monitoring/prometheus:9090");
    assert_eq!(sys.get("graphdb-cluster").unwrap().value("url"), "cluster://iyakki-data/graphdb:7474");
    assert_eq!(admin.own().len(), 4);
    assert_eq!(admin.get("prometheus").unwrap().value("url"), "http://localhost:9090", "never rewritten without the click");
    assert!(!migrate_all(&dir, "admin"), "idempotent");
    // the plans for the four sources of the owner's Mac: Prometheus is in the cluster, Elasticsearch and ClickHouse are not
    let api = fake_api(&[], Ok(vec![svc("iyakki-monitoring", "prometheus", 9090, Some(true)), svc("iyakki-data", "graphdb", 7474, Some(true))]));
    let plan = |id: &str| repair_plan(&admin.get(id).unwrap(), &api);
    assert_eq!(plan("prometheus"), RepairPlan::UseCluster("cluster://iyakki-monitoring/prometheus:9090".into()));
    assert_eq!(plan("elasticsearch"), RepairPlan::NotInstalled { label: "Elasticsearch", item: Some("elasticsearch") });
    assert_eq!(plan("clickhouse"), RepairPlan::NotInstalled { label: "ClickHouse", item: Some("clickhouse") });
    assert_eq!(plan("duckdb"), RepairPlan::NotLocal);
    assert_eq!(plan("prometheus-cluster"), RepairPlan::NotLocal);
    // once ClickHouse is installed (the Service the Quick install item makes), the hand-added localhost source offers the cluster address
    let with_ch = fake_api(&[], Ok(vec![svc("iyakki-data", "clickhouse", 8123, Some(true))]));
    assert_eq!(repair_plan(&admin.get("clickhouse").unwrap(), &with_ch), RepairPlan::UseCluster("cluster://iyakki-data/clickhouse:8123".into()));
    // no cluster at all: the service is plainly not installed (Install brings k3s along); a stopped cluster is still "could not be asked"
    let none = fake_api(&[], Err(no_cluster("k3s is not installed by IYAKKI on this machine")));
    assert_eq!(repair_plan(&admin.get("clickhouse").unwrap(), &none), RepairPlan::NotInstalled { label: "ClickHouse", item: Some("clickhouse") });
    // something really listens on the port: nothing to say about installing
    let listening = fake_api(&[8123], Ok(vec![]));
    assert_eq!(repair_plan(&admin.get("clickhouse").unwrap(), &listening), RepairPlan::NotLocal);
    let down = fake_api(&[], Err(no_cluster("k3s is stopped")));
    assert!(matches!(repair_plan(&admin.get("prometheus").unwrap(), &down), RepairPlan::NoCluster(m) if m.contains("k3s is stopped")));
    // a shared source can be replaced by the account (its own copy wins) and removed again
    let mut mine = admin.get("prometheus-cluster").unwrap();
    mine.name = "Mine".into();
    admin.save(mine).unwrap();
    assert_eq!(admin.get("prometheus-cluster").unwrap().name, "Mine");
    assert_eq!(admin.list().len(), 6);
}

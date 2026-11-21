//! The app store over the core socket (F-SPLIT-03, D-SPLIT-03-STORE): `StoreApi` and its supertrait `StorePublish` served over a real loopback
//! socket against the reference server. What the tests pin: the core's own fetch and audit stand in for the UI's, the person is read again
//! from the core's accounts (a claim of the UI is ignored, an unknown name is refused), a folder the UI names is judged by the file rules, a
//! signing key never crosses (the UI holds a marker that names it), the long call is a job with every line in order, and a lost core answers
//! the quiet value. Real transport, fakes, no timing.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::{StoreApi, StorePublish};
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::FileScope;
use iyakki_sec::policy::Policy;
use iyakki_shared::store_catalog::{Fetch, Loaded, SourceCfg};
use iyakki_shared::store_core::{Item, Refresh};
use iyakki_shared::store_install::{Actor, Input, Review};
use iyakki_shared::store_installed::{Entry as IEntry, Status, Type};
use iyakki_shared::store_keys::SigningKey;
use iyakki_shared::store_lifecycle::{Check, Done, Life, RemovePlan, Report};
use iyakki_shared::store_license::Token;
use iyakki_shared::store_publishflow::{Card, Stop};
use iyakki_shared::store_trust::{Level, Verdict};
use iyakki_shared::skills::Scope;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope as WireScope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAny, Rpc};

const REAL_SEED: [u8; 32] = [7; 32];

fn real_key() -> SigningKey {
    SigningKey::from_bytes(&REAL_SEED)
}

fn review(name: &str) -> Review {
    Review {
        kind: "app".into(),
        name: name.into(),
        version: "1".into(),
        title: name.into(),
        meta: Default::default(),
        verdict: Verdict { level: Level::Unsigned, signer: None, publisher: None, label: String::new(), chain: Vec::new(), warnings: Vec::new(), refusal: None },
        steps: Vec::new(),
        permissions: Vec::new(),
        core_requests: Vec::new(),
        core_lines: Vec::new(),
        members: Vec::new(),
        needs_newer_iyakki: false,
        warnings: Vec::new(),
        scope: Scope::Shared,
        refusal: None,
        locked: false,
        installed: None,
        preview: Vec::new(),
        sha256: String::new(),
        size: 0,
        member_install: Vec::new(),
        input: Input { bytes: Vec::new(), entry: None, source: String::new(), builtin: false, catalog_expired: false, known: Vec::new(), rollback: false },
    }
}

fn installed(name: &str, scope: Scope) -> IEntry {
    IEntry {
        key: format!("app/{name}"),
        ty: Type::App,
        kind: "app".into(),
        name: name.into(),
        title: name.into(),
        version: "1".into(),
        level: Level::Unsigned,
        trust_label: String::new(),
        status: Status::On,
        size: 0,
        installed_at: 0,
        scope,
        publisher: String::new(),
        license: String::new(),
        source: String::new(),
        summary: String::new(),
        target: None,
        ext_kind: None,
        open: None,
        permissions: Vec::new(),
        files: Vec::new(),
        categories: Vec::new(),
        removable: true,
        can_toggle: true,
    }
}

fn life(name: &str, scope: Scope) -> Life {
    Life {
        entry: installed(name, scope),
        id: format!("app:{name}"),
        source: String::new(),
        source_name: String::new(),
        latest: None,
        check: Check::UpToDate,
        changelog: String::new(),
        added_permissions: Vec::new(),
        pinned: None,
        previous: None,
        history: Vec::new(),
        builtin: false,
        from_other: None,
        cat_source: String::new(),
    }
}

/// A store that notes who asked and with what the core supplied, and keeps one real signing key.
#[derive(Default)]
struct FakeStore {
    seen: Mutex<Vec<String>>,
}
impl FakeStore {
    fn note(&self, s: String) {
        self.seen.lock().unwrap().push(s);
    }
}
impl StorePublish for FakeStore {
    fn pf_yank_note(&self) -> &'static str {
        "yanking does not remove copies"
    }
    fn pf_card(&self, user: &str, dir: &Path, _src: &SourceCfg, key: &(SigningKey, String), fetch: &dyn Fetch) -> Result<(Card, String), String> {
        let page = fetch.get("https://store.test/info", 100).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        self.note(format!("pf_card {user} {} key={} real={} page={page}", dir.display(), key.1, key.0.to_bytes() == REAL_SEED));
        Err("stop here".into())
    }
    fn pf_publish(&self, user: &str, _dir: &Path, _src: &SourceCfg, key: Option<&(SigningKey, String)>, _fetch: &dyn Fetch, now: i64) -> Result<iyakki_shared::store_author::Published, Stop> {
        self.note(format!("pf_publish {user} key={:?} real={:?} now={now}", key.map(|k| k.1.clone()), key.map(|k| k.0.to_bytes() == REAL_SEED)));
        Err(Stop::NeedsToken)
    }
    fn keys_load(&self, user: &str, label: Option<&str>) -> Option<(SigningKey, String)> {
        match (user, label) {
            ("ann", None) => Some((real_key(), "main".into())),
            ("ann", Some("main")) => Some((real_key(), "main".into())),
            _ => None,
        }
    }
    fn author_pack_dir(&self, dir: &Path, key: Option<&SigningKey>) -> Result<(String, Vec<String>), String> {
        self.note(format!("author_pack_dir {} signed_with_real={:?}", dir.display(), key.map(|k| k.to_bytes() == REAL_SEED)));
        Ok(("packed".into(), Vec::new()))
    }
}
impl StoreApi for FakeStore {
    fn plan_remove(&self, user: &str, _lives: &[Life], life: &Life) -> RemovePlan {
        self.note(format!("plan_remove {user} {}", life.id));
        RemovePlan { key: life.entry.key.clone(), title: life.entry.title.clone(), ty: Type::App, removes: vec!["files".into()], data: Vec::new(), vault: Vec::new(), vault_note: String::new(), dependents: Vec::new(), cached: 0 }
    }
    fn review_sideload(&self, actor: &Actor, bytes: Vec<u8>, _scope: Scope, _known: &[(String, String)]) -> Review {
        self.note(format!("review_sideload {} admin={} {} bytes", actor.user, actor.admin, bytes.len()));
        review("sideloaded")
    }
    fn items(&self, actor: &Actor, _loaded: &[Loaded]) -> Vec<Item> {
        self.note(format!("items {} admin={} allowed={}", actor.user, actor.admin, actor.allowed));
        Vec::new()
    }
    fn refresh(&self, fetch: &dyn Fetch) -> Refresh {
        let got = fetch.get("https://store.test/catalog.json", 100);
        self.note(format!("refresh fetched={:?}", got.map(|b| String::from_utf8_lossy(&b).into_owned())));
        Refresh::default()
    }
    fn official_store_url(&self) -> Option<&'static str> {
        Some("https://official.test/")
    }
    fn dir(&self) -> &Path {
        Path::new("/core/store")
    }
    fn local_catalogs(&self) -> Vec<Loaded> {
        Vec::new()
    }
    fn add_license(&self, actor: &Actor, text: &str, signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String> {
        self.note(format!("add_license {} {text} signers={:?}", actor.user, signer_of("tool")));
        Err("no token in this test".into())
    }
    fn update_all(&self, _policy: &Policy, user: &str, _loaded: &[Loaded], fetch: &dyn Fetch, audit: &dyn Fn(&str, &str), progress: &mut dyn FnMut(&str)) -> Report {
        self.note(format!("update_all {user} fetched={}", fetch.get("https://store.test/x", 10).is_ok()));
        progress("checking alpha");
        audit("store.update", "alpha 1 -> 2");
        progress("checking beta");
        Report { items: vec![("alpha".into(), Done::Updated("2".into())), ("beta".into(), Done::Skipped("pinned".into()))] }
    }
}

struct Table;
impl Fetch for Table {
    fn get(&self, url: &str, _max: usize) -> Result<Vec<u8>, String> {
        Ok(format!("core-fetched:{}", url.rsplit('/').next().unwrap_or("")).into_bytes())
    }
}

/// The file rules of a test machine: the home folder of an account is `<data>/home/<name>` and nothing else is readable or writable.
struct Rules;
impl FileScope for Rules {}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    data: PathBuf,
    audit: Arc<Mutex<Vec<(String, String)>>>,
}
impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn machine(tag: &str, store: Arc<FakeStore>) -> Machine {
    let data = std::env::temp_dir().join(format!("iyakki-served-store-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(data.join("home/ann/project")).unwrap();
    // the folder rules compare real paths: the data folder is the real one, as on the machine
    let data = std::fs::canonicalize(&data).unwrap();
    let mut users = iyakki_accounts::UserStore::open(&data.join("users.json")).unwrap();
    users.add_local("ann", "pw-ann-1", iyakki_sec::policy::Role::Admin).unwrap();
    users.add_local("guest", "pw-guest-1", iyakki_sec::policy::Role::Guest).unwrap();
    let accounts: Arc<dyn iyakki_agent_api::AccountsApi> = Arc::new(Arc::new(Mutex::new(users)));
    let lines: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let l2 = lines.clone();
    let audit: iyakki_shared::gitkeys::AuditFn = Arc::new(move |event, detail| l2.lock().unwrap().push((event.to_string(), detail.to_string())));
    let host = Host::new().with_root(Obj::StoreApi(store)).with_store_side(Arc::new(Table), audit, accounts, Some(Arc::new(Rules)), data.clone());
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, rpc: Some(Arc::new(host)), appevents: None, ctx: CallContext::remote("ui", Transport::LocalSocket, &[WireScope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread, data, audit: lines }
}

fn remote(m: &Machine) -> Arc<dyn StoreApi> {
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap());
    match rpc.root("StoreApi") {
        Some(RemoteAny::StoreApi(s)) => s,
        _ => panic!("the trait is generated"),
    }
}

fn src_cfg() -> SourceCfg {
    SourceCfg { id: "s".into(), name: "s".into(), location: "https://store.test".into(), enabled: true, ..Default::default() }
}

fn policy() -> Policy {
    Policy::resolve(iyakki_sec::policy::Role::Guest, &Default::default())
}

/// The actor the UI claims: whatever it says, only the name is read.
fn claimed(user: &str, admin: bool) -> Actor {
    Actor { user: user.into(), allowed: true, can_own: true, can_shared: true, admin }
}

#[test]
fn the_person_is_read_again_from_the_cores_accounts_and_what_the_ui_claims_is_ignored() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("actor", fake.clone());
    let store = remote(&m);
    // a guest who says it is an administrator is a guest; an administrator who says it is not one is one
    assert!(store.items(&claimed("guest", true), &[]).is_empty());
    assert!(store.items(&claimed("ann", false), &[]).is_empty());
    // a name the core does not know is refused and the store is never asked
    assert!(store.items(&claimed("mallory", true), &[]).is_empty());
    let r = store.review_sideload(&claimed("mallory", true), vec![1, 2, 3], Scope::Shared, &[]);
    assert!(r.refusal.as_deref().is_some_and(|w| w.contains("not served over the UI socket")), "{:?}", r.refusal);
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].starts_with("items guest admin=false"), "{seen:?}");
    assert!(seen[1].starts_with("items ann admin=true"), "{seen:?}");
    // a call with an actor of a known account arrives with the core's own reading of it
    let r = store.review_sideload(&claimed("guest", true), vec![9; 4], Scope::Shared, &[]);
    assert_eq!(r.name, "sideloaded");
    assert!(fake.seen.lock().unwrap().last().unwrap().starts_with("review_sideload guest admin=false 4 bytes"));
}

#[test]
fn the_core_fetches_with_its_own_fetch_and_the_uis_object_never_arrives() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("fetch", fake.clone());
    let store = remote(&m);
    struct NeverUsed;
    impl Fetch for NeverUsed {
        fn get(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
            panic!("the UI's transport must not be used")
        }
    }
    let _ = store.refresh(&NeverUsed);
    assert_eq!(fake.seen.lock().unwrap().as_slice(), ["refresh fetched=Ok(\"core-fetched:catalog.json\")"]);
}

#[test]
fn the_long_update_is_a_job_every_line_reaches_the_uis_closure_and_the_audit_line_is_the_cores() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("update", fake.clone());
    let store = remote(&m);
    let mut lines = Vec::new();
    let ui_audit = |_: &str, _: &str| panic!("the UI writes no audit line of its own");
    let report = store.update_all(&policy(), "ann", &[], &Table, &ui_audit, &mut |l| lines.push(l.to_string()));
    assert_eq!(lines, ["checking alpha", "checking beta"]);
    assert_eq!(report.items, vec![("alpha".to_string(), Done::Updated("2".into())), ("beta".to_string(), Done::Skipped("pinned".into()))]);
    assert_eq!(fake.seen.lock().unwrap().as_slice(), ["update_all ann fetched=true"]);
    assert_eq!(m.audit.lock().unwrap().as_slice(), [("store.update".to_string(), "alpha 1 -> 2".to_string())]);
    // an account the core does not know runs nothing and the report is empty
    assert!(store.update_all(&policy(), "mallory", &[], &Table, &ui_audit, &mut |_| panic!("no line")).items.is_empty());
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
}

#[test]
fn the_signing_key_never_crosses_the_ui_holds_a_marker_and_the_core_loads_the_real_key_by_label() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("keys", fake.clone());
    let store = remote(&m);
    let (marker, label) = store.keys_load("ann", None).expect("ann has a key");
    assert_eq!(label, "main");
    assert_ne!(marker.to_bytes(), REAL_SEED, "the UI never holds the real key");
    assert_ne!(marker.verifying_key(), real_key().verifying_key());
    assert!(store.keys_load("guest", None).is_none() && store.keys_load("ann", Some("other")).is_none(), "a key the core does not hold is not made up");
    let dir = m.data.join("home/ann/project");
    let src: SourceCfg = src_cfg();
    let err = store.pf_card("ann", &dir, &src, &(marker.clone(), label.clone()), &Table).unwrap_err();
    assert_eq!(err, "stop here");
    assert!(matches!(store.pf_publish("ann", &dir, &src, Some(&(marker.clone(), label.clone())), &Table, 99), Err(Stop::NeedsToken)));
    assert!(matches!(store.pf_publish("ann", &dir, &src, None, &Table, 99), Err(Stop::NeedsToken)));
    assert_eq!(store.author_pack_dir(&dir, Some(&marker)).unwrap().0, "packed");
    // a key that is not one of the markers (one the UI made itself) is never sent: the pack is made unsigned
    assert_eq!(store.author_pack_dir(&dir, Some(&SigningKey::from_bytes(&[1; 32]))).unwrap().0, "packed");
    let seen = fake.seen.lock().unwrap().clone();
    let d = dir.canonicalize().unwrap().display().to_string();
    assert_eq!(seen[0], format!("pf_card ann {d} key=main real=true page=core-fetched:info"), "the core loaded the real key and used its own fetch");
    assert_eq!(seen[1], "pf_publish ann key=Some(\"main\") real=Some(true) now=99");
    assert_eq!(seen[2], "pf_publish ann key=None real=None now=99");
    assert_eq!(seen[3], format!("author_pack_dir {d} signed_with_real=Some(true)"));
    assert_eq!(seen[4], format!("author_pack_dir {d} signed_with_real=None"));
}

#[test]
fn a_folder_the_ui_names_is_judged_by_the_file_rules_and_an_unknown_person_publishes_nothing() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("dir", fake.clone());
    let store = remote(&m);
    let (marker, label) = store.keys_load("ann", None).unwrap();
    let src: SourceCfg = src_cfg();
    // outside every home of the machine: refused before the core opens it
    let outside = std::env::temp_dir();
    let e = store.pf_card("ann", &outside, &src, &(marker.clone(), label.clone()), &Table).unwrap_err();
    assert!(e.contains("outside the folders"), "{e}");
    assert!(store.author_pack_dir(&outside, None).unwrap_err().contains("outside the folders"));
    // a folder that is not there
    assert!(store.author_pack_dir(&m.data.join("home/ann/missing"), None).is_err());
    // a name the core does not know
    assert!(store.pf_card("mallory", &m.data.join("home/ann/project"), &src, &(marker, label), &Table).is_err());
    assert!(fake.seen.lock().unwrap().is_empty(), "nothing reached the store: {:?}", fake.seen.lock().unwrap());
}

#[test]
fn the_signers_of_a_licence_come_from_the_cores_own_catalogs_and_the_texts_of_the_core_are_read_once() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("license", fake.clone());
    let store = remote(&m);
    let e = store.add_license(&claimed("ann", false), "TOKEN", &|_| panic!("the UI's closure cannot cross")).unwrap_err();
    assert_eq!(e, "no token in this test");
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen, ["items ann admin=true allowed=true", "add_license ann TOKEN signers=[]"]);
    assert_eq!(store.official_store_url(), Some("https://official.test/"));
    assert_eq!(store.pf_yank_note(), "yanking does not remove copies");
    assert_eq!(store.dir(), Path::new("/core/store"));
}

#[test]
fn a_removal_plan_and_a_lookup_in_the_lives_the_ui_holds() {
    let fake = Arc::new(FakeStore::default());
    let m = machine("plan", fake.clone());
    let store = remote(&m);
    let lives = vec![life("alpha", Scope::Shared), life("beta", Scope::Shared), life("beta", Scope::User("ann".into()))];
    // find_life needs no call: it is the shared lookup (a user's own item wins over a shared one of the same name)
    assert_eq!(store.find_life(&lives, "alpha").unwrap().id, "app:alpha");
    assert!(matches!(store.find_life(&lives, "beta").unwrap().entry.scope, Scope::User(_)));
    assert!(store.find_life(&lives, "nothing").unwrap_err().contains("Nothing installed is called"));
    let plan = store.plan_remove("ann", &lives, &lives[0]);
    assert_eq!((plan.removes.as_slice(), plan.title.as_str()), (&["files".to_string()][..], "alpha"));
    assert_eq!(fake.seen.lock().unwrap().as_slice(), ["plan_remove ann app:alpha"]);
}

#[test]
fn a_lost_core_answers_the_quiet_value_of_every_kind_and_never_panics() {
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let Some(RemoteAny::StoreApi(store)) = rpc.root("StoreApi") else { panic!("the trait is generated") };
    assert!(store.items(&claimed("ann", true), &[]).is_empty());
    assert!(store.keys_load("ann", None).is_none());
    assert!(store.official_store_url().is_none());
    assert_eq!(store.pf_yank_note(), "");
    let r = store.review_sideload(&claimed("ann", true), Vec::new(), Scope::Shared, &[]);
    assert!(!r.can_install());
    assert!(store.update_all(&policy(), "ann", &[], &Table, &|_, _| {}, &mut |_| {}).items.is_empty());
    assert!(store.add_license(&claimed("ann", true), "T", &|_| Vec::new()).is_err());
    assert!(store.find_life(&[], "x").is_err());
}

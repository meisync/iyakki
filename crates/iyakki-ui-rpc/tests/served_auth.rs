//! The policy and session hole over the UI socket (D-SEC1-VOUCH): a UI that CLAIMS a right in the arguments of a served call gets the right of the
//! real account, or a refusal. Every test sends a forged "the guest is an administrator" claim over a real loopback socket and asserts what the
//! core's own object received or refused. Real transport, fakes, no timing.
//!
//! * `accounts_*`: the five `AccountsApi` calls that take a `Session`, with the guard (`AccountsGuarded`, as the core serves it) AND without it
//!   (the vouch step alone must hold).
//! * `the_core_overwrites_*`: every call of the table of claims (`served_auth::RULES`, mirrored here by what a call looks like) reaches the core's
//!   object with the account's own policy, role, admin flag or administrator list.
//! * the typed fakes (`FlowsFacet`, `StoreHub`, `ClusterFacet`, `SkillApi`): the generated dispatcher decodes what the vouch step wrote.
//! * `the_stories_*`: the story store the UI builds holds the core's policy, not the one the caller names.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::facets_b2::ClusterFacet;
use iyakki_agent_api::facets_s6b::FlowsFacet;
use iyakki_agent_api::facets_stores::SkillApi;
use iyakki_agent_api::store::StoreHub;
use iyakki_shared::store_tilehub::TileEntry;
use iyakki_agent_api::{AccountsApi, AccountsGuarded};
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::FileScope;
use iyakki_sec::policy::{Policy, Role};
use iyakki_shared::skills::{Scope, Skill, SkillError};
use iyakki_shared::store_catalog::Fetch;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope as WireScope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAny, Rpc};
use serde_json::{json, Value};

struct Table;
impl Fetch for Table {
    fn get(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}
struct Rules;
impl FileScope for Rules {}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    data: std::path::PathBuf,
    users: Arc<Mutex<iyakki_accounts::UserStore>>,
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

/// A machine with two accounts (`ann` administrator, `gus` guest) and the roots `roots` served over a real socket.
fn machine(tag: &str, build: impl FnOnce(Arc<dyn AccountsApi>, Arc<Mutex<iyakki_accounts::UserStore>>) -> Host) -> Machine {
    let data = std::env::temp_dir().join(format!("iyakki-served-auth-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let data = std::fs::canonicalize(&data).unwrap();
    let mut store = iyakki_accounts::UserStore::open(&data.join("users.json")).unwrap();
    store.add_local("ann", "pw-ann-1234", Role::Admin).unwrap();
    store.add_local("gus", "pw-gus-1234", Role::Guest).unwrap();
    let users = Arc::new(Mutex::new(store));
    let accounts: Arc<dyn AccountsApi> = Arc::new(users.clone());
    let audit: iyakki_shared::gitkeys::AuditFn = Arc::new(|_, _| {});
    let host = build(accounts.clone(), users.clone()).with_store_side(Arc::new(Table), audit, accounts, Some(Arc::new(Rules)), data.clone());
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, rpc: Some(Arc::new(host)), appevents: None, ctx: CallContext::remote("ui", Transport::LocalSocket, &[WireScope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread, data, users }
}

fn rpc(m: &Machine) -> Rpc {
    Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap())
}

fn admin_policy() -> Policy {
    Policy::resolve(Role::Admin, &Default::default())
}
fn guest_policy() -> Policy {
    Policy::resolve(Role::Guest, &Default::default())
}
fn forged_session(user: &str) -> iyakki_accounts::Session {
    iyakki_accounts::Session { user: user.into(), role: Role::Admin, policy: admin_policy() }
}

// ---------------------------------------------------------------------------------------------------------------- the accounts

fn accounts_root(guarded: bool) -> impl FnOnce(Arc<dyn AccountsApi>, Arc<Mutex<iyakki_accounts::UserStore>>) -> Host {
    move |a, _| {
        let served: Arc<dyn AccountsApi> = if guarded { Arc::new(AccountsGuarded(a)) } else { a };
        Host::new().with_root(Obj::AccountsApi(served))
    }
}

fn remote_accounts(m: &Machine) -> Arc<dyn AccountsApi> {
    match rpc(m).root("AccountsApi") {
        Some(RemoteAny::AccountsApi(a)) => a,
        _ => panic!("the trait is generated"),
    }
}

/// The same checks with the guard the core serves (`AccountsGuarded`) and without it: the vouch step alone must hold.
fn accounts_refuse_a_forged_administrator(guarded: bool) {
    let m = machine(if guarded { "acc-guarded" } else { "acc-raw" }, accounts_root(guarded));
    let acc = remote_accounts(&m);
    let forged = forged_session("gus");
    // the guest says it is an administrator: every change is refused and nothing is written
    assert!(acc.create_user(&forged, "mallory", "pw-mallory-1", Role::Admin).is_err());
    assert!(acc.set_role(&forged, "gus", Role::Admin).is_err());
    assert!(acc.set_overrides(&forged, "gus", iyakki_sec::policy::PolicyOverride { skills: Some(true), ..Default::default() }).is_err());
    assert!(acc.delete_user(&forged, "ann").is_err());
    assert!(acc.set_password(&forged, "ann", None, "taken-over-12").is_err(), "a guest cannot reset the administrator's password");
    {
        let g = m.users.lock().unwrap();
        assert!(g.get_public("mallory").is_none(), "no account was made");
        assert_eq!(g.get_public("gus").unwrap().role, Role::Guest, "the guest is still a guest");
        assert!(g.get_public("ann").is_some(), "the administrator is still there");
        assert!(g.get_public("gus").unwrap().overrides.skills.is_none(), "no override was written");
    }
    // a name the core does not know is refused, whatever role and policy it carries
    let ghost = forged_session("ghost");
    assert!(acc.create_user(&ghost, "mallory", "pw-mallory-1", Role::Guest).is_err());
    assert!(acc.delete_user(&ghost, "gus").is_err());
    assert!(m.users.lock().unwrap().get_public("mallory").is_none());
    // an administrator who says it is a guest is still an administrator (the claim is rewritten both ways)
    let meek = iyakki_accounts::Session { user: "ann".into(), role: Role::Guest, policy: guest_policy() };
    assert!(acc.create_user(&meek, "bea", "pw-bea-12345", Role::Guest).is_ok());
    assert!(m.users.lock().unwrap().get_public("bea").is_some());
    // the real administrator, claiming anything, can change another account
    assert!(acc.set_role(&meek, "bea", Role::User).is_ok());
    assert_eq!(m.users.lock().unwrap().get_public("bea").unwrap().role, Role::User);
    // a person changes their own password only with the old one, however the session reads
    assert!(acc.set_password(&forged, "gus", None, "new-guest-pass").is_err());
    assert!(acc.set_password(&forged, "gus", Some("pw-gus-1234"), "new-guest-pass").is_ok());
    assert!(acc.delete_user(&meek, "bea").is_ok());
}

#[test]
fn accounts_with_the_guard_refuse_a_forged_administrator_session_over_the_real_socket() {
    accounts_refuse_a_forged_administrator(true);
}

#[test]
fn accounts_without_the_guard_still_refuse_a_forged_administrator_session_because_the_core_rebuilds_the_session_from_its_name() {
    accounts_refuse_a_forged_administrator(false);
}

// ---------------------------------------------------------------------------------------------------------------- what every claim becomes

/// A root of any trait that answers with the arguments it was given: what the core's object would receive.
struct Echo(&'static str);
impl iyakki_ui_rpc::Served for Echo {
    fn iface(&self) -> &'static str {
        self.0
    }
    fn serve(&self, _reg: &iyakki_ui_rpc::Registry<'_>, _method: &str, a: Value) -> Result<Value, String> {
        Ok(a)
    }
}

fn echo_machine(tag: &str, ifaces: &[&'static str]) -> Machine {
    let ifaces = ifaces.to_vec();
    machine(tag, move |_, _| ifaces.iter().fold(Host::new(), |h, i| h.with_root(Obj::Served(Arc::new(Echo(i))))))
}

fn echoed(rpc: &Rpc, iface: &str, method: &str, args: Value) -> Result<Value, String> {
    rpc.call::<Value>(iface, 0, method, args)
}

#[test]
fn the_core_overwrites_every_policy_claim_with_the_policy_of_the_named_account() {
    let m = echo_machine("policy", &["ServicesHandle", "FlowsFacet", "StoreHub", "StoreUpdates", "DashHost"]);
    let r = rpc(&m);
    let (forged, gus, ann) = (json!(admin_policy()), json!(guest_policy()), json!(admin_policy()));
    // (interface, method, arguments the UI sends, index of the claim, name at index)
    let cases: Vec<(&str, &str, Vec<Value>, usize, &str)> = vec![
        ("ServicesHandle", "scope_check_read", vec![forged.clone(), json!("gus"), json!("/etc/shadow")], 0, "gus"),
        ("ServicesHandle", "scope_check_write", vec![forged.clone(), json!("gus"), json!("/etc/shadow")], 0, "gus"),
        ("ServicesHandle", "scope_roots", vec![forged.clone(), json!("gus"), json!("/data")], 0, "gus"),
        ("ServicesHandle", "scope_roots_with_home", vec![forged.clone(), json!("gus"), json!("/data"), json!("/")], 0, "gus"),
        ("ServicesHandle", "rag_refresh_tick", vec![json!("gus"), forged.clone()], 1, "gus"),
        ("FlowsFacet", "open", vec![json!("gus"), forged.clone()], 1, "gus"),
        ("FlowsFacet", "answer_permission", vec![json!("gus"), forged.clone(), json!({}), json!("Allow")], 1, "gus"),
        ("FlowsFacet", "start_for_login", vec![json!("gus"), forged.clone()], 1, "gus"),
        ("StoreHub", "tiles", vec![forged.clone(), json!("gus")], 0, "gus"),
        ("StoreHub", "add_dev_tile", vec![forged.clone(), json!("gus"), json!("/x")], 0, "gus"),
        ("StoreUpdates", "update_check_run", vec![forged.clone(), json!("gus"), json!({}), json!("1"), json!("v")], 0, "gus"),
        ("DashHost", "read_ok", vec![forged.clone(), json!("gus"), json!("/etc/shadow")], 0, "gus"),
    ];
    for (iface, method, args, idx, who) in cases {
        let got = echoed(&r, iface, method, Value::Array(args.clone())).unwrap_or_else(|e| panic!("{iface}::{method}: {e}"));
        let want = if who == "gus" { &gus } else { &ann };
        assert_eq!(&got[idx], want, "{iface}::{method} reached the core with a policy the UI wrote");
        assert_ne!(&got[idx], &forged, "{iface}::{method}: the guest's claim to be an administrator survived");
        // everything else in the call is untouched
        for (i, a) in args.iter().enumerate().filter(|(i, _)| *i != idx) {
            assert_eq!(&got[i], a, "{iface}::{method}: argument {i} changed");
        }
        // a name the core does not know is refused: the core's object is never asked
        let mut ghost = args.clone();
        let name_at = ghost.iter().position(|v| v == &json!("gus")).unwrap();
        ghost[name_at] = json!("ghost");
        let err = echoed(&r, iface, method, Value::Array(ghost)).unwrap_err();
        assert!(err.contains("no account ghost"), "{iface}::{method}: {err}");
    }
}

#[test]
fn the_core_overwrites_the_role_the_admin_flag_and_the_list_of_administrators() {
    let m = echo_machine("role", &["ServicesHandle", "ClusterFacet", "FleetApi", "ServicesFacetB3"]);
    let r = rpc(&m);
    // roles: a guest who says "Admin" is a Guest; an administrator who says "Guest" is an Admin
    let got = echoed(&r, "ClusterFacet", "switch", json!(["/d", "/data", "gus", "admin", "c1"])).unwrap();
    assert_eq!(got[3], json!("guest"), "{got}");
    let got = echoed(&r, "ClusterFacet", "switch", json!(["/d", "/data", "ann", "guest", "c1"])).unwrap();
    assert_eq!(got[3], json!("admin"), "{got}");
    for m in ["start", "stop"] {
        let got = echoed(&r, "FleetApi", m, json!([{}, "c1", "gus", "admin"])).unwrap();
        assert_eq!(got[3], json!("guest"), "FleetApi::{m}: {got}");
    }
    assert!(echoed(&r, "FleetApi", "start", json!([{}, "c1", "ghost", "admin"])).unwrap_err().contains("no account ghost"));
    // the flag: true only for an account that may manage the settings
    for (call, args, at) in [("on_login", json!(["gus", "pw", true]), 2), ("on_resume", json!(["gus", true]), 1), ("on_logout", json!(["gus", true]), 1), ("sharing_notice_tick", json!(["gus", true]), 1)] {
        let got = echoed(&r, "ServicesHandle", call, args).unwrap();
        assert_eq!(got[at], json!(false), "{call}: a guest who says it is an administrator is not one: {got}");
    }
    for (call, args, at) in [("on_login", json!(["ann", "pw", false]), 2), ("on_resume", json!(["ann", false]), 1), ("on_logout", json!(["ann", false]), 1), ("sharing_notice_tick", json!(["ann", false]), 1)] {
        let got = echoed(&r, "ServicesHandle", call, args).unwrap();
        assert_eq!(got[at], json!(true), "{call}: an administrator who says it is not one still is: {got}");
    }
    assert!(echoed(&r, "ServicesHandle", "on_login", json!(["ghost", "pw", true])).unwrap_err().contains("no account ghost"));
    // the list of administrators is the core's: the UI's list (a guest named in it) is dropped
    let got = echoed(&r, "ServicesFacetB3", "computer_announce", json!([["gus", "mallory"]])).unwrap();
    assert_eq!(got[0], json!(["ann"]), "{got}");
}

#[test]
fn the_skill_flags_of_a_personal_scope_are_the_accounts_and_the_shared_scope_is_left_as_a_recorded_gap() {
    let m = echo_machine("skills", &["SkillApi", "PackageApi"]);
    let r = rpc(&m);
    // a guest cannot keep personal skills: it claims it can, the core says it cannot
    let got = echoed(&r, "SkillApi", "add", json!([{"User": "gus"}, "md", true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(false)), "{got}");
    // an administrator can, whatever it claims
    let got = echoed(&r, "SkillApi", "add", json!([{"User": "ann"}, "md", false, false])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(true)), "{got}");
    let got = echoed(&r, "SkillApi", "remove", json!([{"User": "gus"}, "n", true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(false)), "{got}");
    let got = echoed(&r, "PackageApi", "remove", json!([{"User": "gus"}, "n", true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(false)), "{got}");
    let got = echoed(&r, "PackageApi", "install", json!([{"scope": {"User": "gus"}}, true, true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(false)), "{got}");
    assert!(echoed(&r, "SkillApi", "add", json!([{"User": "ghost"}, "md", true, true])).unwrap_err().contains("no account ghost"));
    // the shared scope names nobody: the claim passes (D-SEC1-SHARED, written down in docs/UI-SOCKET-SECURITY.md)
    let got = echoed(&r, "SkillApi", "add", json!(["Shared", "md", true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(true), &json!(true)), "the gap is recorded, not hidden: {got}");
}

#[test]
fn a_session_names_its_account_and_an_unnamed_or_unknown_one_is_refused_before_anything_runs() {
    let m = echo_machine("session", &["AccountsApi"]);
    let r = rpc(&m);
    let got = echoed(&r, "AccountsApi", "delete_user", json!([forged_session("gus"), "ann"])).unwrap();
    assert_eq!(got[0]["role"], json!("guest"), "{got}");
    assert_eq!(got[0]["policy"], json!(guest_policy()), "{got}");
    assert!(echoed(&r, "AccountsApi", "delete_user", json!([{"role": "admin"}, "ann"])).is_err(), "a session that names nobody");
    assert!(echoed(&r, "AccountsApi", "delete_user", json!([])).is_err());
}

// ---------------------------------------------------------------------------------------------------------------- typed objects

#[derive(Default)]
struct Seen(Mutex<Vec<String>>);
impl Seen {
    fn note(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct FlowsFake(Arc<Seen>);
impl FlowsFacet for FlowsFake {
    fn start_for_login(&self, user: &str, policy: &Policy) -> bool {
        self.0.note(format!("start_for_login {user} manage_users={} manage_settings={}", policy.manage_users, policy.manage_settings));
        true
    }
}
struct HubFake(Arc<Seen>);
impl StoreHub for HubFake {
    fn tiles(&self, policy: &Policy, user: &str) -> Vec<TileEntry> {
        self.0.note(format!("tiles {user} manage_settings={}", policy.manage_settings));
        Vec::new()
    }
}
struct ClusterFake(Arc<Seen>);
impl ClusterFacet for ClusterFake {
    fn switch(&self, _dir: &std::path::Path, _data: &std::path::Path, user: &str, role: Role, id: &str) -> Result<iyakki_shared::clusters::ClusterEntry, String> {
        self.0.note(format!("switch {user} {role:?} {id}"));
        Err("recorded".into())
    }
}
struct SkillFake(Arc<Seen>);
impl SkillApi for SkillFake {
    fn add(&self, scope: Scope, _md: &str, can_shared: bool, can_own: bool) -> Result<Skill, SkillError> {
        self.0.note(format!("add {scope:?} shared={can_shared} own={can_own}"));
        Err(SkillError::Forbidden("recorded"))
    }
    fn get(&self, _: &Scope, _: &str) -> Option<Skill> {
        None
    }
    fn list(&self, _: &str) -> Vec<Skill> {
        Vec::new()
    }
    fn list_scope(&self, _: &Scope) -> Vec<Skill> {
        Vec::new()
    }
    fn remove(&self, scope: &Scope, _name: &str, can_shared: bool, can_own: bool) -> Result<(), SkillError> {
        self.0.note(format!("remove {scope:?} shared={can_shared} own={can_own}"));
        Err(SkillError::Forbidden("recorded"))
    }
}

#[test]
fn the_generated_dispatchers_receive_the_accounts_policy_role_and_flags_not_the_uis() {
    let seen = Arc::new(Seen::default());
    let s2 = seen.clone();
    let m = machine("typed", move |_, _| {
        Host::new()
            .with_root(Obj::FlowsFacet(Arc::new(FlowsFake(s2.clone()))))
            .with_root(Obj::StoreHub(Arc::new(HubFake(s2.clone()))))
            .with_root(Obj::ClusterFacet(Arc::new(ClusterFake(s2.clone()))))
            .with_root(Obj::SkillApi(Arc::new(SkillFake(s2.clone()))))
    });
    let r = rpc(&m);
    let Some(RemoteAny::FlowsFacet(flows)) = r.root("FlowsFacet") else { panic!("generated") };
    let Some(RemoteAny::StoreHub(hub)) = r.root("StoreHub") else { panic!("generated") };
    let Some(RemoteAny::ClusterFacet(cluster)) = r.root("ClusterFacet") else { panic!("generated") };
    let Some(RemoteAny::SkillApi(skills)) = r.root("SkillApi") else { panic!("generated") };
    // the guest names the administrator's policy everywhere
    assert!(flows.start_for_login("gus", &admin_policy()));
    assert!(!flows.start_for_login("ghost", &admin_policy()), "an unknown name: the core's object is not asked, the UI gets the quiet value");
    let _ = hub.tiles(&admin_policy(), "gus");
    let _ = cluster.switch(std::path::Path::new("/d"), std::path::Path::new("/data"), "gus", Role::Admin, "c1");
    let _ = skills.add(Scope::User("gus".into()), "md", true, true);
    let _ = skills.remove(&Scope::User("ann".into()), "n", false, false);
    assert_eq!(
        seen.all(),
        [
            "start_for_login gus manage_users=false manage_settings=false",
            "tiles gus manage_settings=false",
            "switch gus Guest c1",
            "add User(\"gus\") shared=false own=false",
            "remove User(\"ann\") shared=false own=true",
        ]
    );
}

// ---------------------------------------------------------------------------------------------------------------- the stories

/// The core's end of the two calls the story store is built from, answering with the account's policy as the vouch step wrote it.
struct StoryCore {
    home: std::path::PathBuf,
    seen: Arc<Seen>,
}
impl iyakki_ui_rpc::Served for StoryCore {
    fn iface(&self) -> &'static str {
        "ServicesHandle"
    }
    fn serve(&self, reg: &iyakki_ui_rpc::Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "scope_home_root" => Ok(json!(self.home)),
            "policy_of" => {
                let (user,): (String,) = serde_json::from_value(a).map_err(|e| e.to_string())?;
                let _ = reg;
                // the core's own accounts answer (the dispatcher of the real core does the same through `Registry::policy_of`)
                self.seen.note(format!("policy_of {user}"));
                Ok(json!(guest_policy()))
            }
            "scope_roots_with_home" => {
                let (p, u, _d, home): (Policy, String, String, std::path::PathBuf) = serde_json::from_value(a).map_err(|e| e.to_string())?;
                self.seen.note(format!("roots {u} manage_users={}", p.manage_users));
                Ok(json!({"home": home, "roots": [home], "mounts": []}))
            }
            other => Err(format!("not served: {other}")),
        }
    }
}

#[test]
fn the_stories_are_built_over_the_policy_the_core_reads_not_the_one_the_caller_names() {
    let home = std::env::temp_dir().join(format!("iyakki-served-auth-{}-tales", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let seen = Arc::new(Seen::default());
    let (h2, s2) = (home.clone(), seen.clone());
    let m = machine("tales", move |_, _| Host::new().with_root(Obj::Served(Arc::new(StoryCore { home: h2, seen: s2 }))));
    let Some(RemoteAny::ServicesFacetB5(b5)) = rpc(&m).root("ServicesFacetB5") else { panic!("generated") };
    let store = b5.story_store(&admin_policy(), "gus", std::path::Path::new("/data")).expect("the core answers");
    let log = seen.all();
    assert!(log.contains(&"policy_of gus".to_string()), "{log:?}");
    assert!(log.contains(&"roots gus manage_users=false".to_string()), "the roots were asked for with the core's reading, never the claimed administrator policy: {log:?}");
    drop(store);
    let _ = std::fs::remove_dir_all(&home);
}

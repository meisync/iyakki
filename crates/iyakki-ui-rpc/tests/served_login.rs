//! The acting person of a connection and the approval ticket (alpha.8 SEC3, D-SEC3-BIND, D-SEC3-TICKET) over a real loopback socket: forged
//! names, forged sessions, the shared-scope right, and tickets that are missing, replayed, expired, for other arguments, another call, another
//! person or another connection. The host clock is a manual one: no sleeps.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::AccountsApi;
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::FileScope;
use iyakki_sec::policy::Role;
use iyakki_shared::store_catalog::Fetch;
use iyakki_testkit::ManualClock;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope as WireScope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAny, Rpc, TICKET_TTL_MS};
use serde_json::{json, Value};

struct Table;
impl Fetch for Table {
    fn get(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}
struct Rules;
impl FileScope for Rules {}

struct Echo(&'static str);
impl iyakki_ui_rpc::Served for Echo {
    fn iface(&self) -> &'static str {
        self.0
    }
    fn serve(&self, _reg: &iyakki_ui_rpc::Registry<'_>, _method: &str, a: Value) -> Result<Value, String> {
        Ok(a)
    }
}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    data: std::path::PathBuf,
    clock: ManualClock,
    audit: Arc<Mutex<Vec<String>>>,
    host: Arc<Host>,
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

const ECHOES: &[&str] = &["ServicesHandle", "SkillApi", "PackageApi", "PlatformApi"];

/// Two accounts (`ann` administrator, `gus` guest), the real accounts object, echo roots for the rest, binding on.
fn machine(tag: &str) -> Machine {
    let data = std::env::temp_dir().join(format!("iyakki-served-login-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let data = std::fs::canonicalize(&data).unwrap();
    let mut store = iyakki_accounts::UserStore::open(&data.join("users.json")).unwrap();
    store.add_local("ann", "pw-ann-1234", Role::Admin).unwrap();
    store.add_local("gus", "pw-gus-1234", Role::Guest).unwrap();
    let users = Arc::new(Mutex::new(store));
    let accounts: Arc<dyn AccountsApi> = Arc::new(users);
    let clock = ManualClock::new(1_000);
    let audit: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = audit.clone();
    let audit_fn: iyakki_shared::gitkeys::AuditFn = Arc::new(move |what, detail| sink.lock().unwrap().push(format!("{what} {detail}")));
    let host = ECHOES.iter().fold(Host::new().with_root(Obj::AccountsApi(accounts.clone())), |h, i| h.with_root(Obj::Served(Arc::new(Echo(i)))));
    let host = Arc::new(host.with_store_side(Arc::new(Table), audit_fn, accounts, Some(Arc::new(Rules)), data.clone()).with_login_binding().with_ticket_clock(Arc::new(clock.reader())));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, rpc: Some(host.clone()), appevents: None, ctx: CallContext::remote("ui", Transport::LocalSocket, &[WireScope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread, data, clock, audit, host }
}

fn rpc(m: &Machine) -> Rpc {
    Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap())
}

fn accounts(r: &Rpc) -> Arc<dyn AccountsApi> {
    match r.root("AccountsApi") {
        Some(RemoteAny::AccountsApi(a)) => a,
        _ => panic!("the trait is generated"),
    }
}

/// A connection with `who` signed in by password.
fn signed_in(m: &Machine, who: &str) -> Rpc {
    let r = rpc(m);
    accounts(&r).authenticate(who, &format!("pw-{who}-1234")).unwrap();
    r
}

fn call(r: &Rpc, iface: &str, method: &str, args: Value) -> Result<Value, String> {
    r.call::<Value>(iface, 0, method, args)
}

/// A call as a hostile UI would send it: straight on the wire, no client step in between.
fn raw(m: &Machine) -> Connection {
    Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap()
}

fn raw_call(c: &mut Connection, iface: &str, method: &str, args: Value) -> Result<Value, String> {
    c.call("rpc.call", json!({"obj": 0, "iface": iface, "method": method, "args": args})).map_err(|e| e.message)
}

fn raw_signed_in(m: &Machine, who: &str) -> Connection {
    let mut c = raw(m);
    let v = raw_call(&mut c, "AccountsApi", "authenticate", json!([who, format!("pw-{who}-1234")])).unwrap();
    assert!(v.get("Ok").is_some(), "{v}");
    c
}

// ---------------------------------------------------------------------------------------------------------------- binding

#[test]
fn a_connection_that_nobody_signed_in_on_may_not_act_as_anyone() {
    let m = machine("unbound");
    let r = rpc(&m);
    let e = call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).unwrap_err();
    assert!(e.contains("nobody is signed in"), "{e}");
    let e = call(&r, "ServicesHandle", "on_logout", json!(["gus", false])).unwrap_err();
    assert!(e.contains("nobody is signed in"), "{e}");
}

#[test]
fn only_a_password_the_core_checked_binds_the_connection() {
    let m = machine("bind");
    let r = rpc(&m);
    assert!(accounts(&r).authenticate("gus", "wrong-password").is_err());
    assert!(call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).unwrap_err().contains("nobody is signed in"));
    accounts(&r).authenticate("gus", "pw-gus-1234").unwrap();
    assert!(call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).is_ok());
    assert!(m.audit.lock().unwrap().iter().any(|l| l == "ui.login.bound gus password"), "{:?}", m.audit.lock().unwrap());
}

#[test]
fn a_call_that_names_another_account_is_refused_with_every_rule_that_carries_a_name() {
    let m = machine("forged-name");
    let r = signed_in(&m, "gus");
    let named: Vec<(&str, &str, Value)> = vec![
        ("ServicesHandle", "scope_check_read", json!([{}, "ann", "/x"])),
        ("ServicesHandle", "scope_roots", json!([{}, "ann", "/d"])),
        ("ServicesHandle", "rag_refresh_tick", json!(["ann", {}])),
        ("ServicesHandle", "sharing_notice_tick", json!(["ann", false])),
        ("ServicesHandle", "on_resume", json!(["ann", false])),
        ("ServicesHandle", "on_login", json!(["ann", "pw-ann-1234", false])),
        ("ServicesHandle", "on_logout", json!(["ann", false])),
    ];
    for (i, mth, args) in named {
        let e = call(&r, i, mth, args).unwrap_err();
        assert!(e.contains("may not act as ann") || e.contains("sign out before"), "{mth}: {e}");
    }
    // the same calls about the account itself run
    assert!(call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).is_ok());
    assert!(m.audit.lock().unwrap().iter().any(|l| l.starts_with("ui.login.refused gus named ann")));
}

#[test]
fn an_administrator_may_act_for_another_account_only_where_the_admin_rule_says_so() {
    let m = machine("admin-acts");
    let r = signed_in(&m, "ann");
    assert!(call(&r, "ServicesHandle", "on_logout", json!(["gus", false])).is_ok(), "on_logout is on the admin list");
    let e = call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).unwrap_err();
    assert!(e.contains("may not act as gus"), "{e}");
    // a guest on the same call is refused
    let g = signed_in(&m, "gus");
    assert!(call(&g, "ServicesHandle", "on_logout", json!(["ann", false])).is_err());
}

#[test]
fn the_accounts_calls_that_take_a_session_must_carry_the_session_of_the_signed_in_account() {
    let m = machine("session");
    let r = signed_in(&m, "gus");
    let acc = accounts(&r);
    let ann = iyakki_accounts::Session { user: "ann".into(), role: Role::Admin, policy: iyakki_sec::policy::Policy::resolve(Role::Admin, &Default::default()) };
    // the real administrator's session, sent by the guest's connection: refused before the accounts object runs
    let e = acc.create_user(&ann, "mallory", "pw-mallory-1", Role::Admin).unwrap_err().to_string();
    assert!(e.contains("may not act as ann"), "{e}");
    let mut admin = signed_in(&m, "ann");
    let acc2 = accounts(&admin);
    assert!(acc2.create_user(&ann, "bea", "pw-bea-12345", Role::Guest).is_ok());
    admin = signed_in(&m, "ann");
    let _ = admin;
}

#[test]
fn the_shared_skill_right_is_the_signed_in_accounts_never_the_uis_claim() {
    let m = machine("shared");
    let g = signed_in(&m, "gus");
    // a guest claims it may edit the shared skills: the core writes its own reading
    let got = call(&g, "SkillApi", "add", json!(["Shared", "md", true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(false)), "{got}");
    let a = signed_in(&m, "ann");
    let got = call(&a, "SkillApi", "remove", json!(["Shared", "n", false, false])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(true), &json!(true)), "{got}");
    let got = call(&a, "PackageApi", "remove", json!(["Shared", "n", false, false])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(true), &json!(true)), "{got}");
    // another person's personal scope is refused, an own one is vouched
    assert!(call(&g, "SkillApi", "add", json!([{"User": "ann"}, "md", true, true])).unwrap_err().contains("may not change the skills of ann"));
    let got = call(&g, "SkillApi", "add", json!([{"User": "gus"}, "md", true, true])).unwrap();
    assert_eq!((&got[2], &got[3]), (&json!(false), &json!(false)), "{got}");
}

#[test]
fn a_connection_signed_in_as_one_person_cannot_resume_or_sign_in_as_another_until_it_signs_out() {
    let m = machine("switch");
    let r = signed_in(&m, "gus");
    let acc = accounts(&r);
    assert!(acc.resume_session("ann").is_none(), "refused answers nothing");
    assert!(call(&r, "ServicesHandle", "on_logout", json!(["gus", false])).is_ok());
    // signed out: the connection is free again, and a resume of the same name binds
    assert!(call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).unwrap_err().contains("nobody is signed in"));
    assert!(acc.resume_session("ann").is_some());
    assert!(call(&r, "ServicesHandle", "scope_check_read", json!([{}, "ann", "/x"])).is_ok());
}

#[test]
fn on_login_binds_only_with_the_password_the_core_checks_and_a_connection_close_frees_the_person() {
    let m = machine("on-login");
    let r = rpc(&m);
    assert!(call(&r, "ServicesHandle", "on_login", json!(["gus", "not-the-password", false])).is_err());
    assert!(call(&r, "ServicesHandle", "on_login", json!(["gus", "pw-gus-1234", true])).is_ok());
    assert!(call(&r, "ServicesHandle", "scope_check_read", json!([{}, "gus", "/x"])).is_ok());
    // the host forgets a closed connection (called directly: the transport calls this when the socket closes)
    use iyakki_ui_client::server::RpcServe;
    let ctx = CallContext::remote("ui", Transport::LocalSocket, &[]);
    let v = m.host.call(&ctx, 77, 0, "AccountsApi", "authenticate", json!(["ann", "pw-ann-1234"])).unwrap();
    assert!(v.get("Ok").is_some());
    assert_eq!(m.host.acting(77).as_deref(), Some("ann"));
    m.host.release(77, None);
    assert_eq!(m.host.acting(77), None);
}

// ---------------------------------------------------------------------------------------------------------------- tickets

fn plan() -> Value {
    json!({"name": "demo", "steps": [1, 2], "z": {"b": 1, "a": 2}})
}

fn issue(c: &mut Connection, iface: &str, method: &str, args: Value) -> Result<String, String> {
    raw_call(c, "Approval", "issue", json!([iface, method, args])).map(|v| v.as_str().unwrap().to_string())
}

#[test]
fn an_approved_call_without_a_ticket_is_refused_and_never_runs_as_not_approved() {
    let m = machine("no-ticket");
    let mut c = raw_signed_in(&m, "ann");
    for (mth, args) in [("apply", json!([plan(), true])), ("power", json!([{}, true, true])), ("connect_existing", json!(["u", "t", null, "ns", true]))] {
        let e = raw_call(&mut c, "PlatformApi", mth, args).unwrap_err();
        assert!(e.contains("holds no valid ticket") && e.contains("no ticket"), "{mth}: {e}");
    }
    let e = raw_call(&mut c, "PackageApi", "install", json!([{}, true, false, false])).unwrap_err();
    assert!(e.contains("no ticket"), "{e}");
    // a call that is not approved needs none (the core's own approval path refuses it as before)
    assert!(raw_call(&mut c, "PlatformApi", "apply", json!([plan(), false])).is_ok());
    assert!(m.audit.lock().unwrap().iter().any(|l| l.starts_with("ui.approval.refused ann apply no ticket")));
}

#[test]
fn a_ticket_is_for_one_call_with_one_set_of_arguments_and_one_use() {
    let m = machine("one-use");
    let mut c = raw_signed_in(&m, "ann");
    let t = issue(&mut c, "PlatformApi", "apply", json!([plan(), true])).unwrap();
    // the argument map built in another order is the same call; the core removed the ticket before the object saw the arguments
    let reordered = json!([{"z": {"a": 2, "b": 1}, "steps": [1, 2], "name": "demo"}, true, t]);
    let got = raw_call(&mut c, "PlatformApi", "apply", reordered.clone()).unwrap();
    assert_eq!(got.as_array().unwrap().len(), 2, "the ticket never reaches the object: {got}");
    assert_eq!(got[1], json!(true));
    // a replay of the same ticket is refused
    let e = raw_call(&mut c, "PlatformApi", "apply", reordered).unwrap_err();
    assert!(e.contains("unknown or already used"), "{e}");
    let log = m.audit.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.starts_with("ui.approval.issued ann apply")) && log.iter().any(|l| l.starts_with("ui.approval.used ann apply")), "{log:?}");
    assert_eq!(m.host.open_tickets(), 0);
}

#[test]
fn a_ticket_for_other_arguments_or_another_call_is_refused_and_spent() {
    let m = machine("bound");
    let mut c = raw_signed_in(&m, "ann");
    let t = issue(&mut c, "PlatformApi", "apply", json!([plan(), true])).unwrap();
    let mut other = plan();
    other["name"] = json!("other");
    let e = raw_call(&mut c, "PlatformApi", "apply", json!([other, true, t.clone()])).unwrap_err();
    assert!(e.contains("issued for other arguments"), "{e}");
    // spent: even the right call cannot use it now
    assert!(raw_call(&mut c, "PlatformApi", "apply", json!([plan(), true, t])).unwrap_err().contains("unknown or already used"));
    // a ticket for one method is no ticket for another with the same arguments
    let t = issue(&mut c, "PlatformApi", "apply", json!([plan(), true])).unwrap();
    let e = raw_call(&mut c, "PlatformApi", "apply_with", json!([plan(), true, t])).unwrap_err();
    assert!(e.contains("issued for another call"), "{e}");
    // a method that takes no approval has no ticket to issue
    assert!(issue(&mut c, "ServicesHandle", "on_logout", json!(["ann", false])).unwrap_err().contains("takes no approval ticket"));
}

#[test]
fn an_expired_ticket_is_refused_on_the_manual_clock() {
    let m = machine("expiry");
    let mut c = raw_signed_in(&m, "ann");
    let t = issue(&mut c, "PlatformApi", "push", json!([plan(), true])).unwrap();
    m.clock.advance(Duration::from_millis(TICKET_TTL_MS - 1));
    let t2 = issue(&mut c, "PlatformApi", "push", json!([plan(), true])).unwrap();
    assert!(raw_call(&mut c, "PlatformApi", "push", json!([plan(), true, t])).is_ok(), "one millisecond before the end it still works");
    m.clock.advance(Duration::from_millis(TICKET_TTL_MS));
    let e = raw_call(&mut c, "PlatformApi", "push", json!([plan(), true, t2])).unwrap_err();
    assert!(e.contains("expired"), "{e}");
}

#[test]
fn a_ticket_does_not_work_for_another_person_or_another_connection() {
    let m = machine("elsewhere");
    let mut ann = raw_signed_in(&m, "ann");
    let t = issue(&mut ann, "PlatformApi", "backup", json!([{}, false, true])).unwrap();
    // the same account on a second connection
    let mut ann2 = raw_signed_in(&m, "ann");
    let e = raw_call(&mut ann2, "PlatformApi", "backup", json!([{}, false, true, t])).unwrap_err();
    assert!(e.contains("another person or connection"), "{e}");
    // another account
    let mut gus = raw_signed_in(&m, "gus");
    let t = issue(&mut ann, "PlatformApi", "backup", json!([{}, false, true])).unwrap();
    assert!(raw_call(&mut gus, "PlatformApi", "backup", json!([{}, false, true, t])).is_err());
    // a guess is no ticket
    let e = raw_call(&mut ann, "PlatformApi", "backup", json!([{}, false, true, "00000000000000000000000000000000"])).unwrap_err();
    assert!(e.contains("unknown or already used"), "{e}");
    // nobody signed in: no one to approve
    let mut anon = raw(&m);
    assert!(issue(&mut anon, "PlatformApi", "backup", json!([{}, false, true])).unwrap_err().contains("nobody is signed in"));
}

#[test]
fn the_client_asks_for_the_ticket_by_itself_when_the_card_approved_and_every_platform_call_is_ticketed() {
    let m = machine("client");
    let r = signed_in(&m, "ann");
    let cases = [
        ("PlatformApi", "apply", json!([plan(), true])),
        ("PlatformApi", "apply_with", json!([plan(), true])),
        ("PlatformApi", "power", json!([{}, true, true])),
        ("PlatformApi", "upgrade", json!([plan(), true])),
        ("PlatformApi", "backup", json!([{}, false, true])),
        ("PlatformApi", "remove", json!([plan(), "demo", true])),
        ("PlatformApi", "connect", json!([plan(), true])),
        ("PlatformApi", "push", json!([plan(), true])),
        ("PlatformApi", "create_pull_request", json!([plan(), true])),
        ("PlatformApi", "connect_existing", json!(["u", "t", null, "ns", true])),
        ("PackageApi", "install", json!([plan(), true, false, false])),
    ];
    for (i, mth, args) in cases {
        let got = call(&r, i, mth, args.clone()).unwrap_or_else(|e| panic!("{mth}: {e}"));
        // (the shared-scope flags of an install are the account's own reading, so only the leading arguments are compared)
        assert_eq!(got.as_array().unwrap().len(), args.as_array().unwrap().len(), "{mth}: the ticket never reaches the object");
        assert_eq!(got.as_array().unwrap()[..2], args.as_array().unwrap()[..2], "{mth}: the object receives the call as the person approved it");
    }
    assert_eq!(m.host.open_tickets(), 0, "every ticket was used once");
    // a person who is not signed in gets no ticket, so the call fails before it is sent
    let anon = rpc(&m);
    assert!(call(&anon, "PlatformApi", "apply", json!([plan(), true])).unwrap_err().contains("nobody is signed in"));
}

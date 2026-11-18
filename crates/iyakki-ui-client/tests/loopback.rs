//! The contract client over real loopback sockets, against the reference server serving `iyakki-core-fake`'s settings, and the sample UI
//! program run as a real separate process. Test level: a real transport and a real process, a fake machine.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_core_fake::contract::{FakeFlows, FakeSettings, FakeVault};
use iyakki_ui_client::client::{Connection, Remote, RemoteSettings};
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::*;
use iyakki_core_api::contract::RateLimit;

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    settings: Arc<FakeSettings>,
}

fn machine(scopes: &[&str], rate: RateLimit, edit: impl FnOnce(&mut Hello) + Send + 'static) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let settings = Arc::new(FakeSettings::sample());
    let (s2, st2) = (stop.clone(), settings.clone());
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    let thread = std::thread::spawn(move || {
        let mut hello = settings_hello("test-machine", rate);
        edit(&mut hello);
        let refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
        let served = Served { hello, settings: Some(st2), vault: None, flows: None, ctx: CallContext::remote("laptop", Transport::DevMode, &refs) };
        serve(l, Arc::new(served), s2).unwrap();
    });
    Machine { addr, stop, thread: Some(thread), settings }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

const FREE: RateLimit = RateLimit { calls: 1000, per_ms: 1000 };

fn conn(m: &Machine) -> Connection {
    Connection::tcp(m.addr.as_str(), Duration::from_secs(5)).unwrap()
}

fn ctx() -> CallContext {
    CallContext::local("ignored", false)
}

#[test]
fn hello_negotiation_and_typed_settings_calls_work_over_a_real_socket() {
    let m = machine(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE], FREE, |_| {});
    let mut c = conn(&m);
    let (hello, n) = c.negotiate(&["settings", "flows"]).unwrap();
    assert_eq!(hello.machine, "test-machine");
    assert_eq!((n.usable.clone(), n.missing.clone()), (vec!["settings".to_string()], vec!["flows".to_string()]), "a domain the machine lacks is reported, not a failure");
    let s = RemoteSettings::new(c);
    let schema = s.schema(&ctx()).unwrap();
    assert_eq!(schema.len(), 3);
    assert_eq!(s.get(&ctx(), "display.dark").unwrap(), SettingValue::Bool(false));
    let ch = s.set(&ctx(), "display.dark", SettingValue::Bool(true), Some(&IdempotencyKey::new("change-0001").unwrap())).unwrap();
    assert!(!ch.repeated && ch.seq >= 1);
    // the same key twice is not written twice, and a different request with the same key is a conflict
    assert!(s.set(&ctx(), "display.dark", SettingValue::Bool(true), Some(&IdempotencyKey::new("change-0001").unwrap())).unwrap().repeated);
    assert_eq!(s.set(&ctx(), "display.dark", SettingValue::Bool(false), Some(&IdempotencyKey::new("change-0001").unwrap())).unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(s.get(&ctx(), "display.dark").unwrap(), SettingValue::Bool(true));
    // the events of the change come back with their sequence numbers
    let b = s.watch(&ctx(), Some("display.dark"), Resume::After(0), 10).unwrap();
    assert_eq!(b.events.len(), 1);
    assert_eq!(b.events[0].body.by, "laptop", "the machine, not the client, says who changed it");
    // refusals keep their stable codes and keys
    assert_eq!(s.get(&ctx(), "nope").unwrap_err().message_key, "settings.unknown_key");
    assert_eq!(s.set(&ctx(), "voice.speed", SettingValue::Text("warp".into()), None).unwrap_err().message_key, "settings.bad_value");
    assert!(m.settings.audit().iter().any(|l| l.contains("laptop via devmode") && l.contains("settings.set display.dark")), "{:?}", m.settings.audit());
}

#[test]
fn the_machine_decides_who_the_caller_is_a_client_cannot_make_itself_local_or_widen_its_scopes() {
    let m = machine(&[Scope::SETTINGS_READ], FREE, |_| {});
    let s = RemoteSettings::new(conn(&m));
    // the client passes a local identity; it makes no difference
    let local = CallContext::local("admin", true);
    assert_eq!(s.get(&local, "display.dark").unwrap(), SettingValue::Bool(false), "the granted read works");
    let e = s.set(&local, "display.dark", SettingValue::Bool(true), None).unwrap_err();
    assert_eq!((e.code, e.message_key.as_str()), (ErrorCode::Denied, "core.scope_missing"));
    // a raw request that tries to carry its own identity is read as an ordinary request: the extra field changes nothing
    let mut raw = TcpStream::connect(&m.addr).unwrap();
    writeln!(raw, r#"{{"id":7,"call":"settings.set","args":{{"key":"display.dark","value":{{"type":"bool","value":true}}}},"ctx":{{"transport":"in_process"}},"identity":{{"user":"admin","admin":true}}}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(&raw).read_line(&mut line).unwrap();
    assert!(line.contains("core.scope_missing") && line.contains("\"id\":7"), "{line}");
    assert_eq!(s.get(&ctx(), "display.dark").unwrap(), SettingValue::Bool(false), "nothing was written");
    // the local-only key is refused to any remote caller, even one that holds the write scope
    let m2 = machine(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE, Scope::LOCAL], FREE, |_| {});
    let s2 = RemoteSettings::new(conn(&m2));
    assert_eq!(s2.set(&ctx(), "security.vault_timeout", SettingValue::Int(30), None).unwrap_err().message_key, "core.local_only");
}

#[test]
fn damaged_unknown_and_too_big_lines_are_answered_or_closed_and_never_crash_the_server() {
    let m = machine(&[Scope::SETTINGS_READ], FREE, |_| {});
    let mut raw = TcpStream::connect(&m.addr).unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(raw.try_clone().unwrap());
    let mut ask = |line: &str| {
        writeln!(raw, "{line}").unwrap();
        let mut a = String::new();
        r.read_line(&mut a).unwrap();
        a
    };
    assert!(ask("this is not json").contains("transport.bad_request"));
    assert!(ask(r#"{"id":2,"call":"flows.run","args":{}}"#).contains("transport.unknown_call"));
    assert!(ask(r#"{"id":3,"call":"settings.get","args":{}}"#).contains("`key` is a text"));
    assert!(ask(r#"{"id":4,"call":"settings.set","args":{"key":"display.dark","value":5}}"#).contains("not a setting value"));
    assert!(ask(r#"{"id":5,"call":"settings.set","args":{"key":"display.dark","value":{"type":"bool","value":true},"idempotency_key":"x"}}"#).contains("core.bad_idempotency_key"));
    assert!(ask(r#"{"id":6,"call":"hello"}"#).contains("test-machine"), "the connection goes on after every refusal");
    // a line over the limit is answered once and the connection is closed
    let big = format!("{{\"id\":9,\"call\":\"hello\",\"args\":\"{}\"}}", "a".repeat(MAX_LINE));
    let a = ask(&big);
    assert!(a.contains("too long"), "{a}");
    let mut rest = String::new();
    assert_eq!(r.read_to_string(&mut rest).unwrap_or(0), 0, "closed");
    // a new connection works
    let mut c = conn(&m);
    assert!(c.hello().is_ok());
}

#[test]
fn the_rate_limit_of_hello_is_kept_and_says_when_to_try_again() {
    let m = machine(&[Scope::SETTINGS_READ], RateLimit { calls: 3, per_ms: 60_000 }, |_| {});
    let mut c = conn(&m);
    for _ in 0..3 {
        c.call("hello", serde_json::Value::Null).unwrap();
    }
    let e = c.call("hello", serde_json::Value::Null).unwrap_err();
    assert_eq!(e.code, ErrorCode::RateLimited);
    assert!(e.retry_after_ms.is_some());
}

#[test]
fn a_machine_of_another_major_version_is_refused_by_name_and_a_dead_address_is_a_plain_offline() {
    let m = machine(&[Scope::SETTINGS_READ], FREE, |h| h.contract = ContractVersion { major: 7, minor: 0 });
    let mut c = conn(&m);
    let e = c.negotiate(&["settings"]).unwrap_err();
    assert_eq!(e.code, ErrorCode::VersionMismatch);
    assert!(e.message.contains("update"), "{}", e.message);
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let e = Connection::tcp(dead, Duration::from_millis(500)).err().unwrap();
    assert_eq!(e.code, ErrorCode::Offline);
    assert!(!e.hint.is_empty());
    // a machine that closes the connection mid-way
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    std::thread::spawn(move || drop(l.accept()));
    let mut c = Connection::tcp(a, Duration::from_secs(2)).unwrap();
    assert_eq!(c.hello().unwrap_err().code, ErrorCode::Offline);
}

#[test]
fn the_reference_server_refuses_a_public_address() {
    let l = TcpListener::bind("0.0.0.0:0").unwrap();
    let served = Served { hello: settings_hello("x", FREE), settings: Some(Arc::new(FakeSettings::sample())), vault: None, flows: None, ctx: CallContext::remote("a", Transport::Relay, &[]) };
    assert_eq!(serve(l, Arc::new(served), Arc::new(AtomicBool::new(true))).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

fn run_sample(addr: &str, extra: &[&str]) -> (i32, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_iyakki-ui-sample")).arg(addr).args(extra).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).to_string())
}

#[test]
fn the_sample_ui_runs_as_its_own_process_against_the_fake_machine_through_the_client_alone() {
    let m = machine(&[Scope::SETTINGS_READ, Scope::SETTINGS_WRITE], FREE, |_| {});
    let (code, out) = run_sample(&m.addr, &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Machine test-machine speaks contract 0.2") && out.contains("display.dark = off") && out.contains("voice.speed = normal") && out.contains("security.vault_timeout = 15"), "{out}");
    let (code, out) = run_sample(&m.addr, &["--set", "display.dark", "on"]);
    assert!(code == 0 && out.contains("Set display.dark = on"), "{out}");
    let (_, out) = run_sample(&m.addr, &[]);
    assert!(out.contains("display.dark = on"), "the change was kept by the machine: {out}");
    // a refusal of the machine reaches the person in words
    let (code, out) = run_sample(&m.addr, &["--set", "security.vault_timeout", "30"]);
    assert!(code == 1 && out.contains("Refused:") && out.contains("core.local_only"), "{out}");
    let (code, out) = run_sample(&m.addr, &["--set", "voice.speed", "warp"]);
    assert!(code == 1 && out.contains("settings.bad_value"), "{out}");
    // a machine that only lets it read
    let ro = machine(&[Scope::SETTINGS_READ], FREE, |_| {});
    let (code, out) = run_sample(&ro.addr, &["--set", "display.dark", "on"]);
    assert!(code == 1 && out.contains("core.scope_missing"), "{out}");
    // a machine with no settings domain: a plain note, not a crash
    let none = machine(&[], FREE, |h| h.domains.clear());
    let (code, out) = run_sample(&none.addr, &[]);
    assert!(code == 0 && out.contains("no settings screen"), "{out}");
    // nobody listening
    let (code, out) = run_sample("127.0.0.1:1", &[]);
    assert!(code == 1 && out.contains("cannot connect"), "{out}");
    assert_eq!(Command::new(env!("CARGO_BIN_EXE_iyakki-ui-sample")).output().unwrap().status.code(), Some(2));
}

#[test]
fn the_sample_ui_and_the_client_name_no_other_iyakki_crate() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let sample = std::fs::read_to_string(dir.join("src/bin/iyakki-ui-sample.rs")).unwrap();
    for (i, l) in sample.lines().enumerate() {
        for bad in ["iyakki_core_api", "iyakki_core_fake", "iyakki_agent", "iyakki_shell", "iyakki_uikit", "iyakki_sec", "std::fs", "std::process::Command", "std::net::TcpStream"] {
            assert!(!l.contains(bad), "the sample UI line {} names {bad}", i + 1);
        }
    }
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let deps: Vec<&str> = manifest.split("[dev-dependencies]").next().unwrap().lines().filter(|l| l.starts_with("iyakki-")).collect();
    assert_eq!(deps.len(), 1, "{deps:?}");
    assert!(deps[0].starts_with("iyakki-core-api") && deps[0].contains("default-features = false"), "the one IYAKKI dependency is the contract types without the native parts: {deps:?}");
}

#[test]
fn vault_and_flows_go_over_the_transport_with_the_same_guards_and_no_secret_and_no_unlock_ever_crosses_it() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let vault = Arc::new(FakeVault::new("login-pw").with_secret("deploy-key", "S3CRET-VALUE", "deploy"));
    let flows = Arc::new(FakeFlows::sample());
    let served = Served {
        hello: settings_hello("m", FREE),
        settings: None,
        vault: Some(vault.clone()),
        flows: Some(flows.clone()),
        ctx: CallContext::remote("laptop", Transport::DevMode, &[Scope::VAULT_STATUS, Scope::VAULT_USE, Scope::FLOWS_READ, Scope::FLOWS_RUN, Scope::FLOWS_APPROVE]),
    };
    let s2 = stop.clone();
    let t = std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap());
    let r = Remote::new(Connection::tcp(addr.as_str(), Duration::from_secs(5)).unwrap());
    let (v, f) = (r.vault(), r.flows());
    // vault: status is readable, using a secret while locked is `locked`, and nothing a client sends can unlock it
    assert_eq!(v.status().unwrap(), VaultStatus { locked: true, entries: 1 });
    assert_eq!(v.use_secret("deploy", None).unwrap_err().code, ErrorCode::Locked);
    let mut raw = TcpStream::connect(&addr).unwrap();
    writeln!(raw, r#"{{"id":1,"call":"vault.unlock","args":{{"password":"login-pw"}}}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(&raw).read_line(&mut line).unwrap();
    assert!(line.contains("transport.unknown_call"), "{line}");
    assert!(v.status().unwrap().locked, "still locked");
    assert!(vault.audit().iter().any(|l| l.contains("refused: locked")));
    // flows: list, start (idempotent), approve, finish; a local-only approval is refused to this remote caller
    let names: Vec<String> = f.list().unwrap().into_iter().map(|i| i.id).collect();
    assert_eq!(names, ["tidy", "ask", "look"]);
    let key = IdempotencyKey::new("start-0001").unwrap();
    let run = f.start("ask", &key).unwrap();
    assert_eq!(f.start("ask", &key).unwrap(), run);
    assert_eq!(flows.runs_started(), 1, "a repeated start did not start a second run");
    assert_eq!(f.start("tidy", &key).unwrap_err().code, ErrorCode::Conflict);
    let st = f.status(&run).unwrap();
    assert_eq!(st.state, RunState::WaitingApproval);
    let ap = st.pending_approval.unwrap();
    assert_eq!(f.approve(&run, &ap.id, true).unwrap().state, RunState::Running);
    let look = f.start("look", &IdempotencyKey::new("start-0002").unwrap()).unwrap();
    let e = f.approve(&look, "a2", true).unwrap_err();
    assert_eq!((e.code, e.message_key.as_str()), (ErrorCode::NeedsConsent, "flows.approval_local_only"), "a prompt that only the person at the machine may answer");
    let b = f.events(Some(&run), Resume::After(0), 20).unwrap();
    assert!(b.events.iter().any(|e| matches!(e.body, FlowEvent::ApprovalAnswered { ref by, .. } if by == "laptop")));
    assert_eq!(f.stop(&look).unwrap().state, RunState::Stopped);
    assert_eq!(f.status(&RunId("run-none".into())).unwrap_err().message_key, "flows.unknown_run");
    // a domain the machine does not serve is `unsupported`, not a hang
    let e = r.settings().get(&ctx(), "display.dark").unwrap_err();
    assert_eq!((e.code, e.message_key.as_str()), (ErrorCode::Unsupported, "transport.no_domain"));
    stop.store(true, Ordering::Relaxed);
    t.join().unwrap();
}

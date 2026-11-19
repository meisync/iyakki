//! The remote objects over a real loopback socket against the reference server: a root object, a handle returned by a call and released on
//! drop, the answers of a lost core, and the refusal of an object that is not there. Test level: real transport, fake machine.

use std::io::Cursor;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::{MediaFacet, MediaPlay};
use iyakki_shared::media::{Check, QueueState};
use iyakki_ui_client::server::RpcServe;
use iyakki_core_api::contract::RateLimit;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAny, Rpc};

#[derive(Default)]
struct Player {
    queue: Mutex<QueueState>,
    played: Mutex<Vec<(String, String, String)>>,
}

impl MediaPlay for Player {
    fn play(&self, user: &str, label: &str, target: &str) -> Result<(), String> {
        if target.is_empty() {
            return Err("nothing to play".into());
        }
        self.played.lock().unwrap().push((user.into(), label.into(), target.into()));
        Ok(())
    }
    fn set_queue(&self, user: &str, items: Vec<String>, pos: usize, _active: bool) -> u64 {
        let mut q = self.queue.lock().unwrap();
        q.user = user.into();
        q.items = items;
        q.rev += 1;
        let _ = pos;
        q.rev
    }
    fn queue_state(&self) -> QueueState {
        self.queue.lock().unwrap().clone()
    }
    fn check(&self) -> Check {
        Check::Playing { user: "ann".into(), label: "song".into() }
    }
    fn now_playing(&self) -> Option<String> {
        self.played.lock().unwrap().last().map(|p| p.1.clone())
    }
}

struct Media(Arc<Player>);

impl MediaFacet for Media {
    fn player(&self) -> Arc<dyn MediaPlay> {
        self.0.clone()
    }
}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    host: Arc<Host>,
    player: Arc<Player>,
}

fn machine() -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let player = Arc::new(Player::default());
    let host = Arc::new(Host::new().with_root(Obj::MediaFacet(Arc::new(Media(player.clone())))));
    let served = Served {
        hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }),
        settings: None,
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        rpc: Some(host.clone()),
        appevents: None,
        ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread, host, player }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn media(rpc: &Rpc) -> Arc<dyn MediaFacet> {
    match rpc.root("MediaFacet").expect("the trait is generated") {
        RemoteAny::MediaFacet(m) => m,
        _ => panic!("the root of MediaFacet is a MediaFacet"),
    }
}

#[test]
fn a_handle_returned_by_a_call_is_a_remote_object_that_is_released_on_drop() {
    let m = machine();
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(5)).unwrap());
    let facet = media(&rpc);
    let player = facet.player();
    assert_eq!(m.host.held_total(), 1, "the core holds the one handle it handed out");

    // plain calls on the handle: arguments in, results out, the error text of the core as it is
    assert_eq!(player.play("ann", "song", "file:///a.mp3"), Ok(()));
    assert_eq!(player.play("ann", "song", ""), Err("nothing to play".to_string()));
    assert_eq!(m.player.played.lock().unwrap().as_slice(), &[("ann".to_string(), "song".to_string(), "file:///a.mp3".to_string())], "the call ran on the core's player");
    assert_eq!(player.now_playing().as_deref(), Some("song"));
    assert_eq!(player.set_queue("ann", vec!["a".into(), "b".into()], 0, true), 1);
    let q = player.queue_state();
    assert_eq!((q.user.as_str(), q.items.len(), q.rev), ("ann", 2, 1), "a structure comes back whole");
    assert_eq!(player.check(), Check::Playing { user: "ann".into(), label: "song".into() }, "an enum with fields comes back whole");
    assert!(!player.plays_mp3_in_process());

    drop(player);
    // the release is a call on the same connection: the next call is answered after it
    let second = facet.player();
    assert_eq!(second.now_target(), None);
    assert_eq!(m.host.held_total(), 1, "the first handle was released, the second is the one held now");
    drop(second);
    assert_eq!(facet.player().elapsed(), None);
    assert_eq!(m.host.held_total(), 0, "a temporary handle is released at the end of its statement");
}

#[test]
fn a_second_connection_cannot_use_the_handles_of_the_first() {
    let m = machine();
    let a = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(5)).unwrap());
    let b = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(5)).unwrap());
    let p = media(&a).player();
    assert_eq!(p.set_queue("ann", vec!["x".into()], 0, true), 1);
    // the same id on the other connection is refused as "no such object": the answer of a lost call, not the first connection's player
    let stolen: Result<u64, String> = b.call("MediaPlay", 1, "set_queue", serde_json::json!(["eve", ["y"], 0, true]));
    assert!(stolen.unwrap_err().contains("no MediaPlay object"), "an object belongs to its connection");
    assert_eq!(m.player.queue.lock().unwrap().user, "ann");
}

#[test]
fn a_connection_that_ends_frees_every_handle_it_held() {
    let host = Host::new().with_root(Obj::MediaFacet(Arc::new(Media(Arc::new(Player::default())))));
    let ctx = CallContext::remote("ui", Transport::LocalSocket, &[]);
    for _ in 0..3 {
        host.call(&ctx, 7, 0, "MediaFacet", "player", serde_json::json!([])).unwrap();
    }
    host.call(&ctx, 8, 0, "MediaFacet", "player", serde_json::json!([])).unwrap();
    assert_eq!((host.held(7), host.held(8)), (3, 1));
    host.release(7, None); // the connection ended (a UI that crashed never releases)
    assert_eq!((host.held(7), host.held(8)), (0, 1), "only that connection's objects are gone");
}

#[test]
fn a_lost_core_answers_every_call_with_the_quiet_value_and_never_panics() {
    // a connection whose other end is already closed
    let rpc = Rpc::new(Connection::new(Box::new(Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let facet = media(&rpc);
    let p = facet.player();
    assert!(p.play("a", "b", "c").unwrap_err().contains("closed"), "a result carries the reason");
    assert_eq!(p.now_playing(), None);
    assert_eq!(p.set_queue("a", vec![], 0, false), 0);
    assert_eq!(p.queue_state(), QueueState::default());
    assert_eq!(p.check(), Check::Idle);
}

#[test]
fn a_trait_the_machine_does_not_serve_is_refused_by_name() {
    let m = machine();
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(5)).unwrap());
    let r: Result<bool, String> = rpc.call("GeminiFacet", 0, "voice_enabled", serde_json::json!(["/x"]));
    assert!(r.unwrap_err().contains("no GeminiFacet object"));
    assert!(rpc.root("NoSuchTrait").is_none());
}

/// A chat run over the socket: the UI sends a request (data), polls events, answers an approval; the tool chain is the core's.
#[test]
fn a_chat_run_is_started_polled_and_decided_over_the_socket_and_only_by_its_user() {
    use iyakki_agent_api::{ChatRunTable, ChatRuns, FakeRunStarter, RunRequest};
    use iyakki_shared::agent::{RunEvent, Verdict};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let starter = Arc::new(FakeRunStarter::new(vec![RunEvent::Model { text: "hi".into(), calls: vec![] }]));
    let table: Arc<dyn ChatRuns> = Arc::new(ChatRunTable::new(starter.clone()));
    let host = Arc::new(Host::new().with_root(Obj::ChatRuns(table)));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, appevents: None, rpc: Some(host), ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let t = std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap());
    let rpc = Rpc::new(Connection::tcp(addr.as_str(), Duration::from_secs(5)).unwrap());
    let runs = match rpc.root("ChatRuns").expect("generated") {
        RemoteAny::ChatRuns(r) => r,
        _ => panic!("the root of ChatRuns is a ChatRuns"),
    };
    let id = runs.start(RunRequest { user: "ann".into(), prompt: "hello".into(), autopilot: true, ..Default::default() }).unwrap();
    assert_eq!(starter.requests.lock().unwrap()[0].prompt, "hello", "the core saw the request as data");
    assert!(runs.poll("eve", &id).is_empty());
    assert_eq!(runs.poll("ann", &id).len(), 1);
    assert!(runs.is_done("ann", &id));
    runs.decide("ann", &id, "c1", Verdict::Approve).unwrap();
    assert_eq!(starter.decided.lock().unwrap().len(), 1);
    assert!(runs.decide("eve", &id, "c2", Verdict::Approve).is_err());
    stop.store(true, Ordering::Relaxed);
    t.join().unwrap();
}

/// A concrete member store as a remote object: the real `Inbox` and `AuditLog` of the core served over the socket and used through their traits.
#[test]
fn member_stores_are_served_over_the_socket_and_the_ui_reads_what_the_core_holds() {
    use iyakki_agent_api::{AuditApi, InboxApi};
    use iyakki_shared::inbox::{Inbox, Kind};
    let dir = std::env::temp_dir().join(format!("iyakki-rpc-stores-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let inbox = Arc::new(Inbox::new(&dir.join("inbox")));
    let audit = Arc::new(iyakki_accounts::AuditLog::new(&dir.join("audit.log")));
    audit.record("ann", "test.event", "detail");
    inbox.add("ann", Kind::Agent, "Hello", "body", "test", None, 100).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let host = Arc::new(Host::new().with_root(Obj::InboxApi(inbox.clone())).with_root(Obj::AuditApi(audit.clone())));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, appevents: None, rpc: Some(host), ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let t = std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap());
    let rpc = Rpc::new(Connection::tcp(addr.as_str(), Duration::from_secs(5)).unwrap());
    let remote_inbox: Arc<dyn InboxApi> = match rpc.root("InboxApi").expect("generated") {
        RemoteAny::InboxApi(i) => i,
        _ => panic!("InboxApi root"),
    };
    let remote_audit: Arc<dyn AuditApi> = match rpc.root("AuditApi").expect("generated") {
        RemoteAny::AuditApi(a) => a,
        _ => panic!("AuditApi root"),
    };
    let listed = remote_inbox.list("ann");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Hello");
    // a write through the socket lands in the core's store
    remote_inbox.add("ann", Kind::Agent, "Second", "b", "test", None, 101).unwrap();
    assert_eq!(inbox.list("ann").len(), 2);
    assert!(remote_audit.recent(5).iter().any(|l| l.contains("test.event")));
    stop.store(true, Ordering::Relaxed);
    t.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The handle says the core is real only while it answers.
#[test]
fn the_remote_handle_is_real_only_while_the_core_answers() {
    let dir = std::env::temp_dir().join(format!("iyakki-rpc-handle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let inbox = Arc::new(iyakki_shared::inbox::Inbox::new(&dir));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let host = Arc::new(Host::new().with_root(Obj::InboxApi(inbox)));
    let served = Served { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, appevents: None, rpc: Some(host), ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let t = std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap());
    let h = iyakki_ui_rpc::RemoteServicesHandle::new(Rpc::new(Connection::tcp(addr.as_str(), Duration::from_secs(5)).unwrap()));
    assert!(h.is_real());
    assert!(h.inbox().list("ann").is_empty());
    // a handle onto a core that serves nothing is not real
    let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr2 = l2.local_addr().unwrap().to_string();
    let s3 = stop.clone();
    let served2 = Served { hello: settings_hello("rpc", RateLimit { calls: 100, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, appevents: None, rpc: Some(Arc::new(Host::new())), ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let t2 = std::thread::spawn(move || serve(l2, Arc::new(served2), s3).unwrap());
    let none = iyakki_ui_rpc::RemoteServicesHandle::new(Rpc::new(Connection::tcp(addr2.as_str(), Duration::from_secs(5)).unwrap()));
    assert!(!none.is_real(), "a core without the root is not the real services");
    stop.store(true, Ordering::Relaxed);
    t.join().unwrap();
    t2.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

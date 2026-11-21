//! The plain chat model, the picture and vision models of LAB, and the host of the machine, served over a real loopback socket against the reference
//! server (F-SPLIT-03, D-SPLIT-03-APPS): a turn is a polled job whose text arrives as it is written, a stop reaches the model, bytes bigger than a
//! line travel in chunks, and the host does only what the core's own host says it can. Test level: real transport, fake machine, no timing.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::host::WasmIo;
use iyakki_core_api::HostApi;
use iyakki_shared::chat::{ChatBackend, Message, ModelTurn, ToolCall, ToolSpec};
use iyakki_shared::lab::{CallTrace, PictureBackend, PictureRequest, VisionAnswer, VisionBackend, VisionRequest};
use iyakki_shared::providers::ImageTrace;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{remote_frame_source, serve_frame_source, FramePath, remote_chat_backend, remote_host, remote_picture_backend, remote_vision_backend, serve_chat_backend, serve_host, serve_picture_backend, serve_uploads, serve_vision_backend, Host, Rpc};

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn machine(host: Host) -> Machine {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served {
        hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }),
        settings: None,
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        rpc: Some(Arc::new(host)),
        appevents: None,
        ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn rpc(m: &Machine) -> Rpc {
    Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap())
}

fn user(text: &str) -> Message {
    Message::user(text)
}

// ---------------------------------------------------------------------------------------------------------------- chat

/// A model that echoes, writes its answer in pieces, asks for a tool when it is given tools, and (for "never ends") writes until it is told to stop.
#[derive(Default)]
struct Model {
    stopped: AtomicBool,
    seen: Mutex<Vec<String>>,
}
impl ChatBackend for Model {
    fn provider_name(&self) -> String {
        "test-provider".into()
    }
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        let last = history.last().map(|m| m.content.clone()).unwrap_or_default();
        self.seen.lock().unwrap().push(last.clone());
        if last == "fail" {
            return Err("the model is down".into());
        }
        Ok(format!("echo: {last}"))
    }
    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        match tools.first() {
            Some(t) => Ok(ModelTurn { text: String::new(), calls: vec![ToolCall { id: "c1".into(), name: t.name.clone(), args: serde_json::json!({"q": history.len()}) }] }),
            None => self.complete(history).map(|text| ModelTurn { text, calls: Vec::new() }),
        }
    }
    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        if history.last().is_some_and(|m| m.content == "never ends") {
            // writes until the UI says stop: the end is the stop, so nothing here waits for time
            let mut said = on_text("a");
            while said {
                said = on_text("");
            }
            self.stopped.store(true, Ordering::SeqCst);
            return Err("stopped".into());
        }
        for piece in ["Hel", "lo ", "there"] {
            if !on_text(piece) {
                return Err("stopped".into());
            }
        }
        self.complete_tools(history, tools).map(|mut t| {
            t.text = "Hello there".into();
            t
        })
    }
}

#[test]
fn the_chat_model_of_the_core_answers_over_the_socket_and_its_errors_arrive_as_they_are() {
    let model = Arc::new(Model::default());
    let m = machine(Host::new().with_root(serve_chat_backend(model.clone())));
    let chat = remote_chat_backend(&rpc(&m), 0);
    assert_eq!(chat.provider_name(), "test-provider");
    assert_eq!(chat.complete(&[user("hi")]), Ok("echo: hi".to_string()));
    assert_eq!(chat.complete(&[user("fail")]), Err("the model is down".to_string()));
    let spec = ToolSpec { name: "radio_play".into(), description: "Play a station.".into(), schema: serde_json::json!({"type": "object"}) };
    let turn = chat.complete_tools(&[user("play")], &[spec]).unwrap();
    assert_eq!((turn.text.as_str(), turn.calls.len(), turn.calls[0].name.as_str()), ("", 1, "radio_play"));
    // no tools: the plain answer
    assert_eq!(chat.complete_tools(&[user("hey")], &[]).unwrap().text, "echo: hey");
    assert_eq!(model.seen.lock().unwrap().as_slice(), ["hi", "fail", "hey"], "the core's own model answered every turn");
}

#[test]
fn the_text_of_a_turn_arrives_piece_by_piece_and_a_stop_reaches_the_model() {
    let model = Arc::new(Model::default());
    let m = machine(Host::new().with_root(serve_chat_backend(model.clone())));
    let chat = remote_chat_backend(&rpc(&m), 0);
    let mut got = String::new();
    let turn = chat.complete_tools_stream(&[user("tell")], &[], &mut |p| {
        got.push_str(p);
        true
    });
    assert_eq!((got.as_str(), turn.unwrap().text.as_str()), ("Hello there", "Hello there"));
    // the UI says stop at the first piece: the model's own `on_text` answers false, it ends, and the UI gets how it ended
    let mut pieces = 0;
    let r = chat.complete_tools_stream(&[user("never ends")], &[], &mut |_| {
        pieces += 1;
        false
    });
    assert_eq!(r, Err("stopped".to_string()));
    assert!(pieces >= 1);
    assert!(model.stopped.load(Ordering::SeqCst), "the stop reached the model on the core");
}

#[test]
fn a_chat_model_on_a_lost_core_says_why_and_never_panics() {
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let chat = remote_chat_backend(&rpc, 7);
    assert_eq!(chat.provider_name(), "");
    assert!(chat.complete(&[user("hi")]).is_err());
    assert!(chat.complete_tools_stream(&[user("hi")], &[], &mut |_| true).is_err());
}

// ---------------------------------------------------------------------------------------------------------------- LAB

struct Painter;
impl PictureBackend for Painter {
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace) {
        let trace = ImageTrace { provider: r.provider.clone(), model: r.model.clone(), width: r.width, height: r.height, status: Some(200), ..Default::default() };
        if r.prompt == "refuse" {
            return (Err("the server refused".into()), ImageTrace { error: "refused".into(), status: Some(500), ..trace });
        }
        // 150 000 bytes do not fit a line of the transport: they come as a download
        (Ok(((0..150_000u32).map(|i| (i % 251) as u8).collect(), "png")), trace)
    }
}

#[derive(Default)]
struct Looker {
    got: Mutex<Vec<Vec<u8>>>,
}
impl VisionBackend for Looker {
    fn ask(&self, r: &VisionRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> (Result<VisionAnswer, String>, CallTrace) {
        *self.got.lock().unwrap() = r.images.clone();
        if r.question == "until stopped" {
            let mut n = 0;
            while !cancel.load(Ordering::SeqCst) {
                on_text(if n == 0 { "so far" } else { "" });
                n += 1;
            }
            return (Err("stopped".into()), CallTrace { error: "stopped".into(), ..Default::default() });
        }
        on_text("It is ");
        on_text("a cat.");
        (Ok(VisionAnswer { text: "It is a cat.".into(), tokens: Some((12, 4)), streamed: true }), CallTrace { status: Some(200), request_bytes: r.images.iter().map(Vec::len).sum::<usize>() as u64, ..Default::default() })
    }
}

#[test]
fn a_picture_bigger_than_a_line_comes_whole_with_its_trace_and_a_refusal_keeps_the_servers_words() {
    let m = machine(Host::new().with_root(serve_picture_backend(Arc::new(Painter))));
    let painter = remote_picture_backend(&rpc(&m), 0);
    let req = PictureRequest { provider: "lemonade".into(), model: "sd".into(), prompt: "a fox".into(), negative: None, width: 512, height: 384, steps: Some(4), seed: 9 };
    let (r, trace) = painter.draw(&req);
    let (bytes, ext) = r.unwrap();
    assert_eq!((bytes.len(), ext, bytes[250], bytes[251]), (150_000, "png", 250, 0));
    assert_eq!((trace.provider.as_str(), trace.width, trace.status), ("lemonade", 512, Some(200)));
    let (r, trace) = painter.draw(&PictureRequest { prompt: "refuse".into(), ..req });
    assert_eq!((r.err().as_deref(), trace.status, trace.error.as_str()), (Some("the server refused"), Some(500), "refused"));
}

#[test]
fn the_pictures_of_a_question_go_up_whole_and_the_answer_is_read_as_it_is_written() {
    let looker = Arc::new(Looker::default());
    let m = machine(Host::new().with_root(serve_uploads()).with_root(serve_vision_backend(looker.clone())));
    let eyes = remote_vision_backend(&rpc(&m), 0);
    let big: Vec<u8> = (0..90_000u32).map(|i| (i % 253) as u8).collect();
    let req = VisionRequest { provider: "p".into(), model: "v".into(), question: "what is it".into(), images: vec![big.clone(), vec![1, 2, 3], Vec::new()] };
    let mut text = String::new();
    let (r, trace) = eyes.ask(&req, &AtomicBool::new(false), &mut |t| text.push_str(t));
    let a = r.unwrap();
    assert_eq!((a.text.as_str(), a.tokens, a.streamed, text.as_str()), ("It is a cat.", Some((12, 4)), true, "It is a cat."));
    assert_eq!(trace.request_bytes, 90_003);
    let got = looker.got.lock().unwrap();
    assert_eq!((got.len(), got[0] == big, got[1].as_slice(), got[2].len()), (3, true, &[1u8, 2, 3][..], 0), "the core got the pictures as the UI had them, cut where they were");
}

#[test]
fn a_stop_flag_of_the_caller_stops_the_vision_model_on_the_core() {
    let m = machine(Host::new().with_root(serve_vision_backend(Arc::new(Looker::default()))));
    let eyes = remote_vision_backend(&rpc(&m), 0);
    let stop = AtomicBool::new(false);
    let req = VisionRequest { provider: "p".into(), model: "v".into(), question: "until stopped".into(), images: vec![vec![9]] };
    let mut first = String::new();
    let (r, _) = eyes.ask(&req, &stop, &mut |t| {
        if first.is_empty() {
            first = t.to_string();
        }
        stop.store(true, Ordering::SeqCst);
    });
    assert_eq!((first.as_str(), r.err().as_deref()), ("so far", Some("stopped")));
}

// ---------------------------------------------------------------------------------------------------------------- host

/// A machine that can do everything and does nothing but note it.
struct Box_ {
    power: bool,
    sleeps: bool,
    log: Mutex<Vec<String>>,
}
impl HostApi for Box_ {
    fn detect_env(&self) -> String {
        "a test box".into()
    }
    fn net_status(&self) -> String {
        "online".into()
    }
    fn run_wasm(&self, user: &str, policy: &iyakki_sec::policy::Policy, wasm: &[u8], entry: &str) -> Result<String, String> {
        self.log.lock().unwrap().push(format!("run {user} {entry} {} bytes manage_users={}", wasm.len(), policy.manage_users));
        Ok(format!("ran {entry}"))
    }
    fn run_wasm_io(&self, user: &str, policy: &iyakki_sec::policy::Policy, wasm: &[u8], entry: &str, io: &WasmIo) -> Result<String, String> {
        if entry == "until stopped" {
            // the end is the stop flag, so nothing here waits for time
            while !io.cancel.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            return Err("stopped".into());
        }
        self.log.lock().unwrap().push(format!("input {}", String::from_utf8_lossy(&io.stdin)));
        self.run_wasm(user, policy, wasm, entry)
    }
    fn power_off(&self) {
        self.log.lock().unwrap().push("power_off".into());
    }
    fn reboot(&self) {
        self.log.lock().unwrap().push("reboot".into());
    }
    fn can_power(&self) -> bool {
        self.power
    }
    fn can_sleep(&self) -> bool {
        self.sleeps
    }
    fn sleep(&self) {
        self.log.lock().unwrap().push("sleep".into());
    }
    fn wasm_tasks(&self) -> Vec<iyakki_core_api::WasmTask> {
        vec![iyakki_core_api::WasmTask { id: 3, user: "ann".into(), entry: "main".into(), bytes: 42, started: 1000 }]
    }
}

fn accounts(tag: &str) -> (Arc<dyn iyakki_agent_api::AccountsApi>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("iyakki-served-apps-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = iyakki_accounts::UserStore::open(&dir.join("users.json")).unwrap();
    store.add_local("ann", "pw-ann-1", iyakki_sec::policy::Role::Admin).unwrap();
    store.add_local("guest", "pw-guest-1", iyakki_sec::policy::Role::Guest).unwrap();
    (Arc::new(Arc::new(Mutex::new(store))), dir)
}

fn audit() -> (iyakki_shared::gitkeys::AuditFn, Arc<Mutex<Vec<(String, String)>>>) {
    let lines: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let l2 = lines.clone();
    (Arc::new(move |u, e| l2.lock().unwrap().push((u.to_string(), e.to_string()))), lines)
}

#[test]
fn the_machine_is_switched_only_when_the_cores_host_can_and_every_change_is_in_the_cores_audit_log() {
    let (note, lines) = audit();
    let on = Arc::new(Box_ { power: true, sleeps: true, log: Default::default() });
    let off = Arc::new(Box_ { power: false, sleeps: false, log: Default::default() });
    let m_on = machine(Host::new().with_root(serve_host(on.clone(), None, note.clone())));
    let m_off = machine(Host::new().with_root(serve_host(off.clone(), None, note)));
    let (h_on, h_off) = (remote_host(&rpc(&m_on), 0), remote_host(&rpc(&m_off), 0));
    assert_eq!((h_on.detect_env().as_str(), h_on.net_status().as_str(), h_on.can_power(), h_on.can_sleep()), ("a test box", "online", true, true));
    assert_eq!((h_off.can_power(), h_off.can_sleep()), (false, false));
    h_on.power_off();
    h_on.reboot();
    h_on.sleep(); // flaky-ok: the host call that suspends the machine, nothing waits
    h_off.power_off();
    h_off.reboot();
    h_off.sleep(); // flaky-ok: the host call that suspends the machine, nothing waits
    assert_eq!(on.log.lock().unwrap().as_slice(), ["power_off", "reboot", "sleep"]);
    assert!(off.log.lock().unwrap().is_empty(), "a host that cannot is never asked to: the UI has no way around it");
    let said: Vec<String> = lines.lock().unwrap().iter().map(|(u, e)| format!("{u}: {e}")).collect();
    assert_eq!(said.len(), 6);
    assert!(said[0].contains("host.power_off") && said[3].contains("refused"), "{said:?}");
}

#[test]
fn a_webassembly_run_uses_the_policy_of_the_account_the_core_reads_and_gets_its_input_and_its_stop() {
    let (accounts, dir) = accounts("wasm");
    let (note, lines) = audit();
    let host = Arc::new(Box_ { power: false, sleeps: false, log: Default::default() });
    let m = machine(Host::new().with_root(serve_uploads()).with_root(serve_host(host.clone(), Some(accounts), note)));
    let h = remote_host(&rpc(&m), 0);
    // the UI names a policy of its own (the default: it says nothing); the core's account for "guest" is not an admin and "ann" is
    let named = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::Admin, &Default::default());
    let io = WasmIo { stdin: b"hello".to_vec(), cancel: Default::default() };
    assert_eq!(h.run_wasm_io("guest", &named, b"\0asm", "main", &io), Ok("ran main".to_string()));
    assert_eq!(h.run_wasm("ann", &named, &vec![7u8; 100_000], "start"), Ok("ran start".to_string()));
    assert_eq!(host.log.lock().unwrap().as_slice(), ["input hello", "run guest main 4 bytes manage_users=false", "input ", "run ann start 100000 bytes manage_users=true"], "a module bigger than a line arrives whole; the guest is a guest whatever the UI named");
    assert!(h.run_wasm("nobody", &named, b"\0asm", "main").unwrap_err().contains("no account nobody"));
    let cancel = Arc::new(AtomicBool::new(false));
    // the stop flag is set by another thread once the run was started: the run ends with the stop
    let flag = cancel.clone();
    let stopper = std::thread::spawn(move || flag.store(true, Ordering::SeqCst));
    let r = h.run_wasm_io("ann", &named, b"\0asm", "until stopped", &WasmIo { stdin: Vec::new(), cancel });
    stopper.join().unwrap();
    assert!(r.unwrap_err().contains("stopped"));
    let tasks = h.wasm_tasks();
    assert_eq!((tasks.len(), tasks[0].id, tasks[0].user.as_str(), tasks[0].bytes), (1, 3, "ann", 42));
    assert!(lines.lock().unwrap().iter().any(|(u, e)| u == "ann" && e.contains("host.run_wasm start (100000 bytes)")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_core_without_accounts_refuses_a_run_by_name_and_a_lost_core_answers_like_a_machine_that_cannot() {
    let (note, _) = audit();
    let m = machine(Host::new().with_root(serve_host(Arc::new(Box_ { power: true, sleeps: true, log: Default::default() }), None, note)));
    let h = remote_host(&rpc(&m), 0);
    let named = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::Admin, &Default::default());
    assert!(h.run_wasm("ann", &named, b"\0asm", "main").unwrap_err().contains("not served over the UI socket"));
    let lost = remote_host(&Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink()))), 0);
    assert_eq!((lost.can_power(), lost.can_sleep(), lost.detect_env().as_str(), lost.wasm_tasks().len()), (false, false, "", 0));
    lost.power_off();
    assert!(lost.run_wasm("ann", &named, b"\0asm", "main").is_err());
}

// ---------------------------------------------------------------------------------------------------------------- TALES

/// A core that answers only the two file-rule questions the story store is built from.
struct Rules {
    home: std::path::PathBuf,
}
impl iyakki_ui_rpc::Served for Rules {
    fn iface(&self) -> &'static str {
        "ServicesHandle"
    }
    fn serve(&self, _reg: &iyakki_ui_rpc::Registry<'_>, method: &str, a: serde_json::Value) -> Result<serde_json::Value, String> {
        match method {
            "scope_home_root" => Ok(serde_json::json!(self.home)),
            "policy_of" => Ok(serde_json::json!(iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::Admin, &Default::default()))),
            "scope_roots_with_home" => {
                let (_p, _u, _d, home): (serde_json::Value, String, String, std::path::PathBuf) = serde_json::from_value(a).map_err(|e| e.to_string())?;
                Ok(serde_json::json!({"home": home, "roots": [home], "mounts": []}))
            }
            other => Err(format!("not served: {other}")),
        }
    }
}

#[test]
fn the_story_store_is_built_in_the_ui_over_the_cores_home_and_roots_and_keeps_its_stories_there() {
    let home = std::env::temp_dir().join(format!("iyakki-served-apps-{}-tales", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    // the call that names a person is vouched against the core's accounts (D-SEC1-VOUCH): the core here has them
    struct NoFetch;
    impl iyakki_shared::store_catalog::Fetch for NoFetch {
        fn get(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
            Err("no web here".into())
        }
    }
    let (accounts, adir) = accounts("tales-accounts");
    let (note, _) = audit();
    let m = machine(Host::new().with_root(iyakki_ui_rpc::Obj::Served(Arc::new(Rules { home: home.clone() }))).with_store_side(Arc::new(NoFetch), note, accounts, None, adir.clone()));
    let Some(iyakki_ui_rpc::RemoteAny::ServicesFacetB5(b5)) = rpc(&m).root("ServicesFacetB5") else { panic!("the trait is generated") };
    let policy = iyakki_sec::policy::Policy::resolve(iyakki_sec::policy::Role::Admin, &Default::default());
    let store = b5.story_store(&policy, "ann", std::path::Path::new("/data")).expect("a core that answers the rules gives a store");
    assert_eq!(store.dir(), home.join("Stories"));
    let story = iyakki_shared::story::Story { title: "The fox".into(), blocks: vec![iyakki_shared::story::Block::Para("Once upon a time there was a fox.".into())], ..Default::default() };
    let entry = store.save(&story, &iyakki_shared::story::StoryOptions { topic: "a fox".into(), ..Default::default() }, 1_700_000_000).unwrap();
    assert_eq!(store.list().iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["The fox"]);
    assert!(store.dir().join(&entry.id).join("story.md").exists(), "the story is a folder under the person's home");
    // a write outside the roots the core gave is refused by the roots, not by the UI's goodwill
    assert!(store.roots.contains(&home) && !store.roots.contains(&home.parent().unwrap().join("elsewhere")));
    // a core that does not answer the rules gives no store (the tile says so)
    let lost = remote_any_lost();
    assert!(lost.story_store(&policy, "ann", std::path::Path::new("/data")).is_none());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&adir);
}

fn remote_any_lost() -> Arc<dyn iyakki_agent_api::ServicesFacetB5> {
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    match rpc.root("ServicesFacetB5") {
        Some(iyakki_ui_rpc::RemoteAny::ServicesFacetB5(b)) => b,
        _ => panic!("the trait is generated"),
    }
}

// ---------------------------------------------------------------------------------------------------------------- video frames

/// A frame source that notes the files it was asked for, makes a frame from the time asked and, for a time of -1, waits for the cancel flag.
#[derive(Default)]
struct Frames {
    asked: Mutex<Vec<String>>,
}
impl iyakki_shared::lab::FrameSource for Frames {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn info(&self, file: &str) -> Result<iyakki_shared::lab::VideoInfo, String> {
        self.asked.lock().unwrap().push(format!("info {file}"));
        Ok(iyakki_shared::lab::VideoInfo { duration: 12.5, size: (64, 48) })
    }
    fn frame_rgba(&self, file: &str, t: f64, size: (u32, u32), cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        self.asked.lock().unwrap().push(format!("frame {file} {t}"));
        if t < 0.0 {
            while !cancel.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            return Err("stopped".into());
        }
        Ok((0..size.0 as usize * size.1 as usize * 4).map(|i| (i % 251) as u8).collect())
    }
}

fn only_videos() -> FramePath {
    Arc::new(|file: &str| if file.starts_with("/home/ann/") { Ok(file.replace("/home/ann/", "/data/home/ann/")) } else { Err(format!("outside the folders your account may read: {file}")) })
}

#[test]
fn a_frame_bigger_than_a_line_comes_whole_the_file_is_named_by_the_cores_rule_and_a_refused_file_runs_nothing() {
    let frames = Arc::new(Frames::default());
    let m = machine(Host::new().with_root(serve_frame_source(frames.clone(), only_videos())));
    let src = remote_frame_source(&rpc(&m), 0);
    assert_eq!(src.available(), Ok(()));
    let info = src.info("/home/ann/clip.mp4").unwrap();
    assert_eq!((info.duration, info.size), (12.5, (64, 48)));
    let frame = src.frame_rgba("/home/ann/clip.mp4", 3.0, (320, 200), &Arc::new(AtomicBool::new(false))).unwrap();
    assert_eq!((frame.len(), frame[250], frame[251]), (320 * 200 * 4, 250, 0));
    // a file outside the person's folders is refused by the core's rule and the source never hears of it
    assert!(src.info("/etc/shadow").unwrap_err().contains("outside the folders"));
    assert!(src.frame_rgba("/etc/shadow", 0.0, (2, 2), &Arc::new(AtomicBool::new(false))).unwrap_err().contains("outside the folders"));
    assert_eq!(frames.asked.lock().unwrap().as_slice(), ["info /data/home/ann/clip.mp4", "frame /data/home/ann/clip.mp4 3"]);
}

#[test]
fn a_stop_flag_of_the_caller_stops_the_frame_on_the_core() {
    let m = machine(Host::new().with_root(serve_frame_source(Arc::new(Frames::default()), only_videos())));
    let src = remote_frame_source(&rpc(&m), 0);
    let stop = Arc::new(AtomicBool::new(true));
    assert_eq!(src.frame_rgba("/home/ann/clip.mp4", -1.0, (2, 2), &stop).unwrap_err(), "stopped");
}

#[test]
fn a_lost_core_says_why_frames_cannot_be_taken_and_never_panics() {
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let src = remote_frame_source(&rpc, 7);
    assert!(src.available().is_err());
    assert!(src.info("/home/ann/clip.mp4").is_err());
    assert!(src.frame_rgba("/home/ann/clip.mp4", 0.0, (2, 2), &Arc::new(AtomicBool::new(false))).is_err());
}

//! The running tile apps of the store over the core socket (F-SPLIT-03, D-SPLIT-03-TILES), over a real loopback socket against the reference
//! server. What the tests pin: the core runs the app (an HTML page in a fake page engine, a WebAssembly tile in a fake runner) and the UI only
//! draws what comes back; the page model and the display list cross intact; a click names a control of the page the core itself handed out; the
//! entry that runs is the core's own listing, never what the UI describes; the web object of the UI is never used, the core's own is; a person the
//! core does not know is refused. Real transport, fakes, no timing.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_agent_api::{StoreApi, StorePublish, TileEmbed, TileEmbedData, TileRt, TileTake, WasmTileRt};
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::FileScope;
use iyakki_sec::policy::Policy;
use iyakki_shared::skills::Scope;
use iyakki_shared::store_catalog::Fetch;
use iyakki_shared::store_tile::Request;
use iyakki_shared::store_tilehub::{TileEntry, DEVELOPER_TRUST};
use iyakki_tile_abi::{Color, DisplayList, Event, Op, Rect};
use iyakki_tile_host::ImageTable;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope as WireScope, Transport};
use iyakki_ui_rpc::{Host, Obj, RemoteAny, Rpc};
use iyakki_web::Doc;

fn entry(key: &str, name: &str, dir: Option<PathBuf>) -> TileEntry {
    TileEntry { key: key.into(), name: name.into(), title: name.into(), icon: String::new(), icon_picture: None, group: String::new(), version: "1".into(), publisher: "core".into(), trust: "Signed".into(), dev: false, dir, scope: Scope::Shared, caps: Default::default(), wasm: false }
}

fn page() -> Doc {
    let html = "<h1>Counter</h1><p>clicks: <b>0</b> <a href=\"/more\">more</a></p><form action=\"/go\"><input name=\"q\" value=\"x\"><select name=\"s\"><option value=\"a\">A</option><option value=\"b\" selected>B</option></select><button>Go</button></form><table><tr><td>1</td><td>2</td></tr></table><pre>fixed</pre><hr>";
    iyakki_web::doc::build(&iyakki_web::html::parse(html))
}

/// The page engine of the test: it notes every call and hands out the same page.
struct FakeRt {
    log: Arc<Mutex<Vec<String>>>,
    web_answer: String,
}
impl TileRt for FakeRt {
    fn page_text(&mut self) -> String {
        "Counter clicks: 0".into()
    }
    fn doc(&mut self) -> Doc {
        page()
    }
    fn pump(&mut self) -> bool {
        self.log.lock().unwrap().push("pump".into());
        true
    }
    fn resolve(&mut self, id: u32, ok: bool, value: &str) {
        self.log.lock().unwrap().push(format!("resolve {id} {ok} {value}"));
    }
    fn resolve_open(&mut self, id: u32, json: &str) {
        self.log.lock().unwrap().push(format!("resolve_open {id} {json}"));
    }
    fn click_control(&mut self, doc: &Doc, idx: usize) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("click_control {idx} on_core_page={}", *doc == page()));
        if idx < doc.controls.len() {
            Ok(())
        } else {
            Err("no such control".into())
        }
    }
    fn type_into(&mut self, doc: &Doc, idx: usize, text: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("type_into {idx} {text} on_core_page={}", *doc == page()));
        Ok(())
    }
    fn page_input(&mut self, node: usize, value: &str) -> bool {
        self.log.lock().unwrap().push(format!("page_input {node} {value}"));
        true
    }
    fn advance(&mut self, ms: u64) -> bool {
        self.log.lock().unwrap().push(format!("advance {ms}"));
        false
    }
    fn answer_sdk(&mut self, id: u32, allow: bool) -> bool {
        self.log.lock().unwrap().push(format!("answer_sdk {id} {allow}"));
        true
    }
    fn console(&self) -> Vec<(String, String)> {
        vec![("log".into(), self.web_answer.clone())]
    }
    fn page_notes(&self) -> Vec<String> {
        vec!["a note".into()]
    }
    fn take(&mut self) -> TileTake {
        TileTake { toasts: vec!["saved".into()], title: Some("Counter".into()), notices: vec![("n".into(), "m".into())], requests: vec![Request { id: 4, kind: "clipboard.read".into(), args: serde_json::json!({"a": 1}) }], audit: vec![("tile.save".into(), "x".into())] }
    }
    fn drop_request(&mut self, id: u32) {
        self.log.lock().unwrap().push(format!("drop_request {id}"));
    }
    fn refused(&self) -> Vec<String> {
        vec!["net refused".into()]
    }
    fn sdk_summary(&self) -> Option<String> {
        Some("model on".into())
    }
    fn sdk_embeds(&self) -> Vec<TileEmbed> {
        vec![TileEmbed { id: 2, kind: "chart".into(), target_json: "{}".into(), text: "t".into(), hosts: vec!["h".into()], profile: "p".into(), options_json: "{}".into() }]
    }
    fn sdk_embed_calls(&mut self, id: u32) -> Vec<(String, String)> {
        vec![(format!("call{id}"), "[]".into())]
    }
    fn sdk_embed_data(&mut self, id: u32) -> Option<TileEmbedData> {
        (id == 2).then(|| TileEmbedData { heading: "H".into(), lines: vec!["l".into()], progress: Some((1, 2)) })
    }
}

/// A WebAssembly tile of the test: it counts events and draws a number of rectangles equal to the count.
struct FakeWasm {
    log: Arc<Mutex<Vec<String>>>,
    events: usize,
}
impl WasmTileRt for FakeWasm {
    fn init(&mut self, w: i32, h: i32, scale_pct: u32, theme: Event) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("init {w}x{h} {scale_pct} {theme:?}"));
        Ok(())
    }
    fn event(&mut self, events: &[Event]) -> Result<bool, String> {
        self.log.lock().unwrap().push(format!("event {events:?}"));
        self.events += events.len();
        if events.iter().any(|e| matches!(e, Event::Key { code: 0, .. })) {
            return Err("the tile ran out of fuel".into());
        }
        Ok(true)
    }
    fn click(&mut self, x: i32, _y: i32) -> Result<Option<u32>, String> {
        Ok((x < 10).then_some(7))
    }
    fn frame(&mut self) -> Result<DisplayList, String> {
        let mut l = DisplayList::new();
        for i in 0..self.events as i32 {
            l.push(Op::Rect { r: Rect::new(i * 4, 0, 3, 3), color: Color(1, 2, 3, 255) });
        }
        l.push(Op::Text { x: 1, y: 2, face: 0, size: 12, color: Color(9, 9, 9, 255), text: "tile".into() });
        Ok(l)
    }
    fn requests(&self) -> Vec<(u32, String, String)> {
        vec![(3, "sdk.approve".into(), "{}".into())]
    }
    fn answer(&self, id: u32, allow: bool) -> bool {
        self.log.lock().unwrap().push(format!("answer {id} {allow}"));
        true
    }
    fn take_dock_open(&self) -> Option<String> {
        Some("edit".into())
    }
    fn takes_app_events(&self) -> bool {
        true
    }
}

struct FakeStore {
    log: Arc<Mutex<Vec<String>>>,
    seen: Mutex<Vec<String>>,
}
impl StorePublish for FakeStore {
    fn tile_listed(&self, _policy: &Policy, user: &str, key: &str) -> Option<TileEntry> {
        (key == "tile/shared/counter").then(|| entry(key, "counter", None)).filter(|_| user != "nobody")
    }
    fn tile_open(&self, _policy: &Policy, user: &str, e: &TileEntry, web: Arc<dyn iyakki_web::Fetch>) -> Result<Box<dyn TileRt>, String> {
        // what the page engine would fetch goes through the web object it is given: the core's own
        let got = web.get(&iyakki_web::url::Url::parse("https://app.test/data.json").unwrap());
        self.seen.lock().unwrap().push(format!("tile_open {user} {} dev={} trust={} dir={:?}", e.name, e.dev, e.trust, e.dir.as_ref().map(|d| d.file_name().unwrap().to_string_lossy().into_owned())));
        Ok(Box::new(FakeRt { log: self.log.clone(), web_answer: got.map(|r| String::from_utf8_lossy(&r.body).into_owned()).unwrap_or_else(|e| e) }))
    }
    fn tile_open_wasm(&self, _policy: &Policy, user: &str, e: &TileEntry) -> Result<Box<dyn WasmTileRt>, String> {
        self.seen.lock().unwrap().push(format!("tile_open_wasm {user} {}", e.name));
        if e.name == "off" {
            return Err("WebAssembly tiles are off".into());
        }
        Ok(Box::new(FakeWasm { log: self.log.clone(), events: 0 }))
    }
    fn tile_pictures(&self, _policy: &Policy, _user: &str, e: &TileEntry) -> Result<ImageTable, String> {
        let mut t = ImageTable::new();
        if e.name == "pics" {
            t.put(1, 2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).map_err(|e| e.to_string())?;
            t.put(9, 1, 1, vec![1, 2, 3, 4]).map_err(|e| e.to_string())?;
        }
        Ok(t)
    }
}
impl StoreApi for FakeStore {
    fn plan_remove(&self, _user: &str, _lives: &[iyakki_shared::store_lifecycle::Life], _life: &iyakki_shared::store_lifecycle::Life) -> iyakki_shared::store_lifecycle::RemovePlan {
        panic!("not used")
    }
    fn review_sideload(&self, _actor: &iyakki_shared::store_install::Actor, _bytes: Vec<u8>, _scope: Scope, _known: &[(String, String)]) -> iyakki_shared::store_install::Review {
        panic!("not used")
    }
}

/// The core's own web object: it answers from a fixed table and notes the addresses.
struct CoreWeb(Arc<Mutex<Vec<String>>>);
impl iyakki_web::Fetch for CoreWeb {
    fn get(&self, url: &iyakki_web::url::Url) -> Result<iyakki_web::fetch::Response, String> {
        self.0.lock().unwrap().push(url.to_string());
        Ok(iyakki_web::fetch::Response { status: 200, body: b"core-web".to_vec(), ..Default::default() })
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
    #[allow(dead_code)] // the machine keeps the log its audit sink writes to; other tests read it
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

fn machine(tag: &str, store: Arc<FakeStore>, web_log: Arc<Mutex<Vec<String>>>) -> Machine {
    let data = std::env::temp_dir().join(format!("iyakki-served-tiles-{}-{tag}", std::process::id()));
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
    let lines2 = web_log.clone();
    let host = Host::new().with_root(Obj::StoreApi(store)).with_store_side(Arc::new(Table), audit, accounts, Some(Arc::new(Rules)), data.clone()).with_web_fetch(Arc::new(CoreWeb(lines2.clone())));
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


fn policy() -> Policy {
    Policy::resolve(iyakki_sec::policy::Role::Guest, &Default::default())
}

/// The UI's own web object: it must never be used.
struct UiWeb;
impl iyakki_web::Fetch for UiWeb {
    fn get(&self, _: &iyakki_web::url::Url) -> Result<iyakki_web::fetch::Response, String> {
        panic!("the UI's web object must not be used")
    }
}

struct Rig {
    m: Machine,
    store: Arc<dyn StoreApi>,
    fake: Arc<FakeStore>,
    log: Arc<Mutex<Vec<String>>>,
    web: Arc<Mutex<Vec<String>>>,
}

fn rig(tag: &str) -> Rig {
    let log: Arc<Mutex<Vec<String>>> = Default::default();
    let web: Arc<Mutex<Vec<String>>> = Default::default();
    let fake = Arc::new(FakeStore { log: log.clone(), seen: Default::default() });
    let m = machine(tag, fake.clone(), web.clone());
    let store = remote(&m);
    Rig { m, store, fake, log, web }
}

#[test]
fn an_html_app_runs_in_the_core_and_the_page_model_crosses_intact() {
    let r = rig("html");
    let mut rt = r.store.tile_open(&policy(), "ann", &entry("tile/shared/counter", "counter", None), Arc::new(UiWeb)).unwrap();
    // the core's own web object served the page engine, the UI's was never called
    assert_eq!(rt.console(), vec![("log".to_string(), "core-web".to_string())]);
    assert_eq!(r.web.lock().unwrap().as_slice(), ["https://app.test/data.json"]);
    assert_eq!(rt.page_text(), "Counter clicks: 0");
    let doc = rt.doc();
    assert_eq!(doc, page(), "the page model is the same on both sides");
    assert!(!doc.controls.is_empty() && !doc.links.is_empty() && !doc.forms.is_empty(), "a page with a control, a link and a form");
    assert_eq!(rt.page_notes(), vec!["a note".to_string()]);
    assert_eq!(rt.refused(), vec!["net refused".to_string()]);
    assert_eq!(rt.sdk_summary().as_deref(), Some("model on"));
    assert_eq!(rt.sdk_embeds().len(), 1);
    assert_eq!(rt.sdk_embed_calls(2), vec![("call2".to_string(), "[]".to_string())]);
    assert_eq!(rt.sdk_embed_data(2).unwrap().progress, Some((1, 2)));
    assert!(rt.sdk_embed_data(3).is_none());
    let t = rt.take();
    assert_eq!((t.toasts, t.title, t.requests.len(), t.requests[0].args["a"].clone()), (vec!["saved".to_string()], Some("Counter".to_string()), 1, serde_json::json!(1)));
    assert_eq!(t.audit, vec![("tile.save".to_string(), "x".to_string())]);
}

#[test]
fn a_click_names_a_control_of_the_page_the_core_handed_out_never_a_page_the_ui_sends() {
    let r = rig("click");
    let mut rt = r.store.tile_open(&policy(), "ann", &entry("tile/shared/counter", "counter", None), Arc::new(UiWeb)).unwrap();
    // before the page was read there is nothing to click on
    let none = Doc::default();
    assert!(rt.click_control(&none, 0).unwrap_err().contains("page was not read"));
    let doc = rt.doc();
    // the UI sends an empty page: the core uses its own copy
    rt.click_control(&none, 0).unwrap();
    rt.type_into(&none, 1, "hello").unwrap();
    assert!(rt.click_control(&doc, 99).unwrap_err().contains("no such control"));
    rt.resolve(5, true, "yes");
    rt.resolve_open(6, "{}");
    assert!(rt.page_input(3, "b"));
    assert!(!rt.advance(16));
    assert!(rt.answer_sdk(8, false));
    assert!(rt.pump());
    rt.drop_request(4);
    let log = r.log.lock().unwrap().clone();
    assert_eq!(log, ["click_control 0 on_core_page=true", "type_into 1 hello on_core_page=true", "click_control 99 on_core_page=true", "resolve 5 true yes", "resolve_open 6 {}", "page_input 3 b", "advance 16", "answer_sdk 8 false", "pump", "drop_request 4"]);
}

#[test]
fn the_entry_that_runs_is_the_cores_own_listing_and_a_person_or_folder_it_does_not_know_is_refused() {
    let r = rig("entry");
    // a claim that dresses a listed app as a developer app with a folder of the UI's choosing: the listing wins
    let mut claim = entry("tile/shared/counter", "counter", Some(PathBuf::from("/etc")));
    claim.dev = true;
    claim.trust = DEVELOPER_TRUST.into();
    assert!(r.store.tile_open(&policy(), "ann", &claim, Arc::new(UiWeb)).is_ok());
    assert_eq!(r.fake.seen.lock().unwrap().last().unwrap(), "tile_open ann counter dev=false trust=Signed dir=None");
    // a person the core does not know is never served
    let e = r.store.tile_open(&policy(), "mallory", &claim, Arc::new(UiWeb)).err().unwrap();
    assert!(e.contains("not served over the UI socket"), "{e}");
    // an app that is not listed and names no folder is gone
    assert!(r.store.tile_open(&policy(), "ann", &entry("tile/shared/other", "other", None), Arc::new(UiWeb)).err().unwrap().contains("no longer installed"));
    // an app that is not listed and names a folder no account may read is refused
    assert!(r.store.tile_open(&policy(), "ann", &entry("dev/x", "x", Some(PathBuf::from("/etc"))), Arc::new(UiWeb)).err().unwrap().contains("outside the folders"));
    // a name that leaves the app's own folder is refused
    let e = r.store.tile_open(&policy(), "ann", &entry("dev/x", "../x", Some(r.m.data.join("home/ann/project"))), Arc::new(UiWeb)).err().unwrap();
    assert!(e.contains("no usable name"), "{e}");
    // a folder an account may read runs as a developer app, whatever the UI called it
    let mut dev = entry("dev/mine", "mine", Some(r.m.data.join("home/ann/project")));
    dev.trust = "Signed".into();
    dev.dev = false;
    assert!(r.store.tile_open(&policy(), "ann", &dev, Arc::new(UiWeb)).is_ok());
    assert_eq!(r.fake.seen.lock().unwrap().last().unwrap(), "tile_open ann mine dev=true trust=Developer (local) dir=Some(\"project\")");
    assert!(r.web.lock().unwrap().len() >= 2);
}

#[test]
fn a_webassembly_app_runs_in_the_core_and_the_ui_gets_the_display_lists() {
    let r = rig("wasm");
    let mut w = r.store.tile_open_wasm(&policy(), "ann", &entry("tile/shared/counter", "counter", None)).unwrap();
    let theme = Event::Theme { dark: true, accent: Color(1, 2, 3, 4), background: Color(5, 6, 7, 8), foreground: Color(9, 10, 11, 12) };
    w.init(320, 200, 100, theme.clone()).unwrap();
    assert!(w.takes_app_events());
    assert!(!w.event(&[]).unwrap(), "nothing is sent for no events");
    let events = [Event::PointerDown { x: 3, y: 4, button: 0 }, Event::TextInput { text: "héllo".into() }, Event::App { seq: 2, late: false, replayed: true, name: "n".into(), payload: "{}".into() }];
    assert!(w.event(&events).unwrap());
    let list = w.frame().unwrap();
    assert_eq!(list.ops.len(), 4, "one rectangle for each of the three events and the text");
    assert!(matches!(&list.ops[3], Op::Text { text, .. } if text == "tile"));
    assert_eq!(w.click(2, 2).unwrap(), Some(7));
    assert_eq!(w.click(50, 2).unwrap(), None);
    assert_eq!(w.requests(), vec![(3, "sdk.approve".to_string(), "{}".to_string())]);
    assert!(w.answer(3, true));
    assert_eq!(w.take_dock_open().as_deref(), Some("edit"));
    // a tile that fails says why and the UI stops it
    assert!(w.event(&[Event::Key { code: 0, mods: 0, down: true }]).unwrap_err().contains("ran out of fuel"));
    let log = r.log.lock().unwrap().clone();
    assert!(log[0].starts_with("init 320x200 100 Theme"), "{log:?}");
    assert!(log[1].starts_with("event [PointerDown"), "{log:?}");
    assert_eq!(log[2], "answer 3 true");
    // a tile the core refuses to start says why in the core's words
    assert!(r.store.tile_open_wasm(&policy(), "ann", &entry("tile/shared/counter", "off", None)).is_ok());
}

#[test]
fn a_tile_the_core_will_not_open_answers_the_cores_words_and_the_pictures_cross_as_pixels() {
    let r = rig("pics");
    // the listing gives the entry, the fake answers by the listed name: the claim's name is ignored for a listed key
    let t = r.store.tile_pictures(&policy(), "ann", &entry("tile/shared/counter", "pics", None)).unwrap();
    assert!(t.is_empty(), "the listed app is `counter`, which has no pictures");
    let dev = entry("dev/pics", "pics", Some(r.m.data.join("home/ann/project")));
    let t = r.store.tile_pictures(&policy(), "ann", &dev).unwrap();
    assert_eq!(t.handles(), vec![1, 9]);
    assert_eq!((t.get(1).unwrap().w, t.get(1).unwrap().h, t.get(1).unwrap().rgba.clone()), (2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]));
    assert_eq!(t.get(9).unwrap().rgba, vec![1, 2, 3, 4]);
    // an unknown person gets no pictures and a reason
    assert!(r.store.tile_pictures(&policy(), "mallory", &dev).unwrap_err().contains("not served over the UI socket"));
    let off = entry("dev/off", "off", Some(r.m.data.join("home/ann/project")));
    assert_eq!(r.store.tile_open_wasm(&policy(), "ann", &off).err().unwrap(), "WebAssembly tiles are off");
}

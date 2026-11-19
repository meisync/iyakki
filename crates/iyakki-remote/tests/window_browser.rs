//! `iyakki-connect window` end to end: the program's real browser start (installed Chrome in application mode, a private profile, the start
//! page that sends it on to the loopback address; made headless by the extra arguments the test appends) against the session endpoint of the
//! instance over REAL TLS on the loopback with a fake screen. The link is made the way Settings makes it (with the certificate pin in it), read
//! and pinned by the program, the page is the instance's own, and the device key stays in the program:
//! * first run: the link enrols the device (the key is the program's, never the browser's), wrong password, sign in, the picture, a key and a
//!   click reach the screen; the browser's storage holds no key; the instance lists the device with the program's public key;
//! * second run, by the saved name and no link: the program signs the challenge for the page, sign in, the picture again;
//! * a revoke on the instance closes the session and the program forgets the device (record and key file).
//!
//! Ignored by default because it needs `google-chrome` and `node` (22 or newer), like the other browser tests.
//! Run: cargo test -p iyakki-remote --test window_browser -- --ignored --nocapture
//! NOT covered: Firefox, Safari, Edge, macOS, Windows, a real network, hardware.

use iyakki_agent::clientlink as link;
use iyakki_connect::browser;
use iyakki_connect::instances::Instances;
use iyakki_core_api::appcaps::Approval;
use iyakki_core_api::broker::{ApprovalLookup, Broker, CardAnswer, CardAsk, Caller, MemoryLog};
use iyakki_remote::clientsession::{CallHandler, ClientEndpoint, FailLimit};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const W: u32 = 160;
const H: u32 = 100;

struct Pic {
    inputs: Vec<String>,
}

struct Fake {
    pic: Arc<Mutex<Pic>>,
    armed: bool,
    sent: bool,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.pic.lock().unwrap().inputs.push(format!("{i:?}"));
    }
    fn viewing(&mut self, _on: bool) {}
    fn step(&mut self) -> Option<Frame> {
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        if self.armed && !self.sent {
            self.sent = true;
            let rgba = (0..W * H).flat_map(|i| if i % W < W / 2 { [10u8, 20, 30, 255] } else { [240u8, 140, 20, 255] }).collect();
            return Some(Pixels { w: W, h: H, rgba });
        }
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate {
    pic: Arc<Mutex<Pic>>,
    ep: Arc<ClientEndpoint>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "Correct-Horse-Battery-9" {
            Ok(Box::new(Fake { pic: self.pic.clone(), armed: false, sent: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn client_endpoint(&self) -> Option<&ClientEndpoint> {
        Some(&self.ep)
    }
}

struct Book;
impl ApprovalLookup for Book {
    fn approval(&self, _: &str) -> Option<Approval> {
        None
    }
}
struct NoAsk;
impl CardAsk for NoAsk {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        CardAnswer::Unavailable
    }
}
struct Calls;
impl CallHandler for Calls {
    fn role_allows(&self, _: &str, _: &str) -> bool {
        true
    }
    fn perform(&self, _: &str, c: &str, _: Option<&str>) -> Result<String, String> {
        Ok(c.into())
    }
}

/// Drives the browser the program started (it listens on the debugging port the test gave it).
const DRIVER: &str = r##"const [PORT, MODE] = process.argv.slice(2);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { mode: MODE, requests: [], logs: [], errors: [] };
const done = (code) => { console.log("RESULT " + JSON.stringify(result)); setTimeout(() => process.exit(code), 100); };
process.on("uncaughtException", (e) => { result.errors.push("driver: " + e.message); done(1); });
let tab; for (let i = 0; i < 150 && !tab; i++) { try { const tabs = await (await fetch("http://127.0.0.1:" + PORT + "/json")).json(); tab = tabs.find((t) => t.type === "page" && /^http:\/\/127\.0\.0\.1:\d+\/connect/.test(t.url)); } catch (e) {} if (!tab) await sleep(200); }
if (!tab) { result.errors.push("the application window never reached the loopback page"); done(1); }
const ws = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pending = new Map();
ws.onmessage = (m) => {
  const j = JSON.parse(m.data);
  if (j.id && pending.has(j.id)) return pending.get(j.id)(j);
  if (j.method === "Network.requestWillBeSent") result.requests.push(j.params.request.url);
  if (j.method === "Network.webSocketCreated") result.requests.push(j.params.url);
  if (j.method === "Log.entryAdded") result.logs.push(j.params.entry.level + ": " + j.params.entry.text);
  if (j.method === "Runtime.consoleAPICalled") result.logs.push("console." + j.params.type + ": " + j.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
  if (j.method === "Runtime.exceptionThrown") result.logs.push("exception: " + JSON.stringify(j.params.exceptionDetails.exception?.description ?? j.params.exceptionDetails.text));
};
const call = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => { const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); if (r.result.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.result.exceptionDetails.exception?.description)); return r.result.result.value; };
const waitFor = async (expr, what, ms = 25000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await ev(expr)) return; } catch (e) {} await sleep(100); } throw new Error("timed out waiting for " + what + "; status=" + (await ev("document.getElementById('status').textContent + ' | ' + document.getElementById('ptitle').textContent").catch(() => "?")) + "; page=" + (await ev("document.documentElement.outerHTML.slice(0, 300)").catch(() => "?"))); };
const signin = (pw) => ev("document.getElementById('user').value = 'ann'; document.getElementById('pass').value = '" + pw + "'; document.getElementById('signin').requestSubmit(); true");
try {
  await call("Network.enable"); await call("Log.enable"); await call("Runtime.enable");
  result.resources = await ev("performance.getEntriesByType('resource').map((e) => e.name)");
  result.url = await ev("location.href");
  if (MODE === "enrol") {
    await waitFor("!document.getElementById('enrol').hidden", "the enrol form");
    result.hashAfterLoad = await ev("location.hash");
    result.keyinfo = await ev("document.getElementById('keyinfo').textContent");
    await ev("document.getElementById('devname').value = \"Maya's window\"; document.getElementById('enrol').requestSubmit(); true");
    await waitFor("!document.getElementById('signin').hidden", "the sign-in form");
    await signin("wrong-password");
    await waitFor("document.getElementById('status').textContent.includes('Wrong name or password')", "the wrong password message");
    result.wrongPassword = await ev("document.getElementById('status').textContent");
  } else {
    // a saved device: no link, no enrol form; the program signs for the page
    await waitFor("!document.getElementById('signin').hidden", "the sign-in form (saved device)");
    result.enrolShown = await ev("!document.getElementById('enrol').hidden");
  }
  await signin("Correct-Horse-Battery-9");
  await waitFor("!document.getElementById('stage').hidden && document.getElementById('screen').width > 0", "the picture");
  await sleep(300);
  result.canvas = await ev("(() => { const c = document.getElementById('screen'); const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; const at = (x, y) => Array.from(d.slice((y * c.width + x) * 4, (y * c.width + x) * 4 + 4)); return { w: c.width, h: c.height, left: at(5, 5), right: at(c.width - 5, c.height - 5) }; })()");
  result.passwordFieldCleared = await ev("document.getElementById('pass').value === ''");
  result.databases = await ev("indexedDB.databases ? indexedDB.databases().then((d) => d.map((x) => x.name)) : []");
  await ev("document.getElementById('screen').focus(); true");
  await call("Input.dispatchKeyEvent", { type: "keyDown", key: "a", code: "KeyA", text: "a" });
  await call("Input.dispatchKeyEvent", { type: "keyUp", key: "a", code: "KeyA" });
  await call("Input.dispatchMouseEvent", { type: "mouseMoved", x: 100, y: 100 });
  await call("Input.dispatchMouseEvent", { type: "mousePressed", x: 100, y: 100, button: "left", clickCount: 1 });
  await call("Input.dispatchMouseEvent", { type: "mouseReleased", x: 100, y: 100, button: "left", clickCount: 1 });
  console.log("KEY_SENT");
  if (MODE === "saved") {
    await waitFor("!document.getElementById('problem').hidden", "the revoked message", 30000);
    result.revoked = await ev("document.getElementById('ptitle').textContent + ' | ' + document.getElementById('pmsg').textContent");
  }
  done(0);
} catch (e) { result.errors.push(String(e.message || e)); done(1); }
"##;

struct Instance {
    _srv: Server,
    addr: std::net::SocketAddr,
    fingerprint: String,
    data: PathBuf,
    ep: Arc<ClientEndpoint>,
    pic: Arc<Mutex<Pic>>,
}

fn start_instance(dir: &std::path::Path) -> Instance {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    link::set_enabled(&data, true).unwrap();
    let log = Arc::new(MemoryLog::new(100));
    let broker = Arc::new(Broker::new(Arc::new(Book), Arc::new(NoAsk), log, Arc::new(|| iyakki_agent::clock::now_unix())));
    let ep = Arc::new(ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(|_: &str, _: &str, _: &str| {}), FailLimit::new(50, Duration::from_secs(60))));
    let pic = Arc::new(Mutex::new(Pic { inputs: vec![] }));
    // the repository's own certificate helper: a self-signed certificate kept in a folder, with the fingerprint Settings shows
    let id = iyakki_remote::tls::identity(&dir.join("tls"), &["localhost".to_string()]).unwrap();
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: Some(id.config) }, Arc::new(FakeGate { pic: pic.clone(), ep: ep.clone() })).unwrap();
    let addr = srv.local_addr();
    Instance { _srv: srv, addr, fingerprint: id.fingerprint, data, ep, pic }
}

fn run_node(dir: &std::path::Path, port: u16, mode: &str) -> std::process::Child {
    std::fs::write(dir.join("cdp.mjs"), DRIVER).unwrap();
    Command::new("node").arg(dir.join("cdp.mjs")).args([&port.to_string(), mode]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("node")
}

/// Open the window the way the program does, with headless arguments added, and return (the browser process, the profile folder).
fn open_browser(w: &iyakki_connect::window::Window, debug_port: u16, base: &std::path::Path) -> (std::process::Child, PathBuf) {
    let b = browser::find_here(None).expect("a browser");
    assert_eq!(b.kind, browser::Kind::Chromium, "{b:?}");
    let (profile, start) = browser::prepare_profile(base, w.launch_url(), b.kind).unwrap();
    let extra: Vec<String> = ["--headless=new", "--no-sandbox", "--disable-gpu", &format!("--remote-debugging-port={debug_port}")].iter().map(|s| s.to_string()).collect();
    let c = browser::command(&b, &profile, &start, &extra);
    let child = Command::new(&c.program).args(&c.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("the browser starts");
    (child, start.parent().unwrap().to_path_buf())
}

fn collect(mut node: std::process::Child, mut each: impl FnMut()) -> serde_json::Value {
    let stdout = node.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    let t0 = Instant::now();
    let mut result = None;
    while t0.elapsed() < Duration::from_secs(120) && result.is_none() {
        each();
        if let Ok(l) = rx.recv_timeout(Duration::from_millis(100)) {
            if let Some(r) = l.strip_prefix("RESULT ") {
                result = Some(r.to_string());
            }
        }
    }
    let _ = node.kill();
    let err = node.wait_with_output().map(|o| String::from_utf8_lossy(&o.stderr).to_string()).unwrap_or_default();
    let text = result.unwrap_or_else(|| panic!("no result from the browser driver; stderr: {err}"));
    eprintln!("{text}");
    serde_json::from_str(&text).unwrap()
}

#[test]
#[ignore]
fn a_window_enrols_with_a_pinned_link_keeps_the_key_in_the_program_signs_in_again_by_name_and_a_revoke_is_forgotten_in_a_real_chrome() {
    for tool in ["google-chrome", "node"] {
        if Command::new(tool).arg("--version").output().is_err() {
            panic!("{tool} is needed for this test");
        }
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("iyakki-windowbrowser-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let inst = start_instance(&dir);
    let cfg = dir.join("config"); // this computer's connector folder: never the real home
    let ctx = iyakki_connect::Ctx { store: iyakki_connect::pins::Store::new(&cfg), timeout: Duration::from_secs(8), interactive: false };
    let base = format!("https://127.0.0.1:{}", inst.addr.port());
    let debug = 9300 + (std::process::id() % 300) as u16;

    // ---- first run: the link as Settings makes it, with the pin in it
    let token = link::create_token(&inst.data, "ann", iyakki_agent::clock::now_unix(), 300).unwrap();
    let url = link::link_url_pinned(&base, &token, Some(&inst.fingerprint));
    assert!(url.contains("?pin=") && url.ends_with(&format!("#{token}")), "{url}");
    let args: Vec<String> = ["window", &url, "--name", "test instance"].iter().map(|s| s.to_string()).collect();
    let p = iyakki_connect::windowcmd::prepare(&args, &ctx, &mut std::io::empty(), &mut Vec::new()).expect("the pinned link opens a window without a question");
    assert_eq!(ctx.store.get(&format!("127.0.0.1:{}", inst.addr.port())).unwrap().unwrap().display(), inst.fingerprint, "the link's pin was remembered");
    let (mut chrome, profile) = open_browser(&p.window, debug, &dir);
    let node = run_node(&dir, debug, "enrol");
    let pic = inst.pic.clone();
    let v = collect(node, || {});
    let _ = chrome.kill();
    let _ = chrome.wait();
    let _ = std::fs::remove_dir_all(&profile);
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    assert!(v["url"].as_str().unwrap().starts_with(&format!("http://127.0.0.1:{}/connect", p.window.port())), "the page is on the loopback address: {}", v["url"]);
    assert_eq!(v["hashAfterLoad"], "", "the token is out of the address bar");
    assert!(v["wrongPassword"].as_str().unwrap().contains("Wrong name or password"));
    assert_eq!(v["passwordFieldCleared"], true);
    assert_eq!(v["canvas"]["left"], serde_json::json!([10, 20, 30, 255]));
    assert_eq!(v["canvas"]["right"], serde_json::json!([240, 140, 20, 255]));
    assert_eq!(v["databases"], serde_json::json!([]), "the browser holds no device key (its storage is empty)");
    for u in v["requests"].as_array().unwrap().iter().chain(v["resources"].as_array().unwrap()) {
        let u = u.as_str().unwrap();
        assert!(u.starts_with(&format!("http://127.0.0.1:{}/", p.window.port())) || u.starts_with(&format!("ws://127.0.0.1:{}/", p.window.port())) || u.starts_with("file://") || u.starts_with("about:") || u.starts_with("data:"), "a request to another address: {u}");
    }
    let logs = v["logs"].to_string();
    assert!(!logs.contains("Correct-Horse") && !logs.contains("wrong-password") && !logs.contains(&token), "a secret in the console: {logs}");
    let inputs = pic.lock().unwrap().inputs.clone();
    assert!(inputs.iter().any(|i| i.contains("Char('a')")) && inputs.iter().any(|i| i.contains("MouseDown")), "{inputs:?}");
    // the instance lists one device, and its public key is the one in the program's folder
    let devices = link::list_devices(&inst.data);
    assert_eq!(devices.len(), 1, "{devices:?}");
    assert_eq!(devices[0].name, "Maya's window");
    let store = Instances::new(&cfg);
    let rec = store.find("test instance").unwrap().expect("the program saved the instance");
    assert_eq!(rec.device.as_deref(), Some(devices[0].id.as_str()));
    assert_eq!(devices[0].public, store.key(&rec.addr).unwrap().unwrap().public_hex(), "the enrolled key is the program's");
    assert_eq!(p.window.identity(), (Some(devices[0].id.clone()), true));
    let first_id = devices[0].id.clone();
    drop(p);

    // ---- second run: by the saved name, no link
    inst.pic.lock().unwrap().inputs.clear();
    let args: Vec<String> = ["window", "test instance"].iter().map(|s| s.to_string()).collect();
    let p = iyakki_connect::windowcmd::prepare(&args, &ctx, &mut std::io::empty(), &mut Vec::new()).expect("a saved instance opens by name");
    let (mut chrome, profile) = open_browser(&p.window, debug + 1, &dir);
    let node = run_node(&dir, debug + 1, "saved");
    let (ep, pic2, data) = (inst.ep.clone(), inst.pic.clone(), inst.data.clone());
    let mut revoked = false;
    let v = collect(node, || {
        let inputs = pic2.lock().unwrap().inputs.clone();
        if !revoked && inputs.iter().any(|i| i.contains("Char('a')")) && inputs.iter().any(|i| i.contains("MouseDown")) {
            assert_eq!(ep.open_sessions(), 1);
            assert_eq!(ep.revoke(&first_id, "ann"), Ok(true));
            revoked = true;
        }
    });
    let _ = chrome.kill();
    let _ = chrome.wait();
    let _ = std::fs::remove_dir_all(&profile);
    assert_eq!(v["errors"], serde_json::json!([]), "{v}");
    assert_eq!(v["enrolShown"], false, "no link, no enrolment");
    assert_eq!(v["canvas"]["left"], serde_json::json!([10, 20, 30, 255]), "signed in with a signature the program made for the page");
    assert_eq!(v["databases"], serde_json::json!([]));
    assert!(v["revoked"].as_str().unwrap().contains("revoked"), "{v}");
    assert_eq!(link::list_devices(&data).len(), 1, "no second device was made");
    // the page told the program the device is gone; the program forgot it (record and key file)
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) && !store.list().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(store.list().unwrap().is_empty(), "the revoked device was forgotten: {:?}", store.list());
    assert_eq!(std::fs::read_dir(cfg.join("device-keys")).map(|d| d.count()).unwrap_or(0), 0);
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

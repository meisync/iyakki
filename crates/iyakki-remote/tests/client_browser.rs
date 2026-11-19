//! The client page in a real headless Chrome (driven through the DevTools protocol by node) against the session endpoint on a local plain
//! socket with a fake screen: open the one-time link, add the device, sign in, see the picture on the canvas, send a key, and see a revoke
//! close the session with the right message. Run twice: once with the browser's own Ed25519 key (WebCrypto, non-extractable) and once with
//! WebCrypto's Ed25519 made to fail, which makes the page use the engine's fallback key. Also checks: the token is gone from the address bar,
//! the page made no request to any other address, no console error (a content-security-policy violation would be one), and the password
//! never appears in the console.
//!
//! Ignored by default because it needs `google-chrome` and `node` (22 or newer), like the speech test.
//! Run: cargo test -p iyakki-remote --test client_browser -- --ignored --nocapture

use iyakki_agent::clientlink as link;
use iyakki_core_api::broker::{ApprovalLookup, Broker, CardAnswer, CardAsk, Caller, MemoryLog};
use iyakki_core_api::appcaps::Approval;
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
    viewing: bool,
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
    fn viewing(&mut self, on: bool) {
        self.pic.lock().unwrap().viewing = on;
    }
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
            // left half dark blue, right half orange: the test reads one pixel of each from the canvas
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

const DRIVER: &str = r##"const [BASE, TOKEN, DIR, MODE, PORT] = process.argv.slice(2);
import { spawn } from "node:child_process";
const chrome = spawn("google-chrome", ["--headless=new", "--no-sandbox", "--disable-gpu", "--remote-debugging-port=" + PORT, "--user-data-dir=" + DIR + "/prof-" + MODE, "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { mode: MODE, requests: [], logs: [], errors: [] };
const done = (code) => { try { chrome.kill(); } catch {} console.log("RESULT " + JSON.stringify(result)); setTimeout(() => process.exit(code), 100); };
process.on("uncaughtException", (e) => { result.errors.push("driver: " + e.message); done(1); });
let tabs; for (let i = 0; i < 60; i++) { try { tabs = await (await fetch("http://127.0.0.1:" + PORT + "/json")).json(); if (tabs.length) break; } catch (e) {} await sleep(200); }
const ws = new WebSocket(tabs.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pending = new Map();
ws.onmessage = (m) => {
  const j = JSON.parse(m.data);
  if (j.id && pending.has(j.id)) return pending.get(j.id)(j);
  if (j.method === "Network.requestWillBeSent") result.requests.push(j.params.request.url);
  if (j.method === "Log.entryAdded") result.logs.push(j.params.entry.level + ": " + j.params.entry.text);
  if (j.method === "Runtime.consoleAPICalled") result.logs.push("console." + j.params.type + ": " + j.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
  if (j.method === "Runtime.exceptionThrown") result.logs.push("exception: " + JSON.stringify(j.params.exceptionDetails.exception?.description ?? j.params.exceptionDetails.text));
};
const call = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => { const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); if (r.result.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.result.exceptionDetails.exception?.description)); return r.result.result.value; };
const waitFor = async (expr, what, ms = 20000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await ev(expr)) return; } catch (e) {} await sleep(100); } throw new Error("timed out waiting for " + what + "; status=" + (await ev("document.getElementById('status').textContent + ' | ' + document.getElementById('ptitle').textContent").catch(() => "?"))); };
try {
  await call("Network.enable"); await call("Log.enable"); await call("Runtime.enable"); await call("Page.enable");
  if (MODE === "fallback") {
    // a browser without Ed25519 in WebCrypto: the page must fall back to the engine's key and say so
    await call("Page.addScriptToEvaluateOnNewDocument", { source: "const g = crypto.subtle.generateKey.bind(crypto.subtle); crypto.subtle.generateKey = (a, ...r) => (a && a.name === 'Ed25519' ? Promise.reject(new Error('no ed25519')) : g(a, ...r));" });
  }
  await call("Page.navigate", { url: BASE + "/connect#" + TOKEN });
  await waitFor("!document.getElementById('enrol').hidden", "the enrol form");
  result.hashAfterLoad = await ev("location.hash + '|' + location.href");
  result.keyinfo = await ev("document.getElementById('keyinfo').textContent");
  result.experimental = await ev("document.body.innerText.includes('EXPERIMENTAL')");
  await ev("document.getElementById('devname').value = \"Maya's test browser\"; document.getElementById('enrol').requestSubmit(); true");
  await waitFor("!document.getElementById('signin').hidden", "the sign-in form");
  await ev("document.getElementById('user').value = 'ann'; document.getElementById('pass').value = 'wrong-password'; document.getElementById('signin').requestSubmit(); true");
  await waitFor("document.getElementById('status').textContent.includes('Wrong name or password')", "the wrong password message");
  result.wrongPassword = await ev("document.getElementById('status').textContent");
  await ev("document.getElementById('user').value = 'ann'; document.getElementById('pass').value = 'Correct-Horse-Battery-9'; document.getElementById('signin').requestSubmit(); true");
  await waitFor("!document.getElementById('stage').hidden && document.getElementById('screen').width > 0", "the picture");
  await sleep(300);
  result.canvas = await ev("(() => { const c = document.getElementById('screen'); const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; const at = (x, y) => Array.from(d.slice((y * c.width + x) * 4, (y * c.width + x) * 4 + 4)); return { w: c.width, h: c.height, left: at(5, 5), right: at(c.width - 5, c.height - 5) }; })()");
  result.passwordFieldCleared = await ev("document.getElementById('pass').value === ''");
  // a real key press into the focused canvas
  await ev("document.getElementById('screen').focus(); true");
  await call("Input.dispatchKeyEvent", { type: "keyDown", key: "a", code: "KeyA", text: "a" });
  await call("Input.dispatchKeyEvent", { type: "keyUp", key: "a", code: "KeyA" });
  await ev("window.__m = []; for (const t of ['mousedown','pointerdown','mouseup']) window.addEventListener(t, (e) => __m.push(t + ':' + (e.target.id || e.target.tagName) + ':' + e.clientX), true); true");
  await call("Input.dispatchMouseEvent", { type: "mouseMoved", x: 100, y: 100 });
  await call("Input.dispatchMouseEvent", { type: "mousePressed", x: 100, y: 100, button: "left", clickCount: 1 });
  await call("Input.dispatchMouseEvent", { type: "mouseReleased", x: 100, y: 100, button: "left", clickCount: 1 });
  result.dbg = await ev("JSON.stringify(__m) + ' at ' + (document.elementFromPoint(100,100)||{}).id + ' ' + JSON.stringify(document.getElementById('screen').getBoundingClientRect())");
  console.log("KEY_SENT");
  // the test revokes the device now; the page must say so and stop
  await waitFor("!document.getElementById('problem').hidden", "the revoked message", 30000);
  result.revoked = await ev("document.getElementById('ptitle').textContent + ' | ' + document.getElementById('pmsg').textContent");
  result.deviceKept = await ev("new Promise((res) => { const r = indexedDB.open('iyakki-client', 1); r.onsuccess = () => { const q = r.result.transaction('kv').objectStore('kv').get('device'); q.onsuccess = () => res(!!q.result); q.onerror = () => res('error'); }; r.onerror = () => res('error'); })");
  // a reload without a link: no saved device any more
  await call("Page.navigate", { url: BASE + "/connect" });
  await waitFor("!document.getElementById('problem').hidden", "the no-link message");
  result.noLink = await ev("document.getElementById('ptitle').textContent");
  done(0);
} catch (e) { result.errors.push(String(e.message || e)); done(1); }
"##;

#[test]
#[ignore]
fn the_client_page_enrols_signs_in_shows_the_picture_sends_a_key_and_a_revoke_closes_it_in_a_real_chrome() {
    for tool in ["google-chrome", "node"] {
        if Command::new(tool).arg("--version").output().is_err() {
            panic!("{tool} is needed for this test");
        }
    }
    for (n, mode) in ["webcrypto", "fallback"].into_iter().enumerate() {
        run(mode, 9700 + (std::process::id() % 200) as u16 + n as u16 * 200);
    }
}

fn run(mode: &str, debug_port: u16) {
    let dir: PathBuf = std::env::temp_dir().join(format!("iyakki-clientbrowser-{mode}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    link::set_enabled(&data, true).unwrap();
    let log = Arc::new(MemoryLog::new(100));
    let broker = Arc::new(Broker::new(Arc::new(Book), Arc::new(NoAsk), log, Arc::new(|| iyakki_agent::clock::now_unix())));
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let ep = Arc::new(ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(move |u: &str, a: &str, d: &str| a2.lock().unwrap().push(format!("{u}|{a}|{d}"))), FailLimit::new(50, Duration::from_secs(60))));
    let pic = Arc::new(Mutex::new(Pic { inputs: vec![], viewing: false }));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { pic: pic.clone(), ep: ep.clone() })).unwrap();
    let addr = srv.local_addr();
    let token = link::create_token(&data, "ann", iyakki_agent::clock::now_unix(), 300).unwrap();

    std::fs::write(dir.join("cdp.mjs"), DRIVER).unwrap();
    let mut child = Command::new("node")
        .arg(dir.join("cdp.mjs"))
        .args([&format!("http://{addr}"), &token, dir.to_string_lossy().as_ref(), mode, &debug_port.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node");
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });

    // the device appears in the list once the page enrolled it; wait for the key and the click to arrive, then revoke
    let t0 = Instant::now();
    let mut result = None;
    let mut revoked = false;
    while t0.elapsed() < Duration::from_secs(90) && result.is_none() {
        if !revoked && pic.lock().unwrap().inputs.iter().any(|i| i.contains("Char('a')")) && pic.lock().unwrap().inputs.iter().any(|i| i.contains("MouseDown")) {
            let devices = link::list_devices(&data);
            assert_eq!(devices.len(), 1, "one device was added");
            assert_eq!(devices[0].name, "Maya's test browser");
            assert_eq!(ep.open_sessions(), 1);
            assert_eq!(ep.revoke(&devices[0].id, "ann"), Ok(true));
            revoked = true;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(l) if l.starts_with("RESULT ") => result = Some(l[7..].to_string()),
            _ => {}
        }
    }
    if !revoked {
        eprintln!("{mode}: never saw the key and the click; the screen got {:?}", pic.lock().unwrap().inputs);
    }
    let _ = child.kill();
    let err = child.wait_with_output().map(|o| String::from_utf8_lossy(&o.stderr).to_string()).unwrap_or_default();
    let text = result.unwrap_or_else(|| panic!("{mode}: no result from the browser driver; stderr: {err}"));
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("{mode}: {text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["errors"], serde_json::json!([]), "{mode}: {text}");

    // the link
    let h = v["hashAfterLoad"].as_str().unwrap();
    assert!(!h.contains(&token) && h.starts_with('|'), "{mode}: the token is out of the address bar and the history: {h}");
    assert_eq!(v["experimental"], true, "labelled EXPERIMENTAL");
    let keyinfo = v["keyinfo"].as_str().unwrap();
    if mode == "webcrypto" {
        assert!(keyinfo.contains("cannot be read out"), "{keyinfo}");
    } else {
        assert!(keyinfo.contains("weaker"), "the fallback key is labelled weaker: {keyinfo}");
    }
    // sign in
    assert!(v["wrongPassword"].as_str().unwrap().contains("Wrong name or password"));
    assert_eq!(v["passwordFieldCleared"], true);
    // the picture, drawn by the engine from the instance's tiles
    assert_eq!(v["canvas"]["w"], W);
    assert_eq!(v["canvas"]["h"], H);
    assert_eq!(v["canvas"]["left"], serde_json::json!([10, 20, 30, 255]));
    assert_eq!(v["canvas"]["right"], serde_json::json!([240, 140, 20, 255]));
    // input and revoke
    let inputs = pic.lock().unwrap().inputs.clone();
    assert!(inputs.iter().any(|i| i.contains("Char('a')")), "{inputs:?}");
    assert!(inputs.iter().any(|i| i.contains("MouseDown")) && inputs.iter().any(|i| i.contains("MouseUp")), "{inputs:?}");
    assert!(v["revoked"].as_str().unwrap().contains("revoked"), "{mode}: {text}");
    assert_eq!(v["deviceKept"], false, "a revoked device is forgotten by the page");
    assert_eq!(v["noLink"], "No link");
    // nothing left the machine, nothing was logged but quietly
    for u in v["requests"].as_array().unwrap() {
        let u = u.as_str().unwrap();
        assert!(u.starts_with(&format!("http://{addr}/")) || u.starts_with(&format!("ws://{addr}/")) || u.starts_with("about:") || u.starts_with("data:"), "{mode}: a request to another address: {u}");
    }
    let logs = v["logs"].to_string();
    assert!(!logs.contains("Correct-Horse") && !logs.contains("wrong-password") && !logs.contains(&token), "{mode}: a secret in the console: {logs}");
    assert!(!logs.contains("error") || logs.contains("WebSocket connection") , "{mode}: console errors: {logs}");
    // the audit log: facts, no secrets
    let a = audit.lock().unwrap().join("\n");
    for want in ["client.enrol", "client.signin", "client.revoke"] {
        assert!(a.contains(want), "{a}");
    }
    assert!(!a.contains("Correct-Horse") && !a.contains(&token), "{a}");
    drop(srv);
}

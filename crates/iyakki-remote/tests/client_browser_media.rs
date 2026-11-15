//! The media half of the client page in a real headless Chrome (fake camera and microphone devices, driven through the DevTools protocol by
//! node) against the session endpoint on a local plain socket with fakes: sound played through the audio worklet while the machine's speakers
//! stay quiet, the clipboard on a button press, this device's camera and microphone offered with the red band, and everything gone after
//! Disconnect. Also checks the content security policy lets the worklet load, no request leaves the machine, and no console error.
//!
//! Ignored by default because it needs `google-chrome` and `node` (22 or newer). Headless Chrome only: no real phone, speaker, microphone or camera.
//! Run: cargo test -p iyakki-remote --test client_browser_media -- --ignored --nocapture

use iyakki_agent::clientlink as link;
use iyakki_agent::clientmedia::{grant_device, ClientConsent, MediaHost};
use iyakki_core_api::broker::{Broker, CardAnswer, CardAsk, Caller, MemoryLog};
use iyakki_core_api::client::media::SourceKind;
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

struct Fake {
    inputs: Arc<Mutex<Vec<String>>>,
    armed: bool,
    sent: bool,
    copied: bool,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.inputs.lock().unwrap().push(format!("{i:?}"));
    }
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<Frame> {
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        // the person copied something on the machine
        if std::mem::replace(&mut self.copied, true) { None } else { Some("text from the machine".into()) }
    }
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        if self.armed && !self.sent {
            self.sent = true;
            return Some(Pixels { w: W, h: H, rgba: (0..W * H).flat_map(|_| [10u8, 20, 30, 255]).collect() });
        }
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate {
    inputs: Arc<Mutex<Vec<String>>>,
    ep: Arc<ClientEndpoint>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "Correct-Horse-Battery-9" {
            Ok(Box::new(Fake { inputs: self.inputs.clone(), armed: false, sent: false, copied: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn client_endpoint(&self) -> Option<&ClientEndpoint> {
        Some(&self.ep)
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

/// The person at the machine presses Allow.
struct Allow;
impl CardAsk for Allow {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        CardAnswer::Allow
    }
}

const DRIVER: &str = r##"const [BASE, TOKEN, DIR, PORT] = process.argv.slice(2);
import { spawn } from "node:child_process";
const chrome = spawn("google-chrome", ["--headless=new", "--no-sandbox", "--disable-gpu", "--autoplay-policy=no-user-gesture-required", "--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", "--remote-debugging-port=" + PORT, "--user-data-dir=" + DIR + "/prof", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { requests: [], logs: [], errors: [] };
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
const ev = async (expr) => { const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true, userGesture: true }); if (r.result.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.result.exceptionDetails.exception?.description)); return r.result.result.value; };
const waitFor = async (expr, what, ms = 20000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { try { if (await ev(expr)) return; } catch (e) {} await sleep(100); } throw new Error("timed out waiting for " + what + "; status=" + (await ev("document.getElementById('status').textContent + ' | ' + document.getElementById('sound-state').textContent + ' | ' + document.getElementById('toast').textContent").catch(() => "?"))); };
const click = (id) => ev("document.getElementById('" + id + "').click(); true");
const text = (id) => ev("document.getElementById('" + id + "').textContent");
const hidden = (id) => ev("document.getElementById('" + id + "').hidden");
try {
  await call("Network.enable"); await call("Log.enable"); await call("Runtime.enable"); await call("Page.enable");
  await call("Page.navigate", { url: BASE + "/connect#" + TOKEN });
  await waitFor("!document.getElementById('enrol').hidden", "the enrol form");
  await ev("document.getElementById('devname').value = \"Maya's test browser\"; document.getElementById('enrol').requestSubmit(); true");
  await waitFor("!document.getElementById('signin').hidden", "the sign-in form");
  console.log("ENROLLED");
  await sleep(1500); // the test grants the device its capabilities now
  await ev("document.getElementById('user').value = 'ann'; document.getElementById('pass').value = 'Correct-Horse-Battery-9'; document.getElementById('signin').requestSubmit(); true");
  await waitFor("!document.getElementById('stage').hidden && document.getElementById('screen').width > 0", "the picture");
  // the media button is there because the machine agreed to the media capabilities
  await waitFor("!document.getElementById('devices').hidden", "the Devices button");
  await click("devices");
  result.panelOpen = !(await hidden("panel"));
  result.rows = await ev("JSON.stringify(['sound-row','clip-row','cam-row','offer-row'].map((i) => !document.getElementById(i).hidden))");

  // sound: the click creates the audio context and the worklet; the machine's speakers are muted by the instance
  await click("sound-btn");
  await waitFor("document.getElementById('sound-state').textContent.startsWith('Playing here (the machine is quiet)')", "the sound to open");
  console.log("SOUND_OPEN");
  await waitFor("/Queued \\d+ ms/.test(document.getElementById('sound-state').textContent)", "the worklet's report", 20000);
  result.soundState = await text("sound-state");
  console.log("SOUND_REPORTED");

  // clipboard: the page's clipboard is stubbed (headless has none to read); the press sends it
  await ev("Object.defineProperty(navigator, 'clipboard', { value: { readText: async () => 'text from this device', writeText: async (t) => { window.__written = t; } }, configurable: true }); true");
  await click("clip-send");
  await waitFor("document.getElementById('toast').textContent.includes('Sent to the machine')", "the clipboard to be sent");
  await click("clip-get");
  await waitFor("window.__written === 'text from the machine'", "the machine's clipboard", 15000);

  // this device's camera and microphone, offered
  await click("offer-camera");
  await waitFor("!document.getElementById('shared').hidden && document.getElementById('shared').textContent.includes('camera')", "the red band for the camera");
  await click("offer-mic");
  await waitFor("document.getElementById('shared').textContent.includes('microphone')", "the red band for the microphone");
  console.log("OFFERS_OPEN");
  await sleep(2500);
  result.shared = await text("shared");
  await click("offer-camera"); // stop the camera again; the microphone stays
  await waitFor("!document.getElementById('shared').textContent.includes('camera')", "the camera to stop");
  console.log("CAMERA_STOPPED");
  result.sharedAfterCamera = await text("shared");

  // disconnect: everything stops, the band goes
  await click("bye");
  await waitFor("!document.getElementById('problem').hidden", "the disconnected message");
  result.sharedAfterBye = await ev("document.getElementById('shared').hidden");
  done(0);
} catch (e) { result.errors.push(String(e.message || e)); done(1); }
"##;

#[test]
#[ignore]
fn the_client_page_plays_sound_sends_the_clipboard_and_offers_its_devices_in_a_real_chrome() {
    for tool in ["google-chrome", "node"] {
        if Command::new(tool).arg("--version").output().is_err() {
            panic!("{tool} is needed for this test");
        }
    }
    let dir: PathBuf = std::env::temp_dir().join(format!("iyakki-clientbrowser-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    link::set_enabled(&data, true).unwrap();
    let log = Arc::new(MemoryLog::new(200));
    let host = Arc::new(MediaHost::new(Arc::new(ClientConsent::new(Some(Arc::new(Allow))))));
    host.audio.set_audible(|p| p == "ann");
    // the real approval book, in the temporary data folder (granting is done with the same function Settings will use)
    let broker = Arc::new(Broker::new(Arc::new(iyakki_agent::accessbroker::FileLookup::for_data(&data)), host.card(), log.clone(), Arc::new(|| iyakki_agent::clock::now_unix())));
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let ep = Arc::new(ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(move |u: &str, a: &str, d: &str| a2.lock().unwrap().push(format!("{u}|{a}|{d}"))), FailLimit::new(50, Duration::from_secs(60))).with_media(host.clone()));
    let inputs = Arc::new(Mutex::new(Vec::<String>::new()));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { inputs: inputs.clone(), ep: ep.clone() })).unwrap();
    let addr = srv.local_addr();
    let token = link::create_token(&data, "ann", iyakki_agent::clock::now_unix(), 300).unwrap();
    let port = 9500 + (std::process::id() % 200) as u16;

    std::fs::write(dir.join("cdp.mjs"), DRIVER).unwrap();
    let mut child = Command::new("node")
        .arg(dir.join("cdp.mjs"))
        .args([&format!("http://{addr}"), &token, dir.to_string_lossy().as_ref(), &port.to_string()])
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

    let t0 = Instant::now();
    let mut result = None;
    let mut granted = false;
    let mut tapping = false;
    let mut seen = vec![];
    let mut muted_while_playing = false;
    let mut cam_frames = 0u64;
    let mut mic_frames = 0u64;
    let mut indicator_text = None;
    let mut last_tap = Instant::now();
    while t0.elapsed() < Duration::from_secs(120) && result.is_none() {
        if !granted && seen.contains(&"ENROLLED".to_string()) {
            let id = link::list_devices(&data)[0].id.clone();
            grant_device(&data, &id, "ann", &["speaker.play", "clipboard.client", "camera.view", "client.camera.offer", "client.microphone.offer"], 1_700_000_000).unwrap();
            granted = true;
        }
        if seen.contains(&"SOUND_OPEN".to_string()) && !tapping {
            tapping = true;
        }
        if tapping && last_tap.elapsed() >= Duration::from_millis(20) {
            // what the mixer would give the tee: 20 ms of a tone
            let n = (last_tap.elapsed().as_millis() / 20).min(3) as usize;
            for _ in 0..n {
                let tone: Vec<i16> = (0..1920).map(|i| (((i / 2) % 100) as i16 - 50) * 100).collect();
                host.audio.tap(48_000, 2, &tone);
            }
            last_tap = Instant::now();
            if host.audio.machine_muted() {
                muted_while_playing = true;
            }
        }
        if seen.contains(&"OFFERS_OPEN".to_string()) {
            for s in host.sources.list("") {
                let c = host.sources.counts(s.session, s.kind).unwrap_or((0, 0));
                match s.kind {
                    SourceKind::Camera => cam_frames = cam_frames.max(c.0),
                    SourceKind::Microphone => mic_frames = mic_frames.max(c.0),
                }
            }
            if indicator_text.is_none() {
                indicator_text = host.sources.indicator().text();
            }
        }
        if seen.contains(&"CAMERA_STOPPED".to_string()) && host.sources.list("").iter().any(|s| s.kind == SourceKind::Camera) {
            panic!("the camera offer is still registered after the client stopped it");
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(l) if l.starts_with("RESULT ") => result = Some(l[7..].to_string()),
            Ok(l) => seen.push(l),
            _ => {}
        }
    }
    let _ = child.kill();
    let err = child.wait_with_output().map(|o| String::from_utf8_lossy(&o.stderr).to_string()).unwrap_or_default();
    let text = result.unwrap_or_else(|| panic!("no result from the browser driver; stdout {seen:?}; stderr: {err}"));
    eprintln!("{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["errors"], serde_json::json!([]), "{text}");
    assert_eq!(v["panelOpen"], true);
    assert_eq!(v["rows"], "[true,true,true,true]", "every media row is shown when the machine agreed to every capability");
    assert!(v["soundState"].as_str().unwrap().contains("Queued"), "{text}");
    assert!(muted_while_playing, "the machine's speakers were silenced while the client played the sound");
    wait_until(|| !host.audio.machine_muted() && host.audio.sessions() == 0, "speakers restored after Disconnect");
    wait_until(|| host.sources.list("").is_empty() && !host.sources.indicator().active(), "sources gone after Disconnect");
    assert!(cam_frames >= 3 && mic_frames >= 3, "frames of the fake camera and microphone reached the instance and were dropped: camera {cam_frames}, microphone {mic_frames}");
    assert_eq!(indicator_text.as_deref().map(|t| t.contains("camera") && t.contains("microphone") && t.contains("Maya's test browser")), Some(true), "{indicator_text:?}");
    // the red band on the page
    assert!(v["shared"].as_str().unwrap().contains("camera") && v["shared"].as_str().unwrap().contains("microphone"));
    assert!(!v["sharedAfterCamera"].as_str().unwrap().contains("camera") && v["sharedAfterCamera"].as_str().unwrap().contains("microphone"));
    assert_eq!(v["sharedAfterBye"], true, "the band is gone after Disconnect");
    // the clipboard reached the screen as a paste
    assert!(inputs.lock().unwrap().iter().any(|i| i.contains("Paste") && i.contains("text from this device")));
    // nothing left the machine; no CSP violation or console error
    for u in v["requests"].as_array().unwrap() {
        let u = u.as_str().unwrap();
        assert!(u.starts_with(&format!("http://{addr}/")) || u.starts_with(&format!("ws://{addr}/")) || u.starts_with("about:") || u.starts_with("data:") || u.starts_with("blob:"), "a request to another address: {u}");
    }
    let logs = v["logs"].to_string();
    assert!(!logs.contains("Content Security Policy") && !logs.contains("Refused to"), "a policy violation: {logs}");
    assert!(!logs.contains("Correct-Horse") && !logs.contains(&token) && !logs.contains("text from this device"), "a secret in the console: {logs}");
    // the access log and audit
    let rows = log.rows();
    for cap in ["speaker.play", "clipboard.client", "client.camera.offer", "client.microphone.offer"] {
        assert!(rows.iter().any(|e| e.capability == cap && e.allowed && e.device == "Maya's test browser"), "{cap}: {rows:?}");
    }
    let a = audit.lock().unwrap().join("\n");
    for want in ["client.audio.open", "client.clipboard.put", "client.offer.camera", "client.offer.microphone", "client.offer.end"] {
        assert!(a.contains(want), "{want} missing in {a}");
    }
    assert!(!a.contains("text from this device"), "the audit holds no clipboard text");
    let _ = std::fs::remove_dir_all(&dir);
    drop(srv);
}

fn wait_until(f: impl Fn() -> bool, what: &str) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(5), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

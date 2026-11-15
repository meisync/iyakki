//! The browser voice helper page in a REAL Chrome (ignored by default): `iyakki-agent::voicehelper` listens on 127.0.0.1, a real headless Chrome (driven over the DevTools
//! protocol by a small node program) opens the one-time link, the page connects back over its WebSocket with the right `Host` and `Origin`, announces what the browser can
//! speak, and answers a request to speak; Disconnect closes the page. Needs `google-chrome` and `node` (22). Run: `cargo test -p iyakki-agent --test voicehelper_browser -- --ignored --nocapture`.
//! Row: F-VOICE-06. Nothing is heard (headless).

use iyakki_agent::hostvoice::{HostVoice, Support, UttState};
use iyakki_agent::voicehelper::{HelperState, VoiceHelper};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DRIVER: &str = r##"const [URL, DIR, PORT] = process.argv.slice(2);
import { spawn } from "node:child_process";
const chrome = spawn("google-chrome", ["--headless=new", "--no-sandbox", "--disable-gpu", "--remote-debugging-port=" + PORT, "--user-data-dir=" + DIR + "/prof", "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const result = { logs: [], errors: [], requests: [] };
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
};
const call = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (expr) => { const r = await call("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }); return r.result.result?.value; };
try {
  await call("Network.enable"); await call("Log.enable"); await call("Runtime.enable"); await call("Page.enable");
  await call("Page.navigate", { url: URL });
  await sleep(1500);
  result.hasSynth = await ev("typeof speechSynthesis !== 'undefined'");
  // the person clicks inside the page once (Chrome lets a page speak only after a user gesture)
  await call("Input.dispatchMouseEvent", { type: "mousePressed", x: 60, y: 60, button: "left", clickCount: 1 });
  await call("Input.dispatchMouseEvent", { type: "mouseReleased", x: 60, y: 60, button: "left", clickCount: 1 });
  console.log("NAVIGATED");
  // wait for the parent to say it is over (a line on stdin)
  await new Promise((r) => { process.stdin.once("data", r); });
  await sleep(500);
  result.title = await ev("document.title");
  result.text = await ev("document.body.innerText.slice(0, 400)");
  done(0);
} catch (e) { result.errors.push(String(e.message || e)); done(1); }
"##;

#[test]
#[ignore]
fn the_helper_page_connects_in_a_real_chrome_answers_a_speak_request_and_closes_on_disconnect() {
    for tool in ["google-chrome", "node"] {
        if Command::new(tool).arg("--version").output().is_err() {
            panic!("{tool} is needed for this test");
        }
    }
    let hv = Arc::new(HostVoice::new());
    let helper = VoiceHelper::start(hv.clone()).expect("start the helper");
    let url = helper.url().to_string();
    let dir = std::env::temp_dir().join(format!("iyakki-voicehelper-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let js = dir.join("driver.mjs");
    std::fs::write(&js, DRIVER).unwrap();
    let port = 9300 + (std::process::id() % 400);
    let mut node = Command::new("node").arg(&js).args([&url, dir.to_str().unwrap(), &port.to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut out = BufReader::new(node.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert!(line.starts_with("NAVIGATED"), "driver said: {line}");

    // the page connects back (right Host and Origin) and says what it can speak
    let end = Instant::now() + Duration::from_secs(25);
    while Instant::now() < end && !(hv.helper_active() && !matches!(hv.support(), Support::Checking | Support::Unknown)) {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(helper.state(), HelperState::Connected, "the page did not connect");
    assert!(hv.helper_active());
    let support = hv.support();
    assert!(matches!(support, Support::Yes | Support::No(_)), "the page must answer what it can speak: {support:?}");
    eprintln!("the real Chrome answered: support {support:?}, {} voice(s)", hv.voices().len());
    // the link works once
    let again = {
        use std::io::Read;
        let mut s = std::net::TcpStream::connect(helper.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let path = url.split_once(&format!("{}", helper.addr().port())).unwrap().1;
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", helper.addr().port()).unwrap();
        let mut b = Vec::new();
        let _ = s.read_to_end(&mut b);
        String::from_utf8_lossy(&b).to_string()
    };
    assert!(!again.contains("<script"), "the one-time link must not give the page twice: {again:.200}");
    // a request to speak is answered by the page (started, ended or failed with its reason: headless Chrome has no sound card)
    let id = hv.speak("This is the browser voice helper.", None, Some("en"), 1.0).expect("speak");
    let end = Instant::now() + Duration::from_secs(20);
    let mut state = hv.state(id);
    while Instant::now() < end && state == UttState::Sent {
        std::thread::sleep(Duration::from_millis(200));
        state = hv.state(id);
    }
    eprintln!("the utterance ended as {state:?}");
    assert_ne!(state, UttState::Sent, "the page never answered the request to speak");
    // with a click inside the page Chrome lets it speak: the page started the utterance (a sound card is not needed to start; nothing is heard)
    assert!(matches!(state, UttState::Started | UttState::Ended), "after the click the page should speak: {state:?}");
    // Disconnect closes the listener and the page
    helper.stop();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(helper.state(), HelperState::Closed("Disconnected.".into()));
    node.stdin.as_mut().unwrap().write_all(b"stop\n").unwrap();
    let mut result = String::new();
    while out.read_line(&mut result).unwrap() > 0 && !result.starts_with("RESULT") {
        result.clear();
    }
    eprintln!("{}", result.trim());
    assert!(result.contains("\"hasSynth\":true"), "Chrome has speechSynthesis: {result}");
    assert!(!result.contains("\"errors\":[\""), "the driver had errors: {result}");
    let _ = node.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
